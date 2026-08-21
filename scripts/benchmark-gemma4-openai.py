#!/usr/bin/env python3
"""Benchmark latency and throughput of the production OpenAI-compatible endpoint."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import math
import os
import statistics
import sys
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

_DEFAULT_PROMPT = (
    "Explain why deterministic validation matters for a decentralized consensus "
    "protocol. Give a technically precise answer with concrete failure modes."
)


@dataclass(frozen=True)
class RequestResult:
    latency_seconds: float
    time_to_first_token_seconds: float | None
    prompt_tokens: int
    completion_tokens: int
    output_characters: int
    finish_reason: str | None


def _percentile(values: list[float], percentile: float) -> float:
    if not values:
        return math.nan
    ordered = sorted(values)
    rank = (len(ordered) - 1) * percentile
    lower = math.floor(rank)
    upper = math.ceil(rank)
    if lower == upper:
        return ordered[lower]
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)


def _headers(api_key: str, content_type: bool = False) -> dict[str, str]:
    headers = {"authorization": f"Bearer {api_key}"} if api_key else {}
    if content_type:
        headers["content-type"] = "application/json"
    return headers


def _get_json(base_url: str, path: str, api_key: str, timeout: float) -> Any:
    request = urllib.request.Request(
        f"{base_url}{path}", headers=_headers(api_key), method="GET"
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


def _nonstream_request(
    base_url: str, payload: bytes, api_key: str, timeout: float
) -> RequestResult:
    request = urllib.request.Request(
        f"{base_url}/v1/chat/completions",
        data=payload,
        headers=_headers(api_key, content_type=True),
        method="POST",
    )
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=timeout) as response:
        value = json.load(response)
    latency = time.perf_counter() - started
    usage = value.get("usage") or {}
    choice = value["choices"][0]
    output = str(choice["message"].get("content") or "")
    return RequestResult(
        latency_seconds=latency,
        time_to_first_token_seconds=None,
        prompt_tokens=int(usage.get("prompt_tokens") or 0),
        completion_tokens=int(usage.get("completion_tokens") or 0),
        output_characters=len(output),
        finish_reason=choice.get("finish_reason"),
    )


def _stream_request(
    base_url: str, payload: bytes, api_key: str, timeout: float
) -> RequestResult:
    request = urllib.request.Request(
        f"{base_url}/v1/chat/completions",
        data=payload,
        headers=_headers(api_key, content_type=True),
        method="POST",
    )
    started = time.perf_counter()
    first_token: float | None = None
    usage: dict[str, Any] = {}
    output_parts: list[str] = []
    finish_reason: str | None = None
    with urllib.request.urlopen(request, timeout=timeout) as response:
        for raw_line in response:
            line = raw_line.decode("utf-8").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            value = json.loads(data)
            if value.get("usage"):
                usage = value["usage"]
            for choice in value.get("choices", []):
                content = str((choice.get("delta") or {}).get("content") or "")
                if content:
                    if first_token is None:
                        first_token = time.perf_counter() - started
                    output_parts.append(content)
                finish_reason = choice.get("finish_reason") or finish_reason
    latency = time.perf_counter() - started
    return RequestResult(
        latency_seconds=latency,
        time_to_first_token_seconds=first_token,
        prompt_tokens=int(usage.get("prompt_tokens") or 0),
        completion_tokens=int(usage.get("completion_tokens") or 0),
        output_characters=sum(len(part) for part in output_parts),
        finish_reason=finish_reason,
    )


def _summary(values: list[float]) -> dict[str, float]:
    return {
        "min": min(values),
        "p50": _percentile(values, 0.50),
        "p95": _percentile(values, 0.95),
        "p99": _percentile(values, 0.99),
        "max": max(values),
        "mean": statistics.fmean(values),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:8000")
    parser.add_argument(
        "--api-key", default=os.getenv("VLLM_API_KEY", os.getenv("OPENAI_API_KEY", ""))
    )
    parser.add_argument(
        "--model",
        default=os.getenv("VLLM_SERVED_MODEL_NAME", "gemma-4-31b-it-pearl"),
    )
    parser.add_argument("--prompt", default=_DEFAULT_PROMPT)
    parser.add_argument("--prompt-file", type=Path)
    parser.add_argument("--requests", type=int, default=20)
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--max-tokens", type=int, default=128)
    parser.add_argument("--timeout", type=float, default=300.0)
    parser.add_argument("--stream", action="store_true")
    parser.add_argument("--vary-prompts", action="store_true")
    parser.add_argument("--include-requests", action="store_true")
    args = parser.parse_args()
    if args.requests <= 0 or args.concurrency <= 0 or args.warmup < 0:
        parser.error(
            "requests and concurrency must be positive; warmup must be nonnegative"
        )
    if args.max_tokens <= 0 or args.timeout <= 0:
        parser.error("max-tokens and timeout must be positive")
    if args.prompt_file:
        args.prompt = args.prompt_file.read_text()
    if not args.prompt:
        parser.error("prompt must not be empty")

    base_url = args.base_url.rstrip("/")
    models = _get_json(base_url, "/v1/models", args.api_key, args.timeout)
    served_models = [str(value["id"]) for value in models.get("data", [])]
    if args.model not in served_models:
        parser.error(f"model {args.model!r} is not served; available: {served_models}")

    request_value: dict[str, Any] = {
        "model": args.model,
        "temperature": 0,
        "top_p": 1,
        "seed": 0,
        "max_tokens": args.max_tokens,
        "stream": args.stream,
    }
    if args.stream:
        request_value["stream_options"] = {"include_usage": True}

    def payload(index: int) -> bytes:
        prompt = args.prompt
        if args.vary_prompts:
            prompt = f"{prompt}\n\nBenchmark request id: {index}"
        value = {
            **request_value,
            "messages": [{"role": "user", "content": prompt}],
        }
        return json.dumps(value).encode()

    request_fn = _stream_request if args.stream else _nonstream_request

    try:
        for index in range(args.warmup):
            request_fn(base_url, payload(-index - 1), args.api_key, args.timeout)
        batch_started = time.perf_counter()
        with concurrent.futures.ThreadPoolExecutor(
            max_workers=args.concurrency
        ) as executor:
            futures = [
                executor.submit(
                    request_fn, base_url, payload(index), args.api_key, args.timeout
                )
                for index in range(args.requests)
            ]
            results = [future.result() for future in futures]
        wall_seconds = time.perf_counter() - batch_started
    except (OSError, ValueError, urllib.error.URLError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        return 1

    latencies = [result.latency_seconds for result in results]
    ttfts = [
        value
        for result in results
        if (value := result.time_to_first_token_seconds) is not None
    ]
    prompt_tokens = sum(result.prompt_tokens for result in results)
    completion_tokens = sum(result.completion_tokens for result in results)
    output: dict[str, Any] = {
        "endpoint": base_url,
        "model": args.model,
        "stream": args.stream,
        "requests": args.requests,
        "concurrency": args.concurrency,
        "warmup_requests": args.warmup,
        "vary_prompts": args.vary_prompts,
        "max_tokens": args.max_tokens,
        "wall_seconds": wall_seconds,
        "request_throughput_per_second": args.requests / wall_seconds,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens,
        "output_tokens_per_second": completion_tokens / wall_seconds,
        "total_tokens_per_second": (prompt_tokens + completion_tokens) / wall_seconds,
        "latency_seconds": _summary(latencies),
        "time_to_first_token_seconds": _summary(ttfts) if ttfts else None,
        "finish_reasons": sorted(
            {result.finish_reason for result in results if result.finish_reason}
        ),
    }
    if args.include_requests:
        output["request_results"] = [asdict(result) for result in results]
    print(json.dumps(output, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
