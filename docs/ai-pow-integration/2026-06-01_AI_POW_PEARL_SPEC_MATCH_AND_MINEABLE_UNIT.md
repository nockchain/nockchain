# AI-PoW vs Pearl: Proof / PoW Spec Match and Mineable Unit

Date: 2026-06-01
Status: Current compatibility audit, updated after legacy NCMN removal

> Update, 2026-06-01: this document has been revised after removal of the
> legacy NCMN miner/verifier path. The current Nockchain-side submission target
> is Pearl-format-compatible `%ai-pow` with opaque `ai-pow-nonce=[len data]`
> and no Pearl-specific Hoon molds. The Rust miner also submits Pearl target
> hits to Pearl Gateway via `submitPlainProof`; that payload never enters Hoon.
> See
> `2026-06-01_PEARL_MERGE_MINING_COMPATIBILITY_SPEC.md` for the wire-level
> production shape.
>
> Update, 2026-06-02: the Pearl-compatible production spec now adopts Pearl's
> tile-ticket work model. A Pearl work instance is keyed by `sigma || mu` and
> may expose multiple public tile tickets. Ticket offsets are public proof
> parameters for an opened tile; they are not additional entropy before
> `kappa`.

The live implementation mines Pearl-compatible ticket attempts in Rust,
submits Nockchain hits as `%ai-pow`, submits Pearl hits as Gateway
`PlainProof`, and has no submission-mode switch or native NCMN miner fallback.
Hoon sees only an opaque AI-PoW nonce envelope plus the recursive certificate
noun; Pearl-specific construction remains Rust-side.

## Executive Summary

The current `ai-pow` implementation is close to Pearl at the level of the
core mineable primitive:

- BLAKE3 commitment chain shape.
- Row-major `A` and column-major `B` matrix commitments.
- Low-rank noise generation.
- Noised tiled matmul accumulator.
- 16-word jackpot/tile-state update.
- Little-endian 256-bit target comparison.
- Shape-aware target weight
  `dot_product_length * rows_pattern.size() * cols_pattern.size()`.

The important caveat is that "close to Pearl" means close at the Pearl
work-instance and tile-ticket layer, not identical at the proof-system or
chain-submission layer:

- The mineable attempt nonce is a Rust-owned Pearl-compatible `AIP1` ticket
  envelope. Hoon treats it as opaque bytes and does not model Pearl fields.
- `sigma` is the Pearl incomplete header bytes, including the Nockchain aux
  commitment. `mu` is the Pearl mining configuration. Pearl derives `kappa`,
  matrix commitments, noise, noised matrices, and matmul-derived tile states
  from `sigma || mu`, the committed matrices, and the configured puzzle shape.
- Public proof parameters choose a tile ticket from that committed work
  instance. Changing a ticket offset changes the opened tile and jackpot
  statement, not the `kappa` or noised matmul instance.
- The Rust miner evaluates each attempt against the Pearl adjusted target and
  the Nockchain target adjusted by Pearl's pattern-size work factor
  independently. It submits to Pearl Gateway for Pearl target hits and submits
  `%ai-pow` to Nockchain for Pearl-priced Nockchain target hits. It submits to
  both only when the same attempt satisfies both targets.
- The canonical Nockchain block artifact is the recursive ZK certificate noun,
  not Pearl's plain block-opening proof and not a raw Layer-0 STARK blob.

The main implementation gap is no longer an unresolved lottery rule. The spec
rule is Pearl's rule: a full noised tiled matmul work instance can yield one or
more tile tickets, and each ticket is priced by Pearl's target factor
`dot_product_length * rows_pattern.size() * cols_pattern.size()`, where
`dot_product_length = common_dim - common_dim % rank`. The Nockchain
production spec supports the same puzzle requirements as Pearl: canonical
`MiningConfiguration`, canonical row/column `PeriodicPattern` values, valid
`t_rows`/`t_cols` offsets, and the exact shifted row/column sets selected by
those public proof parameters. Current implementation admission and the
Layer-0 recursive bridge bind those explicit shifted row/column schedules
instead of rewriting them to a native square tile.

Therefore:

- At the protocol/spec level, the unit of mineable work is the same
  Pearl-style work instance/tile-ticket execution, with Nockchain aux
  commitment binding included in `sigma`.
- At the implementation level, Nockchain must not replace Pearl's ticket set
  with a one-verifier-selected-tile lottery while still claiming Pearl
  compatibility. The proof statement binds the public Pearl ticket schedule.

## Pearl Baseline

The Pearl whitepaper's relevant PoW pipeline is:

1. A mining configuration `mu` fixes matrix dimensions, noise rank `r`, tile
   shape, and difficulty `b`.
2. The blockchain state `sigma` and mining configuration feed a job key:
   `kappa = BLAKE3(sigma || mu)`.
3. Matrix commitments are keyed by `kappa`:
   - `H_A = BLAKE3(Flatten(A), key=kappa)`.
   - `H_B = BLAKE3(Flatten(B^T), key=kappa)`, so `B` is committed in
     column-major order for compact column openings.
4. Noise seeds are chained from commitments:
   - `s_B = BLAKE3(kappa || H_B)`.
   - `s_A = BLAKE3(s_B || H_A)`.
5. Low-rank noise matrices `E = E_L * E_R` and `F = F_L * F_R` are generated
   from `s_A` and `s_B`.
6. The miner computes `(A + E) * (B + F)` with a tiled matmul algorithm.
   Each output tile carries a compact 16-word state `M`.
7. A ticket wins when the BLAKE3 hash of that ticket's tile state is below:
   Pearl's base target adjusted by
   `dot_product_length * rows_pattern.size() * cols_pattern.size()`,
   interpreted as a little-endian `uint256`.
8. A block-opening proof lets the verifier reconstruct the opened tile from
   matrix openings and commitments. Pearl then compresses this proof with a
   recursive ZK proof.

Pearl's stated unit is therefore tied to the actual tiled noised matmul work:
the ticket is not "hash a nonce"; it is "prove a tile state produced by the
prescribed noised matmul under commitments derived from the chain state."

## Current Pearl-Compatible Pipeline

The current Rust pipeline is:

1. `PearlNockchainAux` binds the Nockchain chain id, candidate block
   commitment, target epoch/height, Nockchain target, recursive ZK params, and
   domain-separated extra data.
2. `PearlMergeMiningJob` combines the Pearl header template, Pearl mining
   configuration, canonical aux bytes, matrix bytes, and Nockchain target.
3. The Pearl work instance derives:
   `kappa = BLAKE3(sigma || mu)`, where `sigma` is the aux-bearing incomplete
   Pearl header and `mu` is the mining configuration.
4. The work instance computes Pearl commitments and seeds:
   - `H_A = BLAKE3(pad(A_row_major), key=kappa)`;
   - `H_B = BLAKE3(pad(B_col_major), key=kappa)`;
   - `s_B = BLAKE3(kappa || H_B)`;
   - `s_A = BLAKE3(s_B || H_A)`.
5. The miner computes the noised tiled matmul `(A + E) * (B + F)`.
   Each eligible output tile has a Pearl 16-word state `M`.
6. Each public proof-parameter ticket selects valid `t_rows` and `t_cols`
   from that work instance. The jackpot digest is
   `BLAKE3(M_ticket, key=s_A)`.
7. The ticket worker evaluates the same Pearl jackpot digest against the
   adjusted Pearl target and the Pearl-priced Nockchain target independently.
   Recursive proof construction happens after a Nockchain target hit, not for
   target misses.
8. The ZK statement precheck re-derives `kappa`, `s_A`, `s_B`, `HASH_A`,
   `HASH_B`, trace height, target satisfaction, and the submitted ticket
   metadata before accepting the recursive certificate metadata.
9. The canonical artifact intended for Hoon/block persistence is:
   `[%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]`, where
   `ai-pow-nonce=[len=@ud data=@uxaipownonce]` is opaque to Hoon and the
   certificate carries commitments `[h-a-chunk h-b-chunk]` only.

This is reusable only in the way Pearl permits. A miner may evaluate multiple
valid tile tickets from one committed noised matmul work instance. A miner must
not add a separate Nockchain nonce or cheap hash loop that creates fresh
Nockchain target trials without producing a Pearl-valid tile ticket from a
properly committed work instance.

## What Matches Pearl Closely

### Commitment Chain

Pearl's Algorithm 2 shape is preserved:

```text
kappa = BLAKE3(sigma || mu)
H_A   = BLAKE3(pad(A_row_major), key=kappa)
H_B   = BLAKE3(pad(B_col_major), key=kappa)
s_B   = BLAKE3(kappa || H_B)
s_A   = BLAKE3(s_B || H_A)
```

The naming differs because Nockchain distinguishes:

- `h_a_chunk` / `h_b_chunk`: production proof-bound chunk commitments.
- `h_a` / `h_b`: legacy plain `MatmulProof` row/column opening roots.

Only `h_a_chunk` / `h_b_chunk` are canonical production commitments.

### Matrix Orientation

Nockchain follows Pearl's useful orientation:

- `A` is row-major.
- `B` is treated column-major, equivalent to Pearl flattening `B^T`.

That preserves compact opening semantics: an opened tile needs rows of `A` and
columns of `B`.

### Noise and Tile State

The low-rank noise and 16-word tile/jackpot state are Pearl-shaped. Existing
fixture tests compare the byte-level primitives against the vendored Pearl
reference. The ZK layer then proves the corresponding composite AIR execution.

### Target Formula

For square-contiguous diagnostic parameters, Nockchain's local
`difficulty_target(params)` uses:

```text
target = 2^(256 - b) * r * tile^2
```

with little-endian `uint256` comparison. In Pearl-compatible production, the
normative pricing is the Pearl pattern-size formula:

```text
target = base_target * dot_product_length * rows_pattern.size() * cols_pattern.size()
```

When consensus supplies an arbitrary chain target, `ai-pow-miner` passes that
explicit 32-byte base target instead of relying on `difficulty_bits`, then
applies Pearl's pattern-size factor before checking the ticket's jackpot hash.
The ticket's Pearl pattern size remains part of the work statement.

## Deliberate Differences

### Opaque Pearl-Compatible Attempt Nonce

Pearl removes the Bitcoin-style nonce from the block header and carries a
variable-size proof/certificate. Nockchain carries an explicit AI-PoW nonce
inside the `%ai-pow` command, but that nonce is no longer an NCMN structure.
It is an opaque Rust-owned Pearl-compatible ticket envelope.

This is safe only because the envelope carries a Pearl-valid public statement
and Nockchain aux evidence for a work instance whose `sigma` already commits to
the candidate Nockchain block. If the envelope introduced an independent
Nockchain nonce that only keyed the final BLAKE3 jackpot hash, one expensive
Pearl work instance could be reused for many cheap Nockchain-only hash trials.
That bug class remains forbidden.

### Jackpot Key

Pearl's Algorithm 4 uses `BLAKE3(M, key=s_A)` in the tile check.

Pearl-compatible Nockchain submissions use that same Pearl jackpot digest for
attempt search and target checking:

```text
hash = BLAKE3(M_ticket, key=s_A)
```

Native diagnostic helpers may still use additional local domain separation for
tests or non-consensus experiments, but Pearl-compatible consensus must bind
the recursive certificate to the same Pearl jackpot digest carried by the
submitted ticket.

### Pearl-Style Tile Tickets

Pearl's implementation checks the block-opening condition on ticket row/column
sets selected by public proof parameters. A work instance creates the valid
ticket set induced by `rows_pattern`, `cols_pattern`, `t_rows`, and `t_cols`;
each ticket is weighted by
`dot_product_length * rows_pattern.size() * cols_pattern.size()`.

Nockchain adopts this Pearl-style ticket rule for Pearl-compatible consensus.
The submitted ticket selects a valid tile from the committed work instance, and
the target formula prices the tile with Pearl's pattern-size factor. This is
not miner-selected grinding in the forbidden sense: the ticket must be an
opened Pearl tile from the noised matmul committed by `sigma || mu`, and the
same jackpot digest is checked by both chains under their own targets.

The older native recursive helper that derived one verifier-selected
`found_idx` remains useful as a diagnostic/smoke-profile guard. It is not the
multi-tile consensus rule for Pearl compatibility. Current consensus-facing
code avoids accepting the wrong economics by failing closed for unsupported
recursive configurations until Pearl's full tile-ticket requirements are
implemented.

### Proof System and Artifact

Pearl's paper describes a Plonky2 recursive proof stack and reports a final
proof below roughly 60 KB.

Nockchain's current `ai-pow-zk` stack is different:

- Layer-0 is a Plonky3-style composite STARK over Goldilocks.
- Recursive verification uses the local `Plonky3-recursion` substrate with
  Tip5 transcript machinery.
- The canonical artifact is a structured Hoon-compatible recursive certificate
  noun, not a single opaque byte atom and not the legacy plain `MatmulProof`.

This affects proof size, verification code, wire shape, and audit surface, but
it does not by itself change the underlying mineable matmul computation.

### Parameter Shape and Current Admission

Pearl supports a richer tile-shape/configuration language than native
Nockchain's square-tile helpers. Pearl-compatible recursive proving uses an
explicit strip schedule derived from Pearl public parameters, so the legacy
`tile` field is not the source of truth for the opened row/column set.

The spec requirement is Pearl's puzzle requirement set: any canonical
`PeriodicPattern` accepted by Pearl, valid shifted offsets, in-bounds opened
row/column sets, and Pearl's pattern-size target pricing. The implementation
now carries those shifted row/column sets into the canonical program and proof
statement as an explicit strip schedule.

## Is the Unit of Mineable Work the Same?

### Spec-Level Answer

Yes. Nockchain's Pearl-compatible spec uses the same unit in the operational
sense that matters for soundness:

- one Pearl-compatible work instance builds commitments, noise, noised
  matrices, and the tile state from `sigma || mu`;
- public Pearl ticket parameters select a valid shifted periodic row/column set
  from that work instance;
- the proof certifies that ticket computation;
- the jackpot hash is checked against a Pearl-shaped target.

The unit is not a cheap nonce hash. It is the ticket-bound noised matmul work
for that Pearl-valid ticket.

### Current Implementation Status

Implementation support now follows the Pearl ticket schedule for contiguous,
non-contiguous, and rectangular row/column patterns that satisfy Pearl's public
parameter sanity envelope and the recursive prover's stripe capacity.

### Multi-Tile Real AI Workloads

Spec yes; current explicit-schedule implementation yes.

Pearl's model gives a large matmul many ticket opportunities proportional to
the useful work performed. Nockchain's production spec adopts that rule: every
valid Pearl ticket from the committed work instance may be checked against
Nockchain's target after applying the same
`dot_product_length * rows_pattern.size() * cols_pattern.size()` pricing Pearl uses
for the ticket.

The current recursive certificate proves one explicit Pearl ticket. The proof
maps `t_rows`/`t_cols` and the Pearl periodic patterns to an explicit strip
schedule, then binds the opened ticket to the committed Pearl work instance.

## Current Security Posture

The current branch is sounder than the earlier pre-fix design because it closes
the cache-friendly Nockchain-only grinding bug:

- The Nockchain candidate block is committed through Pearl aux data in
  `sigma`, before `kappa` is derived.
- Pearl public ticket parameters select an opened tile from that committed work
  instance; they do not introduce independent Nockchain hash entropy.
- The jackpot digest is Pearl's `BLAKE3(M_ticket, key=s_A)` for the opened
  tile state.
- The recursive statement precheck rejects aux, ticket, commitment, target, or
  public-input metadata inconsistent with the verifier-recomputed Pearl work.

Remaining caveats:

- Hoon consensus still rejects `%ai-pow`; real verifier work is deliberately
  out of scope for the current milestone.
- Broad real recursive L1 proof-generation coverage remains opt-in because it
  is expensive, but the cheap Layer-0 and metadata tests cover explicit
  contiguous, non-contiguous, rectangular, and non-native-grid schedules.
- The current recursive proof stack is not Pearl's Plonky2 three-layer stack,
  so proof size and verification assumptions differ from Pearl's paper.
- Plain `MatmulProof` remains a diagnostic artifact and still carries
  row/column opening roots; it must not be treated as a production block proof.

## Action Plan

1. Keep `%ai-pow` fail-closed in Hoon. Do not pursue real verifier wiring in
   the current milestone.
2. Keep Pearl-style per-tile tickets as the production lottery rule. Do not
   introduce a Nockchain-only verifier-selected tile rule for Pearl-compatible
   consensus.
3. Keep the recursive certificate bound to the exact Pearl ticket schedule and
   Pearl-priced Nockchain target.
4. Keep broad explicit-schedule tests in place so future native square-tile
   refactors cannot silently narrow Pearl admission.
5. Keep the canonical commitment surface as exactly `[h-a-chunk h-b-chunk]`.
   Do not reintroduce row/column opening roots as commitment parameters.
6. Rename or document the `COMMITMENT_HASH` public-input slot as the
   nonce-derived jackpot key to avoid future Pearl/Nockchain confusion.
7. Keep Rust metadata-precheck tests broad enough to cover aux tamper,
   `h_a_chunk` / `h_b_chunk` tamper, invalid ticket metadata, target misses,
   and multi-tile rejection while proof support is incomplete. Do not add Hoon
   verifier-acceptance tests until real verifier work is explicitly scheduled.
