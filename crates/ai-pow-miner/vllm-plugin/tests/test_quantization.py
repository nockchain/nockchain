"""
Tests for quantization operators and PearlKernel mode comparison.

Focuses on non-trivial tests:
- smooth_scale application
- per-token scale computation
- Mining vs non-mining mode correlation
"""

import pytest
import torch
from utils import DEFAULT_LAYER_PARAM_NAMES, DEFAULT_QUANT_CONFIG

# =============================================================================
# Quantization Operator Tests
# =============================================================================
from vllm_miner.quantization_operators import (
    quant_7bit,
    quant_8bit,
)

# Parametrize: (quant_func, max_val, name)
QUANT_FUNCS = [
    pytest.param(quant_7bit, 63, id="int7"),
    pytest.param(quant_8bit, 127, id="int8"),
]


class TestQuantWithSmoothScale:
    """Tests for quant_7bit and quant_8bit with an optional smooth scale."""

    @pytest.mark.parametrize("quant_func,max_val", QUANT_FUNCS)
    def test_smooth_scale_applied(self, quant_func, max_val):
        """Test that smooth_scale is correctly applied."""
        x = torch.randn(32, 128, dtype=torch.bfloat16, device="cuda")
        smooth_scale = torch.randn(128, dtype=torch.bfloat16, device="cuda").abs() + 0.5

        xq_no_smooth, scale_no_smooth, _ = quant_func(x)
        xq_with_smooth, scale_with_smooth, _ = quant_func(x, smooth_scale=smooth_scale)

        assert not torch.equal(xq_no_smooth, xq_with_smooth), "Quantized values should differ"
        assert not torch.equal(scale_no_smooth, scale_with_smooth), "Scales should differ"

    @pytest.mark.parametrize("quant_func,max_val", QUANT_FUNCS)
    def test_per_token_scales(self, quant_func, max_val):
        """Test that scales are computed per-token (per-row)."""
        x = torch.randn(16, 64, dtype=torch.bfloat16, device="cuda")
        x[0, :] *= 100  # Make first row much larger

        _, xq_scales, _ = quant_func(x)

        assert xq_scales[0] > xq_scales[1:].mean() * 10

    @pytest.mark.parametrize("quant_func,max_val", QUANT_FUNCS)
    def test_output_range(self, quant_func, max_val):
        """Test that quantized values are within expected range.

        Note: int8 range is [-128, 127], so we allow -128 but not +128.
        """
        x = torch.randn(64, 256, dtype=torch.bfloat16, device="cuda") * 10

        xq, _, _ = quant_func(x)

        assert xq.min() >= -128, f"Min {xq.min()} below -128 (int8 min)"
        assert xq.max() <= max_val, f"Max {xq.max()} above {max_val}"


# =============================================================================
# PearlKernel Mode Comparison Tests
# =============================================================================


class DummyLayerWithSmoothScale(torch.nn.Module):
    """Mock layer for testing PearlKernel."""

    def __init__(self, n, k, device="cuda"):
        super().__init__()
        self.weight_q = torch.nn.Parameter(
            torch.randint(-63, 63, (n, k), dtype=torch.int8, device=device),
            requires_grad=False,
        )
        self.weight_s = torch.nn.Parameter(
            torch.ones(n, dtype=torch.float32, device=device) / 64.0,
            requires_grad=False,
        )
        self.input_s = None
        self.input_zp = None
        self.azp_adj = None
        self.logical_widths = [n]
        self.smooth_quant_scale = None


@pytest.fixture
def quant_config():
    """Standard quantization config for tests."""
    return DEFAULT_QUANT_CONFIG


@pytest.fixture(autouse=True)
def async_manager_for_kernel_tests():
    """
    Fixture providing initialized AsyncManager for PearlKernel tests.

    Configures the mining subsystem in isolated test mode:
    - debug=True: Enables verbose logging for test diagnostics
    - no_gateway=True: Disables network communication with mining gateway
    - no_mining=True: Disables proof-of-work computation

    Also initializes required subsystems (pinned memory pool).

    Yields:
        AsyncManager instance if initialization succeeds, None otherwise.
    """
    try:
        from miner_base.settings import MinerSettings
        from vllm_miner.mining_state import (
            get_async_manager,
            init_async_manager,
            init_pinned_pool,
        )

        init_async_manager(MinerSettings(debug=True, no_gateway=True, no_mining=True))
        init_pinned_pool()
        yield get_async_manager()
    except Exception:
        yield None


class TestPearlKernelModeComparison:
    """Tests comparing mining vs non-mining modes."""

    def test_both_modes_produce_correlated_results(self, quant_config):
        """Both modes should produce highly correlated results for same input."""
        from vllm_miner.vllm_kernels import PearlKernel

        m, n, k = 512, 1024, 256

        # Create two kernels
        mining_kernel = PearlKernel(
            quant_config,
            DEFAULT_LAYER_PARAM_NAMES,
            mining_enabled=True,
        )
        non_mining_kernel = PearlKernel(
            quant_config,
            DEFAULT_LAYER_PARAM_NAMES,
            mining_enabled=False,
        )

        # Same layer for both (copy weights to ensure identical)
        layer_mining = DummyLayerWithSmoothScale(n, k)
        layer_non_mining = DummyLayerWithSmoothScale(n, k)
        layer_non_mining.weight_q.data = layer_mining.weight_q.data.clone()
        layer_non_mining.weight_s.data = layer_mining.weight_s.data.clone()

        mining_kernel.process_weights_after_loading(layer_mining)
        non_mining_kernel.process_weights_after_loading(layer_non_mining)

        x = torch.randn(m, k, dtype=torch.bfloat16, device="cuda")
        output_mining = mining_kernel.apply_weights(layer_mining, x)
        output_non_mining = non_mining_kernel.apply_weights(layer_non_mining, x)

        # Both should have same shape and dtype
        assert output_mining.shape == output_non_mining.shape
        assert output_mining.dtype == output_non_mining.dtype

        # Results should be highly correlated (different quantization but same computation)
        correlation = torch.corrcoef(
            torch.stack([output_mining.flatten(), output_non_mining.flatten()])
        )[0, 1]
        assert correlation > 0.9, f"Outputs should be highly correlated, got {correlation}"
