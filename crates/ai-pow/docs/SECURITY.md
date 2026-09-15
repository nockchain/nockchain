# AI-PoW security model

## Security claim

A valid `%ai-pow` certificate is intended to show that the miner evaluated the committed Pearl-compatible dense or grouped-GEMM ticket bound to the candidate block and obtained the certified jackpot. The protocol prices that work by the committed ticket shape and accepts it only when the jackpot satisfies the consensus-computed AI target.

The claim relies on the composite AIR and recursive proof system being sound.

## Threat model

The miner controls the model values, ticket configuration within the admitted envelope, extranonce, routing data, openings, and proof bytes. A remote peer may send malformed artifacts without having run the reference miner. The verifier must remain deterministic, bounded, and fail-closed under both.

The principal attack classes are:

- nonce-only jackpot grinding after one matmul;
- precomputing or reusing work across block commitments;
- proving a cheaper matrix/tile than the difficulty-priced statement;
- opening unauthenticated rows or columns;
- MoE routing/column divergence;
- transcript or encoding ambiguity between Pearl, Rust, Hoon, and the circuit;
- reusing one Pearl work instance for multiple Nockchain blocks;
- supplying a favorable verifier setup, profile, or canonical program;
- malformed-certificate CPU or memory amplification; and
- cross-puzzle target or fork-choice confusion.

## Enforced bindings

### Per-attempt work

The extranonce changes the commitment and noise transcript before matrix execution. `kappa`, `H_A/H_B`, `s_A/s_B`, noised operands, tile state, and jackpot all change. There is no post-matmul nonce accepted by Nockchain. The cost-bearing matmul therefore precedes every independent jackpot trial.

### Block and replay binding

The candidate block commitment is included in exactly one `NOCKCHAIN-AI-POW-AUX` record. Certificate public inputs and the canonical Layer-0 program derive from that attempt. A proof for another candidate or a coinbase carrying multiple Nockchain commitments rejects.

### Matrix and opening binding

BLAKE3 commitments authenticate the model data. The canonical opened schedule is verifier-derived, and the Layer-0 program binds every opened row/column, noise expansion, matmul input, fold state, and public jackpot through direct constraints and LogUp buses.

### MoE binding

Pearl bounds such as `top_k < experts`, routing span, selected expert, `n_e`,
and expert-local columns are checked before proof work. `routing_data` is
strictly increasing within each expert span, so a token appears once per expert
and valid row-pattern offsets cannot reuse an A row across that expert's tiles.
The proof binds routed token positions to opened rows and prevents columns from
escaping the selected expert.

### Difficulty binding

Shape adjustment uses checked arithmetic over statement-bound dimensions. The final jackpot comparison uses the exact Nockchain AI target supplied by Hoon. AI and ZK targets are separate consensus values; this crate never converts one into the other.

### Verifier ownership

The verifier derives the public statement, trace height, canonical program, proof profile, and expected setup digest. Certificate metadata is checked against those values and cannot select weaker parameters.

## Resource safety

Parameter validation precedes large allocations. Noun decoding and proof reconstruction enforce depth, node, atom, aggregate-byte, list, nonce, pattern, routing, and trace-height limits. Setup selection is from a finite committed table, so an untrusted trace height cannot trigger circuit construction.

The consensus jet catches decode/verifier panics and rejects deterministically; local setup-file failure is a node fault rather than a block-invalid vote.

## Cryptographic assumptions

The design relies on:

- BLAKE3 collision, preimage, and keyed-hash security for commitments and jackpots;
- Tip5 security in the recursive transcript/commitment roles where used;
- Goldilocks-field arithmetic matching the circuit and Rust reference;
- FRI low-degree-testing soundness at the configured profiles;
- Fiat-Shamir behaving as a random oracle with correct transcript ordering and domain separation;
- LogUp multiplicities and the composite AIR fully constraining every claimed relation;
- recursion correctly verifying the Layer-0 statement and binding the canonical program; and
- canonical JAM/postcard encodings having one accepted representation.

## Consensus assumptions outside this crate

The Hoon kernel must enforce one activation boundary, puzzle-specific branch-local ASERT, exact target equality, equal expected-work normalization, accumulated-work fork choice, puzzle-specific coinbase recipients, and normal transaction/block validity. `ai-pow` cannot compensate for a defect in those rules.

## Assurance status

The dated [dual-puzzle audit](2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md) records implemented findings and regression evidence.
