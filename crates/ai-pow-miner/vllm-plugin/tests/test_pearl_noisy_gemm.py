from unittest.mock import patch

import pytest
import torch
from miner_base.gpu_matmul_config import GPUMatmulConfigFactory
from miner_base.settings import MinerSettings
from miner_utils import get_logger
from pearl_gemm import get_host_signal_header_size, get_host_signal_sync_size, noisy_gemm
from vllm_miner.gemm_operators import (
    make_pow_target_tensor,
    pearl_gemm_noisy,
    pearl_gemm_vanilla,
)
from vllm_miner.mining_state import (
    delete_state,
    get_async_manager,
)

logger = get_logger(__name__)


@pytest.fixture(autouse=True)
def async_manager():
    try:
        from miner_base.settings import MinerSettings
        from vllm_miner.mining_state import (
            get_async_manager,
            init_async_manager,
            init_pinned_pool,
        )

        init_async_manager(MinerSettings(debug=True, no_gateway=True))
        init_pinned_pool()
        yield get_async_manager()
        delete_state()
    except ImportError:
        # Skip if vLLM miner modules not available
        yield None


@pytest.fixture(autouse=True)
def reset_mining_all(async_manager):
    try:
        import torch

        if torch.cuda.is_available():
            torch.cuda.empty_cache()
            torch.cuda.synchronize()
        yield
        if async_manager is not None:
            async_manager.wait_until_done_submitting_blocks()
        if torch.cuda.is_available():
            torch.cuda.empty_cache()
            torch.cuda.synchronize()
    except ImportError:
        pass


@pytest.mark.flaky(reruns=3)
def test_pearl_gemm_noisy_with_block_found(
    get_mining_job, make_random_test_matrices, default_matmul_config
):
    A, B, scale_a, scale_b, out_dtype, _ = make_random_test_matrices(2048, 2048, 2048)

    get_async_manager()._conf.no_gateway = True
    get_async_manager()._conf.no_mining = False
    noise_rank = get_async_manager()._conf.noise_rank

    matmul_config = GPUMatmulConfigFactory.create(k=A.shape[1], noise_rank=noise_rank)
    mining_job = get_mining_job(
        mining_config=matmul_config.mining_config,
        target=2**242,
    )

    with patch.object(get_async_manager(), "_client") as mock_mining_client:
        mock_mining_client.get_mining_info.return_value = mining_job
        get_async_manager()._mining_job = get_async_manager()._client.get_mining_info()

        _ = pearl_gemm_noisy(A, B, scale_a.squeeze(), scale_b.squeeze(), out_dtype)

        get_async_manager().wait_until_done_submitting_blocks()
        assert get_async_manager().blocks_submitted == 1


def test_pearl_gemm_noisy_with_zero_noise(make_random_test_matrices):
    """Test pearl_gemm_noisy with zero noise (should behave like vanilla GEMM)."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    assert get_async_manager() is not None

    a, b, scale_a, scale_b, out_dtype, _ = make_random_test_matrices(1024, 1024, 1024)

    # Test noisy GEMM with zero noise matrices
    output_with_zero_noise, ApEA, BpEB = pearl_gemm_noisy_with_zero_noise(
        a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype
    )

    # Validate that noised matrices equal original matrices when using zero noise
    print("Validating noised matrices with zero noise...")
    print(f"BpEB shape: {BpEB.shape}, B shape: {b.shape}")
    print(f"ApEA shape: {ApEA.shape}, A shape: {a.shape}")


def test_pearl_gemm_noisy_with_real_noise(make_random_test_matrices):
    """Test pearl_gemm_noisy with real noise generation."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    assert get_async_manager() is not None

    m, n, k = 1024, 1024, 1024
    # Use K divisible by 64 to avoid "K must be divisible by bK" error
    a, b, scale_a, scale_b, out_dtype, _ = make_random_test_matrices(m, n, k)

    # Test with zero noise (our custom function)
    print("Testing with zero noise...")
    output_with_zero_noise, ApEA_zero, BpEB_zero = pearl_gemm_noisy_with_zero_noise(
        a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype
    )

    # Test with real noise using the actual pearl_gemm_noisy function
    print("Testing with real noise...")
    output_with_real_noise = pearl_gemm_noisy(a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype)

    # Basic validation
    assert output_with_zero_noise.shape == (m, n)
    assert output_with_zero_noise.dtype == out_dtype
    assert output_with_real_noise.shape == (m, n)
    assert output_with_real_noise.dtype == out_dtype

    # Compare statistics
    print("\nComparison between zero noise and real noise:")
    print(
        "Zero noise output range:",
        f"[{output_with_zero_noise.min().item():.6f}, {output_with_zero_noise.max().item():.6f}]",
    )
    print(f"Zero noise output mean: {output_with_zero_noise.mean().item():.6f}")
    print(f"Zero noise output std: {output_with_zero_noise.std().item():.6f}")
    print(
        "Real noise output range:",
        f"[{output_with_real_noise.min().item():.6f}, {output_with_real_noise.max().item():.6f}]",
    )
    print(f"Real noise output mean: {output_with_real_noise.mean().item():.6f}")
    print(f"Real noise output std: {output_with_real_noise.std().item():.6f}")

    # Check if real noise output is constant (this would indicate a bug)
    if output_with_real_noise.std().item() < 1e-6:
        print(f"❌ ERROR: Real noise output is constant: {output_with_real_noise[0, 0].item()}")
        print("This indicates a bug in the pearl_gemm_noisy function")
        raise AssertionError("Real noise should not produce constant output")

    # Check if zero noise output is constant (this would also indicate a bug)
    if output_with_zero_noise.std().item() < 1e-6:
        print(f"❌ ERROR: Zero noise output is constant: {output_with_zero_noise[0, 0].item()}")
        print("This indicates a bug in the zero noise implementation")
        raise AssertionError("Zero noise should not produce constant output")

    # Compare with vanilla GEMM (all should match vanilla)
    print("\nComparing with vanilla GEMM:")
    vanilla_output = pearl_gemm_vanilla(a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype)

    zero_vs_vanilla_diff = torch.abs(output_with_zero_noise - vanilla_output).max().item()
    real_vs_vanilla_diff = torch.abs(output_with_real_noise - vanilla_output).max().item()

    print(f"Zero noise vs vanilla max diff: {zero_vs_vanilla_diff:.6f}")
    print(f"Real noise vs vanilla max diff: {real_vs_vanilla_diff:.6f}")
    print(
        "Entries: ",
        {torch.abs(output_with_real_noise - vanilla_output).argmax().item()},
    )
    # All should be very close to vanilla (since noisy_gemm includes denoising)
    assert zero_vs_vanilla_diff < 1e-1, "Zero noise should match vanilla GEMM closely"
    assert real_vs_vanilla_diff < 1e-1, "Real noise should match vanilla GEMM closely"
    print("✓ Confirmed that both implementations match vanilla GEMM")

    # Test multiple runs to see noise variation
    print("\nTesting noise variation across multiple runs:")
    outputs = []
    for _i in range(5):
        output = pearl_gemm_noisy(a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype)
        outputs.append(output)

    # Stack outputs to compute statistics across runs
    outputs_tensor = torch.stack(outputs)
    output_std_across_runs = outputs_tensor.std(dim=0).mean().item()
    output_range_across_runs = (outputs_tensor.max() - outputs_tensor.min()).item()

    print(f"Mean std across runs: {output_std_across_runs:.6f}")
    print(f"Total range across runs: {output_range_across_runs:.6f}")

    # There should be minimal variation across runs since denoising
    # should produce consistent results
    # (The noise is random but the denoising should compensate for it)
    if output_std_across_runs > 1e-2:
        print(
            "⚠️  WARNING: High variation across runs - this might indicate denoising is not working correctly"
        )
    else:
        print("✓ Confirmed that denoising produces consistent results across runs")


def pearl_gemm_noisy_with_zero_noise(a, b, scale_a, scale_b, out_dtype):
    """
    Modified version of pearl_gemm_noisy that uses zero noise matrices.
    This follows the pattern: noise_B first, then noisy_gemm.
    Returns intermediate tensors for validation.
    """
    assert out_dtype is torch.bfloat16 or out_dtype is torch.float16
    assert get_async_manager() is not None

    m = a.shape[0]
    n = b.shape[0]
    k = a.shape[1]
    r = MinerSettings().noise_rank

    A = a
    B = b
    A_scales = scale_a
    B_scales = scale_b
    C = torch.empty((m, n), dtype=out_dtype, device=a.device)

    # Create zero noise factors instead of random ones
    E_AL = torch.zeros((m, r), dtype=torch.int8, device=a.device)
    E_AL_fp16 = torch.zeros((m, r), dtype=torch.float16, device=a.device)
    EAR_R_major = torch.zeros((k, r), dtype=torch.int8, device=a.device)
    EBL_R_major = torch.zeros((k, r), dtype=torch.int8, device=a.device)
    EAR_K_major = torch.zeros((r, k), dtype=torch.int8, device=a.device)
    EBL_K_major = torch.zeros((r, k), dtype=torch.int8, device=a.device)
    E_BR = torch.zeros((n, r), dtype=torch.int8, device=a.device)
    E_BR_fp16 = torch.zeros((n, r), dtype=torch.float16, device=a.device)

    # Create noising tensors - fastest allocation
    ApEA = torch.zeros((m, k), dtype=torch.int8, device=a.device)
    BpEB = torch.zeros((n, k), dtype=torch.int8, device=a.device)
    A_E_BL = torch.zeros((m, r), dtype=torch.float16, device=a.device)
    EAR_BpEB = torch.zeros((n, r), dtype=torch.float16, device=a.device)

    # Create misc tensors - fastest allocation
    host_signal_header_size = get_host_signal_header_size()
    host_signal_header_pinned = torch.zeros(
        (host_signal_header_size,), dtype=torch.int8, device="cuda"
    )
    host_signal_sync_size = get_host_signal_sync_size()
    host_signal_sync = torch.zeros((host_signal_sync_size,), dtype=torch.int8, device="cuda")

    # First, call noise_B
    # torch.ops.pearl_gemm.noise_B(
    #     B,           # Input matrix B (n x k)
    #     E_AR,        # Zero noise factor E_AR (k x r)
    #     E_BL,        # Zero noise factor E_BL (k x r)
    #     E_BR,        # Zero noise factor E_BR (n x r)
    #     EAR_BpEB,    # Output tensor for EAR * BpEB (n x r)
    #     BpEB,        # Output tensor for B + EB (n x k)
    #     B_scales,    # Scale factors for B
    #     B_sums,      # Sum tensors for B
    #     128,          # tile_size_n
    # )

    # Then, call noisy_gemm (which handles noise_A internally)
    noisy_gemm(
        A=A,  # Input matrix A (m x k)
        B=B,  # Input matrix B (n x k)
        EAL=E_AL,  # Zero noise factor E_AL (m x r)
        EAL_fp16=E_AL_fp16,
        EBR=E_BR,  # Zero noise factor E_BR (n x r)
        EBR_fp16=E_BR_fp16,
        EAR_R_major=EAR_R_major,
        EBL_R_major=EBL_R_major,
        EAR_K_major=EAR_K_major,
        EBL_K_major=EBL_K_major,
        AxEBL_fp16=A_E_BL,  # Intermediate tensor A * E_BL (m x r)
        EARxBpEB_fp16=EAR_BpEB,  # Output tensor for EAR * BpEB (n x r)
        ApEA=ApEA,  # Output tensor for A + EA (m x k)
        BpEB=BpEB,  # Output tensor for B + EB (n x k)
        A_scales=A_scales,  # Scale factors for A
        B_scales=B_scales,  # Scale factors for B
        C=C,  # Output matrix C (m x n)
        host_signal_header_pinned=host_signal_header_pinned,
        host_signal_sync=host_signal_sync,
        pow_target=make_pow_target_tensor(0),
        pow_key=torch.randint(0, 255, (32,), dtype=torch.uint8, device="cuda").view(torch.uint32),
        tile_size_m=128,  # tile_size_m
        tile_size_n=256,  # tile_size_n
        tile_size_k=128,  # tile_size_k
        tile_size_m_noising_A=None,  # tile_size_noising_a
        tile_size_n_noising_B=None,  # tile_size_noising_b
        pipeline_stages=3,  # stages
        swizzle=None,  # swizzle
        run_noising_B=True,  # run_noising_B (already done above)
        skip_reduction=False,  # skip_reduction
        skip_denoising=False,  # skip_denoising
    )
    torch.cuda.synchronize()

    # Clean up
    del host_signal_header_pinned
    del host_signal_sync
    del A_E_BL
    del EAR_BpEB
    del E_AL
    del E_BR

    return C, ApEA, BpEB


@pytest.mark.flaky(reruns=3)
def test_pearl_gemm_noisy_with_controlled_noise(make_random_test_matrices):
    """Test that pearl_gemm_noisy produces consistent results with controlled noise."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    # Use K divisible by 64 to avoid "K must be divisible by bK" error
    m, n, k = 8192, 6144, 4096

    a, b, scale_a, scale_b, out_dtype, _ = make_random_test_matrices(m, n, k)

    # Test with zero noise (our custom function)
    print("Testing with zero noise...")
    output_with_zero_noise, ApEA_zero, BpEB_zero = pearl_gemm_noisy_with_zero_noise(
        a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype
    )

    # Test with real noise using the actual pearl_gemm_noisy function
    print("Testing with real noise...")
    output_with_real_noise = pearl_gemm_noisy(a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype)

    # Basic validation
    assert output_with_zero_noise.shape == (m, n)
    assert output_with_zero_noise.dtype == out_dtype
    assert output_with_real_noise.shape == (m, n)
    assert output_with_real_noise.dtype == out_dtype

    # Compare statistics
    print("\nComparison between zero noise and real noise:")
    print(
        f"Zero noise output range: "
        f"[{output_with_zero_noise.min().item():.6f}, "
        f"{output_with_zero_noise.max().item():.6f}]"
    )
    print(f"Zero noise output mean: {output_with_zero_noise.mean().item():.6f}")
    print(f"Zero noise output std: {output_with_zero_noise.std().item():.6f}")
    print(
        f"Real noise output range: "
        f"[{output_with_real_noise.min().item():.6f}, "
        f"{output_with_real_noise.max().item():.6f}]"
    )
    print(f"Real noise output mean: {output_with_real_noise.mean().item():.6f}")
    print(f"Real noise output std: {output_with_real_noise.std().item():.6f}")

    # Check if real noise output is constant (this would indicate a bug)
    if output_with_real_noise.std().item() < 1e-6:
        print(f"❌ ERROR: Real noise output is constant: {output_with_real_noise[0, 0].item()}")
        print("This indicates a bug in the pearl_gemm_noisy function")
        raise AssertionError("Real noise should not produce constant output")

    # Check if zero noise output is constant (this would also indicate a bug)
    if output_with_zero_noise.std().item() < 1e-6:
        print(f"❌ ERROR: Zero noise output is constant: {output_with_zero_noise[0, 0].item()}")
        print("This indicates a bug in the zero noise implementation")
        raise AssertionError("Zero noise should not produce constant output")

    # Compare with vanilla GEMM (all should match vanilla)
    print("\nComparing with vanilla GEMM:")
    vanilla_output = pearl_gemm_vanilla(a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype)

    zero_vs_vanilla_diff = torch.abs(output_with_zero_noise - vanilla_output).max().item()
    real_vs_vanilla_diff = torch.abs(output_with_real_noise - vanilla_output).max().item()

    print(
        f"Zero noise vs vanilla max,mean diff: "
        f"{zero_vs_vanilla_diff:.6f}, "
        f"{torch.abs(output_with_zero_noise - vanilla_output).mean().item():.6f}"
    )
    print(
        f"Real noise vs vanilla max,mean diff: "
        f"{real_vs_vanilla_diff:.6f}, "
        f"{torch.abs(output_with_real_noise - vanilla_output).mean().item():.6f}"
    )

    # All should be very close to vanilla (since noisy_gemm includes denoising)
    assert zero_vs_vanilla_diff < 1e-2, "Zero noise should match vanilla GEMM closely"
    assert real_vs_vanilla_diff < 1e-2, "Real noise should match vanilla GEMM closely"
    print("✓ Confirmed that both implementations match vanilla GEMM")

    # Test multiple runs to see noise variation
    print("\nTesting noise variation across multiple runs:")
    outputs = []
    for _i in range(5):
        output = pearl_gemm_noisy(a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype)
        outputs.append(output)

    # Stack outputs to compute statistics across runs
    outputs_tensor = torch.stack(outputs)
    output_std_across_runs = outputs_tensor.std(dim=0).mean().item()
    output_range_across_runs = (outputs_tensor.max() - outputs_tensor.min()).item()

    print(f"Mean std across runs: {output_std_across_runs:.6f}")
    print(f"Total range across runs: {output_range_across_runs:.6f}")

    # There should be minimal variation across runs since denoising should
    # produce consistent results
    # (The noise is random but the denoising should compensate for it)
    if output_std_across_runs > 1e-2:
        print(
            "⚠️  WARNING: High variation across runs - this might indicate denoising is not working correctly"
        )
    else:
        print("✓ Confirmed that denoising produces consistent results across runs")
