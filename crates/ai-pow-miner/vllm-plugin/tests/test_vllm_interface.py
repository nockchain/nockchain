"""
Tests for PearlKernel vLLM interface.

Tests both mining_enabled=True and mining_enabled=False modes.
"""

from unittest.mock import MagicMock, patch

import pytest
import torch
from compressed_tensors.quantization import QuantizationArgs
from utils import (
    DEFAULT_LAYER_PARAM_NAMES,
    DEFAULT_QUANT_CONFIG,
    create_mock_layer,
)
from vllm import _custom_ops as vllm_ops
from vllm_miner import PearlKernel
from vllm_miner.config import config as pearl_config
from vllm_miner.quantization_operators import quant_8bit
from vllm_miner.vllm_config import PearlConfig
from vllm_miner.vllm_scheme import PearlScheme


@pytest.fixture(autouse=True)
def async_manager():
    try:
        from miner_base.settings import MinerSettings
        from vllm_miner.mining_state import (
            get_async_manager,
            init_async_manager,
            init_pinned_pool,
        )

        init_async_manager(MinerSettings(debug=True, no_gateway=True))
        init_pinned_pool()
        yield get_async_manager()
    except ImportError:
        # Skip if vLLM miner modules not available
        yield None


@pytest.fixture(autouse=True)
def reset_mining_all(async_manager):
    try:
        import torch

        if torch.cuda.is_available():
            torch.cuda.empty_cache()
            torch.cuda.synchronize()
        yield
        if async_manager is not None:
            async_manager.wait_until_done_submitting_blocks()
        if torch.cuda.is_available():
            torch.cuda.empty_cache()
            torch.cuda.synchronize()
    except ImportError:
        pass


@pytest.mark.parametrize("m, n, k", [(1024, 4096, 128)])
def test_mining_kernel_rejects_non_gemma_shape(m, n, k, async_manager):
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    kernel = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
        mining_enabled=True,
    )
    layer = create_mock_layer(n, k)
    kernel.process_weights_after_loading(layer)
    x = torch.rand((m, k), dtype=torch.bfloat16, device="cuda") * 2 - 1

    with pytest.raises(ValueError, match="native Gemma mining requires K=5376"):
        kernel.apply_weights(layer, x)


@pytest.mark.parametrize("m, n, k", [(1024, 4096, 128)])
def test_apply_weights_mining_disabled(m, n, k, async_manager):
    """Test PearlKernel with mining_enabled=False (uses int8 + vanilla GEMM)."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    kernel = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
        mining_enabled=False,
    )

    layer = create_mock_layer(n, k)
    kernel.process_weights_after_loading(layer)

    # Create bfloat16 input tensor
    x = torch.rand((m, k), dtype=torch.bfloat16, device="cuda") * 2 - 1  # Range [-1, 1]

    output = kernel.apply_weights(layer, x)

    # Check output shape and dtypes
    assert output.shape == (m, n)
    assert output.dtype == torch.bfloat16  # Output should be bfloat16

    # Compare with the same INT8 accumulator and scaling contract.
    x_quantized_ref, x_s, _ = quant_8bit(x)
    if kernel._weight_transposed:
        assert layer.weight_q.shape == (k, n)
        assert layer.weight_q.is_contiguous()
        accumulator = torch._int_mm(x_quantized_ref, layer.weight_q)
        ref_output = (
            accumulator.float() * x_s.reshape(-1, 1) * layer.weight_s.reshape(1, -1)
        ).to(torch.bfloat16)
    else:
        ref_output = vllm_ops.cutlass_scaled_mm(
            x_quantized_ref,
            layer.weight_q.T,
            scale_a=x_s,
            scale_b=layer.weight_s,
            out_dtype=torch.bfloat16,
            bias=None,
        )

    # Check that outputs are close
    assert torch.allclose(output, ref_output, atol=1.25e-1, rtol=1e-2), (
        f"Output should match INT8 GEMM within BF16 accumulation tolerance. "
        f"Max diff: {torch.abs(output - ref_output).max().item()}"
    )

    # Verify mining is disabled
    assert not kernel.is_mining_enabled(), "Mining should be disabled for this kernel"


def test_kernel_default_is_mining_enabled():
    """Test that PearlKernel defaults to mining_enabled=True."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    # Create kernel without specifying mining_enabled
    kernel = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
    )

    # Default should be mining_enabled=True
    assert kernel.is_mining_enabled(), "Default should be mining_enabled=True"


@pytest.mark.parametrize("mining_enabled", [True, False])
def test_apply_weights_with_smooth_quant_scale(mining_enabled, async_manager):
    """Test that smooth_quant_scale is correctly applied in apply_weights path."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    m, n, k = 512, 1024, 256

    kernel = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
        mining_enabled=mining_enabled,
    )

    # Create matching layers before weight processing, which can change the
    # device-specific inference layout.
    layer_with_smooth = create_mock_layer(n, k)
    smooth_scale = torch.randn(k, dtype=torch.float32, device="cuda").abs() + 0.5
    layer_with_smooth.smooth_quant_scale = smooth_scale

    layer_without_smooth = create_mock_layer(n, k)
    layer_without_smooth.weight_q.data.copy_(layer_with_smooth.weight_q.data)
    layer_without_smooth.weight_s.data.copy_(layer_with_smooth.weight_s.data)

    kernel_no_smooth = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
        mining_enabled=mining_enabled,
    )
    kernel.process_weights_after_loading(layer_with_smooth)
    kernel_no_smooth.process_weights_after_loading(layer_without_smooth)

    # Same input tensor
    x = torch.rand((m, k), dtype=torch.bfloat16, device="cuda") * 2 - 1

    if mining_enabled:
        with pytest.raises(ValueError, match="native Gemma mining requires K=5376"):
            kernel.apply_weights(layer_with_smooth, x)
        return

    output_with_smooth = kernel.apply_weights(layer_with_smooth, x)
    output_without_smooth = kernel_no_smooth.apply_weights(layer_without_smooth, x)

    # Outputs should have correct shape and dtype
    assert output_with_smooth.shape == (m, n)
    assert output_with_smooth.dtype == torch.bfloat16

    # Outputs should be DIFFERENT when smooth_scale is applied
    # (smooth_scale divides the input before quantization, changing the result)
    assert not torch.allclose(output_with_smooth, output_without_smooth, atol=1e-3), (
        "Outputs should differ when smooth_quant_scale is applied"
    )

    # But outputs should still be correlated (same underlying computation)
    correlation = torch.corrcoef(
        torch.stack([output_with_smooth.flatten(), output_without_smooth.flatten()])
    )[0, 1]
    assert correlation > 0.5, f"Outputs should be correlated, got {correlation}"


# =============================================================================
# Noisy GEMM Selection Threshold Tests
# =============================================================================


class TestNoisyGemmSelectionThresholds:
    """Tests for noisy GEMM selection based on matrix dimensions."""

    def test_should_use_noisy_gemm_all_above_threshold(self):
        """Test that noisy GEMM is selected when all dimensions >= threshold."""
        # Default thresholds are min_m=1024, min_n=256, min_k=1024
        # All dimensions at or above threshold
        assert pearl_config.should_use_noisy_gemm(1024, 1024, 1024) is True
        assert pearl_config.should_use_noisy_gemm(2048, 2048, 2048) is True
        assert pearl_config.should_use_noisy_gemm(4096, 8192, 1024) is True

    def test_should_use_noisy_gemm_below_m_threshold(self):
        """Test that vanilla GEMM is selected when m < threshold."""
        # m below threshold
        assert pearl_config.should_use_noisy_gemm(512, 1024, 1024) is False
        assert pearl_config.should_use_noisy_gemm(1, 2048, 2048) is False

    def test_should_use_noisy_gemm_below_n_threshold(self):
        """Test that vanilla GEMM is selected when n < threshold."""
        # n below threshold
        assert pearl_config.should_use_noisy_gemm(1024, 128, 1024) is False
        assert pearl_config.should_use_noisy_gemm(2048, 1, 2048) is False

    def test_should_use_noisy_gemm_below_k_threshold(self):
        """Test that vanilla GEMM is selected when k < threshold."""
        # k below threshold
        assert pearl_config.should_use_noisy_gemm(1024, 1024, 512) is False
        assert pearl_config.should_use_noisy_gemm(2048, 2048, 1) is False

    def test_should_use_noisy_gemm_degenerate_dimensions(self):
        """Test that degenerate dimensions (1) always use vanilla GEMM."""
        # Any dimension == 1 should return False (degenerate case)
        assert pearl_config.should_use_noisy_gemm(1, 2048, 2048) is False
        assert pearl_config.should_use_noisy_gemm(2048, 1, 2048) is False
        assert pearl_config.should_use_noisy_gemm(2048, 2048, 1) is False

    def test_should_use_noisy_gemm_boundary_cases(self):
        """Test boundary cases at exactly the threshold."""
        # Exactly at threshold - should use noisy GEMM
        assert pearl_config.should_use_noisy_gemm(1024, 1024, 1024) is True
        assert pearl_config.should_use_noisy_gemm(1024, 256, 1024) is True

        # Just below threshold - should use vanilla GEMM
        assert pearl_config.should_use_noisy_gemm(1023, 1024, 1024) is False
        assert pearl_config.should_use_noisy_gemm(1024, 255, 1024) is False
        assert pearl_config.should_use_noisy_gemm(1024, 1024, 1023) is False


# =============================================================================
# PearlConfig Tests
# =============================================================================


class TestPearlConfig:
    """Tests for PearlConfig layer detection and scheme selection."""

    def _make_quant_args(
        self, num_bits: int, strategy: str = "token", dynamic: bool = True
    ):
        """Helper to create QuantizationArgs for testing."""
        return QuantizationArgs(
            num_bits=num_bits,
            type="int",
            strategy=strategy,
            symmetric=True,
            dynamic=dynamic,
        )

    def test_is_mining_layer_7bit(self):
        """Test that 7-bit config is detected as mining layer."""
        weight_quant = self._make_quant_args(7, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(7, strategy="token", dynamic=True)

        assert PearlConfig._is_mining_layer(weight_quant, input_quant) is True

    def test_is_mining_layer_8bit_not_mining(self):
        """Test that 8-bit config is NOT detected as mining layer."""
        weight_quant = self._make_quant_args(8, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(8, strategy="token", dynamic=True)

        assert PearlConfig._is_mining_layer(weight_quant, input_quant) is False

    def test_is_non_mining_layer_8bit(self):
        """Test that 8-bit config is detected as non-mining layer."""
        weight_quant = self._make_quant_args(8, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(8, strategy="token", dynamic=True)

        assert PearlConfig._is_non_mining_layer(weight_quant, input_quant) is True

    def test_is_non_mining_layer_7bit_not_non_mining(self):
        """Test that 7-bit config is NOT detected as non-mining layer."""
        weight_quant = self._make_quant_args(7, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(7, strategy="token", dynamic=True)

        assert PearlConfig._is_non_mining_layer(weight_quant, input_quant) is False

    def test_get_scheme_mines_only_selected_gate_up_layer(self):
        cfg = PearlConfig(
            target_scheme_map={},
            ignore=[],
            quant_format=None,
        )
        weight_quant = self._make_quant_args(7, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(7, strategy="token", dynamic=True)

        selected = cfg._get_scheme_from_parts(
            weight_quant,
            input_quant,
            layer_name="model.language_model.layers.0.mlp.gate_up_proj",
        )
        other = cfg._get_scheme_from_parts(
            weight_quant,
            input_quant,
            layer_name="model.language_model.layers.1.mlp.gate_up_proj",
        )
        assert isinstance(selected, PearlScheme)
        assert selected.mining_enabled is True
        assert selected.input_num_bits == 7
        assert isinstance(other, PearlScheme)
        assert other.mining_enabled is False
        assert other.input_num_bits == 7

    def test_get_scheme_returns_non_mining_scheme_for_8bit(self):
        """Test that _get_scheme_from_parts returns PearlScheme with mining_enabled=False for 8-bit."""
        # Create a minimal PearlConfig with all required arguments
        cfg = PearlConfig(
            target_scheme_map={},
            ignore=[],
            quant_format=None,
        )

        weight_quant = self._make_quant_args(8, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(8, strategy="token", dynamic=True)

        scheme = cfg._get_scheme_from_parts(
            weight_quant, input_quant, layer_name="test_layer"
        )

        assert isinstance(scheme, PearlScheme)
        assert scheme.mining_enabled is False
        assert scheme.input_num_bits == 8

    def test_channel_strategy_also_works(self):
        """Test that channel strategy (not just tensor) works for detection."""
        weight_quant = self._make_quant_args(7, strategy="channel", dynamic=False)
        input_quant = self._make_quant_args(7, strategy="token", dynamic=True)

        assert PearlConfig._is_mining_layer(weight_quant, input_quant) is True

    def test_none_quant_args_returns_false(self):
        """Test that None quant args return False for layer detection."""
        assert PearlConfig._is_mining_layer(None, None) is False
        assert PearlConfig._is_non_mining_layer(None, None) is False

        weight_quant = self._make_quant_args(7, strategy="tensor", dynamic=False)
        assert PearlConfig._is_mining_layer(weight_quant, None) is False
        assert PearlConfig._is_mining_layer(None, weight_quant) is False


# =============================================================================
# Quantization Scheme Tests
# =============================================================================


@pytest.fixture
def mock_vllm_distributed():
    """Mock vLLM's distributed parallel state for testing."""
    # Create a mock GroupCoordinator
    mock_tp_group = MagicMock()
    mock_tp_group.rank_in_group = 0
    mock_tp_group.world_size = 1

    with (
        patch("vllm.distributed.parallel_state._TP", mock_tp_group),
        patch(
            "vllm.distributed.parallel_state.get_tp_group", return_value=mock_tp_group
        ),
        patch(
            "vllm.distributed.parallel_state.get_tensor_model_parallel_rank",
            return_value=0,
        ),
        patch(
            "vllm.distributed.parallel_state.get_tensor_model_parallel_world_size",
            return_value=1,
        ),
    ):
        yield


class TestPearlScheme:
    """Tests for PearlScheme."""

    @pytest.mark.parametrize("mining_enabled", [True, False])
    def test_scheme_creates_kernel_with_correct_mode(
        self, mock_vllm_distributed, mining_enabled
    ):
        """Test that PearlScheme creates kernel with correct mining_enabled setting."""
        scheme = PearlScheme(
            strategy="tensor",
            is_static_input_scheme=False,
            input_symmetric=True,
            mining_enabled=mining_enabled,
            input_num_bits=7 if mining_enabled else 8,
        )

        layer = torch.nn.Module()

        def weight_loader(param, loaded_weight, *args, **kwargs):
            param.data.copy_(loaded_weight)

        scheme.create_weights(
            layer=layer,
            output_partition_sizes=[512],
            input_size_per_partition=256,
            params_dtype=torch.bfloat16,
            weight_loader=weight_loader,
        )

        assert hasattr(scheme, "kernel")
        assert scheme.kernel.mining_enabled is mining_enabled
        assert scheme.kernel.input_num_bits == (7 if mining_enabled else 8)


def test_native_tp_gate_up_reordering_is_canonical():
    rank_major = torch.tensor([[10], [20], [11], [21]], dtype=torch.int8)
    canonical = PearlKernel._canonical_gate_up_shards(rank_major, 2)
    assert canonical.tolist() == [[10], [11], [20], [21]]
    full_output = torch.tensor([[10, 11, 20, 21]], dtype=torch.bfloat16)
    local = PearlKernel._local_gate_up_output(full_output, rank=1, world_size=2)
    assert local.tolist() == [[11, 21]]


def test_native_tp_follower_reuses_local_sm120_weight_layout():
    kernel = object.__new__(PearlKernel)
    kernel._tp_follower_weight_transposed = None
    kernel._tp_follower_weight_key = None
    x_q = torch.empty((2, 3), dtype=torch.int8)
    x_s = torch.empty((2, 1), dtype=torch.float32)
    w_q = torch.empty((4, 3), dtype=torch.int8)
    w_s = torch.empty((4, 1), dtype=torch.float32)
    expected = torch.empty((2, 4), dtype=torch.bfloat16)
    with (
        patch(
            "vllm_miner.vllm_kernels.torch.cuda.get_device_capability",
            return_value=(12, 0),
        ),
        patch(
            "vllm_miner.vllm_kernels.pearl_gemm_vanilla",
            return_value=expected,
        ) as gemm,
    ):
        assert kernel._apply_tp_follower_gemma4(x_q, x_s, w_q, w_s) is expected
        assert kernel._apply_tp_follower_gemma4(x_q, x_s, w_q, w_s) is expected

    first_weight = gemm.call_args_list[0].args[1]
    second_weight = gemm.call_args_list[1].args[1]
    assert first_weight.shape == (3, 4)
    assert first_weight.is_contiguous()
    assert second_weight.data_ptr() == first_weight.data_ptr()
    assert all(call.kwargs["weight_transposed"] for call in gemm.call_args_list)


def test_native_tp_follower_does_not_run_mining():
    kernel = object.__new__(PearlKernel)
    x_q = torch.empty((2, 3), dtype=torch.int8)
    x_s = torch.empty((2, 1), dtype=torch.float32)
    w_q = torch.empty((4, 3), dtype=torch.int8)
    w_s = torch.empty((4, 1), dtype=torch.float32)
    full_weight = torch.empty((8, 3), dtype=torch.int8)
    full_scale = torch.empty((8, 1), dtype=torch.float32)
    expected = torch.empty((2, 4), dtype=torch.bfloat16)
    kernel._full_tp_weight = MagicMock(return_value=(full_weight, full_scale, 1, 4))
    kernel._apply_tp_follower_gemma4 = MagicMock(return_value=expected)

    with (
        patch(
            "vllm_miner.vllm_kernels.get_tensor_model_parallel_world_size",
            return_value=2,
        ),
        patch("vllm_miner.vllm_kernels.get_async_manager") as manager,
    ):
        result = kernel._apply_native_gemma4_impl(0, x_q, x_s, w_q, w_s)

    assert result is expected
    kernel._apply_tp_follower_gemma4.assert_called_once_with(x_q, x_s, w_q, w_s)
    manager.assert_not_called()


def test_native_sessions_remain_live_across_batch_shapes():
    kernel = object.__new__(PearlKernel)
    kernel._native_sessions = {}
    a_256 = torch.empty((256, 2), dtype=torch.int8)
    a_512 = torch.empty((512, 2), dtype=torch.int8)
    weight = torch.empty((4, 2), dtype=torch.int8)
    session_256 = MagicMock()
    session_512 = MagicMock()
    with (
        patch("vllm_miner.vllm_kernels.torch.cuda.current_device", return_value=0),
        patch(
            "vllm_miner.vllm_kernels.NativeGemma4Session",
            side_effect=[session_256, session_512],
        ) as create,
    ):
        assert kernel._native_session_for(a_256, weight) is session_256
        assert kernel._native_session_for(a_256, weight) is session_256
        assert kernel._native_session_for(a_512, weight) is session_512
        assert kernel._native_session_for(a_256, weight) is session_256
    assert create.call_count == 2
    assert session_256.bind.call_count == 2
    session_512.bind.assert_not_called()


def test_native_tp_weight_uses_rank_order_and_caches_full_matrix():
    kernel = object.__new__(PearlKernel)
    kernel._native_full_weight = None
    kernel._native_full_scale = None
    kernel._native_full_weight_key = None
    local_weight = torch.empty((21504, 5376), dtype=torch.int8, device="meta")
    local_scale = torch.empty((21504, 1), dtype=torch.float32, device="meta")
    full_weight = torch.empty((43008, 5376), dtype=torch.int8, device="meta")
    full_scale = torch.empty((43008, 1), dtype=torch.float32, device="meta")
    with (
        patch(
            "vllm_miner.vllm_kernels.get_tensor_model_parallel_world_size",
            return_value=2,
        ),
        patch(
            "vllm_miner.vllm_kernels.get_tensor_model_parallel_rank",
            return_value=1,
        ),
        patch(
            "vllm_miner.vllm_kernels.tensor_model_parallel_all_gather",
            side_effect=[full_weight, full_scale],
        ) as gather,
    ):
        weight, scale, rank, local_n = kernel._full_tp_weight(local_weight, local_scale)
        cached = kernel._full_tp_weight(local_weight, local_scale)
    assert weight.shape == (43008, 5376)
    assert scale.shape == (43008, 1)
    assert cached[0] is weight
    assert cached[1] is scale
    assert rank == 1
    assert local_n == 21504
    assert gather.call_count == 2
