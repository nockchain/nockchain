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
depend on one quantity nobody in this repository has measured yet (§9). It does
assume the burn primitive (`%brn`) and the fixed-percentage-split precedent that
Aletheia already established, rather than proposing new consensus machinery for
either.

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
| **Coverage.** PoW proves one tile (`h·w ≤ 256`) out of a 4096×14336 = 58.7M-element output — a ~4×10⁻⁶ sample | Fundamental | Full-forward-pass ZK for 8B is out of reach; consensus caps Layer-0 trace at 2¹⁹ (`AI_POW_MAX_TRACE_HEIGHT`). Mitigated in breadth by Tier 0 (§5) |
| **FP nondeterminism.** Norms, attention, dequant, and FP8 layers are not bit-reproducible across a heterogeneous fleet | Blocking | Bit-exact replay fails on honest miners; needs a tolerance-bearing commitment |
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

## 5. The mechanism: fingerprint, commit, sample, dispute

Four tiers. Each is cheap where it is common and expensive only where it is rare.

The tiers exist because **no single check has both breadth and soundness.** A
locality-sensitive activation fingerprint covers the whole forward pass but is
statistical; a STARK is cryptographically sound but covers one tile of one GEMM.
Layering them is not redundancy — each covers precisely the other's blind spot.

### Tier 0 — TOPLOC fingerprint (every response, ~free)

Every response carries a TOPLOC commitment: a locality-sensitive hash of the
top-`k` values of the **last hidden state**, polynomial-encoded [1]. Because the
final layer depends on every prior computation, a fingerprint of it is sensitive
to the entire stack — attention, normalization, activation functions, dequant
scales, and the FP8 layers the STARK cannot reach.

The reported properties are an unusually good fit here:

- **258 bytes per 32 new tokens** on Llama-3.1-8B-Instruct — a ~1000× reduction
  versus recording activations. Against the ~512 KiB/token of raw per-layer
  activations considered below, this is not an optimization, it is a different
  order of thing.
- **Detects modified model, prompt, or precision** at 100% accuracy with no false
  positives or negatives in the paper's evaluation.
- **Robust across GPU types, attention implementations, tensor-parallel layouts,
  and algebraic reorderings** — thresholds are set on mantissa deviation where
  exponents agree, chosen empirically to survive reordering.
- **Verification up to 100× faster than generation**, because a validator passes
  all committed tokens through a *single prefill* instead of decoding them
  autoregressively.

The evaluation model is Llama-3.1-8B-Instruct — the same family as the AI-PoW
production model — and the cost profile lands on the right side of §3's roofline:
a top-`k` reduction over a 4,096-wide hidden state is negligible compute, and its
bandwidth cost (~8 KiB/token read) is nothing against the ~17 GB moved per decode
step. Taken as an asynchronous tee off the serving path, it should cost neither
TTFT nor throughput. That claim still belongs in the measurement gate (§9), not
in the assumptions.

### Tier 1 — Commitment (every request, ~free)

The miner signs, and publishes, a commitment binding:

- the model manifest digest (§6),
- the request digest — prompt, sampling parameters, explicit seed,
- the Tier-0 fingerprint, extended to a **per-layer fingerprint chain** so
  disputes can bisect (32 layers × 258 B/32 tokens ≈ 8 KiB per 32 tokens — still
  negligible),
- the BLAKE3 roots of the **integer GEMM operands** for the provable layers, and
- the miner's staked identity.

The purpose is to **pin the computation before any challenge is issued.** After
Tier 1 the miner has no remaining freedom to choose what it "computed."

> **Correction to an earlier draft of this design.** A previous version committed
> to a bit-exact Merkle root over the per-layer activation stream and asserted
> that "execution is deterministic given the manifest and the seed." That is false
> for a heterogeneous consumer fleet, and the error is not cosmetic: different GPU
> architectures, attention kernels, and — critically — *different batch
> compositions* produce different floating-point reduction orders. Since §3 shows
> batch composition varies continuously under a KV-capped scheduler, two honest
> miners would routinely produce different roots, and the dispute tier would fire
> on honest disagreement.
>
> The precise statement is narrower and survives: **the INT7/INT8 GEMM accumulate
> is integer and therefore exactly reproducible** — this is why the roofline can
> rely on `mma.sync.satfinite` matching wrapping scalar accumulation, and why `Q`
> can be bit-lossless. Bit-exact commitment is correct *for the integer operands*.
> Everything float-contaminated — norms, softmax, attention, dequant scales, the
> FP8 `down_proj` — needs a tolerance-bearing commitment, which is exactly what
> Tier 0 supplies.

### Tier 2 — Sampled spot proofs (rare, expensive)

A public beacon the miner could not predict — the next block hash — selects a
`(layer ℓ, tile i, j)`. The miner must produce the compact recursive certificate
for exactly that tile:

- `B` (weights) opened against the model manifest,
- `A` (activations) opened against the Tier-1 integer-operand root for layer ℓ,
- the schedule from `StripIndexSchedule::from_tile`,
- the claimed output tile.

This is the current `ai-pow-zk` statement with the jackpot comparison removed and
the noise zeroed. It proves: *the certified weights, times the activations this
miner already committed to, produce the tile it claims.*

Commit-then-challenge is what makes a 4×10⁻⁶ tile sample meaningful. The sample
is tiny, but the miner must be correct everywhere to survive a uniformly random
draw it cannot anticipate.

### Tier 3 — Dispute by bisection (very rare)

Any second miner, client, or watchtower re-runs the request as a **single prefill
of the claimed output** and compares Tier-0 fingerprints under the published
tolerance. At up to 100× cheaper than generation, this is affordable enough that
a client can verify *its own* traffic in full rather than relying on sampling.

On mismatch, the parties bisect the per-layer fingerprint chain to the first
divergent layer in ~5 rounds, then to tiles within it. The terminal step is
exactly a Tier-2 proof: one proof system serves both, and the dispute game adds
no new cryptography.

**A fingerprint mismatch escalates; it never slashes directly.** Tier 0 is a
trigger, not a verdict — see §10.5. Slashing follows only from a failed or
contradicted Tier-2 certificate, or from a timeout.

### Why both, and not either alone

| | Tier 0 — TOPLOC | Tier 2 — STARK |
|---|---|---|
| Cost | ~free, every response | expensive, sampled |
| Coverage | whole forward pass, incl. attention, norms, FP8 | one tile of one INT7 GEMM |
| Robust to FP nondeterminism | yes, by construction | n/a — integer path |
| Guarantee | statistical | cryptographic |
| Detection latency for precision/model swap | **1 request** | ~`1/p` requests |

**TOPLOC gives breadth without soundness; the STARK gives soundness without
breadth.** Dropping either is the failure mode to guard against — and the
temptation runs toward dropping the expensive one, which is why §10.5 states the
argument against it explicitly.

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
  tokens, sampling params, seed, bid per MAC-equivalent, deadline) to an offchain
  mempool; miners claim them. Payment escrows in `$NOCK` through the existing UTXO
  and wallet crates and settles on Tier-1 commitment plus a quiet dispute window,
  splitting `(1−β)` to the miner and `β` to a `%brn` output (§8).
- **Interleaving:** the miner serves inference when paid demand exists and falls
  back to PoW otherwise — and per §3 it can do *both at once* during decode,
  because decode leaves the tensor cores mostly free.
- **Preemption granularity:** the CUDA miner already runs 3–50 ms batched launches
  with cancellation on candidate replacement (`ai-pow-miner`'s stale-work path).
  That is finer than a typical 20–50 ms per-token budget, so mining can yield to
  inference without violating interactive latency.

## 8. Settlement: burn-denominated inference

### 8.1 The reserve price already exists

Nockchain already runs a spot market for exactly this hardware doing exactly
these GEMMs. Block-reward EV per GPU-second is measurable from sustained TMAC/s
and the current AI target — and `DIFFICULTY.md` is explicit that the target `T`
prices *one MAC-equivalent of matmul work*, not one attempt, which is precisely
the denomination needed.

That gives a floor: inference must outbid mining EV per GPU-second, or the miner
keeps mining. No token subsidy is required to price the fleet, and supply is
elastic in both directions.

### 8.2 Price per MAC-equivalent, quantity fixed by the manifest

Denominate inference in the same unit the puzzle already uses: **`$NOCK` per
MAC-equivalent**. Two properties follow, and both remove trust rather than add it.

A miner's allocation decision becomes a scalar comparison in one unit — mining EV
per MAC-equivalent against the inference bid per MAC-equivalent — with no
conversion and no oracle.

And because the model manifest (§6) fixes every layer's shape, the MAC count of a
request is **deterministic from `(manifest, prompt length, max tokens)` and
computable by both parties before execution.** There is no metering to trust, no
usage counter to falsify, and no billing dispute surface: the quantity is
objective and known in advance, so only the price is negotiated.

### 8.3 Burn is the right rail, and not primarily for deflation

Payment should be a burn split: the client's payment settles as `(1−β)` to the
serving miner and `β` to a provably unspendable output.

The mechanism needs no new consensus primitive. `%brn` is already a first-class
lock primitive in the v1 tx engine (`LockPrimitive::Burn` in
`crates/nockchain-types/src/tx_engine/v1/tx.rs`, `%brn` in
`hoon/common/tx-engine-1.hoon`), and Aletheia already established the precedent
for splitting value by fixed percentage to a well-known destination — its 80/20
miner/protocol-fund split of new issuance.

The usual argument for a burn is deflation. That is real here given the 2³² hard
cap, but it is the weaker reason. Two stronger ones:

**Burn is the only self-dealing-proof payment rail.** The Tier-1/2/3 design in §5
secures *correctness* — that a miner computed what it claimed — through stake and
slashing. It does nothing whatsoever about **fake demand**: a miner paying itself
to manufacture the appearance of volume. That matters wherever serving volume
becomes visible and valuable — advertising utilization to attract real clients,
reputation weighting in the scheduler, or any future rule that lets volume
influence anything consensus-visible. Under a 100%-to-miner fee, a self-deal
round-trips at *zero cost* and wash volume is free. Under a burn, every washed
`$NOCK` costs exactly `β`.

**This is the correct criterion for setting `β`**: not a revenue-split intuition
about what feels fair to miners, but the price at which faking demand stops being
profitable relative to the largest plausible benefit of faking it.

**Burn is the only way to raise miner revenue against a hard cap.** Aletheia
deliberately sustains a 64-NOCK floor for ~68 years to extend the chain's revenue
tail, because the original schedule "front-loads ~99% of emissions into the
chain's first thirty years… forcing the network onto fee revenue earlier than the
application ecosystem can sustain." That floor is denominated in NOCK, so its
*real* value is exactly what a burn improves — and improving it by issuance is
impossible against a fixed cap. The loop closes:

```text
inference demand -> burn -> scarcer $NOCK -> higher real value of the
64-NOCK floor -> more hashrate -> more serving capacity -> inference demand
```

A burn converts inference demand into security budget for the whole network
rather than private revenue for whichever miner happened to serve. Given that
Aletheia's stated motivation is precisely the long-run security-budget
transition, the fit is unusually good.

### 8.4 Modulate the market, not the price

"Adjust the rate with supply and demand" is right as an objective and dangerous
as an implementation, and the distinction is where this design most needs care.

**A protocol-set price requires observing supply, and supply is not observable.**
Settled demand is onchain and honest. Total idle GPU capacity across the fleet is
not: it is self-reported. An EIP-1559-style controller works because block space
is a hard cap the protocol *knows*; here the protocol has no idea what the fleet
can do. A controller keyed on self-reported capacity is a cartel lever —
under-report capacity, utilization appears high, the protocol raises the price —
and it pays out precisely to coordinated under-reporting. The fewer large
operators, the cheaper that attack.

**So let the auction set the price and keep the protocol to the split.** Clients
bid, miners accept, and the protocol burns `β` of whatever cleared. Price then
modulates with supply and demand *automatically*, through a real two-sided market
rather than an oracle reading, and there is nothing to manipulate: a miner that
misrepresents its capacity moves no protocol variable, it just fails to win work.
The burn *throughput* still rises and falls with real demand, which is the
deflationary behavior the mechanism is meant to produce.

**If `β` itself moves, key it to the security budget, not the demand cycle.**
There is a defensible moving `β`, but the intuitive version is backwards. Burning
a larger share when demand is high damps the miner-revenue signal exactly when
that signal should be attracting capacity; the supply response is the one thing
that must not be damped. The version that earns its complexity instead lets `β`
*fall* as the block subsidy decays, shifting revenue toward miners as direct
security funding is most needed — a slow monetary dial on a multi-month EMA,
bounded within `[β_min, β_max]`, with hysteresis, and keyed on realized burn
against subsidy rather than on anything a participant self-reports.

### 8.5 What actually bounds `β`

Not fairness, and not revenue maximization: **the off-chain escape hatch.**

A miner and client can always settle privately and never touch the chain. The
burn is a tax on using the onchain rail, so `β` must stay below what that rail is
worth to the marginal client — escrow, the unpredictable beacon that makes Tier-2
sampling unforgeable, slashing, and dispute adjudication. Sophisticated repeat
counterparties who already trust each other will defect off-chain at *any* `β`;
the rail's real market is strangers and one-shot interactions, which is also
exactly where verifiability is worth paying for.

That caps `β` well below a naive revenue-maximizing choice — my instinct is a
10–30% band rather than 50%+ — and it should be set against *observed* off-chain
defection once there is a market to observe, not guessed in advance.

One consequence: keep the split two-way. Adding the Aletheia protocol fund as a
third claimant on inference revenue raises the tax on the rail without a clear
need, and the fund is already financed from issuance. Cheap rail, narrow split.

### 8.6 Slashing, refunds, and paying the challenger

The burn interacts with §5's dispute tier, and getting this wrong makes Tier 3
theater.

- **The client's payment is refunded, never burned, on a failed proof.** If
  clients bore the cost of miner misbehavior the service would be unusable.
- **Slashed stake splits between challenger bounty and burn.** Burning 100% leaves
  challengers unfunded — nobody spends a forward pass to catch a cheat for free,
  and the whole tier collapses. Paying 100% to the challenger funds griefing
  between competing miners. Size the bounty to somewhat exceed replay cost and
  burn the remainder.

### 8.7 Sampling rate and stake

With proof cost `C_p` and request cost `C_r`, sample rate `p` gives verification
overhead `p · C_p / C_r`. Holding it to a budget `β_v` gives `p ≤ β_v · C_r / C_p`.

A miner skipping work gains at most `value(C_r)` and is detected with probability
`p`, so honesty requires `S > value(C_r) / p`. Substituting:
**`S > value(C_p) / β_v`**. At a 5% verification budget, the stake floor is ~20×
the value of a *single proof* — not of a request, and independent of request size.
That mildness is why this can work with consumer-scale operators rather than only
well-capitalized ones.

**Tier 0 changes what `p` has to buy, and this is its largest economic effect.**

The most profitable cheat in a consumer GPU cloud is not fabricating outputs —
those have to look plausible, which is hard. It is **serving a cheaper precision
or a smaller model than was paid for**: a 2–4× cost saving on *every* request,
indefinitely, with outputs that read as fine. Under ZK sampling alone that is
caught at rate `p`, so the cheat earns roughly `1/p` requests of free margin
before detection. TOPLOC detects precision and model substitution on **every**
request, collapsing detection latency from `~1/p` requests to one.

So `p` no longer has to deter the cheap, high-volume cheats. It only has to deter
the residual adversary who constructs activations that spoof top-`k` under the
published tolerance while actually running a cheaper computation — a far narrower
and far more expensive attack. **The same security is bought at a materially lower
`p`**, which lowers verification overhead and, through `S > value(C_p) / β_v`, the
stake floor with it.

That feeds back into §8.5: a cheaper verification layer makes the onchain rail
cheaper overall, which widens the headroom between `β` and the off-chain escape
hatch.

Two second-order effects, both favorable:

- **Challenger economics.** §8.6 needs a bounty large enough to pay someone to
  catch a cheat. At up to 100× cheaper than generation, a challenge costs ~1% of
  serving, so the bounty can be small — which also shrinks the griefing incentive
  that an oversized bounty would create.
- **Client-side verification becomes the default posture.** At that cost a client
  can verify *all* of its own traffic rather than trusting a sampled deterrent.
  The trust model shifts from "sampled deterrence" to "client-verified by default,
  with the STARK as the escalation path when a client and miner disagree."

Systematic cheating is bounded far more tightly than one-shot cheating: replay is
cheap and available to anyone, so a repeat cheat faces detection probability
approaching 1.

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
5. **TOPLOC threshold calibration for the Pearl quantization.** The published
   thresholds were established for Llama-3.1-8B-Instruct; the Pearl variant's
   INT7 group_1 / FP8 group_0 mix is a *different* quantization, so the mantissa
   thresholds must be re-derived against it rather than inherited. Calibrate
   across the fleet's real hardware distribution, including its long tail — a
   false positive on an honest miner with unusual hardware is far more damaging
   than a missed detection, because Tier 2 catches the latter and nothing repairs
   the former.
6. **Tier-0 overhead on the serving path**: confirm the async tee costs neither
   TTFT nor throughput, rather than assuming it from the bandwidth arithmetic.
7. **Off-chain defection rate** once a market exists. This is the only honest way
   to bound `β` (§8.5), and unlike the others it cannot be measured before
   launch — which is the argument for shipping a deliberately low constant `β`
   and raising it against evidence, rather than starting high.

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

**Verification is layered, and only one layer is sound.** Tier 2's *cryptographic*
guarantee covers INT7 group_1 linears and nothing else. Attention, normalization,
activation functions, sampling, and the FP8 layers are covered by Tier 0's
fingerprint and Tier 3's replay — real coverage, and far broader than proof
coverage, but **statistical rather than sound.** The service must state both
halves: soundly proven on a sampled tile of the integer linear path, empirically
fingerprinted end to end. No amount of layering turns the second into the first at
this model scale.

**Tier 0 must not be allowed to displace Tier 2**, and this is the most likely way
the design degrades in practice — it will arrive looking like a cost optimization.

TOPLOC reports 100% detection with no false positives, but that is an *empirical*
result against the modifications its authors tested, not a proof against an
adversary who knows the scheme and optimizes against it. In a research evaluation
that distinction is academic. Here, real money would reward finding a cheaper
computation whose top-`k` last-hidden-state values survive the published
tolerance, and nobody has yet had that incentive. Locality-sensitive hashing is
built for robustness to *incidental* perturbation; robustness to *adversarial*
perturbation is a strictly stronger property and is not established.

Three consequences:

- **Tier 0 escalates; it never slashes.** A fingerprint mismatch opens a Tier-2
  challenge. Money moves on a failed or contradicted certificate, or on a timeout,
  never on a fingerprint alone. This also contains the false-positive risk from
  §9.5 — an honest miner on unusual hardware gets challenged, not slashed, and the
  certificate exonerates it.
- **Keep `p` strictly positive** however good Tier 0 looks in production. Its whole
  job is deterring the adversary Tier 0 cannot bound, and that adversary's absence
  from the logs is not evidence of their impossibility: a cheat designed to pass
  Tier 0 is, by construction, invisible to Tier 0.
- **Thresholds are consensus-visible parameters**, versioned and changed
  deliberately. A silently loosened tolerance weakens the entire scheme with no
  outward sign.

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
6. **Burn-split settlement** over the existing `%brn` primitive: per-MAC bid
   escrow, `(1−β)/β` split on success, refund on failed proof. No price
   controller; `β` fixed for the first deployment.
8. **Tier-3 bisection game** as a NockApp, with slashing and the challenger
   bounty; consensus untouched.
9. **Scheduler and interleaving**, including PoW/inference preemption at the
   existing cancellation granularity.

Stages 2–7 are additive and carry no consensus risk. Stage 8 is where the real
design review is owed, and it should be reviewed against the existing dual-puzzle
audit findings rather than in isolation. A moving `β` (§8.4) is explicitly *not*
in this plan: ship a constant, observe the market, and only then decide whether a
dial is worth its attack surface.

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

Settlement should be a burn split, denominated per MAC-equivalent so the manifest
makes every request's quantity objective in advance. The burn's primary job is not
deflation but making fake demand cost something — it is the only payment rail on
which a miner cannot wash its own volume for free — and, against a hard supply cap,
the only way inference demand can raise the real value of the 64-NOCK floor that
funds long-run security. Price should be set by auction rather than by a protocol
controller: supply is self-reported and therefore a cartel lever, so the protocol
should fix the split and let the market fix the price.

The honest limits are that only one verification layer is cryptographically sound
and it samples the INT7 linear path, that end-to-end coverage is statistical and
rests on adversarial robustness nobody has yet stress-tested with money on the
line, that miners see prompts, that `β` is bounded by
an off-chain escape hatch nobody can measure before launch, and that the dispute
game is the one genuinely risky addition — which is why it belongs in a NockApp
and not in the consensus kernel.

## References

[1] Jack Min Ong, Matthew Di Ferrante, Aaron Pazdera, Ryan Garner, Sami Jaghouar,
Manveer Basra, Max Ryabinin, Johannes Hagemann. *TOPLOC: A Locality Sensitive
Hashing Scheme for Trustless Verifiable Inference.* arXiv:2501.16007, January 2025
(rev. May 2025); ICML 2025. <https://arxiv.org/abs/2501.16007>
