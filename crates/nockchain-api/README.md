# nockchain-api (ALPHA)

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-02-19
Canonical/Legacy: Canonical (Tier 1 scoped authority for public API runtime and operator-facing deployment guidance; protocol authority remains in [`PROTOCOL.md`](../../PROTOCOL.md))

## ALPHA/TESTING GRADE SOFTWARE, TURN BACK YOU ARE NOT SUPPOSED TO BE HERE.

**This is pre-release/alpha infrastructure. If you aren't already comfortable debugging Nockapp/Nockchain in production, turn back now.**

----

No really go away. This is pre-alpha software. We made it public but we're not going to be able to support it or answer questions from the public until it's in a much more complete and stable state.

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

## Minimum config to make it useful

1. Provide the normal Nockchain CLI flags (genesis, mining, peers, etc.) exactly as you would for any full node.
2. Add `--bind-public-grpc-addr host:port` (the socket the public gRPC API will bind to).
3. Add `--bind /ip4/…/udp/…/quic-v1` only if you need an explicit libp2p listen multiaddr (otherwise the node uses its default bind behavior).
4. Start it with `cargo run --release --bin nockchain-api -- <flags>`.

That’s it—the API surface piggybacks on the running node; there is no separate config file.

## Security posture (none)

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
