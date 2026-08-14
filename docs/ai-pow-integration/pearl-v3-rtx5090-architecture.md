# Pearl V3 RTX 5090 Search Architecture

## Decision

Use the existing dense Pearl ticket semantics and evaluate the complete tile grid in one CUDA launch. Do not use one extranonce per small opened tile.

The dense route is the correct throughput shape for this kernel:

- one prepared header binds `A'`, `B'`, `s_A`, and `s_B`;
- each valid `(t_rows, t_cols)` pair is one ticket ordinal;
- all tickets share the same noised matrices and commitment roots;
- the GPU reuses each operand tile across many output tiles;
- the recursive proof opens only the winning row and column strips.

MoE remains useful for model routing, but it adds routing preparation and reduces the regular output grid. It does not improve the dense Tensor Core roofline. The first peak kernel therefore supports dense, contiguous $16 \times 16$ patterns only.

## Search identity

For `row_tiles = m/16` and `col_tiles = n/16`, define:

$$ordinal = row\_tile \times col\_tiles + col\_tile.$$

This is the current `PreparedPearlPatternJob::offsets_at_ordinal` order. The device uses `atomicMin` on this ordinal. Grid scheduling cannot change winner order.

The kernel accepts a half-open ordinal range. A tile outside the range still follows the same regular main loop when a rectangular launch cannot exclude it, but it cannot update the winner. The production scheduler aligns batches to complete tile grids.

## Prepared-template boundary

The host does these steps once for each Pearl work header:

1. Validate the Pearl configuration and proof envelope.
2. Derive the matrix commitments and noise seeds.
3. Build `A'` in row-major order and `B'` in column-major order.
4. Build the Merkle state needed for a later winning strip opening.
5. Create a device session.
6. Copy `A'`, `B'`, `s_A`, and fixed geometry to that session.

A session cannot cross a header or mining-configuration boundary. Template replacement destroys the old session after its stream stops.

## Device pipeline

### Persistent state

Each device owns:

- one non-blocking CUDA stream;
- immutable `A'` and `B'` buffers;
- one 256-bit target buffer;
- one 64-bit lowest-winner slot;
- one 256-bit winning jackpot slot;
- timing events used only by the benchmark API.

The steady-state search call copies only the target and resets the winner. It does not allocate.

### CTA schedule

Use a fixed launch grid of two CTAs per SM. The selected kernel permits one active CTA per SM because each thread uses 248 registers. Each CTA walks the logical $256 \times 128$ output tiles with a grid-stride loop. The logical mapping is deterministic and covers every output tile exactly once.

The K loop uses:

- 16-byte `cp.async.cg.shared.global.L2::128B` copies;
- the CUTLASS crosswise shared-memory swizzle;
- `ldmatrix.x4` fragment loads;
- `mma.sync.aligned.m16n8k32.row.col.s32.s8.s8.s32.satfinite`;
- compile-time K-stage and transcript-cadence indices.

The hot loop has no data-dependent branch. Geometry branches are CTA-uniform. Lane ownership uses predicated instructions and fixed masks; no warp takes different K-loop control flow.

### Transcript

Each warp owns sixteen $16 \times 16$ hash tiles. At each 512-element cadence boundary, it XOR-reduces the cumulative INT32 accumulator for each owned tile.

`k/r = 16`. Each cadence writes a different one of the 16 transcript slots. The usual `rotl13(previous) XOR value` recurrence reduces to a direct write because `previous` is zero for every slot. This removes the shared-memory read-modify-write dependency without changing bytes.

After the main loop, whole warps finalize independent hash tiles with keyed BLAKE3. Finalizer warps do not execute Tensor Core work for another tile until the CTA barrier completes. A target hit computes the canonical ticket ordinal and updates the global winner with `atomicMin`.

No full C matrix is written to global memory.

## CPU and GPU boundary

The GPU returns:

- `UINT64_MAX` for no winner; or
- the lowest ticket ordinal and its 32-byte jackpot hash.

Rust then:

1. checks that the ordinal is inside the dispatched range;
2. maps it to `(t_rows, t_cols)` with the prepared job;
3. recomputes the complete ticket with the scalar evaluator;
4. compares the scalar jackpot with the device jackpot;
5. checks the target again;
6. constructs the existing compact recursive certificate;
7. submits the existing noun wire format.

Any mismatch is fatal for that requested backend. There is no CPU search fallback.

## Cancellation and template replacement

CUDA kernels are not preempted by the miner. Cancellation is observed after the current launch. The selected shape must keep one launch below 100 ms.

A template replacement follows this order:

1. stop dispatch of new launches;
2. synchronize the owned stream;
3. discard the prior winner header;
4. free the old session;
5. create and populate the new session;
6. resume at the new template's first ordinal.

A stale device result cannot enter proof construction because the Rust backend also checks template identity under its dispatch lock.

## Multi-GPU mapping

Each device receives one contiguous ordinal interval. The intervals are adjacent, disjoint, and cover the requested batch. Every device returns its local minimum. The host returns the global minimum after all active devices complete.

Do not interleave ordinals by device. Contiguous intervals preserve the scalar lexicographic order and make cancellation accounting exact.

## Supported first release

The peak path supports only:

- compute capability 12.0;
- dense Pearl jobs;
- contiguous $16 \times 16$ row and column patterns;
- `k=8192`, `r=512`, and full-dot tickets;
- dimensions divisible by 256 rows and 128 columns;
- at most 32 GiB of device memory;
- one prepared template per device session.

Any unsupported shape fails closed. The existing generic CUDA and canonical V3 paths remain available under their existing selectors and are not modified by this path.
