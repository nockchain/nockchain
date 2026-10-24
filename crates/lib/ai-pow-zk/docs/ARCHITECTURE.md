# AI-PoW proof architecture

## Statement ownership

`ai-pow-zk` proves a statement supplied by `ai-pow`; it does not parse a block or decide protocol policy. The bridge constructs:

- circuit parameters derived from the admitted dense or MoE ticket;
- a canonical `StripIndexSchedule`;
- committed model and routing data;
- the noised matmul witness and tile-state transcript;
- typed public inputs; and
- the canonical Layer-0 program for that schedule.

The verifier reconstructs these public structures from block-bound data before calling this crate.

## Layer 0: composite useful-work STARK

Layer 0 is a pinned `p3-batch-stark` proof over a composite trace. Specialized chips share each row under explicit selector bits. The production AIR constrains:

- BLAKE3 message preparation, rounds, and digests;
- matrix/routing commitment openings;
- noise expansion and packed operand ranges;
- dense or grouped matmul steps;
- stripe-XOR and tile-state transitions;
- fold/cumsum evolution; and
- jackpot key, preimage, and digest.

Seven LogUp buses bind values produced by one chip to values consumed by another. The production prover populates lookup frequencies and the verifier checks the globally balanced multiplicities.

The `canonical` module derives the only accepted instruction/program schedule from trusted statement parameters. The verifier-key preprocessing commits to that program, and the P0/D6 statement digest carries the binding through recursion.

## Layer 1: recursive verifier

The Layer-1 circuit verifies the Layer-0 proof with a Tip5-friendly Plonky3 recursion stack. It exposes the Layer-0 statement digest and public values needed by the final layer. Layer 1 is statement-bound: a proof over a different Layer-0 program or public-input layout produces a different digest.

The full Layer-1 checkpoint contains enough data for diagnostics but exceeds the block wire budget. It is not the production certificate.

## Layer 2: compact final proof

Layer 2 is a native BLAKE3 batch STARK over the Layer-1 verifier execution. The final proof is path-pruned to the openings required for verification. The wire certificate contains:

- the compact Layer-2 body; and
- a verifier-key/setup digest used to select and check the verifier-owned context.

It does not carry trusted setup, a canonical program, FRI parameters, or verifier metadata.

## Verifier context

`AiPowCompactBatchVerifierContext` is deterministic for a production trace-height bucket. It contains the proof-independent metadata and preprocessed structures needed to verify the compact body. `ai-pow-jets` owns the runtime table, validates committed digests, and pages contexts from disk.

A certificate's trace height is not trusted. The node recomputes the required Layer-0 size from the statement, selects that bucket, and requires the certificate route to match.

## Public inputs

The composite public-input vector binds the values that connect the work transcript to the block:

- fold/cumsum state;
- tile/jackpot state;
- matrix commitments;
- nonce-bound job key;
- jackpot key; and
- jackpot hash.

The canonical program additionally binds the opened row/column schedule, routing shape, tile dimensions, and instruction placement. Difficulty is checked against the certified jackpot at the chain boundary.

## Dense and MoE routes

Both routes use the same proof layers. MoE adds committed routing data, selected-expert constraints, routed-row mapping, and expert-local grouped matmul. It is not a separate weaker verifier path.

## Non-production surfaces

Unpinned `p3-uni-stark` helpers, raw Layer-0 proofs, oversized checkpoints, and test-support constructors exist for unit isolation and regression. They do not carry the complete production trust contract and must not be wired into consensus.
