#!/usr/bin/env python3
"""
Tests for vLLM execution with PearlMiner mining control.

Parametrized over both dense (Llama-8B) and MoE (Qwen3-30B-A3B) models. Verifies that:
1. vLLM can be initialized with Pearl plugin support
2. Mining can be controlled (enabled/disabled) globally
3. The model generates outputs correctly in both mining and non-mining modes
"""

import asyncio
import contextlib
import json
import os
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path

import pytest
import torch
from miner_base.settings import MinerSettings
from vllm import LLM, SamplingParams
from vllm_miner.mining_state import get_async_manager, init_async_manager

# ---------------------------------------------------------------------------
# Model configurations
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ModelTestConfig:
    """Inputs and expected initialization marker for one vLLM execution model."""

    id: str
    model: str
    prompt: str
    consistency_prompt: str
    plugin_log_msg: str
    max_model_len: int = 2048
    gpu_memory_utilization: float = 0.9


LLAMA_8B = ModelTestConfig(
    id="llama_8b",
    model="pearl-ai/Llama-3.1-8B-Instruct-pearl",
    prompt=(
        "<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n"
        "You are a French translator. Reply with ONLY the French translation of the user's "
        "sentence. No preamble, no quotes, no explanation, no follow-up.<|eot_id|>"
        "<|start_header_id|>user<|end_header_id|>\n"
        "The cat is on the table.<|eot_id|>"
        "<|start_header_id|>assistant<|end_header_id|>\n"
    ),
    consistency_prompt=(
        "<|begin_of_text|><|start_header_id|>user<|end_header_id|>\n"
        "What is 2+2?<|eot_id|>"
        "<|start_header_id|>assistant<|end_header_id|>\n"
    ),
    plugin_log_msg="Using PearlKernel (mining_enabled=True) for mining layer",
)

QWEN3_MOE = ModelTestConfig(
    id="qwen3_moe",
    model="pearl-ai/Qwen3-30B-A3B-Instruct-2507-pearl",
    prompt=(
        "<|im_start|>system\n"
        "You are a French translator. Reply with ONLY the French translation of the user's "
        "sentence. No preamble, no quotes, no explanation, no follow-up.<|im_end|>\n"
        "<|im_start|>user\n"
        "The cat is on the table.<|im_end|>\n"
        "<|im_start|>assistant\n"
    ),
    consistency_prompt="<|im_start|>user\nWhat is 2+2?<|im_end|>\n<|im_start|>assistant\n",
    plugin_log_msg="Using PearlMoEExperts for MoE layer",
)

MODELS = [LLAMA_8B, QWEN3_MOE]
_model_param = pytest.mark.parametrize("model_config", MODELS, ids=lambda c: c.id)

# Path to reference outputs for regression testing
REFERENCE_OUTPUTS_FILE = Path(__file__).parent / "reference_outputs.json"

# Set to True to regenerate reference outputs (for initial creation or updates)
REGENERATE_REFERENCES = os.getenv("REGENERATE_VLLM_REFERENCES", "false").lower() in (
    "true",
    "1",
    "yes",
)

# FlashAttention 3 (Hopper warp-specialization) is non-deterministic across NVIDIA
# driver versions. TRITON_ATTN is bitwise-reproducible.
_ATTENTION_CONFIG = {"backend": "TRITON_ATTN"}


def _reference_key(model_config: ModelTestConfig, suffix: str) -> str:
    """Key into ``reference_outputs.json`` (Pearl only targets H100, so no GPU-family bucket)."""
    return f"{model_config.id}_{suffix}"


class ReferenceOutputManager:
    """Manages reference outputs for regression testing."""

    def __init__(self, filepath: Path):
        self.filepath = filepath
        self.outputs: dict = {}
        self.modified = False
        self._load()

    def _load(self):
        """Load reference outputs from file if it exists."""
        if self.filepath.exists():
            with open(self.filepath) as f:
                self.outputs = json.load(f)

    def save(self):
        """Save reference outputs to file."""
        if self.modified:
            self.filepath.parent.mkdir(parents=True, exist_ok=True)
            with open(self.filepath, "w") as f:
                json.dump(self.outputs, f, indent=2)
            print(f"\nReference outputs saved to {self.filepath}")

    def validate_or_record(self, test_name: str, actual_output: str) -> bool:
        """
        Validate output against reference or record it if regenerating.

        Returns:
            True if validation passed or output was recorded
        """
        if REGENERATE_REFERENCES:
            self.outputs[test_name] = actual_output
            self.modified = True
            print(f"\nRecorded reference output for {test_name}")
            return True

        if test_name not in self.outputs:
            pytest.fail(
                f"No reference output found for test '{test_name}'.\n"
                f"Run with REGENERATE_VLLM_REFERENCES=true to generate reference outputs."
            )

        reference = self.outputs[test_name]

        # Exact match validation
        if actual_output == reference:
            return True

        # If not exact match, provide detailed error
        pytest.fail(
            f"Output mismatch for test '{test_name}':\n"
            f"Expected length: {len(reference)}\n"
            f"Actual length: {len(actual_output)}\n"
            f"First 200 chars of reference: {reference[:200]}\n"
            f"First 200 chars of actual: {actual_output[:200]}\n"
            f"Run with REGENERATE_VLLM_REFERENCES=true to update reference outputs."
        )


@pytest.fixture(scope="session")
def reference_outputs():
    """Fixture for managing reference outputs."""
    manager = ReferenceOutputManager(REFERENCE_OUTPUTS_FILE)
    yield manager
    manager.save()


def cleanup_llm(llm):
    """Shut down an LLM's engine-core subprocess so GPU memory is released."""
    if llm is None:
        return
    with contextlib.suppress(Exception):
        get_async_manager().wait_until_done_submitting_blocks()
    # vLLM v1's LLM has no __del__; we must terminate the subprocess explicitly.
    with contextlib.suppress(Exception):
        llm.llm_engine.engine_core.shutdown()


def _create_llm_instance(tmp_path_factory, model_config: ModelTestConfig, no_mining: bool):
    """Create an LLM instance for ``model_config`` with the given mining configuration."""
    # Set environment variables BEFORE creating LLM
    os.environ["VLLM_WORKER_MULTIPROC_METHOD"] = "spawn"
    os.environ["MINER_DEBUG"] = "true"
    os.environ["MINER_HARD_INNER_HASH"] = "true"
    os.environ["MINER_NO_MINING"] = "true" if no_mining else "false"
    os.environ["MINER_NO_GATEWAY"] = "false"
    os.environ["VLLM_LOGGING_LEVEL"] = "INFO"
    os.environ["PEARL_LOG_LEVEL"] = "DEBUG"

    mode_tag = "no_mining" if no_mining else "with_mining"
    log_dir = tmp_path_factory.mktemp(f"vllm_logs_{model_config.id}_{mode_tag}")
    log_file = log_dir / "vllm_init.log"

    print(f"\n🚀 Creating LLM instance [{model_config.id}] with MINER_NO_MINING={no_mining}")
    print(f"   Logging to {log_file}")

    # Redirect file descriptors at OS level to capture subprocess output
    saved_stdout_fd = os.dup(1)
    saved_stderr_fd = os.dup(2)

    with open(log_file, "w", buffering=1) as log_stream:
        # Redirect both stdout and stderr to the log file at OS level
        os.dup2(log_stream.fileno(), 1)
        os.dup2(log_stream.fileno(), 2)

        try:
            # the plugin is initialized via vLLM's plugin mechanism, see pyproject.toml
            llm = LLM(
                model=model_config.model,
                max_model_len=model_config.max_model_len,
                enforce_eager=True,
                gpu_memory_utilization=model_config.gpu_memory_utilization,
                attention_config=_ATTENTION_CONFIG,
            )
        finally:
            # Flush and restore original file descriptors
            sys.stdout.flush()
            sys.stderr.flush()
            os.fsync(1)
            os.fsync(2)

            os.dup2(saved_stdout_fd, 1)
            os.dup2(saved_stderr_fd, 2)
            os.close(saved_stdout_fd)
            os.close(saved_stderr_fd)

    # Store metadata as attributes on the llm object
    llm._init_log_file = str(log_file)
    llm._no_mining = no_mining
    llm._model_config = model_config

    # A throwaway generate ensures subsequent calls all hit the stable autotuned path.
    llm.generate(model_config.prompt, SamplingParams(max_tokens=1))

    print(f"✅ LLM instance [{model_config.id}] created")

    return llm


@pytest.fixture
def get_llm_instance(tmp_path_factory):
    """
    Fixture that creates LLM instances and automatically cleans them up after the test.

    For tests with one LLM: cleanup is automatic
    For tests with multiple LLMs: call cleanup_llm() between instances, final cleanup is automatic
    """
    created_llms = []

    def _factory(model_config: ModelTestConfig, is_mining_enabled: bool):
        llm = _create_llm_instance(tmp_path_factory, model_config, no_mining=not is_mining_enabled)
        created_llms.append(llm)
        return llm

    yield _factory

    # Automatic cleanup after test
    for llm in created_llms:
        try:
            cleanup_llm(llm)
        except Exception as e:
            print(f"Warning: Error during automatic cleanup: {e}")


@pytest.fixture(autouse=True)
def manage_mining_state(pearl_gateway_process):
    """Reset mining manager state and CUDA cache around each test."""
    manager = get_async_manager()
    previous_state = manager._conf.no_mining

    if torch.cuda.is_available():
        torch.cuda.empty_cache()
        torch.cuda.synchronize()

    try:
        yield
    finally:
        manager.wait_until_done_submitting_blocks()
        manager._conf.no_mining = previous_state
        if torch.cuda.is_available():
            torch.cuda.empty_cache()
            torch.cuda.synchronize()


@pytest.fixture(scope="session", autouse=True)
def pearl_gateway_process(sample_block_template, mining_address):  # noqa: C901
    """
    Start pearl-gateway programmatically once per test session.

    Starts the gateway and populates its work cache with a mock block template
    to enable testing without blockchain node connectivity.
    """
    from pearl_gateway.pearl_gateway import PearlGateway

    # Set mining address environment variable required by PearlConfig
    os.environ["PEARLD_MINING_ADDRESS"] = mining_address

    # Use default socket path from pearl-gateway config
    socket_path = "/tmp/pearlgw.sock"

    # Remove socket if it exists from previous run
    if os.path.exists(socket_path):
        os.unlink(socket_path)

    # Store gateway instance for work cache access
    gateway_instance = None

    # Run gateway in background thread with custom starter that stores instance
    async def start_gateway_with_mock_template():
        nonlocal gateway_instance
        gateway_instance = PearlGateway(debug_mode=True)

        # Start the gateway
        await gateway_instance.start()

        # Populate work cache with mock block template for testing without node
        print("Populating gateway work cache with mock block template...")
        await gateway_instance.work_cache.update_template(sample_block_template)
        print(
            f"✓ Work cache populated with block template at height {sample_block_template.height}"
        )

        # Keep the service running
        try:
            while True:
                await asyncio.sleep(1)
        except asyncio.CancelledError:
            await gateway_instance.stop()

    def run_gateway():
        asyncio.run(start_gateway_with_mock_template())

    gateway_thread = threading.Thread(target=run_gateway, daemon=True)
    gateway_thread.start()

    # Wait for gateway to create socket file
    max_wait = 10  # seconds
    wait_interval = 0.5
    elapsed = 0

    print(f"Waiting for gateway to create socket at: {socket_path}")

    while elapsed < max_wait:
        if os.path.exists(socket_path):
            print(f"Socket created at: {socket_path}")
            break
        time.sleep(wait_interval)
        elapsed += wait_interval

    if not os.path.exists(socket_path):
        raise RuntimeError(f"Gateway socket not created after {max_wait}s")

    # Additional wait for gateway to fully initialize and populate cache
    time.sleep(2)

    # Initialize async manager in main process (reads settings from environment variables)
    # Note: The worker processes will also initialize their own async managers
    test_settings = MinerSettings()
    print(f"\nTest fixture settings: {test_settings}")
    init_async_manager(test_settings)
    manager = get_async_manager()

    gateway_info = {
        "socket_path": socket_path,
        "thread": gateway_thread,
        "gateway": gateway_instance,
    }

    try:
        yield gateway_info
    finally:
        # Wait for any pending submissions
        manager.wait_until_done_submitting_blocks()

        # Cleanup: gateway thread is daemon so will exit with pytest
        # Remove socket file
        if os.path.exists(socket_path):
            os.unlink(socket_path)

        print("\nGateway terminated")


@pytest.fixture
def deterministic_params():
    """Create deterministic sampling parameters for reproducible test outputs."""
    return SamplingParams(temperature=0.0, max_tokens=100, seed=42)


# ---------------------------------------------------------------------------
# Parametrized tests (run for every model in MODELS)
# ---------------------------------------------------------------------------


@_model_param
@pytest.mark.parametrize("is_mining_enabled", [True, False])
def test_plugin_loaded(model_config, get_llm_instance, is_mining_enabled):
    """The Pearl plugin's per-layer log message appears during vLLM init."""
    llm = get_llm_instance(model_config, is_mining_enabled)

    with open(llm._init_log_file) as f:
        log_content = f.read()

    if model_config.plugin_log_msg not in log_content:
        pytest.fail(
            f"Expected log message not found: {model_config.plugin_log_msg!r}\n"
            f"Log file: {llm._init_log_file} ({len(log_content)} bytes)"
        )


@_model_param
def test_mining_generates_text(
    model_config, get_llm_instance, deterministic_params, reference_outputs
):
    """Mining-enabled path produces correct, non-empty text."""
    llm = get_llm_instance(model_config, is_mining_enabled=True)
    manager = get_async_manager()

    text = llm.generate(model_config.prompt, deterministic_params)[0].outputs[0].text

    # Wait for all blocks to be submitted before assertions
    manager.wait_until_done_submitting_blocks()

    assert len(text) > 0, "Generated text should not be empty"
    reference_outputs.validate_or_record(_reference_key(model_config, "mining"), text)


@_model_param
def test_no_mining_generates_text(
    model_config, get_llm_instance, deterministic_params, reference_outputs
):
    """No-mining path produces correct, non-empty text."""
    llm = get_llm_instance(model_config, is_mining_enabled=False)

    text = llm.generate(model_config.prompt, deterministic_params)[0].outputs[0].text

    assert len(text) > 0, "Generated text should not be empty"
    reference_outputs.validate_or_record(_reference_key(model_config, "no_mining"), text)


@_model_param
def test_consistency(model_config, deterministic_params, reference_outputs, get_llm_instance):
    """Both mining modes produce deterministic output on the same prompt."""
    manager = get_async_manager()

    # Phase 1: mining enabled
    print(f"\n📝 [{model_config.id}] Phase 1: mining ENABLED")
    llm_on = get_llm_instance(model_config, is_mining_enabled=True)
    text_on = (
        llm_on.generate(model_config.consistency_prompt, deterministic_params)[0].outputs[0].text
    )
    manager.wait_until_done_submitting_blocks()

    assert len(text_on) > 0, "Mining mode should produce output"
    reference_outputs.validate_or_record(
        _reference_key(model_config, "consistency_mining"), text_on
    )

    # Free the first LLM before creating the second so both fit in GPU memory.
    cleanup_llm(llm_on)

    # Phase 2: mining disabled
    print(f"\n📝 [{model_config.id}] Phase 2: mining DISABLED")
    llm_off = get_llm_instance(model_config, is_mining_enabled=False)
    text_off = (
        llm_off.generate(model_config.consistency_prompt, deterministic_params)[0].outputs[0].text
    )
    manager.wait_until_done_submitting_blocks()

    assert len(text_off) > 0, "Non-mining mode should produce output"
    reference_outputs.validate_or_record(
        _reference_key(model_config, "consistency_no_mining"), text_off
    )
