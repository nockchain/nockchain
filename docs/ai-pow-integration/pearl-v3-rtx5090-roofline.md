# Pearl V3 RTX 5090 Roofline

## Purpose

Define the hardware limits and puzzle shapes for a Pearl V3 CUDA miner on one RTX 5090. Measurements must use the exact ticket transcript and real INT8 data. A raw GEMM number is only a ceiling.

## Hardware limits

The RTX 5090 has 170 streaming multiprocessors, 32 GiB of GDDR7, and 1,792 GB/s of memory bandwidth. At the 2.407 GHz boost clock, its dense INT8 Tensor Core ceiling is approximately:

$$170 \times 4 \times 512 \times 2.407\text{ GHz} = 838\text{ TOPS}.$$

The local Pearl benchmark measured 858.2 TOPS with cuBLAS Lt at $32768 \times 57344 \times 8192$. Clock boost can move this value by approximately 3%. The production comparison must therefore use same-session ratios and record the sustained SM clock, power, and temperature.

Real mining data reaches a lower power-limited ceiling. The local Pearl records show approximately 620–674 TOPS for the full transcript workload and approximately 314–366 TMAC/s. Synthetic all-one inputs can inflate the result by 22–30% and are not valid performance inputs.

## Puzzle geometry

Use these fixed transcript dimensions for the first kernel family:

| Field | Value | Reason |
|---|---:|---|
| `k` | 8,192 | Long enough to saturate the Tensor Core main loop. |
| Noise rank `r` | 512 | `k/r = 16`; each of the 16 transcript slots is written once. |
| Ticket tile | $16 \times 16$ | Matches the native hash tile and the Pearl limit $h\cdot w \le 256$. |
| MMA | `m16n8k32.s8.s8.s32.satfinite` | Matches the scalar INT8-to-INT32 result for this range. |
| CTA tile | $256 \times 128 \times 64$ | Highest measured complete-ticket rate. |
| Pipeline depth | 2 stages | Faster than the measured 3-stage variants. |

The maximum absolute dot product is below $2^{31}$:

$$8192 \times 128 \times 128 = 134,217,728.$$

`mma.sync.satfinite` therefore has the same result as wrapping scalar accumulation for every valid input.

## Candidate shape model

Each $16 \times 16$ ticket costs $256 \times 8192 = 2,097,152$ MACs. At 600 TOPS, every shape below has the same ideal rate of 143.05 million tickets/s. Larger shapes improve occupancy and amortization, but they increase stale-work latency and memory use.

| `m` | `n` | MAC/launch | Input bytes | Ideal launch time at 600 TOPS | Tickets/launch |
|---:|---:|---:|---:|---:|---:|
| 4,096 | 32,768 | 1.100 TMAC | 288 MiB | 3.67 ms | 524,288 |
| 8,192 | 32,768 | 2.199 TMAC | 320 MiB | 7.33 ms | 1,048,576 |
| 16,384 | 32,768 | 4.398 TMAC | 384 MiB | 14.66 ms | 2,097,152 |
| 32,768 | 32,768 | 8.796 TMAC | 512 MiB | 29.32 ms | 4,194,304 |
| 32,768 | 57,344 | 15.393 TMAC | 704 MiB | 51.31 ms | 7,340,032 |

The full $32768 \times 57344 \times 8192$ shape has 41,705 INT8 operations per input byte. The RTX 5090 compute-to-bandwidth ridge is approximately 468 operations per byte. The main loop is compute-bound by a factor of approximately 89 if tile reuse is correct.

The selected shape is the smallest shape that reaches at least 98% of the best measured ticket rate. This rule limits cancellation latency and template-stale work without giving up meaningful throughput.

## RTX 5090 measured selection

The `sm_120` build uses CUDA 12.8 and real deterministic INT8 data. The
topology screening on one Runpod RTX 5090 produced these complete-ticket
rates for $m=4096$, $n=32768$, $k=8192$, and $r=512$:

| CTA | Stages | Registers/thread | Static shared memory | TMAC/s |
|---:|---:|---:|---:|---:|
| $128 \times 128$ | 2 | 232 | 4,096 B | 332.979 |
| $128 \times 128$ | 3 | 234 | 4,096 B | 322.721 |
| $256 \times 128$ | 2 | 248 | 8,192 B | 348.179 |
| $256 \times 128$ | 3 | 234 | 8,192 B | 347.510 |

The selected two-stage topology uses 49,152 bytes of dynamic shared memory.
The CUDA occupancy API reports one active CTA per SM. The launch grid contains
two CTAs per SM, so another CTA is ready when the active CTA completes.

Each shape ran after at least five seconds of warmup. Each table row is the
median of 21 complete searches:

| `m` | `n` | Kernel ms | Wall ms | Million tickets/s | TMAC/s |
|---:|---:|---:|---:|---:|---:|
| 4,096 | 32,768 | 3.158 | 3.184 | 166.025 | 348.179 |
| 8,192 | 32,768 | 6.385 | 6.409 | 164.216 | 344.386 |
| 16,384 | 32,768 | 12.653 | 12.678 | 165.742 | 347.587 |
| 32,768 | 32,768 | 25.755 | 25.784 | 162.854 | 341.530 |
| 4,096 | 57,344 | 5.631 | 5.653 | 162.938 | 341.705 |
| 8,192 | 57,344 | 11.393 | 11.421 | 161.070 | 337.787 |
| 16,384 | 57,344 | 22.371 | 22.403 | 164.050 | 344.037 |

The selected shape is $4096 \times 32768 \times 8192$. It is the smallest
tested shape and has the highest measured complete-ticket rate. One launch is
3.158 ms, well below the 100 ms cancellation limit.

A 60-second sustained run at the selected shape produced 3.285 ms per kernel,
159.617 million tickets/s, and 334.740 TMAC/s. Device samples showed 99% SM
activity, 575 W, 2,355–2,370 MHz, and 51–55 °C. The equivalent complete-search
rate is 669.480 TOPS.

The same pod produced 736.2 TOPS, or 368.1 TMAC/s, with the matching
$256 \times 128 \times 64$ two-stage raw GEMM. The five-second complete-search
rate is 94.6% of that raw rate. The sustained complete-search rate is 90.9%.

The hot kernel uses 248 registers per thread, has no stack frame, and has no
local-memory spills. Nsight Compute hardware counters are not available on the
Runpod host because the NVIDIA driver denies performance-counter access.

## Proof capacity

For `tile=16`, `k=8192`, and `r=512`, the conservative Layer-0 row budget is:

| Component | Rows |
|---|---:|
| A strip opening | 19,592 |
| B strip opening | 19,592 |
| Matmul sweep | 32,768 |
| Noised packed store | 262,145 |
| Fixed rows | 43 |
| Total before padding | 334,140 |
| Trace length | $2^{19}=524,288$ |

The trace is below the $2^{22}$ Pearl bound and the Nockchain $2^{19}$ production verifier cap. Full `m` and `n` do not increase this one-ticket trace because the proof opens only the selected strips and their authentication paths.

## Measured acceptance limits

A production candidate must satisfy all limits:

1. Exact scalar/GPU transcript equality for at least 1,000 patterned vectors, all boundary ordinals, and three deterministic repeats.
2. No Compute Sanitizer `memcheck`, `racecheck`, `initcheck`, or `synccheck` errors.
3. No local-memory stack or spill in the hot kernel. The measured occupancy must match the launch model.
4. At least 600 sustained TOPS or 300 TMAC/s with real patterned inputs on an RTX 5090.
5. At least 80% of the same-session raw-GEMM rate after transcript and BLAKE3 work.
6. At least 140 million complete tickets/s for the selected shape.
7. No-hit host traffic limited to a fixed result header per launch. No full output matrix is stored.
8. No host or device allocation in the steady-state search loop.
9. Kernel duration below 100 ms so candidate replacement and cancellation remain bounded.

## Measurement protocol

1. Build for `sm_120` with CUDA 12.8 or newer.
2. Use deterministic patterned INT8 inputs in the valid `[-127,127]` range. Do not use all-one or zero-page input.
3. Warm the GPU for at least five seconds.
4. Measure at least five samples in one process. Report the median and range.
5. Record SM clock, power, temperature, CUDA version, driver version, registers, stack, and occupancy.
6. Measure raw GEMM and complete ticket search back-to-back in the same process.
7. Report both TMAC/s and complete tickets/s. A ticket counts only after transcript finalization and target comparison.
