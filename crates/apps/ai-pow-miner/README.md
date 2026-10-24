# `ai-pow-miner`

`ai-pow-miner` is the external miner for Nockchain's `%ai-pow` puzzle. It searches Pearl-compatible dense and grouped-GEMM ticket attempts, creates a compact recursive certificate only after a target hit, and submits the canonical block artifact to a node.

The `ai-pow-mine` binary is enabled by the `node` feature. The default library build keeps the ticket loop available without the NockApp and gRPC dependency tree.

## Place in the system

```text
nockchain kernel --%mine-ai effect--> ai-pow-mine
       ^                                  |
       |                                  +-- Pearl-style work attempt
       |                                  +-- Nockchain target check
       |                                  +-- compact certificate on hit
       +-------- %ai-pow poke ------------+
                         \
                          +-- optional Pearl Gateway submission
```

`nockchain-mining-common` supplies the private gRPC client and candidate decoding. `ai-pow` owns the work statement and Pearl compatibility. `ai-pow-zk` proves it. `ai-pow-jets` and the Hoon kernel independently verify every submitted block.

## Modes

- **Node-connected mining:** watches `%mine-ai` effects and submits `%ai-pow` commands through the private NockApp gRPC service.
- **Canonical CPU mode:** constructs valid Nockchain certificates without a Pearl Gateway; intended for fakenet and integration verification rather than competitive throughput.
- **Pearl merge mining:** evaluates one Pearl-compatible work instance and may submit the same hit to Pearl and Nockchain when their independent targets are met.

## Maintained invariants

- A candidate supplies the block commitment, AI target, puzzle variant, and candidate height. The miner selects proof rules using that height, with the updated rules starting at 154,500, including after a reorg. The miner never chooses consensus target or fork-choice weight.
- Every extranonce is upstream of `kappa`, matrix commitments, noise, noised matrices, tile state, and jackpot. A new mining attempt rebuilds nonce-bound state; a nonce-only hash loop is forbidden.
- A recursive certificate is generated only after the ticket's jackpot satisfies the Nockchain target. The node repeats the target and proof checks.
- The certificate and opaque nonce envelope commit to the same Pearl transcript and Nockchain block commitment.
- Pearl auxiliary inclusion contains exactly one Nockchain commitment, preventing one Pearl proof from authorizing multiple Nockchain blocks.
- Dense and MoE artifacts use explicit, canonical variants. Routing, expert-local dimensions, opened schedules, and matrix commitments remain proof-bound.
- Candidate replacement cancels stale work. A proof for an old commitment cannot validate against a new block.
- Hoon sees only the versioned `%ai-pow` artifact and opaque `[len data]` nonce. Pearl gateway metadata never becomes a consensus-kernel concept.

## Trust boundaries

The miner is untrusted from consensus's perspective. Successful local verification is an optimization and diagnostic; only the node's Hoon rules plus mandatory Rust verify jet admit a block. Private gRPC is a loopback-only authenticated control plane: operator credentials permit kernel-level pokes, while restricted miner credentials permit only mining-candidate subscriptions and mined-proof submissions.

Merge mining shares a mineable work unit, not a proof system or chain target. Pearl and Nockchain retain independent acceptance, targets, block commitments, and submission paths.

### Private gRPC authentication

The node requires `NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN` (at least 32 characters).
It may also configure a distinct `NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN` of at
least 32 characters. Missing authentication policy, short tokens, equal role
tokens, miner-only node credentials, and non-loopback binds fail node startup
before consensus preparation or kernel state creation. Credentials and policy
are resolved once; rotate them by restarting the node and clients.

- **Operator-managed miners (recommended):** give the worker only
  `NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN`. Its presence selects restricted mode;
  the worker cannot change payouts or enable/disable mining. Provision the
  approved mining key and enable mining separately with the operator's
  `configure_mining` command, as described in
  [the deployment guide](../../../docs/AI_POW_CUTOVER.md#private-grpc-operator-and-miner-access).
- **Self-managed trusted miner:** give the worker
  `NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN` and no miner token. This grants full
  operator authority and allows its usual mining configuration at startup.
- **Single-user development only:** explicitly start the node with
  `--allow-unauthenticated-private-grpc` or
  `NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC=1`. Tokenless workers can then
  connect. Set the same environment opt-out for the container entrypoint.
  Every local caller has operator authority in this mode; it is unsuitable for
  shared hosts. An opt-out never bypasses configured credentials.

Pass secrets through the environment, not command arguments, checked-in files,
or logs. Remote miners must use an authenticated encrypted tunnel to the node's
loopback listener, for example `ssh -N -L 127.0.0.1:15555:127.0.0.1:5555 node-host`,
then `--node-addr http://127.0.0.1:15555`. Tokens alone do not encrypt HTTP.


## Soundness dependencies

The miner relies on `ai-pow` for Pearl byte compatibility and attempt binding, and on `ai-pow-zk` for certificate construction. Its security-sensitive obligations are canonical serialization, exact statement construction, fresh per-attempt work, and refusal to substitute prover-controlled setup or public parameters. Cryptographic acceptance properties are documented in [`../../lib/ai-pow-zk/docs/SECURITY.md`](../../lib/ai-pow-zk/docs/SECURITY.md).

## GPU container configuration

Build the Linux/amd64 production image:

```sh
docker buildx build \
  --platform linux/amd64 \
  -f tools/docker/Dockerfile.ai-pow-miner-gpu \
  -t ai-pow-miner-gpu .
```

The image mines to `2nFsk7KTv9Fm5zMU3ckWAM4p9eLhUSVeVEKUoPFkfzehyjuzmpXAN8j` by default. Set `MINING_PKH` to direct rewards to a different v1 mining public-key hash. `NODE_ADDR` and an operator/miner credential or explicit development opt-out are required. In restricted mode the operator-approved node payout remains authoritative, regardless of the worker's `MINING_PKH`.

With the tunnel above running on a Linux container host and the miner token
already exported in the shell:

```sh
docker run --rm --gpus all --network host \
  -e NODE_ADDR=http://127.0.0.1:15555 \
  -e NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN \
  -e MINING_PKH=<v1-mining-pkh> \
  ai-pow-miner-gpu
```

For a trusted self-managed worker, pass `-e NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN`
instead and omit the miner token. For a tokenless single-user development node,
pass `-e NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC=1` instead; this does not
change the node's own startup policy. The entrypoint never puts credentials in
arguments or prints them. Do not publish the node's private port: container
networking must reach the local tunnel or share its network namespace, not
require a non-loopback node bind.

The image uses up to eight visible CUDA devices, reference mode, and batches of 32,768 attempts per device by default. Set `CUDA_DEVICES` to `all` or a comma-separated ordinal list such as `0,1,2,3`; set `REFERENCE` or `GPU_BATCH_ATTEMPTS` to override the other values. Non-reference mode also requires `PEARL_GATEWAY`.

## Validation

Upgrade the node and AI miner together for the 154,500 cutover. The `%mine-ai`
effect now ends with `pow-len candidate-height`; the height is required to choose
the correct proof program. The upgraded miner rejects old effects without a
height and asks for a node upgrade. Old miners cannot decode the extended effect.
The `%mine-zk` effect and on-chain proof encoding are unchanged. See
[`AI_POW_CUTOVER.md`](../../../docs/AI_POW_CUTOVER.md) for activation and upgrade
guidance.

```sh
cargo test -p ai-pow-miner
cargo test -p ai-pow-miner --all-features
cargo run --release -p ai-pow-miner --features node --bin ai-pow-mine -- --help
```
