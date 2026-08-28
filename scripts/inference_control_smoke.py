#!/usr/bin/env python3
"""Exercise the typed vLLM/miner lifecycle against a running local bridge."""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

import grpc

_PLUGIN = (
    Path(__file__).resolve().parents[1]
    / "crates"
    / "ai-pow-miner"
    / "vllm-plugin"
    / "src"
    / "vllm_miner"
)
sys.path.insert(0, str(_PLUGIN))

from proto import inference_mining_pb2 as pb  # noqa: E402
from proto import inference_mining_pb2_grpc as pb_grpc  # noqa: E402

_PROTOCOL_VERSION = 3
_CANONICAL_GEMMA4_MU = bytes.fromhex(
    "0015000080000000000f00000000000f00000000" + "00" * 32
)
_CHECKPOINT_CONTENT_DIGEST = bytes.fromhex(
    "c59cb83550f52b26893c1837133555bf32190495372ce00935d989592515ff40"
)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--endpoint", default="127.0.0.1:5590")
    args = parser.parse_args()

    channel = grpc.insecure_channel(args.endpoint)
    stub = pb_grpc.InferenceMiningServiceStub(channel)
    runtime = stub.RegisterRuntime(
        pb.RegisterRuntimeRequest(
            protocol_version=_PROTOCOL_VERSION,
            checkpoint_content_digest=_CHECKPOINT_CONTENT_DIGEST,
            cuda_device_uuid=bytes([0x44]) * 16,
            process_id=7,
        ),
        timeout=5,
    )
    assert len(runtime.runtime_id) == 16
    assert runtime.protocol_version == _PROTOCOL_VERSION
    job = stub.GetMiningJob(
        pb.GetMiningJobRequest(runtime_id=runtime.runtime_id), timeout=5
    )
    assert job.candidate_generation == 1
    assert len(job.incomplete_header) == 76
    assert job.effective_target_le == bytes(32)
    assert job.mining_config == _CANONICAL_GEMMA4_MU

    before = stub.GetStatus(pb.GetStatusRequest(), timeout=5)
    assert before.mode == pb.SCHEDULER_MODE_IDLE_MINING
    started = stub.NotifyWork(
        pb.NotifyWorkRequest(
            runtime_id=runtime.runtime_id,
            work_id=1,
            phase=pb.WORK_PHASE_STARTED,
            layer=0,
            token_count=128,
            common_dim=5376,
            output_dim=43008,
        ),
        timeout=5,
    )
    assert started.active_work_items == 1
    active = stub.GetStatus(pb.GetStatusRequest(), timeout=5)
    assert active.mode == pb.SCHEDULER_MODE_INFERENCE_MINING
    time.sleep(0.02)
    paused = stub.GetStatus(pb.GetStatusRequest(), timeout=5)
    time.sleep(0.02)
    paused_again = stub.GetStatus(pb.GetStatusRequest(), timeout=5)
    assert paused_again.idle_batches == paused.idle_batches

    finished = stub.NotifyWork(
        pb.NotifyWorkRequest(
            runtime_id=runtime.runtime_id,
            work_id=1,
            phase=pb.WORK_PHASE_FINISHED,
            layer=0,
            token_count=128,
            common_dim=5376,
            output_dim=43008,
        ),
        timeout=5,
    )
    assert finished.active_work_items == 0
    deadline = time.monotonic() + 2
    resumed = stub.GetStatus(pb.GetStatusRequest(), timeout=5)
    while resumed.idle_batches == paused.idle_batches and time.monotonic() < deadline:
        time.sleep(0.01)
        resumed = stub.GetStatus(pb.GetStatusRequest(), timeout=5)
    assert resumed.mode == pb.SCHEDULER_MODE_IDLE_MINING
    assert resumed.idle_batches > paused.idle_batches
    assert resumed.inference_batches == before.inference_batches + 1
    print(
        "idle-request-idle ok:",
        f"idle_before={before.idle_batches}",
        f"idle_after={resumed.idle_batches}",
        f"inference={resumed.inference_batches}",
    )
    channel.close()


if __name__ == "__main__":
    main()
