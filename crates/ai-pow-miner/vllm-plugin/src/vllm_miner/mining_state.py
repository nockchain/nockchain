from __future__ import annotations

import os

from miner_base.async_loop_manager import AsyncLoopManager
from miner_base.settings import MinerSettings
from miner_utils import get_logger
from pearl_gateway.config import MinerRpcConfig
from pearl_gemm import HostSignalHeaderPinnedPool

from .config import config
from .nockchain_client import NockchainMiningClient
from .nockchain_manager import NockchainAsyncLoopManager

_LOGGER = get_logger("vllm.pearl_miner")

# Global mining state instances, should be initialized per-process
_async_manager: AsyncLoopManager | None = None
_pinned_pool: HostSignalHeaderPinnedPool | None = None


def get_async_manager() -> AsyncLoopManager:
    if not _async_manager:
        raise AssertionError("Async Loop Manager has not been initialized yet")
    return _async_manager


def _initial_rank_zero() -> bool:
    value = os.getenv("LOCAL_RANK", os.getenv("RANK", "0"))
    try:
        return int(value) == 0
    except ValueError as error:
        raise ValueError(f"invalid runtime rank {value!r}") from error


def init_async_manager(miner_settings: MinerSettings | None = None) -> None:
    """Initialize the global mining state."""
    global _async_manager

    if _async_manager is None or _async_manager._pool is None:
        miner_settings = (
            miner_settings if miner_settings is not None else MinerSettings()
        )
        miner_settings.enable_async_cuda_event_processing = True

        endpoint = os.getenv("NOCKCHAIN_AI_POW_ENDPOINT")
        rpc_config = MinerRpcConfig(
            transport="uds", socket_path=config.gateway_socket_path
        )
        if endpoint:
            _async_manager = NockchainAsyncLoopManager(
                rpc_config,
                miner_settings,
                NockchainMiningClient(
                    endpoint,
                    rank_zero=_initial_rank_zero(),
                    mining_enabled=not miner_settings.no_mining,
                ),
            )
        else:
            miner_settings.no_gateway = True
            miner_settings.no_mining = True
            _async_manager = AsyncLoopManager(rpc_config, miner_settings)
        _async_manager.start()
        config.settings = miner_settings
        _LOGGER.info(f"Mining state initalized, {miner_settings=}")


def get_pinned_pool() -> HostSignalHeaderPinnedPool:
    if _pinned_pool is None:
        raise AssertionError("Pinned pool has not been initialized yet")
    return _pinned_pool


def init_pinned_pool(pool_size: int = 128) -> None:
    global _pinned_pool

    if _pinned_pool is None:
        _pinned_pool = HostSignalHeaderPinnedPool(pool_size)
        _LOGGER.info(f"Pinned pool initialized, {pool_size=}")


def ensure_pinned_pool_at_least(min_size: int) -> None:
    global _pinned_pool

    if _pinned_pool is None:
        init_pinned_pool(min_size)
        return
    if _pinned_pool._pool_size < min_size:
        _pinned_pool = HostSignalHeaderPinnedPool(min_size)
        _LOGGER.info(f"Pinned pool grown to {min_size=}")


def delete_state() -> None:
    global _async_manager
    global _pinned_pool

    if _async_manager is not None:
        _async_manager.wait_until_done_submitting_blocks()
        _async_manager.stop()
        del _async_manager
        _async_manager = None

    if _pinned_pool is not None:
        del _pinned_pool
        _pinned_pool = None
