#!/usr/bin/env python3
"""Local OpenAI-compatible mock that exercises the vLLM/miner gRPC lifecycle."""

from __future__ import annotations

import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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


class MiningControl:
    def __init__(self, endpoint: str) -> None:
        self.channel = grpc.insecure_channel(endpoint)
        self.stub = pb_grpc.InferenceMiningServiceStub(self.channel)
        registered = self.stub.RegisterRuntime(
            pb.RegisterRuntimeRequest(
                protocol_version=1,
                checkpoint_layout_digest=bytes([0x55]) * 32,
                cuda_device_uuid=bytes([0x66]) * 16,
                process_id=8,
            ),
            timeout=5,
        )
        self.runtime_id = registered.runtime_id
        self.next_work_id = 1

    def start(self, token_count: int) -> int:
        work_id = self.next_work_id
        self.next_work_id += 1
        self.stub.NotifyWork(
            pb.NotifyWorkRequest(
                runtime_id=self.runtime_id,
                work_id=work_id,
                phase=pb.WORK_PHASE_STARTED,
                layer=0,
                token_count=max(1, min(token_count, 4096)),
                common_dim=5376,
                output_dim=43008,
            ),
            timeout=5,
        )
        return work_id

    def finish(self, work_id: int, token_count: int) -> None:
        self.stub.NotifyWork(
            pb.NotifyWorkRequest(
                runtime_id=self.runtime_id,
                work_id=work_id,
                phase=pb.WORK_PHASE_FINISHED,
                layer=0,
                token_count=max(1, min(token_count, 4096)),
                common_dim=5376,
                output_dim=43008,
            ),
            timeout=5,
        )


class Handler(BaseHTTPRequestHandler):
    control: MiningControl

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        messages = request.get("messages", [])
        content = " ".join(str(item.get("content", "")) for item in messages)
        token_count = len(content.split())
        work_id = self.control.start(token_count)
        try:
            response = {
                "id": "chatcmpl-local-control-smoke",
                "object": "chat.completion",
                "model": request.get("model", "mock-gemma4"),
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": f"mock Gemma response: {content}",
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {
                    "prompt_tokens": token_count,
                    "completion_tokens": 4,
                    "total_tokens": token_count + 4,
                },
            }
        finally:
            self.control.finish(work_id, token_count)
        encoded = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, format: str, *args: object) -> None:
        return


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", default="127.0.0.1:8000")
    parser.add_argument("--miner-endpoint", default="127.0.0.1:5590")
    args = parser.parse_args()
    host, port = args.listen.rsplit(":", 1)
    Handler.control = MiningControl(args.miner_endpoint)
    server = ThreadingHTTPServer((host, int(port)), Handler)
    print(f"mock vLLM OpenAI server listening on {args.listen}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
