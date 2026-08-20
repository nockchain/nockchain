from typing import TYPE_CHECKING

import torch
import vllm.model_executor.layers.fused_moe.modular_kernel as mk
from compressed_tensors.quantization import QuantizationArgs
from miner_utils import get_logger
from vllm import envs
from vllm.model_executor.layers.fused_moe import (
    FusedMoEActivationFormat,
    FusedMoEMethodBase,
    FusedMoeWeightScaleSupported,
)
from vllm.model_executor.layers.fused_moe.config import (
    FusedMoEConfig,
    FusedMoEQuantConfig,
)
from vllm.model_executor.layers.fused_moe.prepare_finalize import (
    MoEPrepareAndFinalizeNoDPEPModular,
)
from vllm.model_executor.utils import set_weight_attrs

from .mining_state import ensure_pinned_pool_at_least
from .quantization_operators import NO_HADAMARD_BLOCK_SIZE

if TYPE_CHECKING:
    from vllm.model_executor.layers.fused_moe.layer import FusedMoE

_LOGGER = get_logger("vllm.pearl_miner")

WEIGHT_DTYPE = torch.int8
SCALE_DTYPE = torch.float32
SMOOTH_SCALE_DTYPE = torch.bfloat16
SCALE_LAST_DIM = 1
QUANT_METHOD_CHANNEL = FusedMoeWeightScaleSupported.CHANNEL.value
QUANT_METHOD_BLOCK = FusedMoeWeightScaleSupported.BLOCK.value
W13_WEIGHT_SHARDS_WITH_ACT_AND_MUL = 2
W13_WEIGHT_SHARDS_WITHOUT_ACT_AND_MUL = 1
# Checkpoint shard id carrying the shared gate/up smooth scale (gate == "w1").
W13_SHARED_SHARD_ID = "w1"
# Per-layer pinned PoW headers held while a forward's callback is pending.
MOE_POW_HEADER_POOL_DEPTH = 16

# Down projection (GEMM2): not mined, kept in the original fp8 block quantization.
W2_WEIGHT_DTYPE = torch.float8_e4m3fn

MOE_BACKEND_AUTO = "auto"
MOE_BACKEND_TRITON = "triton"


def _shared_w13_loader(
    param: torch.nn.Parameter,
    loaded_weight: torch.Tensor,
    weight_name: str | None = None,
    shard_id: str | None = None,
    expert_id: int | None = None,
    return_success: bool = False,
    **kwargs,
) -> bool | None:
    """Weight loader for the shared per-layer gate/up smooth scale.

    The checkpoint stores a single smooth-quant vector on ``experts.0.gate_proj``
    (the fused-expert mapping routes it here with ``shard_id == "w1"``). It is
    shared across all experts and across gate/up and lives on the (un-sharded)
    hidden input dim, so we copy it verbatim. vLLM's default fused-expert loader
    cannot handle it (it would TP-shard a ``*_scale`` param along the output dim
    and requires a ``quant_method``), so we bypass it with this loader.
    """
    if shard_id in (None, W13_SHARED_SHARD_ID):
        param.data.copy_(loaded_weight.reshape(param.shape).to(param.dtype))
    return True if return_success else None


class PearlMoEMethod(FusedMoEMethodBase):
    def __init__(
        self,
        moe_config: FusedMoEConfig,
        down_weight_quant: QuantizationArgs,
        down_input_quant: QuantizationArgs,
    ):
        super().__init__(moe_config)
        block_shape = down_weight_quant.block_structure
        if not block_shape:
            raise ValueError(
                "PearlMoE down projection requires a fp8 weight block_structure "
                f"in the quantization config; got {block_shape!r}"
            )
        self.block_n, self.block_k = int(block_shape[0]), int(block_shape[1])

        group_size = down_input_quant.group_size
        if group_size is None:
            raise ValueError(
                "PearlMoE down projection requires an activation group_size in "
                "the quantization config; got None"
            )
        self.act_group_size = int(group_size)

        # Resolved from the loaded weights in process_weights_after_loading.
        self.hadamard_block_size = NO_HADAMARD_BLOCK_SIZE

    def _w2_block_shape(self) -> list[int]:
        return [self.block_n, self.block_k]

    def create_weights(
        self,
        layer: torch.nn.Module,
        num_experts: int,
        hidden_size: int,
        intermediate_size_per_partition: int,
        params_dtype: torch.dtype,
        **extra_weight_attrs,
    ) -> None:
        w13_num_shards = (
            W13_WEIGHT_SHARDS_WITH_ACT_AND_MUL
            if self.moe.is_act_and_mul
            else W13_WEIGHT_SHARDS_WITHOUT_ACT_AND_MUL
        )

        w13_weight = torch.nn.Parameter(
            torch.empty(
                num_experts,
                w13_num_shards * intermediate_size_per_partition,
                hidden_size,
                dtype=WEIGHT_DTYPE,
            ),
            requires_grad=False,
        )
        layer.register_parameter("w13_weight", w13_weight)
        set_weight_attrs(w13_weight, extra_weight_attrs)

        # Down projection: fp8 weights with block-wise scales.
        layer.weight_block_size = self._w2_block_shape()
        n_tiles = (hidden_size + self.block_n - 1) // self.block_n
        k_tiles = (intermediate_size_per_partition + self.block_k - 1) // self.block_k

        w2_weight = torch.nn.Parameter(
            torch.empty(
                num_experts,
                hidden_size,
                intermediate_size_per_partition,
                dtype=W2_WEIGHT_DTYPE,
            ),
            requires_grad=False,
        )
        layer.register_parameter("w2_weight", w2_weight)
        set_weight_attrs(w2_weight, extra_weight_attrs)

        w13_weight_scale = torch.nn.Parameter(
            torch.ones(
                num_experts,
                w13_num_shards * intermediate_size_per_partition,
                SCALE_LAST_DIM,
                dtype=SCALE_DTYPE,
            ),
            requires_grad=False,
        )
        layer.register_parameter("w13_weight_scale", w13_weight_scale)

        w2_weight_scale = torch.nn.Parameter(
            torch.ones(num_experts, n_tiles, k_tiles, dtype=SCALE_DTYPE),
            requires_grad=False,
        )
        layer.register_parameter("w2_weight_scale", w2_weight_scale)

        channel_attrs = dict(extra_weight_attrs, quant_method=QUANT_METHOD_CHANNEL)
        set_weight_attrs(w13_weight_scale, channel_attrs)
        block_attrs = dict(extra_weight_attrs, quant_method=QUANT_METHOD_BLOCK)
        set_weight_attrs(w2_weight_scale, block_attrs)

        w13_smooth_quant_scale = torch.nn.Parameter(
            torch.ones(hidden_size, dtype=SMOOTH_SCALE_DTYPE),
            requires_grad=False,
        )
        layer.register_parameter("w13_smooth_quant_scale", w13_smooth_quant_scale)
        set_weight_attrs(w13_smooth_quant_scale, {"weight_loader": _shared_w13_loader})

        w13_hadamard_block_size = torch.nn.Parameter(
            torch.zeros(1, dtype=torch.int32),
            requires_grad=False,
        )
        layer.register_parameter("w13_hadamard_block_size", w13_hadamard_block_size)
        set_weight_attrs(w13_hadamard_block_size, {"weight_loader": _shared_w13_loader})

        layer.w13_input_scale = None
        layer.w2_input_scale = None

    def process_weights_after_loading(self, layer: torch.nn.Module) -> None:
        self._warn_if_backend_overridden()

        # Cache the shared gate/up Hadamard block size
        self.hadamard_block_size = int(layer.w13_hadamard_block_size.reshape(-1)[0].item())

        num_experts = layer.w13_weight.shape[0]
        ensure_pinned_pool_at_least(num_experts * MOE_POW_HEADER_POOL_DEPTH)
        self._setup_kernel(layer)

    def _warn_if_backend_overridden(self) -> None:
        """Warn when vLLM requests a MoE backend other than Triton."""

        requested = getattr(self.moe, "moe_backend", MOE_BACKEND_AUTO)
        if requested and requested not in (MOE_BACKEND_AUTO, MOE_BACKEND_TRITON):
            self._warn_backend_ignored(requested)
            return

        if (envs.is_set("VLLM_USE_DEEP_GEMM") and envs.VLLM_USE_DEEP_GEMM) or (
            envs.is_set("VLLM_MOE_USE_DEEP_GEMM") and envs.VLLM_MOE_USE_DEEP_GEMM
        ):
            self._warn_backend_ignored("deep_gemm")
        elif envs.is_set("VLLM_USE_FLASHINFER_MOE_FP8") and envs.VLLM_USE_FLASHINFER_MOE_FP8:
            self._warn_backend_ignored("flashinfer")

    @staticmethod
    def _warn_backend_ignored(
        requested: str,
    ) -> None:
        _LOGGER.warning(
            "Requested MoE backend '%s' is ignored for PearlMoE; the down "
            "projection always uses the Triton fp8-block GEMM.",
            requested,
        )

    def get_fused_moe_quant_config(self, layer: torch.nn.Module) -> FusedMoEQuantConfig | None:
        return FusedMoEQuantConfig.make(
            quant_dtype=None,
            per_act_token_quant=True,
            per_out_ch_quant=True,
            w1_scale=layer.w13_weight_scale,
            w2_scale=layer.w2_weight_scale,
        )

    def _setup_kernel(self, layer: torch.nn.Module) -> None:
        """Build and store the modular MoE kernel so ``apply`` can delegate."""
        from .pearl_moe_experts import PearlMoEExperts

        try:
            self.moe_quant_config = self.get_fused_moe_quant_config(layer)
            prepare_finalize = MoEPrepareAndFinalizeNoDPEPModular()
            experts = PearlMoEExperts(
                moe_config=self.moe,
                quant_config=self.moe_quant_config,
                layer=layer,
                w2_block_shape=self._w2_block_shape(),
                act_group_size=self.act_group_size,
                hadamard_block_size=self.hadamard_block_size,
            )
            self.moe_kernel = mk.FusedMoEKernel(
                prepare_finalize=prepare_finalize,
                fused_experts=experts,
            )
            _LOGGER.info("Using PearlMoEExperts for MoE layer")
        except Exception:
            # Leave moe_kernel unset; layer may initialise via maybe_init_modular_kernel.
            _LOGGER.exception("Failed to set up PearlMoE kernel; will rely on lazy init")

    def apply(
        self,
        layer: "FusedMoE",
        x: torch.Tensor,
        topk_weights: torch.Tensor,
        topk_ids: torch.Tensor,
        shared_experts_input: torch.Tensor | None,
    ) -> torch.Tensor | tuple[torch.Tensor, torch.Tensor]:
        assert self.moe_kernel is not None, (
            "PearlMoEMethod.apply called before moe_kernel was initialised"
        )
        return self.moe_kernel.apply(
            hidden_states=x,
            w1=layer.w13_weight,
            w2=layer.w2_weight,
            topk_weights=topk_weights,
            topk_ids=topk_ids,
            activation=layer.activation,
            global_num_experts=layer.global_num_experts,
            apply_router_weight_on_input=layer.apply_router_weight_on_input,
            expert_map=layer.expert_map,
            shared_experts_input=shared_experts_input,
        )

    def select_gemm_impl(
        self,
        prepare_finalize: mk.FusedMoEPrepareAndFinalizeModular,
        layer: torch.nn.Module,
    ) -> mk.FusedMoEExpertsModular:
        if prepare_finalize.activation_format == FusedMoEActivationFormat.BatchedExperts:
            raise NotImplementedError(
                "BatchedExperts activation format is not supported for PearlMoE"
            )
        from .pearl_moe_experts import PearlMoEExperts

        return PearlMoEExperts(
            moe_config=self.moe,
            quant_config=self.moe_quant_config,
            layer=layer,
            w2_block_shape=self._w2_block_shape(),
            act_group_size=self.act_group_size,
            hadamard_block_size=self.hadamard_block_size,
        )
