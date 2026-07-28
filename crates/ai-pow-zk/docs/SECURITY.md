# AI-PoW proof security model

## Intended claim

Given a verifier-derived AI-PoW statement, a valid compact certificate is intended to prove that the committed dense or MoE ticket trace satisfies the canonical useful-work program and yields the public jackpot bound to the candidate block.

This is a computational soundness claim, not a claim that the model is valuable, that a miner followed the reference software, or that the block satisfies rules outside the proof statement.

## Proof-system assumptions

Soundness relies on all of the following:

1. **AIR completeness.** Every semantically relevant trace value is range-constrained and connected to the committed inputs and public outputs.
2. **Selector and program binding.** Only the verifier-derived instruction schedule can satisfy the pinned production AIR.
3. **LogUp soundness.** Producer/consumer buses use correct keys and multiplicities, including packed operands, commitment openings, routing, and matmul inputs.
4. **FRI soundness.** The configured blowup, query count, extension field, cap, and proof-of-work settings provide the claimed low-degree-test security for each layer.
5. **Fiat-Shamir soundness.** Transcript observations occur in the required order with unambiguous encoding and domain separation.
6. **Hash security.** Tip5 and BLAKE3 provide the collision/preimage or sponge properties required in their respective commitment and transcript roles.
7. **Recursive-verifier correctness.** Layer 1 correctly verifies Layer 0, Layer 2 correctly proves Layer 1, and the statement digest preserves the canonical-program/public-input binding.
8. **Setup coverage.** The verifier-key digest commits to every preprocessed value and parameter used by verification.

The configured profiles target 60 FRI query bits without proof-system grinding. That parameter count is not by itself an end-to-end security proof; algebraic weaknesses, missing constraints, transcript mistakes, or hash weaknesses can dominate it.

## Adversarial invariants

### Canonical program

The verifier reconstructs the Layer-0 program from the trusted opened schedule. Prover-supplied program rows are never authority. A certificate over a different schedule, routing, matrix opening, or instruction placement must fail the statement-digest or verifier-key binding.

### Committed operands and matmul

Authenticated matrix strips feed commitment-keyed noise and the exact operands consumed by matmul. Range checks enforce Pearl's INT7/plain and packed-noised domains. Cross-chip LogUp buses prevent the prover from committing one tile and computing a cheaper one.

### Fold and jackpot

Matmul outputs drive the stripe/tile reduction and fold state. Public cumsum, tile state, jackpot key, and jackpot digest are tied to the last valid row by explicit keystones. The jackpot cannot be chosen independently of the work trace.

### MoE routing

Routing commitment, selected expert, routed rows, local expert width, and grouped-matmul columns are one relation. The proof must reject a valid matrix computation paired with a different route, a row from another token, or a column outside the selected expert.

### Attempt freshness

Proof-independent setup and prover preprocessing may be cached. Nonce-bound commitments, noise, operands, trace, and witness may not. A new nonce changes the proven statement and requires fresh cost-bearing work.

### Verifier-owned metadata

Trace height, proof profile, public-value layout, canonical program, setup, and expected verifier-key digest come from the verifier. A certificate is never allowed to select a smaller AIR or weaker FRI profile through its own metadata.

## Resource safety

Proof bytes are decoded canonically under node-level limits before verification. Trace height is bounded by the admitted protocol envelope and maps to a finite setup table. Unknown heights reject without building a circuit. Existing-bucket load failures are local faults and must not become deterministic invalid-block decisions.

The native consensus wrapper catches panics from attacker-controlled decode and verification. This protects availability but does not convert an incomplete AIR into a sound one.

## Non-goals

This crate does not prove:

- chain activation, target derivation, ASERT, fork choice, or coinbase distribution;
- transaction validity or state transition correctness of the candidate block;
- model provenance, economic usefulness, confidentiality, or uniqueness;
- Pearl chain acceptance; or
- network authenticity or miner honesty.
