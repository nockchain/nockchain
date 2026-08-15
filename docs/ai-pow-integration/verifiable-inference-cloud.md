# Aggregating AI-PoW miners into a verifiable inference cloud

Status: Draft (design consideration)
Owner: Nockchain Maintainers
Last Reviewed: 2026-08-15
Canonical/Legacy: Legacy (design exploration; no protocol authority)

## Scope

This document considers a mechanism for aggregating Nockchain's AI-PoW miner
fleet into a consumer-GPU cloud that sells *verifiable inference*: a client pays
for tokens from a named model and receives a settlement-grade guarantee that the
tokens came from that model's real weights.

It is a design exploration, not a protocol proposal. It does not change
`PROTOCOL.md`, consensus activation, ASERT, fork choice, or block validity, and
it deliberately stops short of code because the sampling rates it recommends
depend on one quantity nobody in this repository has measured yet (§9).

## 1. The claim

Nockchain is already most of the way to a verifiable inference network, and
almost none of the remaining distance is cryptography.

The AI-PoW puzzle does not mine a synthetic matmul that merely *resembles*
inference. Per [`crates/ai-pow/src/quant.rs`](../../crates/ai-pow/src/quant.rs),
the production model is `pearl-ai/Llama-3.1-8B-Instruct-pearl`, served through a
vLLM mining plugin, and the quant-extraction contract `Q` is documented as
**bit-lossless**: a pure reindex of the operands vLLM already computed into
`ai-pow`'s `(A, B)` layout, with no requantization. The mined integers *are* the
inference integers.

So the primitive on offer is not "a chain that could someday host inference." It
is a fleet of consumer GPUs already executing real Llama-3.1-8B GEMMs, already
committing to their operands, and already able to produce a compact recursive
STARK certificate that a named tile of a named committed matmul was computed
correctly. What is missing is the *service layer*: provenance, addressing,
sampling, dispute, and settlement.

## 2. What the repository already provides

| Capability | Where | Why it matters here |
|---|---|---|
| Real-model INT7/INT8 GEMM as the mined unit | `ai-pow/src/quant.rs`, `params.rs::LLAMA_3_1_8B_GATE_UP` | The work is inference, not an inference-shaped mimic |
| Merkle commitments over operand matrices | `ai-pow/src/commit.rs` (`matrix_commitment`, `a_row_leaf_hash`, `b_col_leaf_hash`) | Weights and activations can both be pinned before challenge |
| **Proof of an arbitrary named tile** | `ai_pow_zk::canonical::StripIndexSchedule::from_tile(zk_params, tile_i, tile_j)` | Verification can target a tile *chosen after the fact*, not just a lottery winner |
| Compact recursive certificate | `ai-pow-zk` Layers 0/1/2, `zk_bridge` | Small, verifier-owned-setup proof suitable for onchain settlement |
| Verifier-owned setup, prover supplies none | `ai-pow-zk/docs/ARCHITECTURE.md` | A dishonest server cannot substitute its own public parameters |
| Grouped-GEMM / MoE routing binding | `ai-pow/src/pearl_moe_routing.rs` | Extends to MoE models without a new proof system |
| Production consumer-GPU throughput | `docs/ai-pow-integration/pearl-v3-rtx5090-roofline.md`; commits `c8d6b13`, `330bece` | ~335–348 TMAC/s sustained on a single RTX 5090 |

`StripIndexSchedule::from_tile` is the load-bearing one. The PoW path proves
whichever tile happened to win the jackpot; that same constructor will prove a
tile that someone else names. Every verification scheme below is built on that,
and it already exists.

The repository is also explicit that it does *not* supply what a service layer
needs. `synth.rs` states outright that "model provenance, economic usefulness,
and uniqueness are network policy concerns outside this deterministic generator,"
and `ai-pow-zk/docs/SECURITY.md` disclaims "model provenance, economic
usefulness, confidentiality, or uniqueness." Those disclaimers are precisely the
gap this design fills — and they are policy gaps, not cryptographic ones.

## 3. The load-bearing observation: decode leaves the tensor cores idle

This is the fact that makes a *consumer* GPU cloud coherent rather than a
worse-priced imitation of a datacenter one. It comes out of the roofline.

An RTX 5090 has 32 GiB of GDDR7 at 1,792 GB/s and sustains ~335 TMAC/s on the
real mining transcript. Llama-3.1-8B is ~8.03B parameters, ~8.5 GB at the shipped
quantization, and costs ~8.03 GMAC per token.

**Weights fit on one card.** No tensor parallelism, no NVLink, no collectives,
no interconnect-sensitive placement. Each miner holds a whole model replica and
the cloud is embarrassingly parallel at the *request* level. This is the single
biggest reason the aggregation is tractable here and is not tractable for a
400B-class model on the same hardware.

**KV cache, not weights, caps the batch.** With ~23 GB free after weights and
GQA KV at ~128 KiB per token across all 32 layers, the card holds ~175k KV
tokens: roughly 20 concurrent sequences at 8k context, ~85 at 2k. That ceiling
sits *below* the compute/bandwidth crossover, which the same roofline puts near
batch ~198. Consumer decode is therefore permanently memory-bound.

Work the consequence through at batch 32, 2k context:

| Quantity | Value |
|---|---|
| Weight traffic per decode step | ~8.5 GB |
| KV traffic per decode step | ~8.6 GB |
| Step time at 1,792 GB/s | ~9.5 ms |
| Throughput | ~3.4k tok/s |
| Tensor-core time actually used | 32 × 24 µs ≈ 0.77 ms |
| **INT8 tensor-core utilization** | **~8%** |

During decode a consumer GPU is using something like a twelfth of its INT8
compute. The remaining ~92% is not merely idle — it is idle *in exactly the units
AI-PoW consumes*, with a tiny working set that does not contend for the bandwidth
decode is starving on.

Prefill is the mirror image: it is compute-bound and shaped like the puzzle
already. `LLAMA_3_1_8B_GATE_UP` is `m=4096, k=4096, n=14336` — and in the `Q`
convention `A` is the *activation* matrix with `m` tokens of rows. The mined work
unit is literally a 4,096-token prefill batch. A 1,000-token prompt's prefill is
~8.03 TMAC, ~24 ms at 335 TMAC/s.

So the two workloads are complements, not competitors:

- **Prefill** is compute-bound and is *already the mined shape* — serving it and
  mining it are close to the same act.
- **Decode** is bandwidth-bound and leaves ~92% of the tensor cores free for PoW
  mining and for proof generation.

A datacenter operator cannot exploit this as cleanly, because their economics
assume high-batch decode on HBM parts where the idle fraction is much smaller.
The consumer fleet's structural weakness — small memory, capped batch — is what
creates the spare compute that pays for verification.

## 4. What is missing

| Gap | Severity | Note |
|---|---|---|
| **Coverage.** PoW proves one tile (`h·w ≤ 256`) out of a 4096×14336 = 58.7M-element output — a ~4×10⁻⁶ sample | Fundamental | Full-forward-pass ZK for 8B is out of reach; consensus caps Layer-0 trace at 2¹⁹ (`AI_POW_MAX_TRACE_HEIGHT`) |
| **Noise.** The puzzle proves `(A+E)(B+F)`, with commitment-keyed low-rank noise for anti-reuse | Design | Inference needs clean `A·B`; needs a domain-separated statement |
| **Provenance.** `HASH_B` binds *some* weights, not *the certified model's* | Blocking | Needs a published model manifest |
| **Addressing.** No request mempool, matching, or escrow | Blocking | Service layer, not protocol |
| **Privacy.** Miners see prompts and activations in the clear | Honest limit | See §10 |

The coverage gap is the central one, and it does not have a cryptographic
solution at this model scale. Proving every GEMM in a 32-layer forward pass is
not within a factor of a few of the 2¹⁹-row Layer-0 budget; it is off by orders
of magnitude. **The mechanism must therefore rest on a sampling-and-stake
argument, with cryptography used to make sampling unforgeable rather than to make
verification exhaustive.** That is the honest framing, and the rest of this
document takes it as the premise.

## 5. The mechanism: commit, sample, dispute

Three tiers. Each is cheap where it is common and expensive only where it is
rare.

### Tier 1 — Commitment (every request, effectively free)

The miner returns the output tokens together with a signed commitment to the
whole computation:

- the model manifest digest (§6),
- the request digest — prompt, sampling parameters, and an explicit seed, so the
  forward pass is deterministic and independently replayable,
- a Merkle root over the **per-layer activation stream**, and
- the miner's staked identity.

Per-layer activations are small: 4,096 dims × 4 bytes ≈ 16 KiB per token per
layer, ~512 KiB per token across 32 layers. Hashing that is microseconds and does
not perturb the decode roofline in §3. Committing at layer boundaries — rather
than to every intermediate GEMM output — is what keeps Tier 1 free.

The purpose is to **pin every intermediate value before any challenge is
issued**. After Tier 1 the miner has no remaining freedom to choose what it
"computed."

### Tier 2 — Sampled spot proofs (rare, expensive)

Once the commitment is published, a public beacon the miner could not predict —
the next block hash is already available and already unpredictable to it —
selects a `(layer ℓ, tile i, j)`. The miner must produce the existing compact
recursive certificate for exactly that tile:

- `B` (weights) opened against the model manifest,
- `A` (activations) opened against the Tier-1 activation root for layer ℓ,
- the schedule from `StripIndexSchedule::from_tile`,
- the claimed output tile.

This is the current `ai-pow-zk` statement with the jackpot/target comparison
removed and the noise zeroed. It proves: *the certified weights, times the
activations this miner already committed to, produce the tile it claims.*
Chained through the per-layer activation roots, a miner that fabricated any layer
is caught whenever the beacon lands on it.

Commit-then-challenge is what makes a 4×10⁻⁶ tile sample meaningful. The sample
is tiny, but the miner must be correct *everywhere* to survive a uniformly random
draw it cannot anticipate.

### Tier 3 — Dispute by bisection (very rare)

Execution is deterministic given the manifest and the seed, so any second miner,
client, or watchtower can replay a request for roughly one forward pass of
consumer GPU time and compare Tier-1 roots. On disagreement it opens a challenge.

Because Tier 1 already commits per layer, the parties bisect over the 32-layer
chain to the first divergent layer in ~5 rounds, then over tiles within that
layer. The terminal step of the bisection is exactly a Tier-2 proof. One proof
system serves both tiers; the dispute game adds no new cryptography.

Failure to answer within the timeout, or an answer that verifies against a
different value, slashes the stake.

The three tiers compose into: cheap always, cryptographic sometimes, adversarial
rarely — with cost concentrated where dishonesty is, not where throughput is.

## 6. Model provenance

Tier 2 is worthless if `HASH_B` can bind arbitrary weights. The network needs a
**model manifest**: for each certified model, the per-tensor BLAKE3 Merkle roots
in the exact layout `commit.rs` uses, plus shape, quantization group, and the
`MatmulParams` profile per layer.

Publishing the manifest onchain makes "this certificate is for the real
Llama-3.1-8B-Instruct-pearl" a checkable statement rather than a claim. This is
the concrete discharge of the provenance concern `synth.rs` explicitly routes to
network policy.

Note the INT-only production scoping already encoded in `params.rs`: `down_proj`
is group_0 FP8 and is guarded off as non-mineable (`Fp8LayerNotMineable`). A
manifest must record which layers are provable and which are attested only by
Tier-1 commitment and Tier-3 replay. **Verifiable inference on this model is
therefore verifiable over its INT7 group_1 linear layers, not over every FLOP in
the forward pass** — and the service must say so plainly rather than imply
end-to-end proof coverage.

## 7. Aggregation and scheduling

- **Unit of supply:** one miner = one whole-model replica on one GPU. Request-level
  parallelism only.
- **Matching:** clients post signed requests (manifest digest, prompt digest, max
  tokens, sampling params, seed, price, deadline) to an offchain mempool; miners
  claim them. Payment escrows in `$NOCK` through the existing UTXO and wallet
  crates, releasing on Tier-1 commitment plus a quiet dispute window.
- **Interleaving:** the miner serves inference when paid demand exists and falls
  back to PoW otherwise — and per §3 it can do *both at once* during decode,
  because decode leaves the tensor cores mostly free.
- **Preemption granularity:** the CUDA miner already runs 3–50 ms batched launches
  with cancellation on candidate replacement (`ai-pow-miner`'s stale-work path).
  That is finer than a typical 20–50 ms per-token budget, so mining can yield to
  inference without violating interactive latency.

## 8. Economics: PoW as the reserve price

The useful economic property is that Nockchain **already runs a continuous spot
market for exactly this hardware doing exactly these GEMMs.** Block-reward EV per
GPU-second is measurable from sustained TMAC/s and the current AI target — and
`DIFFICULTY.md` is explicit that the target `T` prices one MAC-equivalent of
matmul work, not one attempt, which is precisely the denomination needed.

That gives a floor: inference must outbid mining EV per GPU-second, or the miner
keeps mining. No token subsidy or bootstrapping incentive is required to price
the fleet, and supply is elastic in both directions.

**Sampling rate.** With proof cost `C_p` and request cost `C_r` (both GPU-seconds)
and sample rate `p`, verification overhead is `p · C_p / C_r`. Holding overhead
to a budget `β` gives `p ≤ β · C_r / C_p`.

**Stake.** A miner skipping work gains at most the value of `C_r` and is detected
with probability `p`, so honesty requires roughly `S > value(C_r) / p`.
Substituting: `S > value(C_p) / β`. At `β = 5%`, the stake floor is about 20× the
*value of a single proof* — not 20× the value of a request, and independent of
request size. That is a mild requirement, and it is the reason this scheme can
work with consumer-scale operators rather than only well-capitalized ones.

Systematic cheating is bounded much more tightly than one-shot cheating, because
Tier-3 replay is cheap and available to anyone: a miner that cheats repeatedly
faces detection probability approaching 1.

## 9. What must be measured first

**The sampling rates above are unresolved until `C_p` is measured, and I have not
found a measurement of it in this repository.** Everything else in this design is
grounded in code or in the existing roofline; this one number is not, and I have
deliberately not invented a value for it.

Required before any of this is committed to:

1. **Wall-clock cost of one compact recursive certificate** at each production
   trace bucket 2¹³…2¹⁹ — Layer 0 + Layer 1 recursion + Layer 2 — on the target
   consumer GPU and on CPU. `zk_bridge` already instruments `l1_circuit_build_ms`,
   `l1_in_circuit_verify_ms`, and `l1_outer_cert_ms`, so the harness largely
   exists. This sets `p`, and therefore the entire security/overhead trade.
2. **Whether proving can overlap decode** without disturbing the §3 roofline, or
   whether it contends for bandwidth after all. The 92% headroom claim is an
   arithmetic upper bound, not a measurement.
3. **Real served throughput** at realistic context lengths, against the roofline's
   ~3.4k tok/s estimate, following the same measurement discipline as the RTX 5090
   roofline (sustained clocks, real data, median of repeated runs — the roofline
   doc's warning that synthetic all-ones inputs inflate results by 22–30% applies
   here too).
4. **Cost of the zero-noise inference statement**, and confirmation that
   domain-separating it cannot weaken the PoW anti-reuse invariant.

## 10. Risks and honest limits

**No prompt privacy.** The miner sees prompts and activations in the clear, and
mid-stack activations are substantially invertible. The ZK layer here proves
*correctness*, not confidentiality — the client's input is not a secret from the
prover, it is an input to it. Any claim otherwise would be false. Realistic
options are to scope the service to non-sensitive workloads, or to keep the first
and last layers client-side so miners see only mid-stack activations, which
narrows but does not close the exposure.

**Statement separation is a security requirement, not hygiene.** An inference
certificate must never be replayable as a PoW certificate or vice versa. Zeroing
the noise removes the anti-reuse property the puzzle depends on
(`ai-pow/docs/2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md`, finding F01: cached
nonce-independent state let a miner grind without fresh inference). Domain
separation must be enforced at the statement level and tested adversarially in
both directions.

**Consensus surface growth is the main systemic risk.** This repository's own
audit record — the dual-puzzle consensus audit and
`2026-07-29_TIME_BANKED_FORK_EXPLOIT.md` — shows how readily dual-puzzle
economics produce exploits that are invisible in the component crates. A dispute
game with timeouts and slashing added to the consensus kernel is a large new
attack surface against a chain whose fork choice is already carrying two puzzles.

The mitigation follows the repository's own stated architecture rather than
fighting it. `README.md`: "Applications execute offchain as sovereign NockApps and
can settle verifiable results to the shared chain." **The inference cloud should
be a NockApp** — mempool, matching, escrow, and the bisection game all offchain,
settling only outcomes to the chain. The consensus kernel should learn nothing
about inference beyond what it already verifies. Only the model manifest registry
plausibly belongs onchain, and even that could start as a NockApp with a
well-known root.

**Verification is partial by construction.** Tier 2 covers INT7 group_1 linears.
Attention, normalization, activation functions, sampling, and the FP8 layers are
covered by Tier-1 commitment and Tier-3 replay determinism, not by proof. This is
a real and permanent limit at this model scale, and the service must market
itself accordingly.

## 11. Staged plan

Ordered so that each stage produces something independently checkable, and so
that the measurement gate precedes the design commitments that depend on it.

1. **Measure `C_p`** (§9). Nothing downstream is well-posed without it.
2. **Model manifest format and registry**, offline first: per-tensor roots in
   `commit.rs` layout, per-layer provable/attested classification, conformance KAT
   against the shipped model.
3. **Zero-noise inference statement** in `ai-pow`, domain-separated from the PoW
   statement, with cross-replay rejection tests in both directions.
4. **Tier-1 commitment path** in the vLLM plugin boundary: per-layer activation
   roots emitted on the serving path, with the overhead measured against §9.2.
5. **Tier-2 spot proof**, driven by block-hash beacon selection through
   `StripIndexSchedule::from_tile`.
6. **Tier-3 bisection game** as a NockApp, with escrow and slashing; consensus
   untouched.
7. **Scheduler and interleaving**, including PoW/inference preemption at the
   existing cancellation granularity.

Stages 2–5 are additive and carry no consensus risk. Stage 6 is where the real
design review is owed, and it should be reviewed against the existing dual-puzzle
audit findings rather than in isolation.

## 12. Summary

The cryptography for verifiable inference on Nockchain largely exists; the
missing pieces are provenance, addressing, sampling policy, and settlement. The
mechanism that fits the hardware is commit-then-challenge with staked dispute:
free commitments on every request, unforgeable random spot proofs, and cheap
deterministic replay as the backstop.

The structural argument for consumer GPUs specifically is that an 8B model fits
on one card — so the fleet needs no interconnect — and that KV-capped decode
leaves ~92% of the INT8 tensor cores idle in exactly the units AI-PoW consumes.
The verification is paid for out of compute that the serving workload cannot use.

The honest limits are that proof coverage is a sample over the INT7 linear layers
rather than the whole forward pass, that miners see prompts, and that the dispute
game is the one genuinely risky addition — which is why it belongs in a NockApp
and not in the consensus kernel.
