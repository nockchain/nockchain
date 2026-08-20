from dataclasses import dataclass

import torch
from miner_utils import get_logger
from pearl_gateway.comm.dataclasses import (
    CommitmentHash,
    MiningJob,
    MoEBlockInfo,
    OpenedBlockInfo,
)
from pearl_gemm import HostSignalStatus, extract_indices, get_host_signal_header

from .config import config
from .mining_state import (
    get_pinned_pool,
)

_LOGGER = get_logger("vllm.pearl_miner")


@dataclass
class StatusCheckCallback:
    """Note: all tensors are ptrs to GPU memory, except host_signal_header_pinned"""

    host_signal_header_pinned: torch.Tensor
    commitment_hash_A_tensor: torch.Tensor
    commitment_hash_B_tensor: torch.Tensor
    A: torch.Tensor
    B: torch.Tensor
    mining_job: MiningJob

    def __call__(self, handle_submit_block):
        header = get_host_signal_header(self.host_signal_header_pinned)

        if header.status == HostSignalStatus.kSignalTriggered:
            _LOGGER.info(f"Block found! {header=}")

            indices = extract_indices(header)

            commitment_hash = CommitmentHash(
                noise_seed_A=self.commitment_hash_A_tensor.cpu().numpy().tobytes(),
                noise_seed_B=self.commitment_hash_B_tensor.cpu().numpy().tobytes(),
            )

            opened_block_info = OpenedBlockInfo(
                A_row_indices=indices.A_row_indices,
                B_column_indices=indices.B_column_indices,
                # .cpu() creates a copy and no need for clone()
                A=self.A.cpu().detach(),
                # GPU holds B transposed
                B_t=self.B.cpu().detach(),
                commitment_hash=commitment_hash,
                noise_rank=config.settings.noise_rank,
            )

            handle_submit_block(opened_block_info, self.mining_job)

        get_pinned_pool().release(self.host_signal_header_pinned)
        self.host_signal_header_pinned = None
        del self.commitment_hash_A_tensor
        del self.commitment_hash_B_tensor
        del self.A
        del self.B
        del header


@dataclass
class MoEStatusCheckCallback:
    """Check PoW results across all MoE expert headers for one forward pass.

    Headers are acquired from the global pinned pool in ``PearlMoEExperts.apply``
    (one per expert) and released back here in the ``finally`` block. On a hit,
    the expert-local (inner) row indices are mapped to global outer token indices
    via ``routing_data`` so the proof can be built against the full activation.
    The routing hash tensor is retained for lifetime symmetry with the scheduled
    GPU work even though the CPU callback does not inspect it directly.
    """

    pow_headers: list[torch.Tensor]
    commitment_hash_A_tensor: torch.Tensor
    commitment_hash_B_tensor: torch.Tensor
    A_q: torch.Tensor  # (m, k) int8, original token order (non-noised)
    B_stacked: torch.Tensor  # (E*N, k) int8 (non-noised)
    routing_data: torch.Tensor  # (m*top_k,) int32, expert-sorted token indices
    routing_hash: torch.Tensor  # (32,) uint8
    mining_job: MiningJob
    noise_rank: int
    num_experts: int
    n_per_expert: int
    top_k: int
    routing_offsets: list[int]  # (E,) exclusive ends per expert (last == m*top_k)

    def __call__(self, handle_submit_block) -> None:
        """Submit the first triggered expert block and release all pinned headers."""
        try:
            for expert_index in range(self.num_experts):
                header = get_host_signal_header(self.pow_headers[expert_index])
                if header.status != HostSignalStatus.kSignalTriggered:
                    continue

                indices = extract_indices(header)
                expert_routing_start = (
                    0 if expert_index == 0 else self.routing_offsets[expert_index - 1]
                )
                outer_indices = [
                    int(self.routing_data[expert_routing_start + inner_row].item())
                    for inner_row in indices.A_row_indices
                ]
                expert_weight_col_offset = expert_index * self.n_per_expert

                _LOGGER.info(
                    "MoE block found! expert={}, inner_rows={}, outer_indices={}, b_cols={}",
                    expert_index,
                    indices.A_row_indices,
                    outer_indices,
                    indices.B_column_indices,
                )

                commitment_hash = CommitmentHash(
                    noise_seed_A=self.commitment_hash_A_tensor.cpu().numpy().tobytes(),
                    noise_seed_B=self.commitment_hash_B_tensor.cpu().numpy().tobytes(),
                )

                expert_routing_offsets = list(self.routing_offsets)

                moe_block_info = MoEBlockInfo(
                    expert_index=expert_index,
                    num_experts=self.num_experts,
                    n_per_expert=self.n_per_expert,
                    top_k=self.top_k,
                    inner_a_rows=list(indices.A_row_indices),
                    inner_b_cols=list(indices.B_column_indices),
                    routing_data=self.routing_data.cpu(),
                    expert_routing_offsets=expert_routing_offsets,
                )

                opened_block_info = OpenedBlockInfo(
                    A_row_indices=outer_indices,
                    B_column_indices=[
                        expert_weight_col_offset + inner_col
                        for inner_col in indices.B_column_indices
                    ],
                    A=self.A_q.cpu().detach(),
                    B_t=self.B_stacked.cpu().detach(),
                    commitment_hash=commitment_hash,
                    noise_rank=self.noise_rank,
                    moe=moe_block_info,
                )

                handle_submit_block(opened_block_info, self.mining_job)
                break
        finally:
            for header_tensor in self.pow_headers:
                get_pinned_pool().release(header_tensor)
            self.pow_headers = []
            del self.commitment_hash_A_tensor
            del self.commitment_hash_B_tensor
            del self.A_q
            del self.B_stacked
            del self.routing_data
            del self.routing_hash
            del self.routing_offsets
