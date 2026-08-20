import torch
from vllm.model_executor.kernels.linear.scaled_mm import Int8ScaledMMLinearLayerConfig

DEFAULT_QUANT_CONFIG = Int8ScaledMMLinearLayerConfig(
    is_static_input_scheme=False,
    input_symmetric=True,
    is_channelwise=False,
)

DEFAULT_LAYER_PARAM_NAMES = ["weight_q", "weight_s", "input_s", "input_zp", "azp_adj"]


class DummyLayer(torch.nn.Module):
    def __init__(self, n, k, device="cuda"):
        super().__init__()
        self.weight_q = torch.nn.Parameter(
            torch.randint(-63, 63, (n, k), dtype=torch.int8, device=device),
            requires_grad=False,
        )
        self.weight_s = torch.nn.Parameter(
            torch.ones(n, dtype=torch.float32, device=device) / 128.0,
            requires_grad=False,
        )
        self.input_s = torch.nn.Parameter(
            torch.tensor(1.0 / 128.0, device=device), requires_grad=False
        )
        self.input_zp = None
        self.azp_adj = None
        self.logical_widths = [n]


def create_mock_layer(n, k, device="cuda"):
    return DummyLayer(n, k, device=device)
