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
from vllm_miner.quantization_operators import quant_7bit, quant_8bit
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
def test_apply_weights_mining_enabled(m, n, k, async_manager):
    """Test PearlKernel with mining_enabled=True (uses int7 + vanilla/noisy GEMM)."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    kernel = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
        mining_enabled=True,
    )

    layer = create_mock_layer(n, k)
    kernel.process_weights_after_loading(layer)

    # Create bfloat16 input tensor
    x = torch.rand((m, k), dtype=torch.bfloat16, device="cuda") * 2 - 1  # Range [-1, 1]

    output = kernel.apply_weights(layer, x)

    # Check output shape and dtypes
    assert output.shape == (m, n)
    assert output.dtype == torch.bfloat16  # Output should be bfloat16

    # Compare with our own int7 quantization (same as kernel uses)
    x_quantized_ref, x_s, _ = quant_7bit(x)
    ref_output = vllm_ops.cutlass_scaled_mm(
        x_quantized_ref,
        layer.weight_q.T,
        scale_a=x_s,
        scale_b=layer.weight_s,
        out_dtype=torch.bfloat16,
        bias=None,
    )

    # Check that outputs are close
    assert torch.allclose(output, ref_output, atol=1e-2, rtol=1e-2)

    # Verify mining is enabled
    assert kernel.is_mining_enabled(), "Mining should be enabled for this kernel"


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

    # Compare with our own int8 quantization (same as kernel uses)
    x_quantized_ref, x_s, _ = quant_8bit(x)
    ref_output = vllm_ops.cutlass_scaled_mm(
        x_quantized_ref,
        layer.weight_q.T,
        scale_a=x_s,
        scale_b=layer.weight_s,
        out_dtype=torch.bfloat16,
        bias=None,
    )

    # Check that outputs are close
    assert torch.allclose(output, ref_output, atol=1e-2, rtol=1e-2), (
        f"Output should match CUTLASS with int8 quantization. Max diff: {torch.abs(output - ref_output).max().item()}"
    )

    # Verify mining is disabled
    assert not kernel.is_mining_enabled(), "Mining should be disabled for this kernel"


@pytest.mark.parametrize("m, n, k", [(512, 1024, 256)])
def test_apply_weights_with_noisy_gemm(m, n, k, async_manager):
    """Test that mining-enabled kernel with large matrices uses noisy GEMM."""
    if not torch.cuda.is_available():
        pytest.skip("CUDA not available")

    # Ensure we have dimensions that trigger noisy GEMM (based on config thresholds)
    # Default thresholds are min_m=1024, min_n=1024, min_k=1024
    # Use dimensions below threshold to test vanilla GEMM fallback

    kernel = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
        mining_enabled=True,
    )

    layer = create_mock_layer(n, k)
    kernel.process_weights_after_loading(layer)

    x = torch.rand((m, k), dtype=torch.bfloat16, device="cuda") * 2 - 1

    output = kernel.apply_weights(layer, x)

    # Check output shape and dtypes
    assert output.shape == (m, n)
    assert output.dtype == torch.bfloat16

    # Compare with our own int7 quantization (same as kernel uses)
    x_quantized_ref, x_s, _ = quant_7bit(x)
    ref_output = vllm_ops.cutlass_scaled_mm(
        x_quantized_ref,
        layer.weight_q.T,
        scale_a=x_s,
        scale_b=layer.weight_s,
        out_dtype=torch.bfloat16,
        bias=None,
    )

    # For small matrices (vanilla GEMM fallback), outputs should be close
    assert torch.allclose(output, ref_output, atol=1e-1, rtol=1e-1)

    # Verify mining is enabled
    assert kernel.is_mining_enabled(), "Mining should be enabled for this kernel"


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

    # Create layer WITH smooth_quant_scale
    layer_with_smooth = create_mock_layer(n, k)
    # Add smooth_quant_scale attribute (per-column scale, shape [k])
    smooth_scale = torch.randn(k, dtype=torch.float32, device="cuda").abs() + 0.5
    layer_with_smooth.smooth_quant_scale = smooth_scale
    kernel.process_weights_after_loading(layer_with_smooth)

    # Create layer WITHOUT smooth_quant_scale (same weights)
    layer_without_smooth = create_mock_layer(n, k)
    layer_without_smooth.weight_q.data = layer_with_smooth.weight_q.data.clone()
    layer_without_smooth.weight_s.data = layer_with_smooth.weight_s.data.clone()

    kernel_no_smooth = PearlKernel(
        DEFAULT_QUANT_CONFIG,
        DEFAULT_LAYER_PARAM_NAMES,
        mining_enabled=mining_enabled,
    )
    kernel_no_smooth.process_weights_after_loading(layer_without_smooth)

    # Same input tensor
    x = torch.rand((m, k), dtype=torch.bfloat16, device="cuda") * 2 - 1

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

    def _make_quant_args(self, num_bits: int, strategy: str = "token", dynamic: bool = True):
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

    def test_get_scheme_returns_mining_scheme_for_7bit(self):
        """Test that _get_scheme_from_parts returns PearlScheme with mining_enabled=True for 7-bit."""
        # Create a minimal PearlConfig with all required arguments
        cfg = PearlConfig(
            target_scheme_map={},
            ignore=[],
            quant_format=None,
            sparsity_scheme_map={},
            sparsity_ignore_list=[],
        )

        weight_quant = self._make_quant_args(7, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(7, strategy="token", dynamic=True)

        scheme = cfg._get_scheme_from_parts(weight_quant, input_quant, layer_name="test_layer")

        assert isinstance(scheme, PearlScheme)
        assert scheme.mining_enabled is True

    def test_get_scheme_returns_non_mining_scheme_for_8bit(self):
        """Test that _get_scheme_from_parts returns PearlScheme with mining_enabled=False for 8-bit."""
        # Create a minimal PearlConfig with all required arguments
        cfg = PearlConfig(
            target_scheme_map={},
            ignore=[],
            quant_format=None,
            sparsity_scheme_map={},
            sparsity_ignore_list=[],
        )

        weight_quant = self._make_quant_args(8, strategy="tensor", dynamic=False)
        input_quant = self._make_quant_args(8, strategy="token", dynamic=True)

        scheme = cfg._get_scheme_from_parts(weight_quant, input_quant, layer_name="test_layer")

        assert isinstance(scheme, PearlScheme)
        assert scheme.mining_enabled is False

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
        patch("vllm.distributed.parallel_state.get_tp_group", return_value=mock_tp_group),
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
    def test_scheme_creates_kernel_with_correct_mode(self, mock_vllm_distributed, mining_enabled):
        """Test that PearlScheme creates kernel with correct mining_enabled setting."""
        scheme = PearlScheme(
            strategy="tensor",
            is_static_input_scheme=False,
            input_symmetric=True,
            mining_enabled=mining_enabled,
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
