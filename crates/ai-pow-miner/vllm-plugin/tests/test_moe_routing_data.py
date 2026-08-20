import itertools

import pytest
import torch
from moe_testing_helpers import (
    RoutingSkew,
    make_routing_distribution,
    reference_routing_layout,
)
from pearl_gemm.moe import (
    build_routing_data,
    get_build_routing_data_scratchpad_bytes,
)
from vllm_miner.moe_gemm_operators import MoERoutingLayout

pytestmark = [pytest.mark.gpu, pytest.mark.moe]

CUDA_DEVICE = "cuda"

TOKEN_COUNTS = [1, 16, 128, 1024]
TOP_K_VALUES = [1, 2, 4, 8]
EXPERT_COUNTS = [8, 128]
_ROUTING_SHAPE_CASES = list(itertools.product(TOKEN_COUNTS, TOP_K_VALUES, EXPERT_COUNTS))


def _run_build_routing_data(
    topk_ids: torch.Tensor, num_experts: int
) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor]:
    numel = topk_ids.numel()
    routing_data = torch.empty(numel, dtype=torch.int32, device=topk_ids.device)
    slot_indices = torch.empty(numel, dtype=torch.int32, device=topk_ids.device)
    routing_offsets = torch.empty(num_experts, dtype=torch.int32, device=topk_ids.device)
    scratchpad = torch.empty(
        get_build_routing_data_scratchpad_bytes(numel),
        dtype=torch.uint8,
        device=topk_ids.device,
    )
    build_routing_data(
        topk_ids, routing_data, slot_indices, routing_offsets, scratchpad, num_experts
    )
    return routing_data, slot_indices, routing_offsets


def _reference_slot_indices(topk_ids: torch.Tensor) -> torch.Tensor:
    """Stable argsort of flattened slots by expert; the canonical slot ordering."""
    num_tokens, top_k = topk_ids.shape
    total_slots = num_tokens * top_k
    flat_experts = topk_ids.reshape(-1)
    flat_slot_indices = torch.arange(total_slots, device=topk_ids.device, dtype=torch.int64)
    sort_key = flat_experts.to(torch.int64) * total_slots + flat_slot_indices
    return torch.argsort(sort_key, stable=True).to(torch.int32)


def _reference_routing_offsets(topk_ids: torch.Tensor, num_experts: int) -> torch.Tensor:
    expert_counts = torch.bincount(topk_ids.reshape(-1), minlength=num_experts)
    return expert_counts.cumsum(0).to(torch.int32)


def _reference_routing_data(topk_ids: torch.Tensor) -> torch.Tensor:
    """Same as ``build_routing_data``: one flat (m*K,) vector of token indices.

    Semantics: for each expert ``e`` in order ``0..E-1``, list every token
    index that appears in a top-k slot assigned to ``e``, sorted by token index
    (ties broken consistently with stable sort on ``expert*m + token`` over
    flattened slots). The return value is the concatenation of those per-expert
    lists - not a nested structure.
    """
    num_tokens, top_k = topk_ids.shape
    flat_experts = topk_ids.view(-1)
    flat_tokens = torch.arange(
        num_tokens, device=topk_ids.device, dtype=torch.int32
    ).repeat_interleave(top_k)
    sort_key = flat_experts.to(torch.int64) * num_tokens + flat_tokens.to(torch.int64)
    sort_order = torch.argsort(sort_key, stable=True)
    return flat_tokens[sort_order]


def _assert_matches_reference(
    topk_ids: torch.Tensor,
    num_experts: int,
    expected_shape: tuple[int, ...] | None = None,
) -> None:
    """Run the kernel and assert it equals the reference (and optional shape)."""
    result, slot_indices, offsets = _run_build_routing_data(topk_ids, num_experts)
    if expected_shape is not None:
        assert result.shape == expected_shape
    expected = _reference_routing_data(topk_ids)
    assert (result == expected).all(), "Output diverged from reference argsort"
    assert (slot_indices == _reference_slot_indices(topk_ids)).all(), (
        "slot_indices diverged from reference sort order"
    )
    _assert_offsets_match_reference(topk_ids, num_experts, offsets)


def _assert_offsets_match_reference(
    topk_ids: torch.Tensor, num_experts: int, offsets: torch.Tensor
) -> None:
    """Assert kernel offsets equal bincount+cumsum and satisfy structural invariants."""
    expected = _reference_routing_offsets(topk_ids, num_experts)
    assert offsets.shape == (num_experts,)
    assert offsets.dtype == torch.int32
    assert (offsets == expected).all(), "routing_offsets diverged from bincount+cumsum"
    assert int(offsets[-1].item()) == topk_ids.numel(), "last offset must equal m*K"
    assert (offsets[1:] >= offsets[:-1]).all(), "offsets must be non-decreasing"


class TestBuildRoutingDataBasic:
    def test_small_known_good(self):
        _SMALL_M = 10
        _SMALL_K = 2
        _SMALL_E = 4
        _SMALL_FIXED_TOPK_IDS = (
            (0, 1),
            (1, 0),
            (2, 3),
            (0, 2),
            (1, 3),
            (1, 2),
            (2, 3),
            (0, 3),
            (1, 2),
            (2, 1),
        )
        _SMALL_PER_EXPERT_TOKEN_GROUPS = (
            (0, 1, 3, 7),
            (0, 1, 4, 5, 8, 9),
            (2, 3, 5, 6, 8, 9),
            (2, 4, 6, 7),
        )
        _SMALL_KNOWN_GOOD_TOKEN_ORDER = tuple(
            t for group in _SMALL_PER_EXPERT_TOKEN_GROUPS for t in group
        )

        topk_ids = torch.tensor(_SMALL_FIXED_TOPK_IDS, dtype=torch.int32, device=CUDA_DEVICE)
        expected = torch.tensor(
            _SMALL_KNOWN_GOOD_TOKEN_ORDER, dtype=torch.int32, device=CUDA_DEVICE
        )
        result, _slots, offsets = _run_build_routing_data(topk_ids, _SMALL_E)
        assert result.shape == (_SMALL_M * _SMALL_K,)
        assert (result == expected).all()
        _assert_offsets_match_reference(topk_ids, _SMALL_E, offsets)

    @pytest.mark.parametrize("skew", list(RoutingSkew))
    @pytest.mark.parametrize("num_tokens,top_k,num_experts", _ROUTING_SHAPE_CASES)
    def test_matches_reference(self, num_tokens, top_k, num_experts, skew):
        topk_ids = make_routing_distribution(
            num_tokens, num_experts, top_k, device=CUDA_DEVICE, skew=skew
        )
        _assert_matches_reference(topk_ids, num_experts, expected_shape=(num_tokens * top_k,))


class TestBuildRoutingDataEdgeCases:
    @pytest.mark.parametrize("num_experts", EXPERT_COUNTS)
    @pytest.mark.parametrize("top_k", [1, 2, 4])
    def test_empty_experts(self, num_experts, top_k):
        """When some expert IDs never appear, output is still valid."""
        num_tokens = 32
        max_expert_used = min(3, num_experts - 1)
        topk_ids = torch.randint(
            0,
            max_expert_used + 1,
            (num_tokens, top_k),
            dtype=torch.int32,
            device=CUDA_DEVICE,
        )
        _assert_matches_reference(topk_ids, num_experts)

    @pytest.mark.parametrize("top_k", TOP_K_VALUES)
    @pytest.mark.parametrize("num_experts", EXPERT_COUNTS)
    def test_single_token(self, top_k, num_experts):
        """m=1 must work correctly."""
        topk_ids = torch.randint(0, num_experts, (1, top_k), dtype=torch.int32, device=CUDA_DEVICE)
        _assert_matches_reference(topk_ids, num_experts, expected_shape=(top_k,))


def _kernel_layout(topk_ids: torch.Tensor, num_experts: int) -> MoERoutingLayout:
    routing_data, slot_indices, routing_offsets = _run_build_routing_data(topk_ids, num_experts)
    return MoERoutingLayout.from_kernel_outputs(
        routing_data, slot_indices, routing_offsets, num_experts, topk_ids.shape[1]
    )


def _assert_layouts_equal(
    kernel_layout: MoERoutingLayout,
    reference_layout: MoERoutingLayout,
    num_experts: int,
) -> None:
    """Assert the kernel layout matches the reference and is internally well-formed.

    Beyond equality, this checks the construction invariant that makes downstream
    indexing OOB-free: the exclusive-end offsets are exactly ``num_experts``
    contiguous segments that tile ``[0, total_routed_slots)`` with no gaps or
    overlaps. Given that, ``expert_start + inner_row`` (with ``inner_row < count``)
    and ``expert_routing_offsets[expert_index - 1]`` can never exceed the range.
    """
    assert (kernel_layout.token_indices == reference_layout.token_indices).all()
    assert (kernel_layout.slot_indices == reference_layout.slot_indices).all()
    assert (kernel_layout.routing_offsets == reference_layout.routing_offsets).all()
    assert kernel_layout.routing_offsets_host == reference_layout.routing_offsets_host
    assert kernel_layout.routing_offsets.shape == (num_experts,)
    assert len(kernel_layout.routing_offsets_host) == num_experts

    total_routed_slots = kernel_layout.token_indices.numel()
    next_expected_start = 0
    for expert_index in range(num_experts):
        kernel_slice = kernel_layout.expert_slice(expert_index)
        assert kernel_slice == reference_layout.expert_slice(expert_index)
        start, count = kernel_slice
        assert count >= 0, "segment count must be non-negative"
        assert start == next_expected_start, "segments must be contiguous (no gaps/overlaps)"
        assert start + count <= total_routed_slots, "segment must stay within the routing range"
        next_expected_start = start + count
    assert next_expected_start == total_routed_slots, (
        "segments must cover every routed slot exactly once"
    )


class TestRoutingLayout:
    # _ROUTING_SHAPE_CASES x RoutingSkew covers the edge scenarios: num_tokens=1
    # (single token) and ALL_TO_ONE (all tokens to expert 0, every other expert
    # empty -- exercising expert_slice on zero-count segments).
    @pytest.mark.parametrize("skew", list(RoutingSkew))
    @pytest.mark.parametrize("num_tokens,top_k,num_experts", _ROUTING_SHAPE_CASES)
    def test_from_kernel_matches_reference(self, num_tokens, top_k, num_experts, skew):
        topk_ids = make_routing_distribution(
            num_tokens, num_experts, top_k, device=CUDA_DEVICE, skew=skew
        )
        _assert_layouts_equal(
            _kernel_layout(topk_ids, num_experts),
            reference_routing_layout(topk_ids, num_experts),
            num_experts,
        )
