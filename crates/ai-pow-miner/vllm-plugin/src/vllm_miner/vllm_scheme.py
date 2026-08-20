"""
Pearl quantization scheme for vLLM.

Single parametrized scheme supporting both:
- Mining mode (mining_enabled=True): int7 quantization + noisy GEMM
- Non-mining mode (mining_enabled=False): int8 quantization + vanilla GEMM

Both modes handle smooth_quant_scale through the PearlKernel.
"""

from collections.abc import Callable
from typing import override

import torch
from compressed_tensors.quantization import QuantizationStrategy
from miner_utils import get_logger
from vllm.model_executor.kernels.linear.scaled_mm import Int8ScaledMMLinearLayerConfig
from vllm.model_executor.layers.quantization.compressed_tensors.schemes import (
    CompressedTensorsScheme,
)
from vllm.model_executor.parameter import (
    BasevLLMParameter,
    ChannelQuantScaleParameter,
    ModelWeightParameter,
    PerTensorScaleParameter,
    RowvLLMParameter,
)

from .vllm_kernels import PearlKernel

_LOGGER = get_logger("vllm.pearl_miner")


class PearlScheme(CompressedTensorsScheme):
    """
    Unified scheme for pearl quantization layers.

    Args:
        strategy: Quantization strategy (TENSOR or CHANNEL)
        is_static_input_scheme: Whether input quantization is static
        input_symmetric: Whether input quantization is symmetric
        mining_enabled: If True, uses int7 + noisy GEMM; if False, uses int8 + vanilla GEMM
    """

    def __init__(
        self,
        strategy: str,
        is_static_input_scheme: bool,
        input_symmetric: bool,
        mining_enabled: bool,
    ):
        self.strategy = strategy
        self.is_static_input_scheme = is_static_input_scheme
        self.input_symmetric = input_symmetric
        self.mining_enabled = mining_enabled

    @override
    @classmethod
    def get_min_capability(cls) -> int:
        return 9  # Hopper and up (required for pearl GEMM)

    @override
    def create_weights(
        self,
        layer: torch.nn.Module,
        output_partition_sizes: list[int],
        input_size_per_partition: int,
        params_dtype: torch.dtype,
        weight_loader: Callable,
        **kwargs,
    ) -> None:
        layer.logical_widths = output_partition_sizes

        scaled_mm_linear_kernel_config = Int8ScaledMMLinearLayerConfig(
            is_channelwise=(self.strategy == QuantizationStrategy.CHANNEL.value),
            is_static_input_scheme=self.is_static_input_scheme,
            input_symmetric=self.input_symmetric,
        )

        layer_type = "mining" if self.mining_enabled else "non-mining"
        _LOGGER.info(
            f"Using PearlKernel (mining_enabled={self.mining_enabled}) for {layer_type} layer"
        )

        # WEIGHT
        weight = ModelWeightParameter(
            data=torch.empty(
                sum(output_partition_sizes), input_size_per_partition, dtype=torch.int8
            ),
            input_dim=1,
            output_dim=0,
            weight_loader=weight_loader,
        )
        layer.register_parameter("weight", weight)

        # WEIGHT SCALE
        if self.strategy == QuantizationStrategy.CHANNEL.value:
            weight_scale = ChannelQuantScaleParameter(
                data=torch.empty((sum(output_partition_sizes), 1), dtype=torch.float32),
                output_dim=0,
                weight_loader=weight_loader,
            )
        else:
            weight_scale = PerTensorScaleParameter(
                data=torch.empty(len(output_partition_sizes), dtype=torch.float32),
                weight_loader=weight_loader,
            )
        layer.register_parameter("weight_scale", weight_scale)

        # INPUT SCALE
        if self.is_static_input_scheme:
            input_scale = BasevLLMParameter(
                data=torch.empty(1, dtype=torch.float32), weight_loader=weight_loader
            )
            layer.register_parameter("input_scale", input_scale)

            if not self.input_symmetric:
                raise NotImplementedError(
                    "PearlScheme does not support asymmetric quantization. "
                    "Please use symmetric quantization (input_symmetric=True)."
                )

        # SmoothQuant scale
        smooth_quant_scale = RowvLLMParameter(
            data=torch.ones(input_size_per_partition, dtype=torch.bfloat16),
            input_dim=0,
            weight_loader=weight_loader,
        )
        layer.register_parameter("smooth_quant_scale", smooth_quant_scale)

        # Hadamard block size
        hadamard_block_size = BasevLLMParameter(
            data=torch.zeros(1, dtype=torch.int32),
            weight_loader=weight_loader,
        )
        layer.register_parameter("hadamard_block_size", hadamard_block_size)

        # Create kernel
        self.kernel = PearlKernel(
            c=scaled_mm_linear_kernel_config,
            layer_param_names=[
                "weight",
                "weight_scale",
                "input_scale",
                "input_zero_point",
                "azp_adj",
            ],
            mining_enabled=self.mining_enabled,
        )

    @override
    def process_weights_after_loading(self, layer: torch.nn.Module) -> None:
        self.kernel.process_weights_after_loading(layer)

    @override
    def apply_weights(
        self, layer: torch.nn.Module, x: torch.Tensor, bias: torch.Tensor | None
    ) -> torch.Tensor:
        return self.kernel.apply_weights(layer, x, bias)
