"""
Pearl kernel for vLLM.

Unified kernel supporting two modes:
- Mining mode: int7 quantization + noisy GEMM (for proof-of-work)
- Non-mining mode: int8 quantization + vanilla GEMM (standard inference)

Both modes use pearl GEMM kernels and support smooth_quant_scale.
"""

from typing import override

import torch
from miner_utils import get_logger
from vllm.model_executor.kernels.linear.scaled_mm import (
    Int8ScaledMMLinearKernel,
    Int8ScaledMMLinearLayerConfig,
)
from vllm.model_executor.layers.quantization.utils import replace_parameter
from vllm.model_executor.layers.quantization.utils.w8a8_utils import (
    convert_to_channelwise,
)
from vllm.platforms import current_platform

from .config import config
from .gemm_operators import pearl_gemm_noisy, pearl_gemm_vanilla
from .mining_state import get_async_manager
from .quantization_operators import NO_HADAMARD_BLOCK_SIZE, quant_7bit, quant_8bit

_LOGGER = get_logger("vllm.pearl_miner")


class PearlKernel(Int8ScaledMMLinearKernel):
    """
    Unified kernel supporting mining and non-mining modes.

    Both modes use pearl GEMM kernels.

    Args:
        mining_enabled: If True (default), uses int7 quantization + noisy GEMM (for mining).
                        If False, uses int8 quantization + vanilla GEMM (inference only).
    """

    def __init__(
        self,
        c: Int8ScaledMMLinearLayerConfig,
        layer_param_names: list[str],
        mining_enabled: bool = True,
    ):
        super().__init__(c=c, layer_param_names=layer_param_names)
        self.mining_enabled = mining_enabled
        self.w_q_name = layer_param_names[0]
        self.w_s_name = layer_param_names[1]
        self.i_s_name = layer_param_names[2]
        self.i_zp_name = layer_param_names[3]
        self.azp_adj_name = layer_param_names[4]

    def is_mining_enabled(self) -> bool:
        """Return whether mining is enabled for this kernel."""
        return self.mining_enabled

    @classmethod
    def get_min_capability(cls) -> int:
        # Pearl GEMM kernels require Hopper or newer
        return 9

    @override
    @classmethod
    def can_implement(cls, c: Int8ScaledMMLinearLayerConfig) -> tuple[bool, str | None]:
        if not current_platform.is_cuda():
            return False, "PearlKernel requires running on CUDA."
        return True, None

    @override
    @classmethod
    def is_supported(cls, compute_capability: int | None = None) -> tuple[bool, str | None]:
        """Check if PearlKernel is supported on the current hardware."""
        if compute_capability is None:
            if not current_platform.is_cuda():
                return False, "PearlKernel requires CUDA."
            compute_capability = current_platform.get_device_capability()[0]

        if compute_capability < cls.get_min_capability():
            return (
                False,
                f"PearlKernel requires compute capability >= {cls.get_min_capability()}, got {compute_capability}.",
            )

        return True, None

    @override
    def process_weights_after_loading(self, layer: torch.nn.Module) -> None:
        """
        Processes and prepares weights after model loading.

        Configures weight tensors and scales for blockchain mining
        operations when mining is enabled.

        :param layer: Neural network layer containing weights to process
        """
        # WEIGHT
        # Pearl GEMM kernels expect non-transposed weights
        weight = getattr(layer, self.w_q_name)
        replace_parameter(
            layer,
            self.w_q_name,
            torch.nn.Parameter(weight.data, requires_grad=False),
        )

        # WEIGHT SCALE
        # Handle fused modules (QKV, MLP) with per-tensor scales
        is_fused_module = len(layer.logical_widths) > 1
        weight_scale = getattr(layer, self.w_s_name)
        if is_fused_module and not self.config.is_channelwise:
            weight_scale = convert_to_channelwise(weight_scale, layer.logical_widths)
        replace_parameter(
            layer,
            self.w_s_name,
            torch.nn.Parameter(weight_scale.data, requires_grad=False),
        )

        # INPUT SCALE - only symmetric quantization is supported
        if not self.config.input_symmetric:
            raise NotImplementedError("Only symmetric quantization is supported for pearl GEMM")

        if self.config.is_static_input_scheme:
            input_scale = getattr(layer, self.i_s_name)
            replace_parameter(
                layer,
                self.i_s_name,
                torch.nn.Parameter(input_scale.max(), requires_grad=False),
            )
            setattr(layer, self.i_zp_name, None)
        else:
            setattr(layer, self.i_s_name, None)
            setattr(layer, self.i_zp_name, None)

        setattr(layer, self.azp_adj_name, None)

        # Process smooth_quant_scale if present
        if hasattr(layer, "smooth_quant_scale"):
            scale = layer.smooth_quant_scale
            if scale is not None and not isinstance(scale, torch.nn.Parameter):
                layer.smooth_quant_scale = torch.nn.Parameter(scale.data, requires_grad=False)

        hadamard_block_size_param = getattr(layer, "hadamard_block_size", None)
        layer._hadamard_block_size = (
            int(hadamard_block_size_param.item())
            if hadamard_block_size_param is not None
            else NO_HADAMARD_BLOCK_SIZE
        )

    @override
    def apply_weights(
        self,
        layer: torch.nn.Module,
        x: torch.Tensor,
        bias: torch.Tensor | None = None,
    ) -> torch.Tensor:
        """
        Applies quantized weights to input tensor using pearl GEMM.

        Mining mode: int7 quantization + noisy GEMM (for large matrices) or vanilla GEMM
        Non-mining mode: int8 quantization + vanilla GEMM only

        :param layer: Neural network layer containing quantized weights
        :param x: Input tensor to multiply with weights
        :param bias: Optional bias term to add to result
        :return: Output tensor after weight application
        """
        w_q, w_s, _, _, _ = self._get_layer_params(layer)

        # Get smooth_quant_scale if present
        smooth_scale = None
        if hasattr(layer, "smooth_quant_scale") and layer.smooth_quant_scale is not None:
            smooth_scale = layer.smooth_quant_scale

        if self.mining_enabled:
            return self._apply_weights_mining(
                layer, x, w_q, w_s, smooth_scale, layer._hadamard_block_size, bias
            )
        else:
            return self._apply_weights_non_mining(
                layer, x, w_q, w_s, smooth_scale, layer._hadamard_block_size, bias
            )

    def _apply_weights_mining(
        self,
        layer: torch.nn.Module,
        x: torch.Tensor,
        w_q: torch.Tensor,
        w_s: torch.Tensor,
        smooth_scale: torch.Tensor | None,
        hadamard_block_size: int,
        bias: torch.Tensor | None,
    ) -> torch.Tensor:
        """
        Apply weights in mining mode: int7 quantization + noisy/vanilla GEMM.

        Uses noisy GEMM for large matrices (proof-of-work), vanilla GEMM for small ones.
        """
        x_q, x_s, _ = quant_7bit(x, smooth_scale=smooth_scale, block_size=hadamard_block_size)

        m, k, n = x_q.shape[0], x_q.shape[1], w_q.shape[0]

        # Use noisy GEMM for large matrices (mining), vanilla for small ones
        if config.should_use_noisy_gemm(m, n, k) and not config.settings.no_mining:
            manager = get_async_manager()
            notify_started = getattr(manager, "notify_work_started", None)
            work_id = (
                notify_started(
                    layer=int(getattr(layer, "layer_idx", 0)),
                    token_count=m,
                    common_dim=k,
                    output_dim=n,
                )
                if notify_started is not None
                else None
            )
            try:
                output = pearl_gemm_noisy(
                    x_q.contiguous(),
                    w_q.contiguous(),
                    scale_a=x_s.squeeze(-1),
                    scale_b=w_s.squeeze(-1),
                    out_dtype=x.dtype,
                    layer=layer,
                    submit_block=not manager._conf.skip_block_submission,
                )
            except Exception as error:
                notify_finished = getattr(manager, "notify_work_finished", None)
                if notify_finished is not None:
                    notify_finished(work_id, failed=str(error))
                raise
            if work_id is not None:
                completed = torch.cuda.Event()
                completed.record()
                notify_finished = manager.notify_work_finished
                manager.schedule_status_check(
                    completed,
                    lambda _submit_block, current=work_id: notify_finished(current),
                )
            return output
        else:
            return pearl_gemm_vanilla(
                x_q.contiguous(),
                w_q.contiguous(),
                scale_a=x_s.squeeze(-1),
                scale_b=w_s.squeeze(-1),
                out_dtype=x.dtype,
            )

    def _apply_weights_non_mining(
        self,
        layer: torch.nn.Module,
        x: torch.Tensor,
        w_q: torch.Tensor,
        w_s: torch.Tensor,
        smooth_scale: torch.Tensor | None,
        hadamard_block_size: int,
        bias: torch.Tensor | None,
    ) -> torch.Tensor:
        """
        Apply weights in non-mining mode: int8 quantization + vanilla GEMM only.
        """
        x_q, x_s, _ = quant_8bit(x, smooth_scale=smooth_scale, block_size=hadamard_block_size)

        return pearl_gemm_vanilla(
            x_q.contiguous(),
            w_q.contiguous(),
            scale_a=x_s.squeeze(-1),
            scale_b=w_s.squeeze(-1),
            out_dtype=x.dtype,
        )
