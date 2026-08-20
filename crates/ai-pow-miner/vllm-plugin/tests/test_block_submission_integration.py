"""
End-to-end integration tests with a real Pearl node.

Uses configuration from pearl_gateway/config.yaml which contains
the Pearl node connection settings.
"""

import time

import pytest
import torch
from miner_base.settings import MinerSettings
from miner_utils import get_logger
from vllm_miner import config as config_module
from vllm_miner.gemm_operators import pearl_gemm_noisy
from vllm_miner.mining_state import (
    delete_state,
    get_async_manager,
    init_async_manager,
    init_pinned_pool,
)

logger = get_logger(__name__)


@pytest.fixture
def async_manager_real(real_gateway):
    """
    Create an async manager connected to the real gateway.
    This fixture initializes the mining state before starting the manager.
    Monkey patches config to use the gateway's socket path.
    """

    # Monkey patch the config's gateway_socket_path to use the real gateway's socket
    original_socket_path = config_module.config._config.get("gateway_socket_path")
    config_module.config._config["gateway_socket_path"] = real_gateway.config.miner_rpc.socket_path

    miner_settings = MinerSettings(debug=True, no_gateway=False)
    init_async_manager(miner_settings)
    init_pinned_pool()

    manager = get_async_manager()
    yield manager

    delete_state()

    config_module.config._config["gateway_socket_path"] = original_socket_path


@pytest.fixture(autouse=True)
def reset_cuda():
    """Reset CUDA state before and after each test."""
    if torch.cuda.is_available():
        torch.cuda.empty_cache()
        torch.cuda.synchronize()
    yield
    if torch.cuda.is_available():
        torch.cuda.empty_cache()
        torch.cuda.synchronize()


@pytest.mark.integration
def test_block_submission(real_gateway, async_manager_real, make_random_test_matrices):
    """
    Test complete end-to-end flow with a real Pearl node:
    1. Miner connects to gateway via pearl_gemm_noisy
    2. Gateway fetches block template from real node
    3. Miner mines and finds a valid block
    4. Gateway generates ZK proof
    5. Block is submitted to real Pearl node
    6. Node accepts or rejects the block
    """

    async_manager = async_manager_real
    submission_service = real_gateway.submission_service

    # Mine until a block passes secondary testing
    timeout_seconds = 120
    start_time = time.time()
    matmul_count = 0
    m = n = k = 2048

    logger.info("Starting mining loop via pearl_gemm_noisy, waiting for block...")

    while async_manager.blocks_submitted == 0:
        elapsed = time.time() - start_time
        if elapsed > timeout_seconds:
            pytest.fail(
                f"Timeout ({timeout_seconds}s) waiting for block. "
                f"Performed {matmul_count} matmuls, "
                f"blocks_submitted={async_manager.blocks_submitted}"
            )

        # Create random test matrices for each matmul
        A, B, scale_a, scale_b, out_dtype, bias = make_random_test_matrices(m, n, k)

        # some sanity
        assert get_async_manager() is async_manager
        assert async_manager._loop is not None
        assert get_async_manager()._loop is not None

        # Perform noisy GEMM (this triggers mining)
        _ = pearl_gemm_noisy(A, B, scale_a.squeeze(), scale_b.squeeze(), out_dtype, bias)
        matmul_count += 1
        async_manager.wait_until_done_submitting_blocks()

    assert async_manager.blocks_submitted > 0, "No blocks were submitted"
    logger.info(
        f"Block passed secondary testing after {matmul_count} matmuls, waiting for proof generation and submission..."
    )

    # Wait for proof generation and block submission
    proof_timeout = 120
    proof_start = time.time()
    while (submission_service.accepted_blocks + submission_service.rejected_blocks) == 0:
        elapsed = time.time() - proof_start
        if elapsed > proof_timeout:
            pytest.fail(f"Timeout ({proof_timeout}s) waiting for proof generation")
        time.sleep(1.0)

    proof_time = time.time() - proof_start
    logger.info(f"Proof generation completed in {proof_time:.1f} seconds")

    # Check submission results
    logger.info(
        f"Submission results: "
        f"submitted={submission_service.submitted_blocks}, "
        f"accepted={submission_service.accepted_blocks}, "
        f"rejected={submission_service.rejected_blocks}"
    )

    # Verify block was submitted and accepted
    assert submission_service.submitted_blocks >= 1, "No blocks were submitted"
    assert submission_service.accepted_blocks >= 1, (
        f"Block was rejected by the node. "
        f"Submitted: {submission_service.submitted_blocks}, "
        f"Accepted: {submission_service.accepted_blocks}, "
        f"Rejected: {submission_service.rejected_blocks}"
    )

    logger.info("SUCCESS! Block was ACCEPTED by the Pearl node!")
