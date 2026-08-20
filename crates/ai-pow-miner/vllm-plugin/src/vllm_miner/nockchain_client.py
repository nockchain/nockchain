from __future__ import annotations

import itertools
import os
from collections.abc import Iterator
from types import TracebackType
from typing import Self

import grpc
import torch
from miner_utils import get_logger
from pearl_gateway.blockchain_utils.zk_certificate import CertificateVersion
from pearl_gateway.comm.dataclasses import MiningJob, OpenedBlockInfo

from .proto import inference_mining_pb2 as pb
from .proto import inference_mining_pb2_grpc as pb_grpc

_LOGGER = get_logger("vllm.nockchain_ai_pow")
_PROTOCOL_VERSION = 1
_CHUNK_BYTES = 1024 * 1024


def _fixed_hex_env(name: str, length: int) -> bytes:
    value = os.getenv(name)
    if value is None:
        return bytes(length)
    decoded = bytes.fromhex(value)
    if len(decoded) != length:
        raise ValueError(f"{name} must decode to {length} bytes")
    return decoded


class NockchainMiningClient:
    """Synchronous gRPC client used by the miner manager's worker thread."""

    def __init__(self, endpoint: str) -> None:
        self._endpoint = endpoint
        self._channel = grpc.insecure_channel(
            endpoint,
            options=(
                ("grpc.max_send_message_length", 2 * _CHUNK_BYTES),
                ("grpc.max_receive_message_length", 4 * _CHUNK_BYTES),
            ),
        )
        self._stub = pb_grpc.InferenceMiningServiceStub(self._channel)
        registered = self._stub.RegisterRuntime(
            pb.RegisterRuntimeRequest(
                protocol_version=_PROTOCOL_VERSION,
                checkpoint_layout_digest=_fixed_hex_env(
                    "NOCKCHAIN_CHECKPOINT_LAYOUT_DIGEST", 32
                ),
                cuda_device_uuid=_fixed_hex_env("NOCKCHAIN_CUDA_DEVICE_UUID", 16),
                process_id=os.getpid(),
            ),
            timeout=10,
        )
        if registered.protocol_version != _PROTOCOL_VERSION:
            raise RuntimeError(
                f"server selected protocol {registered.protocol_version}; expected {_PROTOCOL_VERSION}"
            )
        if len(registered.runtime_id) != 16:
            raise RuntimeError("server returned an invalid runtime id")
        self._runtime_id = registered.runtime_id
        self._work_ids = itertools.count(1)
        self._work_shapes: dict[int, tuple[int, int, int, int]] = {}
        self._candidate_generation = 0
        _LOGGER.info(f"Connected to Nockchain AI-PoW service at {endpoint}")

    def get_mining_info(self) -> MiningJob:
        value = self._stub.GetMiningJob(
            pb.GetMiningJobRequest(runtime_id=self._runtime_id), timeout=10
        )
        if len(value.incomplete_header) != 76 or len(value.target_le) != 32:
            raise RuntimeError("Nockchain mining job has invalid fixed-width fields")
        self._candidate_generation = value.candidate_generation
        return MiningJob(
            incomplete_header_bytes=value.incomplete_header,
            target=int.from_bytes(value.target_le, "little"),
            cert_version=CertificateVersion(value.certificate_version),
        )

    def notify_work_started(
        self, *, layer: int, token_count: int, common_dim: int, output_dim: int
    ) -> int:
        work_id = next(self._work_ids)
        response = self._stub.NotifyWork(
            pb.NotifyWorkRequest(
                runtime_id=self._runtime_id,
                work_id=work_id,
                phase=pb.WORK_PHASE_STARTED,
                layer=layer,
                token_count=token_count,
                common_dim=common_dim,
                output_dim=output_dim,
            ),
            timeout=10,
        )
        if response.work_id != work_id:
            raise RuntimeError("work-start response id mismatch")
        self._work_shapes[work_id] = (layer, token_count, common_dim, output_dim)
        return work_id

    def notify_work_finished(self, work_id: int, *, failed: str | None = None) -> None:
        try:
            layer, token_count, common_dim, output_dim = self._work_shapes[work_id]
        except KeyError as error:
            raise ValueError(f"work id {work_id} is not active") from error
        phase = pb.WORK_PHASE_FAILED if failed else pb.WORK_PHASE_FINISHED
        response = self._stub.NotifyWork(
            pb.NotifyWorkRequest(
                runtime_id=self._runtime_id,
                work_id=work_id,
                phase=phase,
                layer=layer,
                token_count=token_count,
                common_dim=common_dim,
                output_dim=output_dim,
                error=failed or "",
            ),
            timeout=10,
        )
        if response.work_id != work_id:
            raise RuntimeError("work-finish response id mismatch")
        del self._work_shapes[work_id]

    def submit_opened_block(
        self, opened_block_info: OpenedBlockInfo, mining_job: MiningJob
    ) -> None:
        if opened_block_info.A is None or opened_block_info.B_t is None:
            raise ValueError("opened block submission requires A and B tensors")
        if opened_block_info.commitment_hash is None:
            raise ValueError("opened block submission requires commitment hashes")
        a = opened_block_info.A.detach().cpu().contiguous()
        b_t = opened_block_info.B_t.detach().cpu().contiguous()
        if a.dtype is not torch.int8 or b_t.dtype is not torch.int8:
            raise ValueError("opened block tensors must be int8")

        def parts() -> Iterator[pb.OpenedBlockPart]:
            yield pb.OpenedBlockPart(
                metadata=pb.OpenedBlockMetadata(
                    runtime_id=self._runtime_id,
                    candidate_generation=self._candidate_generation,
                    work_id=0,
                    a_row_indices=opened_block_info.A_row_indices,
                    b_column_indices=opened_block_info.B_column_indices,
                    noise_seed_a=opened_block_info.commitment_hash.noise_seed_A,
                    noise_seed_b=opened_block_info.commitment_hash.noise_seed_B,
                    noise_rank=opened_block_info.noise_rank,
                    a_rows=a.shape[0],
                    b_columns=b_t.shape[0],
                    common_dim=a.shape[1],
                )
            )
            yield from _tensor_parts(pb.OPENED_TENSOR_A, a)
            yield from _tensor_parts(pb.OPENED_TENSOR_B_TRANSPOSED, b_t)

        response = self._stub.SubmitOpenedBlock(parts(), timeout=120)
        if not response.accepted:
            raise RuntimeError(response.detail or "Nockchain opened block was rejected")
        _LOGGER.info(
            f"Submitted Nockchain opened block for generation {self._candidate_generation}"
        )

    def close(self) -> None:
        self._channel.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        type_: type[BaseException] | None,
        value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()


def _tensor_parts(tensor_kind: int, tensor: torch.Tensor) -> Iterator[pb.OpenedBlockPart]:
    data = tensor.view(torch.uint8).numpy().tobytes()
    for offset in range(0, len(data), _CHUNK_BYTES):
        yield pb.OpenedBlockPart(
            tensor_chunk=pb.OpenedTensorChunk(
                tensor=tensor_kind,
                offset=offset,
                data=data[offset : offset + _CHUNK_BYTES],
            )
        )
