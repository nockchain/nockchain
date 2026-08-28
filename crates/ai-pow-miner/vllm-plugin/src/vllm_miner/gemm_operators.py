import torch
from pearl_gemm import gemm
from vllm.utils.torch_utils import direct_register_custom_op

from .config import config


def _pearl_int7_gemm_impl(
    activation: torch.Tensor,
    weight: torch.Tensor,
    scale_a: torch.Tensor,
    scale_b: torch.Tensor,
    out_dtype: torch.dtype,
) -> torch.Tensor:
    output = torch.empty(
        (activation.shape[0], weight.shape[0]),
        dtype=out_dtype,
        device=activation.device,
    )
    gemm(
        A=activation,
        B=weight,
        A_scales=scale_a,
        B_scales=scale_b,
        C=output,
        tile_size_m=config.settings.tile_size_m,
        tile_size_n=config.settings.tile_size_n,
        tile_size_k=config.settings.tile_size_k,
    )
    return output


def _pearl_int7_gemm_fake(
    activation: torch.Tensor,
    weight: torch.Tensor,
    scale_a: torch.Tensor,
    scale_b: torch.Tensor,
    out_dtype: torch.dtype,
) -> torch.Tensor:
    del scale_a, scale_b
    return torch.empty(
        (activation.shape[0], weight.shape[0]),
        dtype=out_dtype,
        device=activation.device,
    )


direct_register_custom_op(
    "pearl_int7_gemm",
    _pearl_int7_gemm_impl,
    fake_impl=_pearl_int7_gemm_fake,
)


def _sm12_int7_gemm_impl(
    activation: torch.Tensor,
    weight_transposed: torch.Tensor,
    scale_a: torch.Tensor,
    scale_b: torch.Tensor,
    out_dtype: torch.dtype,
) -> torch.Tensor:
    logical_m = activation.shape[0]
    if logical_m <= 16:
        padded = torch.zeros(
            (32, activation.shape[1]),
            device=activation.device,
            dtype=activation.dtype,
        )
        padded[:logical_m].copy_(activation)
        accumulator = torch._int_mm(padded, weight_transposed)[:logical_m]
    else:
        accumulator = torch._int_mm(activation, weight_transposed)
    return (accumulator.float() * scale_a.reshape(-1, 1) * scale_b.reshape(1, -1)).to(
        out_dtype
    )


def _sm12_int7_gemm_fake(
    activation: torch.Tensor,
    weight_transposed: torch.Tensor,
    scale_a: torch.Tensor,
    scale_b: torch.Tensor,
    out_dtype: torch.dtype,
) -> torch.Tensor:
    del scale_a, scale_b
    return torch.empty(
        (activation.shape[0], weight_transposed.shape[1]),
        dtype=out_dtype,
        device=activation.device,
    )


direct_register_custom_op(
    "pearl_sm12_int7_gemm",
    _sm12_int7_gemm_impl,
    fake_impl=_sm12_int7_gemm_fake,
)


def pearl_gemm_vanilla(
    A: torch.Tensor,
    B: torch.Tensor,
    scale_a: torch.Tensor,
    scale_b: torch.Tensor,
    out_dtype: torch.dtype,
    *,
    weight_transposed: bool = False,
) -> torch.Tensor:
    """
    Performs standard quantized matrix multiplication without mining operations.

    Computes C = A @ B.T using optimized CUDA kernels for int8 quantized inputs.

    :param A: Input matrix A (int8, quantized)
    :param B: Input matrix B, or its cached transpose when weight_transposed is true
    :param scale_a: Quantization scale factors for matrix A
    :param scale_b: Quantization scale factors for matrix B
    :param out_dtype: Output data type (bfloat16 or float16)
    :param weight_transposed: Whether B already has (K, N) layout
    :return: Result matrix C
    """
    assert out_dtype is torch.bfloat16 or out_dtype is torch.float16
    if torch.cuda.get_device_capability(A.device)[0] == 12:
        weight_for_mm = B if weight_transposed else B.T.contiguous()
        return torch.ops.vllm.pearl_sm12_int7_gemm(
            A,
            weight_for_mm,
            scale_a,
            scale_b,
            out_dtype,
        )

    return torch.ops.vllm.pearl_int7_gemm(
        A,
        B,
        scale_a,
        scale_b,
        out_dtype,
    )
