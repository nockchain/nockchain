"""
Pearl quantization config for vLLM.

Extends CompressedTensorsConfig to add support for:
- Mining layer (7-bit quantization + noisy GEMM)
- Non-mining layer (8-bit quantization + vanilla GEMM)

Both kernels handle smooth_quant_scale internally.
"""

import os
from typing import Any, override

import torch
from compressed_tensors.quantization import (
    QuantizationArgs,
    QuantizationStrategy,
    QuantizationType,
)
from miner_utils import get_logger
from vllm.model_executor.layers.quantization.base_config import QuantizeMethodBase
from vllm.model_executor.layers.quantization.compressed_tensors.compressed_tensors import (
    CompressedTensorsConfig,
)
from vllm.model_executor.layers.quantization.compressed_tensors.schemes import (
    CompressedTensorsScheme,
)

from .vllm_scheme import PearlScheme

_LOGGER = get_logger("vllm.pearl_miner")


class PearlConfig(CompressedTensorsConfig):
    """
    Pearl quantization config extending CompressedTensorsConfig.

    Only overrides _get_scheme_from_parts to handle:
    - Mining layer (7-bit): uses int7 quantization + noisy GEMM
    - Non-mining layer (8-bit): uses int8 quantization + vanilla GEMM

    All other behavior is inherited from CompressedTensorsConfig.
    """

    @override
    def get_name(self) -> str:
        return "pearl"

    @override
    @classmethod
    def from_config(cls, config: dict[str, Any]) -> "PearlConfig":
        """Create PearlConfig from config dict."""
        parent_config = CompressedTensorsConfig.from_config(config)

        return cls(
            target_scheme_map=parent_config.target_scheme_map,
            ignore=parent_config.ignore,
            quant_format=parent_config.quant_format,
            sparsity_scheme_map=parent_config.sparsity_scheme_map,
            sparsity_ignore_list=parent_config.sparsity_ignore_list,
            kv_cache_scheme=parent_config.kv_cache_scheme,
            config=parent_config.config,
            transform_config=getattr(parent_config, "transform_config", None),
            total_num_heads=getattr(parent_config, "total_num_heads", None),
            total_num_kv_heads=getattr(parent_config, "total_num_kv_heads", None),
        )

    @staticmethod
    def _is_mining_layer(
        weight_quant: QuantizationArgs | None,
        input_quant: QuantizationArgs | None,
    ) -> bool:
        """Check if this is a 7-bit mining layer configuration."""
        if weight_quant is None or input_quant is None:
            return False

        is_7_bits = weight_quant.num_bits == input_quant.num_bits == 7
        weight_strategy = (
            weight_quant.strategy == QuantizationStrategy.TENSOR.value
            or weight_quant.strategy == QuantizationStrategy.CHANNEL.value
        )
        is_token = (
            weight_strategy and input_quant.strategy == QuantizationStrategy.TOKEN.value
        )
        is_dynamic = not weight_quant.dynamic and input_quant.dynamic

        return is_7_bits and is_token and weight_quant.symmetric and is_dynamic

    @staticmethod
    def _is_non_mining_layer(
        weight_quant: QuantizationArgs | None,
        input_quant: QuantizationArgs | None,
    ) -> bool:
        """Check if this is an 8-bit non-mining layer configuration."""
        if weight_quant is None or input_quant is None:
            return False

        is_8_bits = weight_quant.num_bits == input_quant.num_bits == 8
        weight_strategy = (
            weight_quant.strategy == QuantizationStrategy.TENSOR.value
            or weight_quant.strategy == QuantizationStrategy.CHANNEL.value
        )
        is_token = (
            weight_strategy and input_quant.strategy == QuantizationStrategy.TOKEN.value
        )
        is_dynamic = not weight_quant.dynamic and input_quant.dynamic

        return is_8_bits and is_token and weight_quant.symmetric and is_dynamic

    @staticmethod
    def _is_fp8_block_layer(
        weight_quant: QuantizationArgs | None,
        input_quant: QuantizationArgs | None,
    ) -> bool:
        """Check for fp8 block-wise weights + dynamic group fp8 activations (down proj)."""
        if weight_quant is None or input_quant is None:
            return False

        def _is_float(quant: QuantizationArgs) -> bool:
            return quant.type in (QuantizationType.FLOAT.value, QuantizationType.FLOAT)

        is_fp8 = (
            weight_quant.num_bits == input_quant.num_bits == 8
            and _is_float(weight_quant)
            and _is_float(input_quant)
        )
        is_block = weight_quant.strategy == QuantizationStrategy.BLOCK.value and bool(
            weight_quant.block_structure
        )
        act_group = (
            input_quant.strategy == QuantizationStrategy.GROUP.value
            and input_quant.dynamic
        )
        return is_fp8 and is_block and act_group

    # Expert-0 projection suffixes used to resolve a FusedMoE layer's per-projection
    # schemes (mirrors vLLM's CompressedTensorsMoEMethod.get_moe_method).
    _GATE_UP_PROBE_SUFFIX = ".0.gate_proj"
    _DOWN_PROBE_SUFFIX = ".0.down_proj"

    def _moe_proj_quant_args(
        self, layer: torch.nn.Module, prefix: str, suffix: str
    ) -> tuple[QuantizationArgs | None, QuantizationArgs | None]:
        """Resolve the (weight, input) quant args for one MoE projection."""
        scheme_dict = self.get_scheme_dict(layer, prefix + suffix)
        if scheme_dict:
            return scheme_dict.get("weights"), scheme_dict.get("input_activations")
        return None, None

    @override
    def get_quant_method(
        self,
        layer: torch.nn.Module,
        prefix: str,
    ) -> "QuantizeMethodBase | None":
        """Route mixed int7-gate/up + fp8-block-down FusedMoE layers to PearlMoE."""
        from vllm.model_executor.layers.fused_moe import FusedMoE

        from .pearl_moe_method import PearlMoEMethod

        if isinstance(layer, FusedMoE):
            gate_weight, gate_input = self._moe_proj_quant_args(
                layer, prefix, self._GATE_UP_PROBE_SUFFIX
            )
            down_weight, down_input = self._moe_proj_quant_args(
                layer, prefix, self._DOWN_PROBE_SUFFIX
            )
            if self._is_mining_layer(
                gate_weight, gate_input
            ) and self._is_fp8_block_layer(down_weight, down_input):
                _LOGGER.debug(
                    f"Pearl MoE (int7 gate/up + fp8 block down) detected for {prefix}"
                )
                return PearlMoEMethod(layer.moe_config, down_weight, down_input)

        return super().get_quant_method(layer, prefix)

    @staticmethod
    def _is_dense_gate_up_layer(layer_name: str | None) -> bool:
        target_layer = int(os.environ.get("NOCKCHAIN_GEMMA4_MINING_LAYER", "0"))
        if not 0 <= target_layer < 60:
            raise ValueError("NOCKCHAIN_GEMMA4_MINING_LAYER must be in 0..=59")
        suffix = f".layers.{target_layer}.mlp.gate_up_proj"
        return bool(layer_name and layer_name.endswith(suffix))

    @override
    def _get_scheme_from_parts(
        self,
        weight_quant: QuantizationArgs,
        input_quant: QuantizationArgs,
        format: str | None = None,
        layer_name: str | None = None,
    ) -> CompressedTensorsScheme:
        """
        Create a quantization scheme based on weight and input quant args.

        Checks for:
        1. Mining layer (7-bit) -> PearlScheme(mining_enabled=True)
        2. Non-mining layer (8-bit) -> PearlScheme(mining_enabled=False)
        3. Otherwise -> delegates to parent

        """
        if self._is_mining_layer(weight_quant, input_quant):
            mining_enabled = self._is_dense_gate_up_layer(layer_name)
            _LOGGER.debug(
                f"INT7 layer detected for {layer_name}; mining_enabled={mining_enabled}"
            )
            return PearlScheme(
                strategy=weight_quant.strategy,
                is_static_input_scheme=False,
                input_symmetric=input_quant.symmetric,
                mining_enabled=mining_enabled,
                input_num_bits=7,
            )

        # Check for 8-bit non-mining layer
        if self._is_non_mining_layer(weight_quant, input_quant):
            _LOGGER.debug(f"Non-mining layer (8-bit) detected for {layer_name}")
            return PearlScheme(
                strategy=weight_quant.strategy,
                is_static_input_scheme=False,
                input_symmetric=input_quant.symmetric,
                mining_enabled=False,
                input_num_bits=8,
            )

        # Fall back to parent's implementation for all other schemes
        return super()._get_scheme_from_parts(
            weight_quant, input_quant, format, layer_name
        )
