import torch
from pearl_gemm.quantization import quantize
from vllm.model_executor.layers.fused_moe.utils import moe_kernel_quantize_input

MAX_VAL_7BIT = 63
MAX_VAL_8BIT = 127
# block_size == 0 leaves the input unchanged; > 0 fuses a Hadamard transform of that width.
NO_HADAMARD_BLOCK_SIZE = 0

_SymmetricQuantResult = tuple[torch.Tensor, torch.Tensor, None]


def _normalize_smooth_scale_for_cute(
    smooth_scale: torch.Tensor | None,
    *,
    device: torch.device,
    num_tokens: int,
    hidden_dim: int,
) -> torch.Tensor | None:
    """CuTe quantize expects ``smooth_scale`` shaped (1, N) broadcast or (M, N) per-token, float32."""
    if smooth_scale is None:
        return None
    s = smooth_scale.to(dtype=torch.float32, device=device, copy=False)
    if s.ndim == 1:
        if s.shape[0] != hidden_dim:
            raise ValueError(f"smooth_scale length {s.shape[0]} != hidden dim {hidden_dim}")
        return s.unsqueeze(0)
    if s.ndim == 2 and s.shape[1] == hidden_dim and s.shape[0] in (1, num_tokens):
        return s
    raise ValueError(
        f"smooth_scale shape {tuple(s.shape)} not compatible with input shape "
        f"({num_tokens}, {hidden_dim}); expected (1, {hidden_dim}) or ({num_tokens}, {hidden_dim})"
    )


def quantize_kernel(
    x: torch.Tensor,
    max_val: int = MAX_VAL_7BIT,
    smooth_scale: torch.Tensor | None = None,
    block_size: int = NO_HADAMARD_BLOCK_SIZE,
) -> _SymmetricQuantResult:
    """Symmetric per-token quantization with optional smooth scaling (CuTe DSL kernel).

    Args:
        x: Input tensor, 2-D, contiguous, CUDA, ``float16`` or ``bfloat16``.
        max_val: Maximum quantization value (63 for 7-bit, 127 for 8-bit).
        smooth_scale: Optional per-column scale; multiplied elementwise with activations.
        block_size: When > 0, fuses a block-diagonal Hadamard transform of this width
            before scaling/quantization. Must be a power of two. 0 disables the transform.

    Returns:
        Quantized int8 tensor, per-token fp32 scales, and ``None`` (unused zero point
        for symmetric quantization).
    """
    num_tokens, hidden_dim = x.shape
    x_q = torch.empty_like(x, dtype=torch.int8)
    x_s = torch.empty((num_tokens, 1), dtype=torch.float32, device=x.device)
    smooth = _normalize_smooth_scale_for_cute(
        smooth_scale, device=x.device, num_tokens=num_tokens, hidden_dim=hidden_dim
    )
    quantize(
        x,
        x_q,
        x_s,
        smooth_scale=smooth,
        max_val=max_val,
        block_size=block_size,
    )
    return x_q, x_s, None


def quant_7bit(
    x: torch.Tensor,
    smooth_scale: torch.Tensor | None = None,
    block_size: int = NO_HADAMARD_BLOCK_SIZE,
) -> _SymmetricQuantResult:
    return quantize_kernel(
        x, max_val=MAX_VAL_7BIT, smooth_scale=smooth_scale, block_size=block_size
    )


def quant_8bit(
    x: torch.Tensor,
    smooth_scale: torch.Tensor | None = None,
    block_size: int = NO_HADAMARD_BLOCK_SIZE,
) -> _SymmetricQuantResult:
    return quantize_kernel(
        x, max_val=MAX_VAL_8BIT, smooth_scale=smooth_scale, block_size=block_size
    )


def quant_fp8_block(x: torch.Tensor, group_size: int) -> tuple[torch.Tensor, torch.Tensor]:
    """Dynamic per-token-group fp8 quantization for the block-scaled GEMM2."""
    return moe_kernel_quantize_input(
        A=x,
        A_scale=None,
        quant_dtype=torch.float8_e4m3fn,
        per_act_token_quant=False,
        block_shape=[group_size, group_size],
    )
