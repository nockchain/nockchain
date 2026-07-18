# `nockchain-api`

Status: Active

Canonical/Legacy: Canonical (Tier 1 scoped authority for `nockchain-api` runtime and deployment behavior; protocol authority remains in [`PROTOCOL.md`](../../PROTOCOL.md))

`nockchain-api` is the full-node binary that enables Nockchain's public gRPC
query services in addition to the private NockApp control service. It boots the
same kernel, networking stack, persistence layer, and consensus jets as the
regular `nockchain` binary.

The public API remains an operator-managed service surface. Deploy it behind
appropriate authentication, transport security, rate limits, and monitoring;
the binary does not make an unrestricted Internet listener safe by itself.

## Canonical Scope

This document is Tier 1 canonical for:
- the runtime/deployment contract of the `nockchain-api` binary.
- operator-facing public gRPC exposure guidance for this binary.
- documented risk posture for current API deployment.

This document is NOT canonical for:
- protocol/consensus semantics (use [`PROTOCOL.md`](../../PROTOCOL.md)).
- global architecture policy (use [`ARCHITECTURE.md`](../../ARCHITECTURE.md)).

## Failure Modes And Limits

- This software is explicitly alpha and may change without backward compatibility.
- Security controls (authn/authz/rate-limiting) are currently incomplete; this doc cannot make an unsafe deployment safe.
- If implementation differs from this doc, code and metrics must be reviewed and this doc corrected in the same PR.

## Verification Contract

When public API behavior, flags, or risk posture changes, update this doc in the same change.

Minimum validation:
- `make -C open docs-check`
- `cargo check -p nockchain-api`

## What it does

`nockchain-api` is the public-facing NockApp gRPC API binary: it boots the standard `nockapp` runtime, loads the `nockchain` kernel, and exposes the gRPC services (`NockchainService` and `NockchainBlockService`) that depend on the live node state. This is the binary to run when you need the API surface enabled.

This is distinct from the regular `nockchain` binary and NockApps more generally: they only expose the private gRPC by default for private peeks and pokes.

__This comes with a considerably different risk surface area and requires expert use and thoughtful configuration, deployment, and monitoring__

## System integration and invariants

- Public RPCs query or index node state; they do not bypass the Hoon kernel's
  block or transaction validation.
- The regular `nockchain` and `nockchain-api` binaries share consensus setup and
  must accept exactly the same chain for the same kernel and constants.
- Cache contents are derived views of the heaviest chain, not protocol state.
  Warm-up and reorganization can produce temporary gaps or stale reads without
  changing consensus.
- Request decoding, pagination, response sizing, and concurrent work must remain
  bounded under untrusted clients.
- Private gRPC permits kernel peeks and pokes and is a separate privileged trust
  domain from the public query services.
- API success means the query or submission reached its service boundary; it
  does not make a pending transaction final or a submitted block canonical.

Correctness depends on `nockapp-grpc` preserving noun encodings, the node
reporting chain changes consistently, and clients tolerating reorganization and
eventual cache convergence. Cryptographic and consensus validity remain the
responsibility of the same Hoon and Rust verifier paths used by `nockchain`.

## Minimum config to make it useful

1. Provide the normal Nockchain CLI flags (genesis, mining, peers, etc.) exactly as you would for any full node.
2. Add `--bind-public-grpc-addr host:port` (the socket the public gRPC API will bind to).
3. Add `--bind /ip4/…/udp/…/quic-v1` only if you need an explicit libp2p listen multiaddr (otherwise the node uses its default bind behavior).
4. Start it with `cargo run --release --bin nockchain-api -- <flags>`.

That’s it—the API surface piggybacks on the running node; there is no separate config file.

## Security posture

- There is **no authentication, authorization, or rate limiting** in the public gRPC service today.
- If you expose `--bind-public-grpc-addr` directly to the Internet you are doing so entirely **at your own risk**.
- Until auth lands, run the API behind whatever you trust (VPN, SSH tunnel, mTLS proxy, private network). Do not put this on an open port.

## Critical operational notes

- The Block Explorer endpoints (`GetBlocks`, `GetTransactionBlock`, `GetTransactionDetails`) are backed by an in-memory cache of the heaviest chain. They do **not** stream mempool contents; pending transactions are only reported as “pending”.
- Cache warm-up: on first successful seed, the newest up to 1024 blocks (one range chunk) are available first, then older heights backfill in the background. Plan for a brief window where pagination returns nothing until seeding succeeds.
- Reorgs: the cache follows the reported heaviest chain but does not yet prune orphaned entries, so short-lived stale data can appear after a reorg.
- Observability: gnort metrics (prefixed `nockchain_public_grpc.*`) emit cache timings, heaviest-chain freshness, and RPC success/error counts. Use them to verify your deployment is healthy.
- This binary shares the same hot prover state (`zkvm-jetpack::produce_prover_hot_state`) as every other Nockchain node; make sure the host has enough RAM for the prover plus the gRPC caches.

Deployments today are integration testbeds, not hardened services. Control access, scrape the metrics, and expect breaking changes until we tag an official release.

