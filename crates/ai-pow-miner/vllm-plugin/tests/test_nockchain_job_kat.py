from __future__ import annotations

import os
import socket
import subprocess
import sys
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import grpc

_REPO_ROOT = Path(__file__).resolve().parents[4]
_PROTO_ROOT = (
    _REPO_ROOT / "crates" / "ai-pow-miner" / "vllm-plugin" / "src" / "vllm_miner"
)
sys.path.insert(0, str(_PROTO_ROOT))

from proto import inference_mining_pb2 as pb  # noqa: E402
from proto import inference_mining_pb2_grpc as pb_grpc  # noqa: E402

_PROTOCOL_VERSION = 3
_CANONICAL_GEMMA4_MU = bytes.fromhex(
    "0015000080000000000f00000000000f00000000" + "00" * 32
)
_CHECKPOINT_CONTENT_DIGEST = bytes.fromhex(
    "c59cb83550f52b26893c1837133555bf32190495372ce00935d989592515ff40"
)


def _free_loopback_endpoint() -> str:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return f"127.0.0.1:{listener.getsockname()[1]}"


@contextmanager
def _rust_bridge(endpoint: str) -> Iterator[None]:
    binary = os.getenv("NOCKCHAIN_INFERENCE_BRIDGE_BIN") or os.getenv(
        "CARGO_BIN_EXE_ai-pow-inference-bridge"
    )
    bridge_args = [
        "--listen",
        endpoint,
        "--mock-idle-batch-ms",
        "1",
    ]
    if binary:
        command = [binary, *bridge_args]
    else:
        command = [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "ai-pow-miner",
            "--bin",
            "ai-pow-inference-bridge",
            "--features",
            "node",
            "--",
            *bridge_args,
        ]

    process = subprocess.Popen(
        command,
        cwd=_REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    channel = grpc.insecure_channel(endpoint)
    ready = grpc.channel_ready_future(channel)
    deadline = time.monotonic() + 300
    try:
        while True:
            if process.poll() is not None:
                output, _ = process.communicate()
                raise AssertionError(
                    f"Rust inference bridge exited with {process.returncode}:\n{output}"
                )
            try:
                ready.result(timeout=0.1)
                break
            except grpc.FutureTimeoutError:
                if time.monotonic() >= deadline:
                    raise AssertionError("Rust inference bridge did not become ready")
        yield
    finally:
        channel.close()
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def test_python_receives_rust_canonical_mining_job() -> None:
    endpoint = _free_loopback_endpoint()
    with _rust_bridge(endpoint):
        channel = grpc.insecure_channel(endpoint)
        try:
            stub = pb_grpc.InferenceMiningServiceStub(channel)
            runtime = stub.RegisterRuntime(
                pb.RegisterRuntimeRequest(
                    protocol_version=_PROTOCOL_VERSION,
                    checkpoint_content_digest=_CHECKPOINT_CONTENT_DIGEST,
                    cuda_device_uuid=bytes([0x44]) * 16,
                    process_id=os.getpid(),
                ),
                timeout=5,
            )
            job = stub.GetMiningJob(
                pb.GetMiningJobRequest(runtime_id=runtime.runtime_id), timeout=5
            )
        finally:
            channel.close()

    assert runtime.protocol_version == _PROTOCOL_VERSION
    assert job.candidate_generation == 1
    assert job.incomplete_header == bytes(76)
    assert job.mining_config == _CANONICAL_GEMMA4_MU
    assert job.effective_target_le == bytes(32)
    assert job.certificate_version == 3
