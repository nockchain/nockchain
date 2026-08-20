import pytest
import torch
from moe_testing_helpers import reference_routing_layout
from vllm_miner.callbacks import MoEStatusCheckCallback
from vllm_miner.mining_state import (
    delete_state,
    ensure_pinned_pool_at_least,
    get_pinned_pool,
    init_pinned_pool,
)
from vllm_miner.moe_gemm_operators import (
    MoERoutingLayout,
    permute_a_side_to_expert_order,
)

pytestmark = [pytest.mark.gpu, pytest.mark.moe]

_NUM_EXPERTS = 8
_HIDDEN_SIZE = 256
_INTERMEDIATE_SIZE = 128
_NUM_TOKENS = 32
_TOP_K = 2
_NOISE_RANK = 16


class TestPermuteASideToExpertOrder:
    @pytest.fixture
    def routing_layout(self) -> MoERoutingLayout:
        flat_expert_ids = torch.arange(_NUM_TOKENS * _TOP_K, device="cuda") % _NUM_EXPERTS
        topk_ids = flat_expert_ids.reshape(_NUM_TOKENS, _TOP_K).to(torch.int32)
        return reference_routing_layout(topk_ids, _NUM_EXPERTS)

    @pytest.fixture
    def a_side_tensors(self) -> dict[str, torch.Tensor]:
        device = "cuda"
        return {
            "A_q": torch.randn(_NUM_TOKENS, _HIDDEN_SIZE, dtype=torch.float32, device=device).to(
                torch.int8
            ),
            "A_scales": torch.rand(_NUM_TOKENS, 1, dtype=torch.float32, device=device),
            "EAL": torch.randn(_NUM_TOKENS, _NOISE_RANK, dtype=torch.float32, device=device).to(
                torch.int8
            ),
            "EAL_fp16": torch.randn(_NUM_TOKENS, _NOISE_RANK, dtype=torch.float16, device=device),
        }

    def test_output_shapes_and_contiguity(
        self,
        routing_layout: MoERoutingLayout,
        a_side_tensors: dict[str, torch.Tensor],
    ) -> None:
        num_routed_slots = _NUM_TOKENS * _TOP_K
        (
            A_q_by_expert,
            A_scales_by_expert,
            EAL_by_expert,
            EAL_fp16_by_expert,
        ) = permute_a_side_to_expert_order(
            routing_layout,
            a_side_tensors["A_q"],
            a_side_tensors["A_scales"],
            a_side_tensors["EAL"],
            a_side_tensors["EAL_fp16"],
        )
        assert A_q_by_expert.shape == (num_routed_slots, _HIDDEN_SIZE)
        assert A_scales_by_expert.shape == (num_routed_slots, 1)
        assert EAL_by_expert.shape == (num_routed_slots, _NOISE_RANK)
        assert EAL_fp16_by_expert.shape == (num_routed_slots, _NOISE_RANK)
        for tensor in (
            A_q_by_expert,
            A_scales_by_expert,
            EAL_by_expert,
            EAL_fp16_by_expert,
        ):
            assert tensor.is_contiguous()

    def test_permutation_follows_token_indices(
        self,
        routing_layout: MoERoutingLayout,
        a_side_tensors: dict[str, torch.Tensor],
    ) -> None:
        A_q_by_expert, _, _, _ = permute_a_side_to_expert_order(
            routing_layout,
            a_side_tensors["A_q"],
            a_side_tensors["A_scales"],
            a_side_tensors["EAL"],
            a_side_tensors["EAL_fp16"],
        )
        expected = a_side_tensors["A_q"][routing_layout.token_indices]
        assert torch.equal(A_q_by_expert, expected)


class TestMoEStatusCheckCallbackRelease:
    @pytest.fixture(autouse=True)
    def _pool(self):
        init_pinned_pool()
        ensure_pinned_pool_at_least(_NUM_EXPERTS * 2)
        yield
        delete_state()

    @staticmethod
    def _make_callback(headers: list[torch.Tensor]) -> MoEStatusCheckCallback:
        device = "cuda"
        return MoEStatusCheckCallback(
            pow_headers=headers,
            commitment_hash_A_tensor=torch.zeros(32, dtype=torch.uint8, device=device),
            commitment_hash_B_tensor=torch.zeros(32, dtype=torch.uint8, device=device),
            A_q=torch.zeros(1, 1, dtype=torch.int8, device=device),
            B_stacked=torch.zeros(1, 1, dtype=torch.int8, device=device),
            routing_data=torch.zeros(1, dtype=torch.int32, device=device),
            routing_hash=torch.zeros(32, dtype=torch.uint8, device=device),
            mining_job=None,
            noise_rank=_NOISE_RANK,
            num_experts=_NUM_EXPERTS,
            n_per_expert=_INTERMEDIATE_SIZE * 2,
            top_k=_TOP_K,
            routing_offsets=[0] * _NUM_EXPERTS,
        )

    def test_releases_all_headers_to_pool(self) -> None:
        pool = get_pinned_pool()
        headers = [pool.acquire() for _ in range(_NUM_EXPERTS)]
        assert len(pool._used_buffers) == _NUM_EXPERTS

        callback = self._make_callback(headers)
        # No expert triggered (zeroed headers): callback must still release them.
        callback(lambda opened_block_info, mining_job: None)
        assert len(pool._used_buffers) == 0
