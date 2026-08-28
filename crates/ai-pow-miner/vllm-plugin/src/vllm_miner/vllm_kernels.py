"""
Pearl kernel for vLLM.

Unified kernel supporting two modes:
- Mining mode: int7 quantization + noisy GEMM (for proof-of-work)
- Non-mining mode: int8 quantization + vanilla GEMM (standard inference)

Both modes use pearl GEMM kernels and support smooth_quant_scale.
"""

from threading import Lock
from typing import Any, override

import torch
from miner_base.gpu_matmul_config import GPUMatmulConfigFactory
from miner_utils import get_logger
from pearl_gateway.comm.dataclasses import CommitmentHash, OpenedBlockInfo
from vllm.distributed import (
    get_tensor_model_parallel_rank,
    get_tensor_model_parallel_world_size,
)
from vllm.distributed.communication_op import tensor_model_parallel_all_gather
from vllm.model_executor.kernels.linear.scaled_mm import (
    Int8ScaledMMLinearKernel,
    Int8ScaledMMLinearLayerConfig,
)
from vllm.model_executor.layers.quantization.utils import replace_parameter
from vllm.model_executor.layers.quantization.utils.w8a8_utils import (
    convert_to_channelwise,
)
from vllm.platforms import current_platform
from vllm.utils.torch_utils import direct_register_custom_op

from .config import config
from .gemm_operators import pearl_gemm_vanilla
from .mining_state import get_async_manager
from .nockchain_manager import NockchainAsyncLoopManager
from .native_gemma4 import NativeGemma4Session, NativeGemma4SessionCache
from .quantization_operators import NO_HADAMARD_BLOCK_SIZE, quant_7bit, quant_8bit

_LOGGER = get_logger("vllm.pearl_miner")
_MINING_KERNELS: dict[int, Any] = {}
_DENSE_MINING_KERNEL_KEY = 0


def _pearl_mining_gemm_impl(
    kernel_id: int,
    layer_idx: int,
    x_q: torch.Tensor,
    x_s: torch.Tensor,
    w_q: torch.Tensor,
    w_s: torch.Tensor,
) -> torch.Tensor:
    kernel = _MINING_KERNELS[kernel_id]
    return kernel._apply_native_gemma4_impl(layer_idx, x_q, x_s, w_q, w_s)


def _pearl_mining_gemm_fake(
    kernel_id: int,
    layer_idx: int,
    x_q: torch.Tensor,
    x_s: torch.Tensor,
    w_q: torch.Tensor,
    w_s: torch.Tensor,
) -> torch.Tensor:
    del kernel_id, layer_idx, x_s, w_s
    return torch.empty(
        (x_q.shape[0], w_q.shape[0]), dtype=torch.bfloat16, device=x_q.device
    )


def _gemma4_mining_transcript(manager: Any, mining_job: Any) -> tuple[bytes, bytes]:
    if isinstance(manager, NockchainAsyncLoopManager):
        return manager.get_mining_transcript(mining_job)
    matmul_config = GPUMatmulConfigFactory.create(
        k=5376, noise_rank=config.settings.noise_rank
    )
    adjusted_target = mining_job.adjust_target(
        mining_config=matmul_config.mining_config
    )
    return (
        bytes(matmul_config.mining_config.to_bytes()),
        adjusted_target.to_bytes(32, "little"),
    )


direct_register_custom_op(
    "pearl_mining_gemm",
    _pearl_mining_gemm_impl,
    fake_impl=_pearl_mining_gemm_fake,
)


class PearlKernel(Int8ScaledMMLinearKernel):
    """
    Unified kernel supporting mining and non-mining modes.

    Both modes use pearl GEMM kernels.

    Args:
        mining_enabled: If True, emits consensus work for this layer.
        input_num_bits: Dynamic activation quantization width (7 or 8).
    """

    def __init__(
        self,
        c: Int8ScaledMMLinearLayerConfig,
        layer_param_names: list[str],
        mining_enabled: bool = True,
        input_num_bits: int = 7,
    ):
        super().__init__(c=c, layer_param_names=layer_param_names)
        self.mining_enabled = mining_enabled
        if input_num_bits not in (7, 8):
            raise ValueError(
                f"PearlKernel input_num_bits must be 7 or 8, got {input_num_bits}"
            )
        self.input_num_bits = input_num_bits
        # Native calls synchronize their completion events before returning.
        # Keep one shape and close it before allocating another full workspace.
        self._native_sessions = NativeGemma4SessionCache()
        self._native_session_lock = Lock()
        self._native_full_weight: torch.Tensor | None = None
        self._native_full_scale: torch.Tensor | None = None
        self._native_full_weight_key: tuple[int, int] | None = None
        self._tp_follower_weight_transposed: torch.Tensor | None = None
        self._tp_follower_weight_key: int | None = None
        self._weight_transposed = False
        self._mining_kernel_id = _DENSE_MINING_KERNEL_KEY
        if mining_enabled:
            _MINING_KERNELS[_DENSE_MINING_KERNEL_KEY] = self
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
    def is_supported(
        cls, compute_capability: int | None = None
    ) -> tuple[bool, str | None]:
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
        # SM12x torch._int_mm consumes a contiguous (K, N) weight. Cache that
        # layout for non-mining layers; recreating it during every decode step
        # copies up to 231 MB per projection.
        weight = getattr(layer, self.w_q_name)
        if (
            not self.mining_enabled
            and weight.is_cuda
            and torch.cuda.get_device_capability(weight.device)[0] == 12
        ):
            weight_data = weight.T.contiguous()
            self._weight_transposed = True
        else:
            weight_data = weight.data
        replace_parameter(
            layer,
            self.w_q_name,
            torch.nn.Parameter(weight_data, requires_grad=False),
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
            raise NotImplementedError(
                "Only symmetric quantization is supported for pearl GEMM"
            )

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
                layer.smooth_quant_scale = torch.nn.Parameter(
                    scale.data, requires_grad=False
                )

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
        if (
            hasattr(layer, "smooth_quant_scale")
            and layer.smooth_quant_scale is not None
        ):
            smooth_scale = layer.smooth_quant_scale

        if self.mining_enabled:
            return self._apply_weights_mining(
                layer, x, w_q, w_s, smooth_scale, layer._hadamard_block_size, bias
            )
        else:
            return self._apply_weights_non_mining(
                layer, x, w_q, w_s, smooth_scale, layer._hadamard_block_size, bias
            )

    def _native_session_for(
        self, a: torch.Tensor, b: torch.Tensor
    ) -> NativeGemma4Session:
        return self._native_sessions.get(a, b)

    @staticmethod
    def _submit_native_winner(
        manager,
        mining_job,
        preparation,
        ordinal: int,
        a: torch.Tensor,
        b: torch.Tensor,
    ) -> None:
        col_tiles = b.shape[0] // 16
        row_start = (ordinal // col_tiles) * 16
        col_start = (ordinal % col_tiles) * 16
        opened = OpenedBlockInfo(
            A_row_indices=list(range(row_start, row_start + 16)),
            B_column_indices=list(range(col_start, col_start + 16)),
            A=a.cpu().detach(),
            B_t=b.cpu().detach(),
            commitment_hash=CommitmentHash(
                noise_seed_A=preparation.s_a,
                noise_seed_B=preparation.s_b,
            ),
            noise_rank=128,
        )
        manager.handle_submit_block(opened, mining_job)

    @staticmethod
    def _canonical_gate_up_shards(
        rank_major: torch.Tensor, world_size: int
    ) -> torch.Tensor:
        if rank_major.shape[0] % (2 * world_size):
            raise ValueError("gate/up TP shards do not divide the fused output")
        per_projection_rank = rank_major.shape[0] // (2 * world_size)
        tail = rank_major.shape[1:]
        shards = rank_major.reshape(world_size, 2, per_projection_rank, *tail)
        permutation = (1, 0, 2, *range(3, shards.ndim))
        return shards.permute(permutation).contiguous().reshape(rank_major.shape)

    @staticmethod
    def _local_gate_up_output(
        full_output: torch.Tensor, rank: int, world_size: int
    ) -> torch.Tensor:
        projection_width = full_output.shape[1] // 2
        if projection_width % world_size:
            raise ValueError("gate/up output does not divide the TP world")
        local_width = projection_width // world_size
        gate_start = rank * local_width
        up_start = projection_width + gate_start
        return torch.cat(
            (
                full_output[:, gate_start : gate_start + local_width],
                full_output[:, up_start : up_start + local_width],
            ),
            dim=1,
        )

    def _full_tp_weight(
        self, w_q: torch.Tensor, w_s: torch.Tensor
    ) -> tuple[torch.Tensor, torch.Tensor, int, int]:
        world_size = get_tensor_model_parallel_world_size()
        rank = get_tensor_model_parallel_rank()
        local_n = w_q.shape[0]
        if local_n * world_size != 43008 or w_q.shape[1] != 5376:
            raise ValueError(
                "native Gemma TP shards must concatenate to B=(43008, 5376)"
            )
        if world_size == 1:
            return w_q, w_s, rank, local_n
        key = (w_q.data_ptr(), world_size)
        if self._native_full_weight is None or self._native_full_weight_key != key:
            rank_major_weight = tensor_model_parallel_all_gather(
                w_q.contiguous(), dim=0
            )
            rank_major_scale = tensor_model_parallel_all_gather(w_s.contiguous(), dim=0)
            self._native_full_weight = self._canonical_gate_up_shards(
                rank_major_weight, world_size
            )
            self._native_full_scale = self._canonical_gate_up_shards(
                rank_major_scale, world_size
            )
            self._native_full_weight_key = key
        assert self._native_full_scale is not None
        return self._native_full_weight, self._native_full_scale, rank, local_n

    def _apply_native_gemma4(
        self,
        layer: torch.nn.Module,
        x_q: torch.Tensor,
        x_s: torch.Tensor,
        w_q: torch.Tensor,
        w_s: torch.Tensor,
        bias: torch.Tensor | None,
    ) -> torch.Tensor:
        if x_q.shape[1] != 5376:
            raise ValueError(f"native Gemma mining requires K=5376, got {x_q.shape[1]}")
        if x_q.shape[0] > 8192:
            raise ValueError("native Gemma mining supports at most 8192 logical tokens")
        if bias is not None:
            raise ValueError("native Gemma gate/up does not support bias")
        return torch.ops.vllm.pearl_mining_gemm(
            self._mining_kernel_id,
            int(getattr(layer, "layer_idx", 0)),
            x_q,
            x_s,
            w_q,
            w_s,
        )

    def _apply_tp_follower_gemma4(
        self,
        x_q: torch.Tensor,
        x_s: torch.Tensor,
        w_q: torch.Tensor,
        w_s: torch.Tensor,
    ) -> torch.Tensor:
        weight = w_q
        weight_transposed = False
        if torch.cuda.get_device_capability(w_q.device)[0] == 12:
            key = w_q.data_ptr()
            if (
                self._tp_follower_weight_transposed is None
                or self._tp_follower_weight_key != key
            ):
                self._tp_follower_weight_transposed = w_q.T.contiguous()
                self._tp_follower_weight_key = key
            weight = self._tp_follower_weight_transposed
            weight_transposed = True
        return pearl_gemm_vanilla(
            x_q,
            weight,
            x_s,
            w_s,
            torch.bfloat16,
            weight_transposed=weight_transposed,
        )

    def _apply_native_gemma4_impl(
        self,
        layer_idx: int,
        x_q: torch.Tensor,
        x_s: torch.Tensor,
        w_q: torch.Tensor,
        w_s: torch.Tensor,
    ) -> torch.Tensor:
        full_weight, full_scale, tp_rank, _local_n = self._full_tp_weight(w_q, w_s)
        tp_world = get_tensor_model_parallel_world_size()
        manager = get_async_manager()
        if isinstance(manager, NockchainAsyncLoopManager):
            manager.set_runtime_state(
                rank_zero=tp_rank == 0,
                mining_enabled=self.mining_enabled and not config.settings.no_mining,
            )
        if tp_rank != 0:
            return self._apply_tp_follower_gemma4(x_q, x_s, w_q, w_s)
        mining_job = manager.get_mining_job()
        mu, target = _gemma4_mining_transcript(manager, mining_job)
        sigma = mining_job.incomplete_header_bytes
        scale_a = x_s.squeeze(-1).to(torch.float32).contiguous()
        scale_b = full_scale.squeeze(-1).to(torch.float32).contiguous()
        outputs = []
        for start in range(0, x_q.shape[0], 4096):
            logical_m = min(4096, x_q.shape[0] - start)
            padded_m = ((logical_m + 255) // 256) * 256
            source = x_q[start : start + logical_m].contiguous()
            if padded_m == logical_m:
                padded = source
            else:
                padded = torch.zeros(
                    (padded_m, 5376), device=x_q.device, dtype=torch.int8
                )
                padded[:logical_m].copy_(source)
            output = torch.empty(
                (logical_m, 43008), device=x_q.device, dtype=torch.bfloat16
            )
            work_id = (
                manager.notify_work_started(
                    layer=layer_idx,
                    token_count=logical_m,
                    common_dim=5376,
                    output_dim=43008,
                )
                if tp_rank == 0
                else None
            )
            try:
                with self._native_session_lock:
                    session = self._native_session_for(padded, full_weight)
                    preparation = session.prepare(sigma, mu)
                    inference = session.infer(
                        logical_m=logical_m,
                        a_scales=scale_a[start : start + logical_m],
                        b_scales=scale_b,
                        target=target,
                        output=output,
                    )
                    if inference.winner_ordinal is not None and tp_rank == 0:
                        self._submit_native_winner(
                            manager,
                            mining_job,
                            preparation,
                            inference.winner_ordinal,
                            padded,
                            full_weight,
                        )
            except Exception as error:
                if work_id is not None:
                    manager.notify_work_finished(work_id, failed=str(error))
                raise
            if work_id is not None:
                manager.notify_work_finished(work_id)
            outputs.append(output)
        full_output = outputs[0] if len(outputs) == 1 else torch.cat(outputs, dim=0)
        return self._local_gate_up_output(full_output, tp_rank, tp_world)

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
        """Apply the selected fused gate/up projection with consensus mining."""
        x_q, x_s, _ = quant_7bit(
            x, smooth_scale=smooth_scale, block_size=hadamard_block_size
        )
        if config.settings.no_mining:
            return pearl_gemm_vanilla(
                x_q.contiguous(),
                w_q.contiguous(),
                scale_a=x_s.squeeze(-1),
                scale_b=w_s.squeeze(-1),
                out_dtype=x.dtype,
            )
        if x.dtype is not torch.bfloat16:
            raise ValueError(f"native Gemma output requires bfloat16, got {x.dtype}")
        return self._apply_native_gemma4(
            layer,
            x_q.contiguous(),
            x_s,
            w_q.contiguous(),
            w_s,
            bias,
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
        """Apply inference-only INT7 or INT8 weights."""
        quantize = quant_7bit if self.input_num_bits == 7 else quant_8bit
        x_q, x_s, _ = quantize(
            x, smooth_scale=smooth_scale, block_size=hadamard_block_size
        )

        return pearl_gemm_vanilla(
            x_q.contiguous(),
            w_q.contiguous(),
            scale_a=x_s.squeeze(-1),
            scale_b=w_s.squeeze(-1),
            out_dtype=x.dtype,
            weight_transposed=self._weight_transposed,
        )
