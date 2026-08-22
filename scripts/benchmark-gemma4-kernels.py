#!/usr/bin/env python3
"""Benchmark Gemma 4 INT7 and block-FP8 inference kernels on CUDA."""

from __future__ import annotations

import argparse
import json
import statistics
from collections.abc import Callable
from typing import Any

import torch
from vllm import _custom_ops as vllm_ops

_INT7_SHAPES = {
    "gate_up": (43008, 5376),
    "gate_or_up": (21504, 5376),
    "sliding_o": (5376, 8192),
    "full_o": (5376, 16384),
}
_FP8_SHAPES = {
    "down": (5376, 21504),
    "sliding_q": (8192, 5376),
    "sliding_kv": (4096, 5376),
    "full_q": (16384, 5376),
    "full_k": (2048, 5376),
}


def _measure(
    operation: Callable[[], torch.Tensor], warmup: int, iterations: int
) -> dict[str, float]:
    for _ in range(warmup):
        operation()
    torch.cuda.synchronize()

    starts = [torch.cuda.Event(enable_timing=True) for _ in range(iterations)]
    ends = [torch.cuda.Event(enable_timing=True) for _ in range(iterations)]
    for start, end in zip(starts, ends, strict=True):
        start.record()
        operation()
        end.record()
    torch.cuda.synchronize()
    samples = [start.elapsed_time(end) for start, end in zip(starts, ends, strict=True)]
    return {
        "min_ms": min(samples),
        "median_ms": statistics.median(samples),
        "max_ms": max(samples),
    }


def _comparison(left: torch.Tensor, right: torch.Tensor) -> dict[str, Any]:
    difference = (left.float() - right.float()).abs()
    return {
        "bit_equal": torch.equal(left, right),
        "max_abs_error": difference.max().item(),
        "mean_abs_error": difference.mean().item(),
    }


def _int7_case(
    *,
    name: str,
    m: int,
    n: int,
    k: int,
    warmup: int,
    iterations: int,
    device: torch.device,
) -> dict[str, Any]:
    from vllm_miner.gemm_operators import pearl_gemm_vanilla

    generator = torch.Generator(device=device).manual_seed(0x4E4F434B + m + n + k)
    activation = torch.randint(
        -63, 64, (m, k), generator=generator, dtype=torch.int8, device=device
    )
    weight = torch.randint(
        -63, 64, (n, k), generator=generator, dtype=torch.int8, device=device
    )
    activation_scale = (
        torch.rand((m,), generator=generator, dtype=torch.float32, device=device) * 0.02
        + 0.001
    )
    weight_scale = (
        torch.rand((n,), generator=generator, dtype=torch.float32, device=device) * 0.02
        + 0.001
    )
    cutlass_weight = weight.T.contiguous()

    def scaled_output(accumulator: torch.Tensor) -> torch.Tensor:
        return (
            accumulator.float()
            * activation_scale.reshape(-1, 1)
            * weight_scale.reshape(1, -1)
        ).to(torch.bfloat16)

    def pearl() -> torch.Tensor:
        return pearl_gemm_vanilla(
            activation,
            weight,
            scale_a=activation_scale,
            scale_b=weight_scale,
            out_dtype=torch.bfloat16,
        )

    def torch_int_mm() -> torch.Tensor:
        if m <= 16:
            padded = torch.zeros((32, k), dtype=torch.int8, device=device)
            padded[:m].copy_(activation)
            accumulator = torch._int_mm(padded, cutlass_weight)[:m]
        else:
            accumulator = torch._int_mm(activation, cutlass_weight)
        return scaled_output(accumulator)

    def cutlass() -> torch.Tensor:
        return vllm_ops.cutlass_scaled_mm(
            activation,
            cutlass_weight,
            scale_a=activation_scale.reshape(-1, 1),
            scale_b=weight_scale.reshape(-1, 1),
            out_dtype=torch.bfloat16,
        )

    pearl_output = pearl()
    torch_output = torch_int_mm()
    result: dict[str, Any] = {
        "mode": "int7",
        "shape": name,
        "m": m,
        "n": n,
        "k": k,
        "torch_comparison": _comparison(pearl_output, torch_output),
        "pearl": _measure(pearl, warmup, iterations),
        "torch_int_mm": _measure(torch_int_mm, warmup, iterations),
    }
    result["torch_speedup"] = (
        result["pearl"]["median_ms"] / result["torch_int_mm"]["median_ms"]
    )
    try:
        cutlass_output = cutlass()
        result["cutlass_comparison"] = _comparison(pearl_output, cutlass_output)
        result["cutlass"] = _measure(cutlass, warmup, iterations)
        result["cutlass_speedup"] = (
            result["pearl"]["median_ms"] / result["cutlass"]["median_ms"]
        )
    except RuntimeError as error:
        result["cutlass_error"] = str(error)
    return result


def _fp8_case(
    *,
    name: str,
    m: int,
    n: int,
    k: int,
    warmup: int,
    iterations: int,
    device: torch.device,
) -> dict[str, Any]:
    if n % 128 or k % 128:
        raise ValueError("block-FP8 benchmark dimensions must divide by 128")
    activation = torch.empty((m, k), dtype=torch.float8_e4m3fn, device=device)
    weight = torch.empty((n, k), dtype=torch.float8_e4m3fn, device=device)
    activation.fill_(1.0)
    weight.fill_(1.0)
    activation_scale = torch.ones((m, k // 128), dtype=torch.float32, device=device)
    weight_scale = torch.ones((n // 128, k // 128), dtype=torch.float32, device=device)

    def cutlass() -> torch.Tensor:
        return vllm_ops.cutlass_scaled_mm(
            activation,
            weight.T,
            scale_a=activation_scale,
            scale_b=weight_scale.T,
            out_dtype=torch.bfloat16,
        )

    cutlass_output = cutlass()
    result: dict[str, Any] = {
        "mode": "fp8",
        "shape": name,
        "m": m,
        "n": n,
        "k": k,
        "cutlass": _measure(cutlass, warmup, iterations),
    }
    try:
        from b12x.gemm import blockscaled

        if not blockscaled.is_supported():
            raise RuntimeError("B12X block FP8 is not supported on this device")

        def b12x() -> torch.Tensor:
            return blockscaled.mm_block_fp8(
                activation,
                activation_scale,
                weight,
                weight_scale,
                out_dtype=torch.bfloat16,
            )

        b12x_output = b12x()
        result["b12x_comparison"] = _comparison(cutlass_output, b12x_output)
        result["b12x"] = _measure(b12x, warmup, iterations)
        result["b12x_speedup"] = (
            result["cutlass"]["median_ms"] / result["b12x"]["median_ms"]
        )
    except (ImportError, RuntimeError) as error:
        result["b12x_error"] = str(error)
    return result


def _parse_positive_list(value: str) -> list[int]:
    values = [int(item) for item in value.split(",")]
    if not values or any(item <= 0 for item in values):
        raise argparse.ArgumentTypeError(
            "values must be positive comma-separated integers"
        )
    return values


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("int7", "fp8", "all"), default="all")
    parser.add_argument("--shape", action="append")
    parser.add_argument("--m", type=_parse_positive_list, default=[1, 8, 32, 256])
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--device", type=int, default=0)
    args = parser.parse_args()
    if args.warmup < 0 or args.iterations <= 0:
        parser.error("warmup must be non-negative and iterations must be positive")
    if not torch.cuda.is_available():
        parser.error("CUDA is required")

    device = torch.device("cuda", args.device)
    modes = ("int7", "fp8") if args.mode == "all" else (args.mode,)
    results: list[dict[str, Any]] = []
    for mode in modes:
        shapes = _INT7_SHAPES if mode == "int7" else _FP8_SHAPES
        selected = args.shape or list(shapes)
        unknown = sorted(set(selected) - set(shapes))
        if unknown:
            parser.error(f"unknown {mode} shape(s): {', '.join(unknown)}")
        for name in selected:
            n, k = shapes[name]
            for m in args.m:
                case = _int7_case if mode == "int7" else _fp8_case
                results.append(
                    case(
                        name=name,
                        m=m,
                        n=n,
                        k=k,
                        warmup=args.warmup,
                        iterations=args.iterations,
                        device=device,
                    )
                )
            torch.cuda.empty_cache()

    print(
        json.dumps(
            {
                "device": torch.cuda.get_device_name(device),
                "capability": list(torch.cuda.get_device_capability(device)),
                "results": results,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
