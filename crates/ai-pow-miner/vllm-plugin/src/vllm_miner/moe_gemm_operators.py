from dataclasses import dataclass

import torch
from blake3 import blake3
from miner_base.commitment_hash import CommitmentHasher
from miner_base.gpu_matmul_config import GPUMatmulConfigFactory
from miner_utils import get_logger
from pearl_gateway.comm.dataclasses import MiningJob
from pearl_gateway.comm.mining_configuration import MoEConfig
from pearl_gemm import (
    commitment_hash_from_merkle_roots,
    get_required_scratchpad_bytes,
    make_pow_target_tensor,
    noisy_gemm,
    tensor_hash,
)
from pearl_gemm.moe import build_routing_data, get_build_routing_data_scratchpad_bytes

from .config import config
from .gemm_operators import generate_noise_factors
from .mining_state import get_async_manager

_LOGGER = get_logger("vllm.pearl_miner")


@dataclass(slots=True)
class MoERoutingLayout:
    """Canonical per-expert token ordering for GEMM1 and proof verification.

    Built directly from ``build_routing_data`` outputs; the index tensors stay
    int32 (kernel-native) and ``routing_offsets`` is the single source of truth
    for per-expert segment boundaries (exclusive ends, last == m*top_k).
    """

    token_indices: torch.Tensor  # (m*top_k,) int32 token IDs in routing order
    slot_indices: torch.Tensor  # (m*top_k,) int32 flat slot IDs in routing order
    routing_offsets: torch.Tensor  # (E,) int32 exclusive ends (last == m*top_k)
    routing_offsets_host: list[int]  # routing_offsets materialized on host
    num_experts: int
    top_k: int

    @classmethod
    def from_kernel_outputs(
        cls,
        routing_data: torch.Tensor,
        slot_indices: torch.Tensor,
        routing_offsets: torch.Tensor,
        num_experts: int,
        top_k: int,
    ) -> "MoERoutingLayout":
        """Build the layout from ``build_routing_data`` outputs (no re-sorting)."""
        return cls(
            token_indices=routing_data,
            slot_indices=slot_indices,
            routing_offsets=routing_offsets,
            routing_offsets_host=routing_offsets.tolist(),
            num_experts=num_experts,
            top_k=top_k,
        )

    def expert_slice(self, expert_idx: int) -> tuple[int, int]:
        """Return ``(start, count)`` for an expert's segment (no GPU sync)."""
        start = 0 if expert_idx == 0 else self.routing_offsets_host[expert_idx - 1]
        end = self.routing_offsets_host[expert_idx]
        return start, end - start


@dataclass
class MoENoiseContext:
    """Pre-computed tensors for one MoE forward pass (shared across experts)."""

    mining_job: MiningJob  # job whose header/target the PoW key was derived from
    commitment_hash_A: torch.Tensor  # (32,) uint8
    commitment_hash_B: torch.Tensor  # (32,) uint8
    EAL: torch.Tensor  # (m, r) int8
    EAL_fp16: torch.Tensor  # (m, r) fp16
    EAR_R_major: torch.Tensor  # (k, r) int8
    EBL_R_major: torch.Tensor  # (k, r) int8
    EAR_K_major: torch.Tensor  # (r, k) int8
    EBL_K_major: torch.Tensor  # (r, k) int8
    EBR: torch.Tensor  # (E*N, r) int8
    EBR_fp16: torch.Tensor  # (E*N, r) fp16
    routing_data: torch.Tensor  # (m*top_k,) int32
    routing_hash: torch.Tensor  # (32,) uint8
    routing_layout: MoERoutingLayout  # canonical per-expert ordering
    pow_target: torch.Tensor  # (8,) uint32
    pow_key: torch.Tensor  # (8,) uint32 (view of commitment_hash_A)
    noise_rank: int


def _to_gpu_u8(b: bytes, device: torch.device) -> torch.Tensor:
    return torch.frombuffer(bytearray(b), dtype=torch.uint8).to(device)


def _hash_2d(data_u8: torch.Tensor, key_tensor: torch.Tensor, device: torch.device) -> torch.Tensor:
    out = torch.empty(blake3.digest_size, device=device, dtype=torch.uint8)
    scratchpad = torch.empty(
        get_required_scratchpad_bytes(data_u8.numel()), dtype=torch.uint8, device=device
    )
    tensor_hash(data_u8, key_tensor, out, scratchpad)
    return out


def _routing_hash(
    routing_data: torch.Tensor, key_tensor: torch.Tensor, device: torch.device
) -> torch.Tensor:
    routing_bytes = routing_data.contiguous().view(torch.uint8)
    return _hash_2d(routing_bytes.reshape(1, -1), key_tensor, device)


def _offsets_hash(
    routing_offsets: torch.Tensor, key_tensor: torch.Tensor, device: torch.device
) -> torch.Tensor:
    offsets_bytes = routing_offsets.contiguous().view(torch.uint8)
    return _hash_2d(offsets_bytes.reshape(1, -1), key_tensor, device)


def prepare_moe_noising(
    A_q: torch.Tensor,
    A_scales: torch.Tensor,
    topk_ids: torch.Tensor,
    B_stacked: torch.Tensor,
    num_experts: int,
) -> MoENoiseContext:
    """Compute hashes, MoE commitment, routing, and noise factors for one pass."""

    device = A_q.device
    num_tokens, hidden_size = A_q.shape
    if A_scales.shape != (num_tokens, 1):
        raise ValueError(f"A_scales must have shape ({num_tokens}, 1); got {tuple(A_scales.shape)}")
    num_stacked_weight_rows = B_stacked.shape[0]
    noise_rank = config.settings.noise_rank
    top_k = topk_ids.shape[1]

    mining_job = get_async_manager().get_mining_job()
    matmul_config = GPUMatmulConfigFactory.create(
        k=hidden_size,
        noise_rank=noise_rank,
        moe=MoEConfig(e=num_experts, top_k=top_k),
    )
    pow_target = make_pow_target_tensor(
        mining_job.adjust_target(mining_config=matmul_config.mining_config)
    )

    hash_key = CommitmentHasher.get_key(
        mining_job.incomplete_header_bytes, matmul_config.mining_config
    )
    key_tensor = _to_gpu_u8(hash_key, device)

    A_hash = _hash_2d(A_q.contiguous().view(torch.uint8), key_tensor, device)
    B_hash = _hash_2d(B_stacked.contiguous().view(torch.uint8), key_tensor, device)

    # V3 (salted) mining: the kernel salts the raw roots before the routing
    # fold. The proof's ``n`` is the per-expert intermediate dimension.
    salted_dims = (
        (num_tokens, num_stacked_weight_rows // num_experts)
        if mining_job.cert_version.uses_salted_seeds
        else None
    )

    num_routed_slots = num_tokens * top_k
    routing_data = torch.empty(num_routed_slots, dtype=torch.int32, device=device)
    slot_indices = torch.empty(num_routed_slots, dtype=torch.int32, device=device)
    routing_offsets = torch.empty(num_experts, dtype=torch.int32, device=device)
    routing_scratchpad = torch.empty(
        get_build_routing_data_scratchpad_bytes(num_routed_slots),
        dtype=torch.uint8,
        device=device,
    )
    build_routing_data(
        topk_ids.to(torch.int32).contiguous(),
        routing_data,
        slot_indices,
        routing_offsets,
        routing_scratchpad,
        num_experts,
    )
    routing_layout = MoERoutingLayout.from_kernel_outputs(
        routing_data, slot_indices, routing_offsets, num_experts, top_k
    )
    routing_hash = _routing_hash(routing_data, key_tensor, device)

    offsets_hash = _offsets_hash(routing_offsets, key_tensor, device)
    commitment_hash_A = torch.empty(blake3.digest_size, device=device, dtype=torch.uint8)
    commitment_hash_B = torch.empty(blake3.digest_size, device=device, dtype=torch.uint8)
    commitment_hash_from_merkle_roots(
        A_hash,
        B_hash,
        key_tensor,
        commitment_hash_A,
        commitment_hash_B,
        routing_root=routing_hash,
        offsets_hash=offsets_hash,
        salted_dims=salted_dims,
    )

    (
        EAL,
        EAR_R_major,
        EBL_R_major,
        EAR_K_major,
        EBL_K_major,
        EBR,
        EAL_fp16,
        EBR_fp16,
    ) = generate_noise_factors(
        num_tokens,
        num_stacked_weight_rows,
        hidden_size,
        noise_rank,
        commitment_hash_A,
        commitment_hash_B,
        device,
    )

    return MoENoiseContext(
        mining_job=mining_job,
        commitment_hash_A=commitment_hash_A,
        commitment_hash_B=commitment_hash_B,
        EAL=EAL,
        EAL_fp16=EAL_fp16,
        EAR_R_major=EAR_R_major,
        EBL_R_major=EBL_R_major,
        EAR_K_major=EAR_K_major,
        EBL_K_major=EBL_K_major,
        EBR=EBR,
        EBR_fp16=EBR_fp16,
        routing_data=routing_data,
        routing_hash=routing_hash,
        routing_layout=routing_layout,
        pow_target=pow_target,
        pow_key=commitment_hash_A.view(torch.uint32),
        noise_rank=noise_rank,
    )


def permute_a_side_to_expert_order(
    layout: MoERoutingLayout,
    A_q: torch.Tensor,
    A_scales: torch.Tensor,
    EAL: torch.Tensor,
    EAL_fp16: torch.Tensor,
) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]:
    """Reindex A-side tensors into routing-canonical expert-sorted order.

    The same token permutation must be applied to activations, scales, and
    A-side noise factors so per-expert GEMMs share the routing proof's row
    numbering.
    """
    token_indices = layout.token_indices
    return (
        A_q[token_indices],
        A_scales[token_indices],
        EAL[token_indices],
        EAL_fp16[token_indices],
    )


def pearl_moe_expert_gemm(
    A_q_e: torch.Tensor,
    B_e: torch.Tensor,
    A_scales_e: torch.Tensor,
    B_scales_e: torch.Tensor,
    EAL_e: torch.Tensor,
    EAL_fp16_e: torch.Tensor,
    EBR_e: torch.Tensor,
    EBR_fp16_e: torch.Tensor,
    EAR_R_major: torch.Tensor,
    EBL_R_major: torch.Tensor,
    EAR_K_major: torch.Tensor,
    EBL_K_major: torch.Tensor,
    C_e: torch.Tensor,
    host_signal_header_pinned: torch.Tensor,
    host_signal_sync: torch.Tensor,
    pow_target: torch.Tensor,
    pow_key: torch.Tensor,
    tile_size_m: int,
    tile_size_n: int,
    tile_size_k: int,
) -> None:
    """Run ``noisy_gemm`` for one expert with PoW extraction.

    Noising runs inside the kernel (``run_noising_A/B=True``), consuming the
    pre-generated, per-expert-sliced noise factors. The A/B "+noise" and
    denoising scratch tensors are allocated here and discarded. The host signal
    header belongs to the caller and is only written by the kernel.
    """
    m_e, k = A_q_e.shape
    n_e = B_e.shape[0]
    r = EAL_e.shape[1]
    device = A_q_e.device

    ApEA = torch.empty((m_e, k), dtype=torch.int8, device=device)
    AxEBL = torch.empty((m_e, r), dtype=torch.float16, device=device)
    BpEB = torch.empty((n_e, k), dtype=torch.int8, device=device)
    EARxBpEB = torch.empty((n_e, r), dtype=torch.float16, device=device)

    host_signal_sync.zero_()

    noisy_gemm(
        A=A_q_e,
        B=B_e,
        EAL=EAL_e,
        EAL_fp16=EAL_fp16_e,
        EBR=EBR_e,
        EBR_fp16=EBR_fp16_e,
        EAR_R_major=EAR_R_major,
        EBL_R_major=EBL_R_major,
        EAR_K_major=EAR_K_major,
        EBL_K_major=EBL_K_major,
        AxEBL_fp16=AxEBL,
        EARxBpEB_fp16=EARxBpEB,
        ApEA=ApEA,
        BpEB=BpEB,
        A_scales=A_scales_e.squeeze(-1).contiguous(),
        B_scales=B_scales_e.squeeze(-1).contiguous(),
        C=C_e,
        host_signal_header_pinned=host_signal_header_pinned,
        host_signal_sync=host_signal_sync,
        pow_target=pow_target,
        pow_key=pow_key,
        tile_size_m=tile_size_m,
        tile_size_n=tile_size_n,
        tile_size_k=tile_size_k,
        run_noising_A=True,
        run_noising_B=True,
        skip_reduction=False,
        skip_denoising=False,
    )
