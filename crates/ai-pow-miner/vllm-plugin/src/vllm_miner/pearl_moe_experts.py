from typing import Any

import torch
import triton.language as tl
import vllm.model_executor.layers.fused_moe.modular_kernel as mk
from pearl_gemm import get_host_signal_sync_size
from vllm import _custom_ops as ops
from vllm.model_executor.layers.fused_moe.activation import MoEActivation
from vllm.model_executor.layers.fused_moe.config import (
    FusedMoEConfig,
    FusedMoEParallelConfig,
    FusedMoEQuantConfig,
)
from vllm.model_executor.layers.fused_moe.fused_moe import (
    invoke_fused_moe_triton_kernel,
    try_get_optimal_moe_config,
)
from vllm.model_executor.layers.fused_moe.moe_align_block_size import (
    moe_align_block_size,
)
from vllm.model_executor.layers.fused_moe.topk_weight_and_reduce import (
    TopKWeightAndReduceNoOP,
)
from vllm.model_executor.layers.fused_moe.utils import _resize_cache
from vllm.platforms import current_platform

from .callbacks import MoEStatusCheckCallback
from .config import config as pearl_config
from .mining_state import get_async_manager, get_pinned_pool
from .moe_gemm_operators import (
    pearl_moe_expert_gemm,
    permute_a_side_to_expert_order,
    prepare_moe_noising,
)
from .quantization_operators import quant_7bit, quant_fp8_block

_GEMM2_TOP_K = 1
_GEMM2_QUANT_SCHEME = "fp8_w8a8"
# MoE noise kernels require at least Hopper (matches dense PearlKernel min capability).
_MIN_COMPUTE_CAPABILITY_MAJOR = 9
# vLLM sentinel meaning "infer expert count from the weight tensor".
GLOBAL_NUM_EXPERTS_INFER_FROM_WEIGHTS = -1

_SUPPORTED_ACTIVATIONS = frozenset(
    {
        MoEActivation.SILU,
        MoEActivation.GELU,
        MoEActivation.SILU_NO_MUL,
        MoEActivation.GELU_NO_MUL,
    }
)

_TORCH_TO_TRITON_DTYPE: dict[torch.dtype, tl.dtype] = {
    torch.bfloat16: tl.bfloat16,
    torch.float16: tl.float16,
    torch.float32: tl.float32,
}


def _torch_dtype_to_triton_compute_type(dtype: torch.dtype) -> tl.dtype:
    result = _TORCH_TO_TRITON_DTYPE.get(dtype)
    if result is None:
        raise ValueError(f"Unsupported activation dtype for MoE compute_type: {dtype}")
    return result


class PearlMoEExperts(mk.FusedMoEExpertsModular):
    """Pearl MoE experts: per-expert noisy GEMM1 (mining) + vanilla GEMM2."""

    def __init__(
        self,
        moe_config: FusedMoEConfig,
        quant_config: FusedMoEQuantConfig,
        w2_block_shape: list[int],
        act_group_size: int,
        hadamard_block_size: int,
        layer: torch.nn.Module | None = None,
    ):
        super().__init__(moe_config, quant_config)
        self._layer = layer
        self._w2_block_shape = w2_block_shape
        self._act_group_size = act_group_size
        self._hadamard_block_size = hadamard_block_size

    @staticmethod
    def activation_format() -> mk.FusedMoEActivationFormat:
        return mk.FusedMoEActivationFormat.Standard

    @staticmethod
    def _supports_current_device() -> bool:
        if not current_platform.is_cuda_alike():
            return False
        return current_platform.get_device_capability()[0] >= _MIN_COMPUTE_CAPABILITY_MAJOR

    @staticmethod
    def _supports_no_act_and_mul() -> bool:
        return True

    @staticmethod
    def _supports_quant_scheme(weight_key: Any, activation_key: Any) -> bool:
        return (weight_key, activation_key) == (None, None)

    @staticmethod
    def _supports_activation(activation: MoEActivation) -> bool:
        return activation in _SUPPORTED_ACTIVATIONS

    @staticmethod
    def _supports_parallel_config(moe_parallel_config: FusedMoEParallelConfig) -> bool:
        return not (
            moe_parallel_config.use_fi_nvl_two_sided_kernels
            or moe_parallel_config.use_fi_nvl_one_sided_kernels
        )

    def supports_expert_map(self) -> bool:
        return True

    def workspace_dtype(self, act_dtype: torch.dtype) -> torch.dtype:
        return act_dtype

    def finalize_weight_and_reduce_impl(self) -> mk.TopKWeightAndReduce:
        return TopKWeightAndReduceNoOP()

    def _get_smooth_scale(self, attr: str) -> torch.Tensor | None:
        if self._layer is None:
            return None
        scale = getattr(self._layer, attr, None)
        if scale is None or scale.numel() == 0:
            return None
        return scale

    def _int8_w8a8_triton_kwargs(self) -> dict[str, Any]:
        return {
            "use_fp8_w8a8": False,
            "use_int8_w8a8": True,
            "use_int8_w8a16": False,
            "use_int4_w4a16": False,
            "per_channel_quant": self.per_act_token_quant,
        }

    def workspace_shapes(
        self,
        M: int,
        N: int,
        K: int,
        topk: int,
        global_num_experts: int,
        local_num_experts: int,
        expert_tokens_meta: mk.ExpertTokensMetadata | None,
        activation: MoEActivation,
    ) -> tuple[tuple[int, ...], tuple[int, ...], tuple[int, ...]]:
        activation_out_dim = self.adjust_N_for_activation(N, activation)
        workspace1 = (M, topk, max(activation_out_dim, K))
        workspace2 = (M, topk, max(N, K))
        output = (M, K)
        return (workspace1, workspace2, output)

    def _quant_a_7bit(self, hidden_states: torch.Tensor, K: int):
        """Quantize activations with the shared gate/up smooth scale and Hadamard."""
        w13_smooth = self._get_smooth_scale("w13_smooth_quant_scale")
        smooth = w13_smooth[:K] if w13_smooth is not None else None
        return quant_7bit(hidden_states, smooth_scale=smooth, block_size=self._hadamard_block_size)

    def apply(
        self,
        output: torch.Tensor,
        hidden_states: torch.Tensor,
        w1: torch.Tensor,
        w2: torch.Tensor,
        topk_weights: torch.Tensor,
        topk_ids: torch.Tensor,
        activation: MoEActivation,
        global_num_experts: int,
        expert_map: torch.Tensor | None,
        a1q_scale: torch.Tensor | None,
        a2_scale: torch.Tensor | None,
        workspace13: torch.Tensor,
        workspace2: torch.Tensor,
        expert_tokens_meta: mk.ExpertTokensMetadata | None,
        apply_router_weight_on_input: bool,
    ) -> None:
        assert hidden_states.is_contiguous()
        assert hidden_states.dim() == 2

        E, num_tokens, N, K, top_k_num = self.moe_problem_size(hidden_states, w1, w2, topk_ids)
        if global_num_experts == GLOBAL_NUM_EXPERTS_INFER_FROM_WEIGHTS:
            global_num_experts = E

        should_mine = (
            not get_async_manager()._conf.no_mining
        ) and pearl_config.should_use_noisy_gemm(num_tokens, N, K)

        A_q, A_scales, _ = self._quant_a_7bit(hidden_states, K)

        intermediate_cache1 = _resize_cache(workspace2, (num_tokens, top_k_num, N))
        cache2_dim = self.adjust_N_for_activation(N, activation)
        intermediate_cache2 = _resize_cache(workspace13, (num_tokens * top_k_num, cache2_dim))
        intermediate_cache3 = _resize_cache(workspace2, (num_tokens, top_k_num, K))

        triton_config = try_get_optimal_moe_config(w1.shape, w2.shape, top_k_num, None, num_tokens)

        sorted_token_ids, expert_ids, num_tokens_post_padded = moe_align_block_size(
            topk_ids,
            triton_config["BLOCK_SIZE_M"],
            global_num_experts,
            expert_map,
        )

        gemm1_compute_type = _torch_dtype_to_triton_compute_type(hidden_states.dtype)
        if should_mine:
            self._apply_per_expert_gemm1(
                A_q=A_q,
                A_scales=A_scales,
                intermediate_cache1=intermediate_cache1,
                topk_ids=topk_ids,
                topk_weights=topk_weights,
                w1=w1,
                E=E,
                N=N,
                K=K,
                num_tokens=num_tokens,
                top_k_num=top_k_num,
                apply_router_weight_on_input=apply_router_weight_on_input,
            )
        else:
            # Vanilla int8 grouped GEMM1 (no mining / no denoise).
            invoke_fused_moe_triton_kernel(
                A=A_q,
                B=w1,
                C=intermediate_cache1,
                A_scale=A_scales,
                B_scale=self.w1_scale,
                topk_weights=topk_weights if apply_router_weight_on_input else None,
                sorted_token_ids=sorted_token_ids,
                expert_ids=expert_ids,
                num_tokens_post_padded=num_tokens_post_padded,
                mul_routed_weight=apply_router_weight_on_input,
                top_k=top_k_num,
                config=triton_config,
                compute_type=gemm1_compute_type,
                **self._int8_w8a8_triton_kwargs(),
            )

        self.activation(activation, intermediate_cache2, intermediate_cache1.view(-1, N))

        # GEMM2 (down projection): not mined, kept in the original fp8 block
        self._gemm2_triton(
            intermediate_cache2=intermediate_cache2,
            w2=w2,
            intermediate_cache3=intermediate_cache3,
            output=output,
            topk_weights=topk_weights,
            sorted_token_ids=sorted_token_ids,
            expert_ids=expert_ids,
            num_tokens_post_padded=num_tokens_post_padded,
            apply_router_weight_on_input=apply_router_weight_on_input,
            align_block_size_m=triton_config["BLOCK_SIZE_M"],
            num_tokens=num_tokens,
            compute_type=_torch_dtype_to_triton_compute_type(hidden_states.dtype),
            w1=w1,
        )

    def _apply_per_expert_gemm1(
        self,
        *,
        A_q: torch.Tensor,
        A_scales: torch.Tensor,
        intermediate_cache1: torch.Tensor,
        topk_ids: torch.Tensor,
        topk_weights: torch.Tensor,
        w1: torch.Tensor,
        E: int,
        N: int,
        K: int,
        num_tokens: int,
        top_k_num: int,
        apply_router_weight_on_input: bool,
    ) -> None:
        """GEMM1: per-expert ``noisy_gemm`` with PoW extraction"""
        B_stacked = w1.reshape(E * N, K)
        ctx = prepare_moe_noising(A_q, A_scales, topk_ids, B_stacked, E)
        layout = ctx.routing_layout

        (
            A_q_by_expert,
            A_scales_by_expert,
            EAL_by_expert,
            EAL_fp16_by_expert,
        ) = permute_a_side_to_expert_order(layout, A_q, A_scales, ctx.EAL, ctx.EAL_fp16)

        cache_flat = intermediate_cache1.view(-1, N)
        out_dtype = cache_flat.dtype
        num_routed_slots = num_tokens * top_k_num
        gemm1_output_by_slot = torch.empty((num_routed_slots, N), dtype=out_dtype, device=w1.device)
        host_signal_sync = torch.zeros(
            get_host_signal_sync_size(), dtype=torch.int8, device=w1.device
        )

        submit = not get_async_manager()._conf.skip_block_submission
        pow_headers = [get_pinned_pool().acquire() for _ in range(E)]

        tile_m = pearl_config.settings.tile_size_m
        tile_n = pearl_config.settings.tile_size_n
        tile_k = pearl_config.settings.tile_size_k

        try:
            for expert_index in range(E):
                routing_start, expert_slot_count = layout.expert_slice(expert_index)
                if expert_slot_count == 0:
                    continue
                expert_weight_start = expert_index * N
                expert_weight_end = expert_weight_start + N
                expert_output = gemm1_output_by_slot[:expert_slot_count]

                pearl_moe_expert_gemm(
                    A_q_e=A_q_by_expert[routing_start : routing_start + expert_slot_count],
                    B_e=B_stacked[expert_weight_start:expert_weight_end],
                    A_scales_e=A_scales_by_expert[
                        routing_start : routing_start + expert_slot_count
                    ],
                    B_scales_e=self.w1_scale[expert_index],
                    EAL_e=EAL_by_expert[routing_start : routing_start + expert_slot_count],
                    EAL_fp16_e=EAL_fp16_by_expert[
                        routing_start : routing_start + expert_slot_count
                    ],
                    EBR_e=ctx.EBR[expert_weight_start:expert_weight_end],
                    EBR_fp16_e=ctx.EBR_fp16[expert_weight_start:expert_weight_end],
                    EAR_R_major=ctx.EAR_R_major,
                    EBL_R_major=ctx.EBL_R_major,
                    EAR_K_major=ctx.EAR_K_major,
                    EBL_K_major=ctx.EBL_K_major,
                    C_e=expert_output,
                    host_signal_header_pinned=pow_headers[expert_index],
                    host_signal_sync=host_signal_sync,
                    pow_target=ctx.pow_target,
                    pow_key=ctx.pow_key,
                    tile_size_m=tile_m,
                    tile_size_n=tile_n,
                    tile_size_k=tile_k,
                )

                expert_slot_indices = layout.slot_indices[
                    routing_start : routing_start + expert_slot_count
                ]
                if apply_router_weight_on_input:
                    expert_weights = topk_weights.reshape(-1)[expert_slot_indices]
                    expert_output = expert_output * expert_weights.unsqueeze(-1)
                cache_flat[expert_slot_indices] = expert_output
        except Exception:
            for header_tensor in pow_headers:
                get_pinned_pool().release(header_tensor)
            raise

        if not submit:
            for header_tensor in pow_headers:
                get_pinned_pool().release(header_tensor)
            return

        cuda_event = torch.cuda.Event()
        cuda_event.record()
        callback = MoEStatusCheckCallback(
            pow_headers=pow_headers,
            commitment_hash_A_tensor=ctx.commitment_hash_A,
            commitment_hash_B_tensor=ctx.commitment_hash_B,
            A_q=A_q,
            B_stacked=B_stacked,
            routing_data=ctx.routing_data,
            routing_hash=ctx.routing_hash,
            mining_job=ctx.mining_job,
            noise_rank=ctx.noise_rank,
            num_experts=E,
            n_per_expert=N,
            top_k=top_k_num,
            routing_offsets=layout.routing_offsets_host,
        )
        get_async_manager().schedule_status_check(cuda_event, callback)

    def _gemm2_triton(
        self,
        *,
        intermediate_cache2: torch.Tensor,
        w2: torch.Tensor,
        intermediate_cache3: torch.Tensor,
        output: torch.Tensor,
        topk_weights: torch.Tensor,
        sorted_token_ids: torch.Tensor,
        expert_ids: torch.Tensor,
        num_tokens_post_padded: torch.Tensor,
        apply_router_weight_on_input: bool,
        align_block_size_m: int,
        num_tokens: int,
        compute_type: tl.dtype,
        w1: torch.Tensor,
    ) -> None:
        """Down projection via the Triton fp8 block grouped GEMM."""
        quantized_intermediate, activation_scale = quant_fp8_block(
            intermediate_cache2, group_size=self._act_group_size
        )

        gemm2_config = dict(
            try_get_optimal_moe_config(
                w1.shape,
                w2.shape,
                _GEMM2_TOP_K,
                _GEMM2_QUANT_SCHEME,
                num_tokens,
                self._w2_block_shape,
            )
        )
        # Reuse the token alignment that produced sorted_token_ids/expert_ids.
        gemm2_config["BLOCK_SIZE_M"] = align_block_size_m

        invoke_fused_moe_triton_kernel(
            quantized_intermediate,
            w2,
            intermediate_cache3,
            activation_scale,
            self.w2_scale,
            topk_weights,
            sorted_token_ids,
            expert_ids,
            num_tokens_post_padded,
            not apply_router_weight_on_input,
            _GEMM2_TOP_K,
            gemm2_config,
            compute_type=compute_type,
            use_fp8_w8a8=True,
            use_int8_w8a8=False,
            use_int8_w8a16=False,
            use_int4_w4a16=False,
            per_channel_quant=False,
            block_shape=self._w2_block_shape,
        )

        ops.moe_sum(intermediate_cache3, output)
