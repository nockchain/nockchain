"""Shared MoE test helpers for vLLM miner tests."""

from dataclasses import dataclass
from enum import Enum, auto

import torch
from vllm_miner.moe_gemm_operators import MoERoutingLayout

MOE_TEST_INT8_LOW = -63
MOE_TEST_INT8_HIGH = 64
_MOE_TEST_WEIGHT_SCALE_DIVISOR = 64.0
# Down projection is fp8 e4m3 with [128, 128] block-wise scales.
_MOE_TEST_W2_DTYPE = torch.float8_e4m3fn
_MOE_TEST_W2_BLOCK = 128


class RoutingSkew(Enum):
    UNIFORM = auto()
    ZIPF = auto()
    ALL_TO_ONE = auto()


def reference_routing_layout(topk_ids: torch.Tensor, num_experts: int) -> MoERoutingLayout:
    """Reference layout via stable argsort."""
    num_tokens, top_k = topk_ids.shape
    total_slots = num_tokens * top_k
    flat_experts = topk_ids.reshape(-1)
    flat_slot_indices = torch.arange(total_slots, device=topk_ids.device, dtype=torch.int64)
    sort_key = flat_experts.to(torch.int64) * total_slots + flat_slot_indices
    sort_order = torch.argsort(sort_key, stable=True)

    slot_indices = sort_order.to(torch.int32)
    token_indices = (sort_order // top_k).to(torch.int32)
    routing_offsets = torch.bincount(flat_experts, minlength=num_experts).cumsum(0).to(torch.int32)

    return MoERoutingLayout.from_kernel_outputs(
        routing_data=token_indices,
        slot_indices=slot_indices,
        routing_offsets=routing_offsets,
        num_experts=num_experts,
        top_k=top_k,
    )


def make_routing_distribution(
    num_tokens: int,
    num_experts: int,
    top_k: int,
    skew: RoutingSkew = RoutingSkew.UNIFORM,
    device: str | torch.device = "cuda",
) -> torch.Tensor:
    """Generate topk_ids with configurable load imbalance.

    Args:
        num_tokens: number of tokens
        num_experts: number of experts
        top_k: top-K experts per token
        skew: one of ``RoutingSkew``
        device: torch device

    Returns:
        topk_ids: (num_tokens, top_k) int32 tensor of expert assignments
    """
    if skew is RoutingSkew.UNIFORM:
        return torch.randint(
            0,
            num_experts,
            (num_tokens, top_k),
            dtype=torch.int32,
            device=device,
        )

    if skew is RoutingSkew.ZIPF:
        weights = 1.0 / torch.arange(1, num_experts + 1, dtype=torch.float32)
        probs = weights / weights.sum()
        sampled_expert_ids = torch.multinomial(probs, num_tokens * top_k, replacement=True).to(
            torch.int32
        )
        return sampled_expert_ids.view(num_tokens, top_k).to(device)

    if skew is RoutingSkew.ALL_TO_ONE:
        return torch.zeros(num_tokens, top_k, dtype=torch.int32, device=device)

    raise ValueError(f"Unknown skew: {skew!r}")


@dataclass(slots=True)
class MoeTestTensors:
    """Synthetic MoE tensors from ``make_moe_tensors``."""

    w13_weight: torch.Tensor
    w2_weight: torch.Tensor
    w13_weight_scale: torch.Tensor
    w2_weight_scale: torch.Tensor
    hidden_states: torch.Tensor
    topk_ids: torch.Tensor
    topk_weights: torch.Tensor


def make_moe_tensors(
    num_experts: int,
    hidden_dim: int,
    intermediate_size: int,
    num_tokens: int,
    top_k: int,
    device: str | torch.device = "cuda",
    *,
    skew: RoutingSkew = RoutingSkew.UNIFORM,
) -> MoeTestTensors:
    """Generate synthetic MoE tensors for testing.

    Shapes and dtypes:
      - w13_weight: (num_experts, 2*intermediate_size, hidden_dim) int8
      - w2_weight: (num_experts, hidden_dim, intermediate_size) fp8 e4m3
      - w13_weight_scale: (num_experts, 2*intermediate_size, 1) float32
      - w2_weight_scale: (num_experts, ceil(hidden/128), ceil(intermediate/128)) float32
      - hidden_states: (num_tokens, hidden_dim) bf16
      - topk_ids: (num_tokens, top_k) int32
      - topk_weights: (num_tokens, top_k) float32 - normalized to sum to 1
      - skew: one of ``RoutingSkew``
    """
    w13_weight = torch.randint(
        MOE_TEST_INT8_LOW,
        MOE_TEST_INT8_HIGH,
        (num_experts, 2 * intermediate_size, hidden_dim),
        dtype=torch.int8,
        device=device,
    )
    weight_scale = 1.0 / _MOE_TEST_WEIGHT_SCALE_DIVISOR
    w13_weight_scale = (
        torch.ones(
            num_experts,
            2 * intermediate_size,
            1,
            dtype=torch.float32,
            device=device,
        )
        * weight_scale
    )

    # Down projection: fp8 e4m3 weights with [128, 128] block-wise scales.
    block = _MOE_TEST_W2_BLOCK
    n_tiles = (hidden_dim + block - 1) // block
    k_tiles = (intermediate_size + block - 1) // block
    w2_weight = (
        torch.randn(num_experts, hidden_dim, intermediate_size, device=device) * weight_scale
    ).to(_MOE_TEST_W2_DTYPE)
    w2_weight_scale = torch.ones(num_experts, n_tiles, k_tiles, dtype=torch.float32, device=device)

    hidden_states = torch.randn(num_tokens, hidden_dim, dtype=torch.bfloat16, device=device)

    topk_ids = make_routing_distribution(num_tokens, num_experts, top_k, skew=skew, device=device)

    raw_weights = torch.rand(num_tokens, top_k, dtype=torch.float32, device=device)
    topk_weights = raw_weights / raw_weights.sum(dim=1, keepdim=True)

    return MoeTestTensors(
        w13_weight=w13_weight,
        w2_weight=w2_weight,
        w13_weight_scale=w13_weight_scale,
        w2_weight_scale=w2_weight_scale,
        hidden_states=hidden_states,
        topk_ids=topk_ids,
        topk_weights=topk_weights,
    )
