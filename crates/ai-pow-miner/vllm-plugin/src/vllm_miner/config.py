import os
from typing import Any

import yaml
from miner_base.settings import MinerSettings


class Config:
    _config: dict = {}

    def __init__(self) -> None:
        self._load_config()
        self.settings = MinerSettings()

    def _load_config(self):
        """Load configuration from YAML file."""
        config_path = os.path.join(os.path.dirname(__file__), "config.yaml")
        try:
            with open(config_path) as f:
                self._config = yaml.safe_load(f)
        except FileNotFoundError:
            raise RuntimeError(
                "config.yaml file not found. Please create it with required configuration.",
            ) from None

    @property
    def gateway_socket_path(self) -> str:
        """Get the gateway socket path from configuration."""
        return self._config["gateway_socket_path"]

    @property
    def matrix_multiplication_config(self) -> dict[str, Any]:
        """Get the matrix multiplication configuration."""
        return self._config["matrix_multiplication"]

    def should_use_noisy_gemm(self, m: int, n: int, k: int) -> bool:
        """Determine if simplified GEMM should be used based on matrix dimensions."""
        config = self.matrix_multiplication_config["use_simplified_gemm"]
        min_m = config["min_m"]
        min_n = config["min_n"]
        min_k = config["min_k"]
        if (n == 1) or (k == 1) or (m == 1):
            return False
        return (m >= min_m) and (n >= min_n) and (k >= min_k)


# Create a singleton instance
config = Config()
