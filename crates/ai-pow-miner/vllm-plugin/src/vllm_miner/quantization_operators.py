import torch
from pearl_gemm.quantization import quantize
from vllm.model_executor.layers.fused_moe.utils import moe_kernel_quantize_input
from vllm.utils.torch_utils import direct_register_custom_op

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
            raise ValueError(
                f"smooth_scale length {s.shape[0]} != hidden dim {hidden_dim}"
            )
        return s.unsqueeze(0)
    if s.ndim == 2 and s.shape[1] == hidden_dim and s.shape[0] in (1, num_tokens):
        return s
    raise ValueError(
        f"smooth_scale shape {tuple(s.shape)} not compatible with input shape "
        f"({num_tokens}, {hidden_dim}); expected (1, {hidden_dim}) or ({num_tokens}, {hidden_dim})"
    )


def _quantize_symmetric_impl(
    x: torch.Tensor,
    smooth_scale: torch.Tensor | None,
    max_val: int,
    block_size: int,
) -> tuple[torch.Tensor, torch.Tensor]:
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
    return x_q, x_s


def _quantize_symmetric_fake(
    x: torch.Tensor,
    smooth_scale: torch.Tensor | None,
    max_val: int,
    block_size: int,
) -> tuple[torch.Tensor, torch.Tensor]:
    del smooth_scale, max_val, block_size
    return (
        torch.empty_like(x, dtype=torch.int8),
        torch.empty((x.shape[0], 1), dtype=torch.float32, device=x.device),
    )


direct_register_custom_op(
    "pearl_quantize_symmetric",
    _quantize_symmetric_impl,
    fake_impl=_quantize_symmetric_fake,
)


def quantize_kernel(
    x: torch.Tensor,
    max_val: int = MAX_VAL_7BIT,
    smooth_scale: torch.Tensor | None = None,
    block_size: int = NO_HADAMARD_BLOCK_SIZE,
) -> _SymmetricQuantResult:
    """Symmetric per-token quantization with optional smooth scaling.

    The custom-operation boundary keeps CuTe compilation and cache lookup out
    of the Torch graph while CUDA graph capture records the compiled kernel.
    """
    x_q, x_s = torch.ops.vllm.pearl_quantize_symmetric(
        x, smooth_scale, max_val, block_size
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


def quant_fp8_block(
    x: torch.Tensor, group_size: int
) -> tuple[torch.Tensor, torch.Tensor]:
    """Dynamic per-token-group fp8 quantization for the block-scaled GEMM2."""
    return moe_kernel_quantize_input(
        A=x,
        A_scale=None,
        quant_dtype=torch.float8_e4m3fn,
        per_act_token_quant=False,
        block_shape=[group_size, group_size],
    )
