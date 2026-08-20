import pytest
import torch
from vllm import _custom_ops as vllm_ops
from vllm_miner.gemm_operators import pearl_gemm_vanilla


@pytest.mark.parametrize("m, n, k", [(1024, 1024, 1024), (8192, 8192, 8192)])
def test_pearl_gemm_vanilla_correctness(make_random_test_matrices, m, n, k):
    """
    Tests that pearl_gemm_vanilla runs without crashing and produces output
    of the expected shape.
    """
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    a, b, scale_a, scale_b, out_dtype, bias = make_random_test_matrices(m, n, k)

    output = pearl_gemm_vanilla(a, b, scale_a.squeeze(), scale_b.squeeze(), out_dtype)

    ref_output = vllm_ops.cutlass_scaled_mm(
        a, b.T, scale_a=scale_a, scale_b=scale_b, out_dtype=out_dtype, bias=bias
    )
    torch.cuda.synchronize()

    assert output.shape == (m, n)
    assert output.dtype == out_dtype
    assert ref_output.shape == (m, n)
    assert ref_output.dtype == out_dtype
    assert torch.allclose(output, ref_output, atol=1e-2, rtol=1e-2)
