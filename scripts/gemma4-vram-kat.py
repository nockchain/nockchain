#!/usr/bin/env python3
"""Exercise every admitted padded batch shape and bound retained native VRAM."""

from __future__ import annotations

import json
from threading import Lock

import torch
from vllm_miner.native_gemma4 import NativeGemma4SessionCache

_GIB = 1024**3
_MU = bytes.fromhex("0015000080000000000f00000000000f00000000" + "00" * 32)


def main() -> None:
    if not torch.cuda.is_available():
        raise RuntimeError("CUDA is required")
    device = torch.device("cuda")
    b = torch.zeros((43008, 5376), dtype=torch.int8, device=device)
    b_scales = torch.ones(43008, dtype=torch.float32, device=device)
    torch.cuda.synchronize()
    baseline_free, total = torch.cuda.mem_get_info()
    sessions = NativeGemma4SessionCache()
    session_lock = Lock()
    minimum_free = baseline_free

    for rows in [*range(256, 4097, 256), 256]:
        a = torch.zeros((rows, 5376), dtype=torch.int8, device=device)
        a_scales = torch.ones(rows, dtype=torch.float32, device=device)
        output = torch.empty((rows, 43008), dtype=torch.bfloat16, device=device)
        with session_lock:
            session = sessions.get(a, b)
            session.prepare(bytes(76), _MU)
            session.infer(
                logical_m=rows,
                a_scales=a_scales,
                b_scales=b_scales,
                target=bytes(32),
                output=output,
            )
        torch.cuda.synchronize()
        free, _ = torch.cuda.mem_get_info()
        minimum_free = min(minimum_free, free)
        del a, a_scales, output

    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    final_free, _ = torch.cuda.mem_get_info()
    retained = baseline_free - final_free
    peak = baseline_free - minimum_free
    if len(sessions) != 1:
        raise AssertionError(f"expected one cached session, got {len(sessions)}")
    if retained >= 2 * _GIB:
        raise AssertionError(f"retained native VRAM is {retained / _GIB:.3f} GiB")

    sessions.close()
    print(
        json.dumps(
            {
                "device": torch.cuda.get_device_name(device),
                "total_gib": total / _GIB,
                "peak_delta_gib": peak / _GIB,
                "retained_delta_gib": retained / _GIB,
                "cache_entries": 1,
                "shapes": 17,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
