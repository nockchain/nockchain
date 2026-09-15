# AI-PoW architecture

## Responsibility boundary

`ai-pow` is the protocol library for the Pearl-compatible mineable work unit. It converts a block-bound job and a model/ticket into the public statement and witness consumed by `ai-pow-zk`.

It deliberately does not contain:

- node networking or candidate distribution;
- ASERT or accumulated-work fork choice;
- Hoon block and transaction validation;
- coinbase construction;
- verifier-setup residency; or
- peer abuse policy.

Those responsibilities belong to the miner, Hoon kernel, node, and verifier-jet crates.

## Attempt data flow

1. **Job binding.** The Nockchain candidate commitment is embedded exactly once in the Pearl auxiliary coinbase field. The opaque attempt envelope carries the Pearl-compatible header, configuration, model commitments, ticket metadata, and extranonce.
2. **Transcript keys.** The attempt derives `kappa`, then matrix/noise keys `s_B` and `s_A`, from the committed job. Dense and MoE routes use the same domain-separated transcript contract.
3. **Commitments and noise.** The miner commits to `A` and `B`, expands low-rank noise, and obtains `A' = A + E` and `B' = B + F` under the attempt-derived keys.
4. **Ticket execution.** A dense ticket selects scheduled rows and columns. A grouped-GEMM ticket additionally binds routing, expert selection, and expert-local columns. The selected tile performs the full prescribed dot products.
5. **Tile-state fold.** Matrix outputs enter the iterative 512-bit tile state. The jackpot digest is keyed by the same attempt transcript and compared as a little-endian 256-bit value.
6. **Target hit.** The work instance may satisfy Pearl's target, Nockchain's independently supplied AI target, both, or neither. No second Nockchain-only nonce is available after the matmul.
7. **Certificate construction.** On a Nockchain hit, `zk_bridge` builds the canonical opened schedule, public inputs, Layer-0 program, and witness for `ai-pow-zk`.
8. **Block artifact.** `ai-pow-miner` encodes the opaque attempt plus compact certificate in the `%ai-pow` noun. The node re-derives and verifies the statement through `ai-pow-jets`.

## Dense and MoE statements

Dense work authenticates selected rows of row-major `A` and selected columns of column-major `B`, applies the commitment-keyed noise, and proves the selected tile's fold and jackpot.

MoE work extends that statement with:

- the routing-data commitment;
- the selected expert and top-k route;
- the mapping from routed token rows to opened `A` rows;
- the expert's local output width `n_e`;
- global-to-local column conversion; and
- grouped matmul using only that expert's column range.

These are one statement. Treating routing as an off-circuit hint or allowing a global column to cross an expert boundary would change the mineable work and is forbidden.

## Encoding boundaries

Pearl compatibility covers byte-level work inputs and hashes. Nockchain's certificate is a separate compact recursive STARK encoded as canonical postcard bytes inside a bounded noun node. The outer noun remains versioned and structurally inspectable; Pearl-specific metadata remains inside the Rust-owned opaque attempt envelope.

Canonical JAM is used where a Hoon noun becomes a hash input. Rust and Hoon must hash the same noun representation, not merely values that print alike.

## Reuse policy

Only proof-independent setup and explicitly prover-side preprocessed data may be reused. Attempt-dependent matrices, noise, opened strips, tile state, jackpot preimage, and witness data are not reusable across extranonces. This is a security boundary rather than a performance preference.

## System integration

- Candidate emission: `hoon/apps/dumbnet` through private gRPC.
- Attempt execution and submission: `ai-pow-miner`.
- Proof construction: `ai-pow-zk` through `zk_bridge`.
- Noun decoding and precheck: `ai-pow-miner::certificate_noun`.
- Native proof verification and setup selection: `ai-pow-jets`.
- Consensus target, activation, work accounting, and final admission: Hoon kernel.
