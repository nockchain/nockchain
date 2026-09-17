# `zk-pow-miner`

`zk-pow-miner` is the standalone miner for Nockchain's ZK-PoW `puzzle-nock` STARK. The `zk-pow-mine` binary connects to a running node, watches ZK mining candidates, searches them on CUDA GPUs or CPU workers, proves winners, and submits `%pow` solutions.

## Place in the system

```text
nockchain kernel --%mine-zk effect--> run loop --> worker pool
       ^                                      /                \
       |                         CUDA V5 nonce search       Serf search
       |                                      \                /
       +---------------- %pow poke ---------- Serf STARK proof
```

The node and miner are separate processes. `nockchain-mining-common` owns candidate decoding and private gRPC transport. CUDA workers evaluate the exact V5 puzzle digest on their GPUs, recheck winning digests in native Rust, then share a small Serf pool that asks `miner.jam` for each winning STARK proof. Legacy proof versions and V5 puzzles longer than 64 products use the same serialized Serf path. The Nockchain node independently validates submitted proofs and applies consensus rules.

## Maintained invariants

- Work is bound to the candidate's version, block commitment, target, and proof length.
- For version `%5`, each worker evaluates the nonce-bound puzzle object and checks its digest first. Losing nonces return without constructing a STARK; a winning nonce is proven once and submitted with that exact proof.
- Workers receive immutable jobs. Replacement candidates cancel or supersede stale work rather than mutating a job in place.
- Worker results are associated with the job that produced them; a late result cannot be submitted as a solution to a newer commitment.
- The miner's `%pow` wire source remains distinct from `%ai-pow` and other kernel commands.
- The private effect stream is best-effort and bounded. Missing an effect can reduce miner liveness but cannot change node consensus.
- Connection setup, RPCs, submissions, and worker shutdown are deadline-bounded and cancellation-aware. Reconnect failures share one capped budget that resets only after a valid candidate arrives.
- A successful `%pow` gRPC response acknowledges kernel processing, not consensus acceptance. An ambiguous timeout or transport failure reconnects without blindly resubmitting the same proof.
- A submitted proof is never trusted because it came from the reference miner. The node checks proof version, exact target, block commitment, and STARK validity.

## Cryptographic and consensus dependencies

The miner relies on the Nock ZKVM/STARK prover and the proof-verification jets used by the node. Soundness depends on the Nock proof system, Fiat-Shamir transcript, hash functions, and Hoon/Rust noun agreement. Chain safety additionally depends on the Hoon kernel's ASERT, work accounting, fork choice, transaction validation, and coinbase rules; none are implemented here.

The miner is a liveness component, not a consensus authority. A faulty miner can waste its own work or submit invalid blocks but must not make a conforming node accept one.

## CUDA backend

The default build has no CUDA dependency and uses the Serf CPU worker pool:

```sh
cargo build --release -p zk-pow-miner --bin zk-pow-mine
```

Enable CUDA explicitly. `ZK_POW_CUDA_ARCH` accepts one or more comma-delimited CUDA architecture numbers; omit it to compile for the build host's native GPU. CUDA 12.8 or newer is required to compile architecture `120` for RTX 5090:

```sh
# RTX 4090 and RTX 5090 fat binary
ZK_POW_CUDA_ARCH=89,120 cargo build --release -p zk-pow-miner --features cuda --bin zk-pow-mine
```

Set `ZK_POW_NVCC=/path/to/nvcc` or `CUDA_PATH=/path/to/cuda` when CUDA is outside `/usr/local/cuda`. A CUDA-enabled binary uses every visible device by default:

```sh
target/release/zk-pow-mine \
  --node-addr http://127.0.0.1:5555 \
  --mining-pkh 9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV
```

Use `--cuda-devices 0,2` to select devices or `--cpu` to force the Serf CPU backend. All GPU workers share one Serf prover by default; increase `--cuda-provers` only when winner proving or CPU fallback is a measured bottleneck. The measured RTX 4090/5090 defaults are `--cuda-batch-size 262144`, `--cuda-blocks-per-sm 12`, and `--cuda-threads-per-block 512`. Smaller batches reduce candidate-cancellation latency at the cost of launch overhead. `RUST_LOG=zk_pow_miner=debug` reports per-device kernel time and hash rate.

The benchmark validates a GPU digest against the native V5 oracle before timing all selected devices concurrently:

```sh
ZK_POW_CUDA_ARCH=89 cargo run --release -p zk-pow-miner \
  --features cuda --bin zk-pow-cuda-bench
```

## Network deployment

The private gRPC endpoint is a trusted, unauthenticated control plane. Keep it on loopback or a private network and reach remote miners through SSH forwarding or a VPN; never expose it directly to the public internet. Use `--rpc-timeout-ms` to bound node calls and `--worker-shutdown-timeout-ms` to bound cooperative worker drain.

Multiple miners may connect to one node, but they must use the same `--mining-pkh`: mining configuration is node-global. Each miner enables mining when it subscribes and deliberately does not disable the shared flag when it disconnects, so one miner shutting down cannot stop its peers.

## Validation

```sh
cargo test -p zk-pow-miner
cargo test -p zk-pow-miner --lib --release serf_worker_proves_only_after_v5_nonce_meets_target -- --ignored
cargo run --release -p zk-pow-miner --bin zk-pow-mine -- --help
```

Changes to `miner.hoon` require rebuilding `assets/miner.jam` before rebuilding this binary.
