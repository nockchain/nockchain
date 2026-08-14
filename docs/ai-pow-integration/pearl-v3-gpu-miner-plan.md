# Pearl V3 GPU Miner Goal and Plan

## Goal

Deliver a production GPU mode for `ai-pow-miner` that reproduces the active Pearl V3 puzzle exactly, searches tickets on NVIDIA GPUs, builds the existing recursive proof on the host after a GPU hit, and submits an accepted `%ai-pow` block to a Nockchain node.

The production interface must require only the mining public key, the Nockchain node address, and optional GPU tuning values. The container must run on Runpod and similar NVIDIA container hosts.

## Required invariants

1. The GPU derives the complete Pearl V3 attempt transcript for every extranonce.
2. The GPU result matches the scalar Rust implementation byte for byte.
3. The GPU returns the lowest successful ordinal in each batch.
4. Rust recomputes and validates every GPU winner before proof construction.
5. A requested GPU backend fails closed. It does not fall back to CPU search.
6. The recursive proof format and consensus verifier remain unchanged.
7. Persistent device allocations do not cross a prepared-template boundary.
8. The steady-state no-hit path does not allocate device or host buffers per attempt.

## Active production shape

| Property | Value |
|---|---:|
| Matrix rows (`m`) | 64 |
| Matrix columns (`n`) | 64 |
| Inner dimension (`k`) | 1,024 |
| Noise rank | 64 |
| Opened rows | 8 |
| Opened columns | 8 |
| Routing experts | 2 |
| Top-k | 1 |
| Rolling stripes | 16 |
| Tile-state words | 16 × `i32` |

Pearl's large consumer-GPU geometry does not apply to this canonical job. Reuse its persistent-session, signed INT8 MMA, fused fold/hash/target, and scalar winner-check design rather than its dimensions.

## RTX 5090 peak dense path

The canonical MoE path remains the stable production path. Its small geometry is
commitment-bound and cannot approach the RTX 5090 Tensor Core limit.

The opt-in `peak` path evaluates the existing dense Pearl tile space as one full
GEMM. One prepared transcript supplies millions of ordered tile tickets, so
operand reuse and transcript work remain on the GPU. These documents define the
independent path:

- `pearl-v3-rtx5090-roofline.md`
- `pearl-v3-rtx5090-architecture.md`
- `pearl-v3-rtx5090-implementation-plan.md`

The peak path must preserve the same scalar winner check, recursive proof, and
consensus wire format. It cannot replace the canonical path until all correctness,
sanitizer, performance, proof, and accepted-block gates pass.

## Compatibility boundary

Rust remains authoritative for job construction, ticket validation, and proof construction. CUDA must reproduce these operations:

- `PearlIncompleteBlockHeader::to_bytes`;
- `PearlMiningConfig::to_bytes`;
- `pearl_kappa`;
- `pearl_matrix_commitments`;
- `canonical_noise_seeds_moe`;
- Pearl `E_L`, `E_R`, `F_L`, and `F_R` expansion;
- `compute_pattern_tile_state_from_slices`;
- `pearl_jackpot_hash`;
- `hash_le_target`.

The transcript is:

```text
kappa  = BLAKE3(sigma || mu)
H_A    = BLAKE3(pad_1024(A), key=kappa)
H_B    = BLAKE3(pad_1024(B), key=kappa)
A'     = BLAKE3(H_A || LE32(m)   || zeroes, key=SEED_SALT_A)
B'     = BLAKE3(H_B || LE32(n/e) || zeroes, key=SEED_SALT_B)
rroot  = BLAKE3(pad_1024(routing_data),    key=kappa)
hoff   = BLAKE3(pad_1024(routing_offsets), key=kappa)
hrout  = BLAKE3(rroot || hoff)
hact   = BLAKE3(A' || hrout)
s_B    = BLAKE3(kappa || B')
s_A    = BLAKE3(s_B || hact)
jackpot = BLAKE3(tile_state_le, key=s_A)
```

Each extranonce changes `sigma.timestamp`. No commitment, seed, noised strip, tile state, or jackpot can be reused across extranonces.

## Implementation plan

### Stage 1: Make the device transcript exact

1. Compare CUDA BLAKE3 against Rust for 64-byte, 128-byte, and 1,024-byte inputs.
2. Compare keyed and unkeyed modes, parent compression, tree-root finalization, and padding.
3. Export focused debug output for `kappa`, `H_A`, `H_B`, routing roots, `s_A`, and `s_B`.
4. Correct the first differing primitive before testing downstream values.
5. Remove diagnostic-only global buffers after the differential tests pass.

### Stage 2: Validate noising and matrix state

1. Compare every opened A and B byte for several deterministic extranonces.
2. Include extranonces `0`, `1`, `u32::MAX - 1`, and `u32::MAX` where a batch can represent them.
3. Validate signed INT8 multiplication and saturating `i32` accumulation with non-uniform inputs.
4. Compare all 16 rolling state words with the scalar Rust evaluator.
5. Test the exact row and expert-column routing order used by the canonical job.

### Stage 3: Validate search semantics and session lifetime

1. Use a maximum target and confirm that a multi-attempt batch returns ordinal zero.
2. Use a zero target and confirm that the batch reports no winner.
3. Exercise adjacent batches on one prepared template.
4. Replace the template and confirm that device state is recreated.
5. Confirm cancellation, attempt accounting, deadline handling, and lowest-winner ordering.
6. Confirm that a device false positive is a fatal backend error after scalar revalidation.

### Stage 4: Validate memory and synchronization

Run CUDA Compute Sanitizer against the focused differential binary:

```text
compute-sanitizer --tool memcheck
compute-sanitizer --tool racecheck
compute-sanitizer --tool initcheck
compute-sanitizer --tool synccheck
```

All four checks must pass without suppressed errors. Confirm that the steady-state no-hit batch path has no `cudaMalloc`, `cudaFree`, stream creation, or variable-size host allocation.

### Stage 5: Validate proof construction

1. Find a ticket with the GPU backend.
2. Recompute it with `PreparedCanonicalMoeTemplate::evaluate`.
3. Build the normal compact recursive certificate.
4. Verify it with the production V3 verifier context.
5. Confirm that no CUDA-specific value enters the proof or noun wire format.

### Stage 6: Validate the Runpod production flow

1. Build the GPU image for Linux/amd64 with CUDA 12.8 and `sm_120` support.
2. Start an RTX 5090 Runpod instance with a persistent container command.
3. Confirm the allocated device with `nvidia-smi`.
4. Run the miner with the mining public key, node address, CUDA device ordinal, and batch size.
5. Connect it to a fakenet Nockchain node.
6. Observe a GPU-found ticket, recursive proof construction, `%ai-pow` submission, and node acceptance.
7. Restart the container with only its production environment configuration and repeat the connection path.

### Stage 7: Measure performance

On the same RTX 5090 host:

1. measure attempts per second and TMAC per second;
2. measure commitment, noising, and MMA/jackpot kernel time separately;
3. measure full-batch GPU and wall time;
4. compare with the dedicated CPU backend;
5. tune batch size only after all correctness gates pass.

GPU throughput must exceed the pod CPU backend before the image is presented as a production accelerator.

## Current implementation state

The production path has:

- persistent, template-scoped CUDA sessions;
- byte-identical Pearl V3 transcript and jackpot evaluation;
- scalar winner revalidation before proof construction;
- fatal handling for CUDA startup, execution, and winner-validation errors;
- explicit CUDA device selection for one to eight visible devices;
- deterministic contiguous batch partitioning and global lowest-winner reduction;
- allocation-free steady-state CUDA search buffers;
- a CUDA 12.8 `sm_120` production image and Runpod entrypoint.

The CUDA differential test covers transcript commitments, noised strips, rolling tile
state, jackpot hashes, and extranonces at `0`, `1`, `7`, `UINT32_MAX - 1`, and
`UINT32_MAX`. Compute Sanitizer `memcheck`, `racecheck`, `initcheck`, and `synccheck`
pass. Maximum-target, zero-target, adjacent-batch, template-replacement, scalar
winner, recursive proof, production verifier, and proof-wire checks pass.

A Runpod RTX 5090 completed the production CLI flow through a fakenet Nockchain
node. The node accepted the submitted `%ai-pow` blocks. A four-RTX-5090 pod also
completed the same flow with devices `[0, 1, 2, 3]`. An eight-GPU run is not
validated because an eight-RTX-5090 allocation was not available.

The throughput metric is
`attempts_per_second * M * N * K / 10^12`, which is the same raw MAC-rate formula
used by the Pearl wheel benchmark. For 65,536 canonical attempts on RTX 5090:

- one GPU: 2.92 million attempts/s and 0.1915 TMAC/s;
- two GPUs: 5.68 million attempts/s and 0.3723 TMAC/s;
- four GPUs: 10.73 million attempts/s and 0.7034 TMAC/s;
- the 120-worker pod CPU backend: 147 thousand attempts/s and 0.00965 TMAC/s.

The commitment kernel takes about 90% of CUDA batch time. The production kernel
keeps rolling tile state in shared memory and writes transcript diagnostics only
for differential tests. The `sm_120` build has no local-memory spills in the
commitment, noising, tile, or winner kernels.

The host library compiles with
`cargo check --locked -p ai-pow-miner --features node,gpu --lib`.

The Linux/amd64 production image is available as
`docker.io/loganallc/nockchain-ai-pow-miner:gpu` and as the immutable
`gpu-c142d390` tag. Both tags resolve to manifest
`sha256:61022736c85e895925f3ac74080c83c9186054e67de86832eb5df7eb17c5f401`.
A one-RTX-5090 Runpod started the image with only `NODE_ADDR` set, submitted
accepted `%ai-pow` blocks, restarted from the same environment, and submitted
accepted blocks again.
