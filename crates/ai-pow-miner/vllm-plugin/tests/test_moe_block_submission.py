import time
from unittest.mock import patch

import pytest
import torch
from compressed_tensors.quantization import QuantizationArgs
from miner_base.gpu_matmul_config import GPUMatmulConfigFactory
from miner_base.settings import MinerSettings
from miner_utils import get_logger
from moe_testing_helpers import make_moe_tensors
from pearl_gateway.comm.mining_configuration import MoEConfig
from vllm.model_executor.layers.fused_moe.activation import MoEActivation
from vllm.model_executor.layers.fused_moe.config import (
    FusedMoEConfig,
    FusedMoEParallelConfig,
    RoutingMethodType,
)
from vllm.v1.worker.workspace import init_workspace_manager, is_workspace_manager_initialized
from vllm_miner import config as config_module
from vllm_miner.mining_state import (
    delete_state,
    get_async_manager,
    init_async_manager,
    init_pinned_pool,
)
from vllm_miner.pearl_moe_method import PearlMoEMethod

pytestmark = [pytest.mark.gpu, pytest.mark.moe]

logger = get_logger(__name__)

_MINING_LOOP_TIMEOUT_SECONDS = 120
_PROOF_TIMEOUT_SECONDS = 120
_PROOF_POLL_INTERVAL_SECONDS = 1.0

_NUM_EXPERTS = 8
_HIDDEN_SIZE = 2048
_INTERMEDIATE_SIZE = 1024
_NUM_TOKENS = 2048
_TOP_K = 2
_EASY_TARGET = 2**242
_HADAMARD_BLOCK_SIZE = 32

_DOWN_WEIGHT_QUANT = QuantizationArgs(
    num_bits=8,
    type="float",
    strategy="block",
    block_structure=[128, 128],
    symmetric=True,
    dynamic=False,
)
_DOWN_INPUT_QUANT = QuantizationArgs(
    num_bits=8,
    type="float",
    strategy="group",
    group_size=128,
    dynamic=True,
    symmetric=True,
)


@pytest.fixture
def async_manager():
    init_async_manager(MinerSettings(debug=True, no_gateway=True))
    init_pinned_pool()
    yield get_async_manager()
    delete_state()


@pytest.fixture
def pearl_moe_layer():
    """Build a PearlMoEMethod + mock layer using the production weight-loading flow."""
    if not is_workspace_manager_initialized():
        init_workspace_manager(torch.device("cuda"))

    moe_config = FusedMoEConfig(
        num_experts=_NUM_EXPERTS,
        experts_per_token=_TOP_K,
        hidden_dim=_HIDDEN_SIZE,
        intermediate_size_per_partition=_INTERMEDIATE_SIZE,
        num_local_experts=_NUM_EXPERTS,
        num_logical_experts=_NUM_EXPERTS,
        activation=MoEActivation.SILU,
        device="cuda",
        routing_method=RoutingMethodType.Default,
        moe_parallel_config=FusedMoEParallelConfig.make_no_parallel(),
        in_dtype=torch.bfloat16,
    )
    moe_method = PearlMoEMethod(moe_config, _DOWN_WEIGHT_QUANT, _DOWN_INPUT_QUANT)

    layer = torch.nn.Module()
    moe_method.create_weights(
        layer,
        num_experts=_NUM_EXPERTS,
        hidden_size=_HIDDEN_SIZE,
        intermediate_size_per_partition=_INTERMEDIATE_SIZE,
        params_dtype=torch.bfloat16,
    )

    tensors = make_moe_tensors(
        num_experts=_NUM_EXPERTS,
        hidden_dim=_HIDDEN_SIZE,
        intermediate_size=_INTERMEDIATE_SIZE,
        num_tokens=_NUM_TOKENS,
        top_k=_TOP_K,
    )

    layer.w13_weight.data.copy_(tensors.w13_weight)
    layer.w2_weight.data.copy_(tensors.w2_weight)
    layer.w13_weight_scale.data.copy_(tensors.w13_weight_scale)
    layer.w2_weight_scale.data.copy_(tensors.w2_weight_scale)

    layer.to("cuda")
    moe_method.process_weights_after_loading(layer)

    layer.activation = MoEActivation.SILU
    layer.global_num_experts = _NUM_EXPERTS
    layer.apply_router_weight_on_input = False
    layer.expert_map = None

    return moe_method, layer, tensors


@pytest.mark.flaky(reruns=3)
def test_block_found_and_proof_verifies(pearl_moe_layer, get_mining_job, async_manager):
    """Full MoE flow: apply() -> PoW hit -> create_proof -> submit (dummy gateway)."""
    moe_method, layer, tensors = pearl_moe_layer

    am = get_async_manager()
    am._conf.no_gateway = True
    am._conf.no_mining = False
    noise_rank = am._conf.noise_rank

    _, _n, k = tensors.w13_weight.shape
    matmul_config = GPUMatmulConfigFactory.create(
        k=k, noise_rank=noise_rank, moe=MoEConfig(e=_NUM_EXPERTS, top_k=_TOP_K)
    )
    mining_job = get_mining_job(mining_config=matmul_config.mining_config, target=_EASY_TARGET)

    with patch.object(am, "_client") as mock_mining_client:
        mock_mining_client.get_mining_info.return_value = mining_job
        am._mining_job = am._client.get_mining_info()

        moe_method.apply(
            layer,
            tensors.hidden_states,
            tensors.topk_weights,
            tensors.topk_ids,
            None,
        )

        am.wait_until_done_submitting_blocks()
        assert am.blocks_submitted == 1


@pytest.fixture
def async_manager_real(real_gateway):
    """Async manager connected to the real gateway (mirrors the dense integration test)."""
    original_socket_path = config_module.config._config.get("gateway_socket_path")
    config_module.config._config["gateway_socket_path"] = real_gateway.config.miner_rpc.socket_path

    init_async_manager(MinerSettings(debug=True, no_gateway=False))
    init_pinned_pool()
    yield get_async_manager()

    delete_state()
    config_module.config._config["gateway_socket_path"] = original_socket_path


class TestBlockSubmissionIntegration:
    """End-to-end MoE flow against a real Pearl node (requires a running node)."""

    @pytest.mark.integration
    def test_block_submission(self, real_gateway, async_manager_real, pearl_moe_layer):
        """Mine via PearlMoEMethod.apply, generate the ZK proof in the gateway, and
        submit to the real node:

        1. PearlMoEMethod.apply mines a block (per-expert noisy_gemm + PoW)
        2. Gateway fetches the block template from the real node
        3. Gateway generates the ZK proof and submits a V2 (MoE) certificate
        4. The node accepts the block
        """
        moe_method, layer, tensors = pearl_moe_layer
        async_manager = async_manager_real
        submission_service = real_gateway.submission_service

        start_time = time.time()
        forward_count = 0
        logger.info("Starting MoE mining loop, waiting for block...")

        while async_manager.blocks_submitted == 0:
            if time.time() - start_time > _MINING_LOOP_TIMEOUT_SECONDS:
                pytest.fail(
                    f"Timeout ({_MINING_LOOP_TIMEOUT_SECONDS}s) waiting for block. "
                    f"Performed {forward_count} forwards, "
                    f"blocks_submitted={async_manager.blocks_submitted}"
                )

            assert get_async_manager() is async_manager
            assert async_manager._loop is not None

            hidden_states = torch.randn(
                _NUM_TOKENS, _HIDDEN_SIZE, dtype=torch.bfloat16, device="cuda"
            )
            moe_method.apply(layer, hidden_states, tensors.topk_weights, tensors.topk_ids, None)
            forward_count += 1
            async_manager.wait_until_done_submitting_blocks()

        assert async_manager.blocks_submitted > 0, "No blocks were submitted"
        logger.info(
            f"Block found after {forward_count} forwards, "
            "waiting for proof generation and submission..."
        )

        proof_start = time.time()
        while (submission_service.accepted_blocks + submission_service.rejected_blocks) == 0:
            if time.time() - proof_start > _PROOF_TIMEOUT_SECONDS:
                pytest.fail(f"Timeout ({_PROOF_TIMEOUT_SECONDS}s) waiting for proof generation")
            time.sleep(_PROOF_POLL_INTERVAL_SECONDS)

        logger.info(
            "Submission results: "
            f"submitted={submission_service.submitted_blocks}, "
            f"accepted={submission_service.accepted_blocks}, "
            f"rejected={submission_service.rejected_blocks}"
        )

        assert submission_service.submitted_blocks >= 1, "No blocks were submitted"
        assert submission_service.accepted_blocks >= 1, (
            f"Block was rejected by the node. "
            f"Submitted: {submission_service.submitted_blocks}, "
            f"Accepted: {submission_service.accepted_blocks}, "
            f"Rejected: {submission_service.rejected_blocks}"
        )
        logger.info("SUCCESS! MoE block was ACCEPTED by the Pearl node!")
