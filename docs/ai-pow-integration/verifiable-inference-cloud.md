# Aggregating AI-PoW miners into a verifiable inference cloud

Status: Draft (design consideration)
Owner: Nockchain Maintainers
Last Reviewed: 2026-08-15
Canonical/Legacy: Legacy (design exploration; no protocol authority)

## Scope

A mechanism for aggregating Nockchain's AI-PoW miner fleet into a consumer-GPU
cloud that sells verifiable inference. One mechanism is chosen per purpose;
rejected alternatives are named once and not revisited. There is a single route
into consensus, and it is one new lock primitive.

This is a design exploration. It does not change `PROTOCOL.md`, activation, ASERT,
fork choice, or block validity, and it stops short of code because the sampling
policy depends on a quantity nobody in this repository has measured (§7).

## 1. The claim

Nockchain is already most of the way to a verifiable inference network, and almost
none of the remaining distance is cryptography.

Per [`crates/ai-pow/src/quant.rs`](../../crates/ai-pow/src/quant.rs), the production
model is `pearl-ai/Llama-3.1-8B-Instruct-pearl`, served through a vLLM mining
plugin, and the quant-extraction contract `Q` is documented as **bit-lossless**: a
pure reindex of the operands vLLM already computed, no requantization. The mined
integers *are* the inference integers.

What is missing is the service layer: provenance, addressing, verification policy,
and settlement.

| Already in the repository | Where |
|---|---|
| Real-model INT7/INT8 GEMM as the mined unit | `ai-pow/src/quant.rs`, `params.rs::LLAMA_3_1_8B_GATE_UP` |
| Merkle commitments over operand matrices | `ai-pow/src/commit.rs` |
| **Proof of an arbitrary named tile** | `StripIndexSchedule::from_tile` |
| Compact recursive certificate | `ai-pow-zk` Layers 0/1/2 |
| **A resident certificate verifier** | `ai-pow-jets::ai_pow_verify_jet`; nodes build the full setup table at boot |
| Burn as a lock primitive | `LockPrimitive::Burn` / `%brn` |
| Fixed-percentage split precedent | Aletheia's 80/20 miner/fund split |
| ~335–348 TMAC/s on one RTX 5090 | `pearl-v3-rtx5090-roofline.md` |

Two of these do more work than the rest. `from_tile` proves a tile *someone else
names*, not just a jackpot winner. And the certificate verifier is **already
resident on every node** — which is what makes §6's consensus route small.

## 2. Why consumer GPUs

The argument is arithmetic. An RTX 5090 has 32 GiB at 1,792 GB/s and sustains ~335
TMAC/s. Llama-3.1-8B is ~8.03B params, ~8.5 GB quantized, ~8.03 GMAC/token.

**Weights fit on one card.** No tensor parallelism, no collectives, no
interconnect-sensitive placement. Each miner holds a whole replica; the cloud is
parallel at the request level. This is why aggregation is tractable here and is not
tractable for a 400B model on the same hardware.

**KV cache caps the batch below the compute crossover.** ~23 GB free after weights,
GQA KV ~128 KiB/token across 32 layers → ~175k KV tokens: ~20 sequences at 8k
context, ~85 at 2k. The compute/bandwidth crossover is near batch ~198. Consumer
decode is therefore permanently memory-bound:

| Decode step · batch 32 · 2k context | |
|---|---:|
| Weight traffic | ~8.5 GB |
| KV traffic | ~8.6 GB |
| Step time at 1,792 GB/s | ~9.5 ms |
| Throughput | ~3.4k tok/s |
| Tensor-core time used | ~0.77 ms |
| **INT8 utilization** | **~8%** |

The other ~92% is idle *in exactly the units AI-PoW consumes*, with a working set
too small to contend for the bandwidth decode is starving on.

Prefill is the mirror image and is already the mined shape: `LLAMA_3_1_8B_GATE_UP`
is `m=4096, k=4096, n=14336`, and in the `Q` convention `A` is the activation
matrix — so the mined unit is a 4,096-token prefill batch. Serving prefill and
mining it are nearly the same act; decode leaves the compute free for both mining
and proving.

A datacenter operator cannot exploit this as cleanly: their economics assume
high-batch decode on HBM parts where the idle fraction is much smaller. The
consumer fleet's structural weakness is what funds its verification.

## 3. Decisions

One mechanism per purpose. Alternatives are named here and not carried further.

| Purpose | Chosen | Not chosen, and why |
|---|---|---|
| Detect wrong model / precision / prompt | **TOPLOC fingerprint on every response** [1] | Bit-exact activation hashing — fails on honest miners (§4) |
| Bind the claim | **The signed fingerprint is the commitment** | A separate commitment tier — it was the same data twice |
| Resolve a disagreement | **Compact recursive certificate on a named tile** | Majority re-execution — pays N× for a sampled guarantee |
| Choose which tile | **Challenger names it; block hash breaks ties** | Interactive bisection — unnecessary (§5) |
| Make cheating unprofitable | **Forfeiture: never pay before verify** | Bonds — redundant once detection is near-certain (§5) |
| Sybil / claim-abandon resistance | **One PoW admission ticket per claim** | Reputation, capital deposits, whitelists |
| Set the price | **Auction, per MAC-equivalent** | Protocol price controller — supply is self-reported, so it is a cartel lever |
| Accrue value, price fake demand | **Fixed burn fraction `β`** | Moving `β` — damps the supply response exactly when it should attract capacity |
| Weight authenticity | **Onchain model manifest** | Trusting `HASH_B` alone — it binds *some* weights, not the certified model's |
| Consensus integration | **One lock primitive (§6)** | A dispute-game NockApp, kernel changes, a second puzzle |

Cut entirely: the challenger bounty (the client is the challenger, motivated by its
own refund), the optional bonded assurance tier, per-layer bisection, and separate
identity-activation and challenge tickets.

## 4. Verification: two layers, chosen for opposite blind spots

**Tier A — TOPLOC fingerprint. Every response, ~free.**

A locality-sensitive hash of the top-`k` values of the last hidden state,
polynomial-encoded [1]. Because the final layer depends on every prior computation,
it is sensitive to the entire stack — attention, normalization, dequant scales, and
the FP8 layers the certificate cannot reach.

- **258 bytes per 32 tokens** on Llama-3.1-8B — ~1000× less than recording
  activations.
- **Detects modified model, prompt, or precision** at 100% accuracy with no false
  positives in the paper's evaluation.
- **Robust across GPU types, attention kernels, tensor-parallel layouts, and
  algebraic reorderings.**
- **Verification up to 100× faster than generation** — one prefill of the claimed
  output instead of autoregressive decode.

The evaluation model is Llama-3.1-8B-Instruct, the same family AI-PoW mines. Cost
lands on the right side of §2: a top-`k` reduction over a 4,096-wide state is
negligible compute and ~8 KiB/token of reads against ~17 GB per step. As an async
tee it should cost neither TTFT nor throughput — a claim that belongs in §7, not in
the assumptions.

**Tier B — Compact recursive certificate. Only on dispute.**

The current `ai-pow-zk` statement with the jackpot comparison removed and the noise
zeroed, over a named tile: weights opened against the manifest, activations against
the committed integer-operand root, schedule from `from_tile`. It proves the
certified weights times the committed activations produce the claimed tile.

**Why both.** TOPLOC gives breadth without soundness; the certificate gives
soundness without breadth. Neither alone is adequate, and the temptation runs toward
dropping the expensive one — see §8.

| | Tier A | Tier B |
|---|---|---|
| Cost | ~free, every response | expensive, on dispute only |
| Coverage | whole forward pass incl. attention, norms, FP8 | one tile of one INT7 GEMM |
| Robust to FP nondeterminism | yes, by construction | n/a — integer path |
| Guarantee | statistical | cryptographic |

> **Correction carried forward.** An earlier draft committed to a *bit-exact* Merkle
> root over per-layer activations and claimed execution is deterministic given
> manifest and seed. That is false for a heterogeneous fleet: architectures,
> attention kernels, and above all varying batch composition change float reduction
> order, and §2 shows batch composition varies continuously. Two honest miners would
> disagree. The surviving statement is narrower — **the INT7/INT8 accumulate is
> integer and exactly reproducible**, which is why the roofline can rely on
> `mma.sync.satfinite` matching scalar accumulation and why `Q` is bit-lossless. Bit-
> exact commitment is correct for the integer operands only; everything
> float-contaminated needs Tier A.

## 5. Settlement: verify before pay

**No capital is posted.** A non-yielding performance bond is not staking — it earns
no yield, carries no fork-choice weight, and does not become the security budget.
The case against requiring one is not principle but **redundancy**: once detection
is near-certain, forfeited payment already exceeds any profitable cheat's gain, so
bonded collateral secures a loss the miner is already fully exposed to. It is also a
capital barrier against the one-GPU participation thesis, and locked `$NOCK` is
genuinely expensive to hold precisely because it does not yield.

The protocol's own analogy: **an invalid block seizes nothing. It wastes the work.**

**The flow.** Client escrows payment with a signed request (manifest digest, prompt
digest, max tokens, sampling params, seed, bid per MAC-equivalent, deadline). Miner
claims it with a PoW admission ticket signed by its serving identity — a keypair
whose standing comes from work, not deposit. Miner serves and publishes tokens plus
fingerprint. Client verifies at ~1% of serving cost and signs; escrow releases
`(1−β)` to the miner and `β` to a `%brn` output. If the client disputes, it names a
tile and the miner must answer with a certificate: verifying releases to the miner,
failing or timing out refunds the client and retires the identity.

**No bisection.** Both parties hold the full fingerprint chain, so the challenger
*names* the first divergent layer directly. The interactive multi-round game was
solving a problem that does not exist, and it was the largest consensus surface in
the design.

**Price per MAC-equivalent.** `DIFFICULTY.md` already prices the target `T` per
MAC-equivalent, so mining EV and inference bids share a unit and a miner's
allocation decision is a scalar comparison. The manifest fixes every shape, so a
request's MAC count is deterministic from `(manifest, prompt length, max tokens)` and
computable by both parties **before** execution — no metering to trust, no billing
dispute surface. Quantity is objective; only price is negotiated, by auction.

**Why burn.** Not primarily deflation. Two stronger reasons:

*It is the only self-dealing-proof rail.* Verification secures correctness and does
nothing about **fake demand** — a miner paying itself to manufacture volume, which
round-trips at zero cost under a fee that goes entirely to the miner. Under a burn
every washed `$NOCK` costs `β`. That, not a fairness intuition, is what sets `β`.

*It is the only way to raise miner revenue against a hard cap.* Aletheia sustains a
64-NOCK floor for ~68 years because the old schedule was "forcing the network onto
fee revenue earlier than the application ecosystem can sustain." That floor is
denominated in NOCK, so its real value is what a burn improves, and issuance cannot
improve it against 2³².

`β` is bounded not by fairness but by the **off-chain escape hatch**: miner and
client can always settle privately, so `β` must stay under what the onchain rail is
worth — escrow, the tie-break beacon, adjudication. Ship a low constant (10–30%) and
raise it against observed defection.

**Deterrence.** Let `M` = mining EV per GPU-second, `C_r` = GPU-seconds to serve,
`γ > 1` the cheat's compute advantage, `P` the payment, `q` the detection
probability before release, and `ρ = P/(M·C_r) > 1` the premium inference pays over
mining. Honest serving earns `P`; cheating earns `(1−q)P + M·C_r(1 − 1/γ)`. Cheating
fails to pay when:

```text
q  >  (1 − 1/γ) / ρ
```

| `γ` | `ρ = 1` | `ρ = 2` | `ρ = 3` |
|---|---:|---:|---:|
| 2 | 0.500 | 0.250 | 0.167 |
| 4 (INT4 served for INT7) | 0.750 | 0.375 | 0.250 |
| 8 | 0.875 | 0.438 | 0.292 |

**Without Tier A this is unreachable** — certificate sampling alone gives `q ≈
0.01–0.05` and misses every row. That is exactly why earlier drafts needed capital:
it was substituting for a detection probability the verification layer could not
deliver. **With Tier A, `q ≈ 1`** on the high-`γ` substitution cheats that dominate
the economics — serving cheaper precision or a smaller model, a 2–4× saving on every
request with plausible outputs. Every row clears with margin.

The residual attack closes it: spoofing top-`k` under tolerance plausibly requires
the honest result to aim at, so `γ ≤ 1` and the threshold goes non-positive. **An
attack costing more than honest work needs no deterrent.** That is an argument, not
a theorem, and it is a measurement gate (§7).

Note the monotonicity: the more inference pays over mining, the less detection is
needed. The mechanism gets safer as the market gets more valuable.

## 6. The consensus route: one lock primitive

Everything above composes from primitives the chain already has, except one thing.

The escrow output is spendable three ways:

1. **Happy path** — miner signature + client signature. Plain multisig, already
   expressible.
2. **Miner never delivered** — client alone after timeout. Plain timelock, already
   expressible.
3. **Client disputes but the miner was honest** — miner alone, by presenting a
   certificate that verifies against the committed statement digest.

Only path 3 needs anything new: **one lock primitive, `%aip`, satisfied by an
AI-PoW certificate verifying against a statement digest committed in the output.**

This is small because the work is already done. `ai_pow_verify_jet` exists, and
production nodes "build and validate the complete setup table at boot" for every
reachable trace height — the verifier and its setup are **already resident for block
acceptance**. The primitive exposes a check the node already performs.

Everything else stays off the consensus path: matching and the request mempool are
offchain, the manifest registry is a NockApp with a well-known root, the burn leg is
an existing `%brn` output, and identities are ordinary keypairs. The kernel learns
nothing about inference, prompts, models, or fingerprints. There is no dispute game
in consensus, no seizure logic, and no second puzzle.

**The one real risk of this route** is transaction-validation cost: a spend that
forces certificate verification is far more expensive to validate than a signature
check, so `%aip` spends need fee pricing that reflects it, and the bounded-verify
discipline that already protects block acceptance has to extend to the mempool.
That is a contained, well-understood problem, and it is the whole of the new
consensus surface.

## 7. What must be measured first

1. **Wall-clock cost of one compact certificate** at trace buckets 2¹³…2¹⁹.
   `zk_bridge` already instruments `l1_circuit_build_ms`, `l1_in_circuit_verify_ms`,
   and `l1_outer_cert_ms`. Nothing downstream is well-posed without it.
2. **`γ` for adversarial top-`k` spoofing.** The bondless construction closes on the
   claim that spoofing costs more than serving honestly. It is load-bearing now that
   no capital backstops a mistake. Attack it directly.
3. **TOPLOC thresholds for the Pearl quantization.** Published thresholds are for
   stock Llama-3.1-8B-Instruct; the INT7 group_1 / FP8 group_0 mix is a different
   quantization. Calibrate across the fleet's real hardware tail — a false positive
   on an honest miner is worse than a missed detection, since Tier B catches the
   latter and nothing repairs the former.
4. **Tier-A overhead on the serving path** — confirm the async tee is free rather
   than assuming it from bandwidth arithmetic.
5. **`%aip` verification cost** per spend, for mempool fee pricing.
6. **Off-chain defection rate**, once a market exists. The only honest way to bound
   `β`, and the only gate that cannot be run before launch.

## 8. Limits

**No prompt privacy.** Miners see prompts and activations; mid-stack activations are
substantially invertible. The proof layer establishes correctness, not
confidentiality — the client's input is an input to the prover, not a secret from
it. Keeping first and last layers client-side narrows but does not close this.

**Only one verification layer is sound.** Tier B's cryptographic guarantee covers
INT7 group_1 linears. Attention, normalization, sampling, and FP8 are covered by
Tier A and replay — real coverage, far broader than proof coverage, but statistical.
The service must state both halves.

**Tier A must not displace Tier B.** This is the likeliest way the design degrades,
and it will arrive as a cost optimization. TOPLOC's reported 100% detection is
empirical against the modifications its authors tested, not a proof against an
adversary who knows the scheme and is paid to defeat it. LSH is built for robustness
to *incidental* perturbation; *adversarial* robustness is strictly stronger and is
not established. So: a fingerprint mismatch **escalates, and is never itself a
verdict** — payment moves only on a certificate or a timeout, which also means an
honest miner on unusual hardware gets challenged and then exonerated. And thresholds
are versioned, consensus-visible parameters; a silently loosened tolerance weakens
everything with no outward sign.

**Forfeiture prices griefing rather than preventing it.** §5 shows cheating does not
pay; it does not constrain an attacker indifferent to profit. No bond would fix that
either. The protection is the per-claim admission ticket, and whether its difficulty
is high enough is a policy question this document does not settle.

**Statement separation is a security requirement.** An inference certificate must
never be replayable as a PoW certificate or vice versa. Zeroing the noise removes the
anti-reuse property the puzzle depends on — finding F01 of the dual-puzzle audit was
exactly that cached nonce-independent state let a miner grind without fresh
inference. Domain-separate at the statement level and test adversarially in both
directions.

## 9. Plan

Each stage is independently checkable. The measurement gate precedes the design
commitments that depend on it.

1. **Measure** certificate cost and adversarial `γ` (§7.1, §7.2).
2. **Model manifest** — per-tensor roots in `commit.rs` layout, provable/attested
   classification per layer, conformance KAT against the shipped model.
3. **Zero-noise inference statement** in `ai-pow`, domain-separated, with cross-replay
   rejection tests in both directions.
4. **Tier A** as an async tee in the vLLM plugin boundary, thresholds recalibrated
   for the Pearl quantization. Highest value per unit effort in the plan: nearly
   free, no consensus involvement, and it closes the substitution cheat that
   dominates the economics.
5. **Tier B** on a named tile via `from_tile`, verified offchain end to end.
6. **`%aip` lock primitive** with mempool fee pricing — the only consensus change,
   and the only stage owed a full protocol review.
7. **Settlement and scheduler** — escrow, auction, burn split, admission tickets,
   PoW/inference interleaving at the existing cancellation granularity.

Stages 2–5 carry no consensus risk and can proceed in parallel once stage 1 lands.
Stage 6 is the review gate.

## 10. Summary

The cryptography largely exists; the missing pieces are provenance, addressing,
verification policy, and settlement. Two verification layers with opposite blind
spots — a near-free fingerprint covering the whole forward pass statistically, and a
sound certificate covering one tile on dispute. Payment escrows and releases only
after verification, so collateral is work rather than capital and no bond is
required. Price by auction per MAC-equivalent, with a fixed burn fraction that makes
fake demand cost something and converts inference demand into security budget
against a hard cap.

The structural argument for consumer GPUs is that an 8B model fits on one card, so
the fleet needs no interconnect, and that KV-capped decode leaves ~92% of the INT8
tensor cores idle in exactly the units AI-PoW consumes. Verification is paid for out
of compute the serving workload cannot use.

The route into consensus is one lock primitive exposing a verifier every node
already runs at boot. Everything else — matching, manifest, disputes, identities —
stays offchain.

The honest limits are that only one layer is sound and it samples the integer linear
path, that end-to-end coverage rests on adversarial robustness nobody has tested with
money on the line, that miners see prompts, and that forfeiture deters profit-seeking
cheats without preventing griefing.

## References

[1] Jack Min Ong, Matthew Di Ferrante, Aaron Pazdera, Ryan Garner, Sami Jaghouar,
Manveer Basra, Max Ryabinin, Johannes Hagemann. *TOPLOC: A Locality Sensitive Hashing
Scheme for Trustless Verifiable Inference.* arXiv:2501.16007, January 2025 (rev. May
2025); ICML 2025. <https://arxiv.org/abs/2501.16007>
