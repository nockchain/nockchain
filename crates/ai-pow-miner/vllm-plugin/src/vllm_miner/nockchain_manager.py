from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from threading import Thread

from miner_base.async_loop_manager import AsyncLoopManager
from miner_base.settings import MinerSettings
from pearl_gateway.comm.dataclasses import MiningJob, OpenedBlockInfo
from pearl_gateway.config import MinerRpcConfig

from .nockchain_client import NockchainMiningClient


class NockchainAsyncLoopManager(AsyncLoopManager):
    """AsyncLoopManager adapter backed by the typed Nockchain gRPC client."""

    def __init__(
        self,
        miner_rpc_config: MinerRpcConfig,
        miner_settings: MinerSettings,
        client: NockchainMiningClient,
    ) -> None:
        super().__init__(miner_rpc_config, miner_settings)
        self._nockchain_client = client

    def start(self) -> None:
        if self._thread is not None or self._pool is not None or self._client is not None:
            raise RuntimeError("Already started?")
        self._pool = ThreadPoolExecutor()
        self._client = self._nockchain_client
        self._mining_job = self._nockchain_client.get_mining_info()
        self._thread = Thread(target=self._run_async_loop, daemon=True)
        self._thread.start()

    def notify_work_started(
        self, *, layer: int, token_count: int, common_dim: int, output_dim: int
    ) -> int:
        return self._nockchain_client.notify_work_started(
            layer=layer,
            token_count=token_count,
            common_dim=common_dim,
            output_dim=output_dim,
        )

    def notify_work_finished(self, work_id: int | None, *, failed: str | None = None) -> None:
        if work_id is not None:
            self._nockchain_client.notify_work_finished(work_id, failed=failed)

    def handle_submit_block(
        self, opened_block_info: OpenedBlockInfo, mining_job: MiningJob
    ) -> None:
        if self._loop is None:
            raise AssertionError("Async loop is not started")
        if self._pool is None:
            raise AssertionError("Thread Pool Executor is not initialized")

        def submit() -> bool:
            self._nockchain_client.submit_opened_block(opened_block_info, mining_job)
            self.blocks_submitted += 1
            return True

        future = self._loop.run_in_executor(self._pool, submit)
        self._block_results.append(future)
