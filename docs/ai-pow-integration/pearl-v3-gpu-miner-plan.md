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
7. Attempt-dependent device state does not cross a prepared-template boundary. Immutable source matrices may remain resident across templates.
8. The steady-state no-hit path does not allocate device or host buffers per attempt.

## Active production shape

| Property | Value |
|---|---:|
| Matrix rows (`m`) | 4,096 |
| Matrix columns (`n`) | 32,768 |
| Inner dimension (`k`) | 8,192 |
| Noise rank | 512 |
| Opened rows | 16 |
| Opened columns | 16 |
| Dense tile tickets | 524,288 |
| Rolling stripes | 16 |
| Tile-state words | 16 × `i32` |
| Layer-0 trace height | $2^{17}$ |

The dense peak kernel is the primary production kernel for
`--gpu --canonical`. The small MoE kernel remains available for CPU diagnosis
and regression tests. Pearl Gateway work continues to use the backend that
matches the Gateway-supplied shape.

The peak path evaluates one complete dense Pearl tile space as one GEMM. One
prepared transcript supplies 524,288 ordered tile tickets. Operand reuse,
rolling transcript updates, jackpot hashing, and target comparison stay on the
GPU.

These documents define the path:

- `pearl-v3-rtx5090-roofline.md`
- `pearl-v3-rtx5090-architecture.md`
- `pearl-v3-rtx5090-implementation-plan.md`

The production cutover requires every correctness, sanitizer, complete-template
performance, proof, multi-GPU, restart, failure, and accepted-block gate.

## Compatibility boundary

Rust remains authoritative for job construction, winner validation, and proof
construction. CUDA must reproduce these dense operations:

- `PearlIncompleteBlockHeader::to_bytes`;
- `PearlMiningConfig::to_bytes`;
- `pearl_kappa`;
- `pearl_matrix_commitments`;
- `canonical_noise_seeds_from_matrix_commitments`;
- Pearl `E_L`, `E_R`, `F_L`, and `F_R` expansion;
- `compute_pattern_tile_state_from_slices`;
- `pearl_jackpot_hash`;
- `hash_le_target`.

The dense transcript is:

```text
kappa  = BLAKE3(sigma || mu)
H_A    = MerkleTree(pad_1024(A), key=kappa).root
H_B    = MerkleTree(pad_1024(B), key=kappa).root
A'     = BLAKE3(H_A || LE32(m) || zeroes, key=SEED_SALT_A)
B'     = BLAKE3(H_B || LE32(n) || zeroes, key=SEED_SALT_B)
s_B    = BLAKE3(kappa || B')
s_A    = BLAKE3(s_B || A')
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
4. Run the image with `NODE_ADDR=http://23.252.122.18:5556`. Do not override
   the image's default `MINING_PKH`,
   `2nFsk7KTv9Fm5zMU3ckWAM4p9eLhUSVeVEKUoPFkfzehyjuzmpXAN8j`.
5. Submit proofs to the mainnet API node maintained by the sibling `solo/`
   Ansible directory. Do not start or connect to a fakenet for a Runpod test.
6. Observe a GPU-found ticket, recursive proof construction, `%ai-pow`
   submission, and acceptance in the solo node logs.
7. Restart the container with the same production environment and observe
   another accepted block through the solo node.

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

Runpod proof-submission validation uses the public mainnet API node maintained
by the sibling `solo/` Ansible directory at `http://23.252.122.18:5556`. The
container uses its default Docker mining key. A Runpod validation must not
start or connect to a fakenet. The eight-GPU gate remains open because an
eight-RTX-5090 allocation was not available.

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

## RTX 5090 peak-path evidence

The isolated dense peak path uses:

- $m=4096$, $n=32768$, $k=8192$, and rank 512;
- $16 \times 16$ tickets;
- a $256 \times 128 \times 64$ CTA;
- two `cp.async` pipeline stages;
- one persistent device session for each prepared dense template.

On one Runpod RTX 5090, the five-second median is 348.179 TMAC/s and
166.025 million complete tickets/s. A 60-second run is 334.740 TMAC/s and
159.617 million tickets/s at 575 W. The matching raw GEMM is 368.1 TMAC/s.

The hot kernel uses 248 registers per thread, 8,192 bytes of static shared
memory, 49,152 bytes of dynamic shared memory, and one active CTA per SM. It
has no stack frame and no spills.

The scalar/device differential covers 1,000 deterministic tickets with three
fused device repetitions for each ticket. It compares all debug transcript
words and brackets every fused search hash with its exact target and unsigned
predecessor. Compute Sanitizer `memcheck`, `racecheck`, `initcheck`, and
`synccheck` report no errors on persistent, adjacent range searches.

The peak library backend is not a production miner mode. Its CLI selector,
recursive proof gate, Docker entrypoint, accepted-block validation, restart
validation, and multi-GPU extension remain open.
