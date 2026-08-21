#!/usr/bin/env python3
"""Compare deterministic Gemma 4 chat output across OpenAI-compatible endpoints."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request

_DEFAULT_MODEL = "/workspace/models/Gemma-4-31B-it-pearl"


def parse_endpoint(value: str) -> tuple[str, str]:
    label, separator, url = value.partition("=")
    if not separator or not label or not url:
        raise argparse.ArgumentTypeError("endpoint must use LABEL=BASE_URL")
    return label, url.rstrip("/")


def request(base_url: str, payload: bytes, timeout: float) -> str:
    http_request = urllib.request.Request(
        f"{base_url}/v1/chat/completions",
        data=payload,
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(http_request, timeout=timeout) as response:
            value = json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise RuntimeError(
            f"{base_url} returned HTTP {error.code}: {detail}"
        ) from error
    return str(value["choices"][0]["message"]["content"])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--endpoint", action="append", required=True, type=parse_endpoint
    )
    parser.add_argument("--prompt", required=True)
    parser.add_argument("--expected")
    parser.add_argument("--model", default=_DEFAULT_MODEL)
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--max-tokens", type=int, default=128)
    parser.add_argument("--timeout", type=float, default=300)
    args = parser.parse_args()
    if args.repeat <= 0:
        parser.error("--repeat must be positive")
    if args.max_tokens <= 0:
        parser.error("--max-tokens must be positive")

    payload = json.dumps(
        {
            "model": args.model,
            "messages": [{"role": "user", "content": args.prompt}],
            "temperature": 0,
            "top_p": 1,
            "seed": 0,
            "max_tokens": args.max_tokens,
        }
    ).encode()
    outputs: dict[str, list[str]] = {}
    for label, endpoint in args.endpoint:
        if label in outputs:
            parser.error(f"duplicate endpoint label: {label}")
        outputs[label] = [
            request(endpoint, payload, args.timeout) for _ in range(args.repeat)
        ]

    stable = all(len(set(values)) == 1 for values in outputs.values())
    first_outputs = {label: values[0] for label, values in outputs.items()}
    cross_device_equal = len(set(first_outputs.values())) == 1
    expected_equal = args.expected is None or all(
        value == args.expected for value in first_outputs.values()
    )
    print(
        json.dumps(
            {
                "stable": stable,
                "cross_device_equal": cross_device_equal,
                "expected_equal": expected_equal,
                "outputs": outputs,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0 if stable and cross_device_equal and expected_equal else 1


if __name__ == "__main__":
    sys.exit(main())
