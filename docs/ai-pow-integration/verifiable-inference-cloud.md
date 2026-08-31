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
| Post and match jobs | **Gossiped offer carrying prompt *length*, not the prompt** | Jobs as transactions carrying prompts — 150 s blocks vs ~24 ms prefill, permanent public prompts, duplicated work, result front-running (§5) |
| Deliver the prompt | **Direct, encrypted to the winning miner's serving key** | Gossiping it to the fleet — every miner would see every prompt |
| Reach ordinary users | **Gateways with an OpenAI-compatible API, passing signed receipts through** | Assuming users run clients and hold `$NOCK`; or gateways that absorb verification and ask to be trusted (§5) |
| Pay per request | **Standing escrow, settled in batches** | One settlement transaction per request — impossible at a 150 s cadence |
| Accrue value, price fake demand | **Fixed burn fraction `β`** | Moving `β` — damps the supply response exactly when it should attract capacity |
| Weight authenticity | **Onchain model manifest** | Trusting `HASH_B` alone — it binds *some* weights, not the certified model's |
| Consensus integration | **None — existing primitives only (§6)** | A new lock primitive up front — it is a tx-engine change putting a STARK verifier in transaction validation; deferred to §6 |

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

## 5. The offchain system: settlement, transport, gateways

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

### What stays offchain, and why

A natural reading of "settle inference onchain" is that jobs are *posted* as
transactions — an output carrying the prompt under a lock that anyone can spend by
performing the inference. It is a clean idea and it does not survive contact with
this chain's parameters. Four reasons, any one of which is disqualifying:

**Latency.** Post-Aletheia block time is **150 s**. Prefill of a 1,000-token prompt
is **~24 ms**. Posting a job as a transaction means waiting for inclusion before a
miner can even see it, and waiting again to settle: roughly 300 s of round trip
around 24 ms of work, ~4 orders of magnitude of overhead. Interactive serving is
impossible at that cadence no matter how the lock is written.

**Permanence and bloat.** A prompt in a transaction is public forever, replicated to
every node. That is a much stronger disclosure than §8's "miners see prompts" — it is
*everyone, permanently*. It also puts kilobytes of application payload into a UTXO
chain's permanent state, which size-based fees will price but never un-store.

**Racing wastes the exact resource this design exists to conserve.** If any miner can
claim by doing the work, `N` miners race and `N−1` results are discarded. Mining
races are fine because the work *is* the lottery; here the work has a client who
wanted it done once. A job that is *assigned* is computed once; a job that is
*claimable* is computed as many times as there are bidders.

**Front-running.** If the result rides in the spending transaction, a second miner
reads it from the mempool and re-submits with a higher fee, collecting the payment
without doing the work. Fixing that needs commit–reveal, which costs another block
round trip and makes the latency worse.

There is a fifth, structural: "spendable by completing the inference properly" means
the *lock* must verify the work — which is exactly the `%aip` transaction-engine
change §6 argues out of a first deployment. The posting model does not avoid that
cost, it requires it.

**The split that does work.** The chain holds the *payment commitment*; everything
with a latency or privacy requirement stays off it:

| Onchain | Offchain |
|---|---|
| Escrowed payment, `%pkh` + `%tim` | The prompt |
| Request **digest** only | Matching and claiming |
| Settlement: `(1−β)` + `%brn` | The result |
| A certificate, only on dispute | Certificate verification, normally |

The chain never sees a prompt, never sees a result, and never sits in the serving
path. It sees an output before, and an output after.

### How prompts reach miners

The prompt never touches the chain (above) and never gossips to the fleet. It goes
to exactly one miner, over a direct authenticated channel, after that miner has been
selected.

**A separate network, on the crate patterns that already exist.**
`nockchain-libp2p-io` already provides QUIC/TLS transport, Kademlia discovery,
request/response, gossip, bounded untrusted input, and peer/IP abuse controls. The
inference network reuses those patterns under **its own protocol IDs**, not the
consensus mesh — inference traffic must never be able to degrade block propagation,
and the two have opposite tuning: consensus wants global flood, inference wants
point-to-point.

**Two phases, and the first one carries no prompt.**

1. **Offer (gossiped, ~hundreds of bytes).** Manifest digest, **prompt length**,
   max tokens, sampling params, seed, bid per MAC-equivalent, deadline, escrow
   reference, client pubkey.
2. **Claim (direct).** Miners with capacity respond, signed by a serving identity and
   carrying an admission ticket.
3. **Prompt (direct, encrypted to the winner).** The client selects a miner and sends
   the prompt to that miner only, encrypted to its serving-identity key — the keypair
   §5 already requires doubles as the transport key.
4. **Response (direct).** Tokens stream back, followed by the signed fingerprint.

The offer works because of a property already established: **a miner can price a job
without seeing it.** MAC count is deterministic from `(manifest, prompt length, max
tokens)`, so `prompt length` — not the prompt — is all a miner needs to bid. The
economics and the privacy boundary happen to want exactly the same field.

**Discovery is per session, not per request.** A gossip round trip before every
request would put network latency in front of ~24 ms of prefill. It does not have to:
after the first match the client keeps talking to the same miner directly, and
re-enters discovery only on failure, deadline miss, or deliberate rotation. Combined
with batched settlement, **both the chain and the gossip mesh are amortized across a
session, and only serving is per request.**

**What this leaks, stated plainly.** The assigned miner sees the prompt in full —
unavoidable here (§8). Beyond that, the *offer* is public: prompt length, token
counts, timing, and client identity go to every listening miner. Length and timing
are a real side channel over a session, and padding token counts to buckets is the
obvious mitigation at some cost in price granularity. The direct connection also
reveals client network identity to the serving miner unless separately routed.

**A griefing vector this transport creates.** A miner can claim a job, receive the
prompt, and never serve — harvesting prompts for the price of one admission ticket.
The client loses no money, since escrow never releases, but it has already disclosed
the prompt. Ticket difficulty is therefore doing double duty: it prices claim-abandon
*and* prompt harvesting, and the second is the harder one to price, because a
harvested prompt may be worth far more than the compute the ticket represents.
Deprioritizing identities that claim without delivering bounds repetition, not the
first offence. This is a limit of the design, not a solved problem.

**Settle in batches, not per request.** Even with the prompt offchain, one settlement
transaction per request cannot work at a 150 s cadence for a service billing in
milliseconds. A standing escrow covering many requests, settled periodically against
an accumulated balance, keeps per-request latency purely offchain and amortizes the
onchain cost across a session. This needs no new primitives — a batch settlement is
one ordinary transaction closing out `N` verified responses.

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

### Gateways, and the trust they re-centralize

Most demand will not arrive from users running a libp2p client, holding `$NOCK`,
managing standing escrows, and verifying fingerprints. It will arrive at a website
with an OpenAI-compatible HTTP API, and that operator will talk to miners. The design
should expect this rather than treat it as a degradation — a gateway is exactly where
the machinery consumer users cannot run was always going to live: fiat handling,
demand aggregation, standing escrows that solve the 150 s settlement cadence, and
persistent sessions that make discovery amortize.

But it must be said plainly what a gateway does to the guarantee.

**Verification protects the gateway from miners. It does not protect the user from
the gateway.** A gateway that serves a cheaper model itself, pockets the difference,
and claims it used the network is doing precisely what TOPLOC's own motivation
identifies as the problem — a provider that may not be using the model configuration
it claims. Every mechanism in this document sits *below* the gateway and none of it
points at the gateway.

**The fix is that the gateway passes the evidence through rather than absorbing it.**
Every API response carries a receipt:

- the miner's **signed TOPLOC fingerprint** of that response,
- the **model manifest digest**,
- the **serving identity** that produced it, and
- the **settlement reference**.

The gateway cannot forge this. The fingerprint is signed by a work-backed serving
identity and is bound to *these* output tokens, so an old receipt cannot be replayed
against a new response. In an OpenAI-compatible shape it is an added response field
or header — inert for clients that ignore it, available to clients that do not.

That converts "trust the operator" into "the operator hands you evidence it did not
author," which is a materially different claim from what an ordinary inference API
can make.

**Being precise about who can check it.** TOPLOC verification requires a prefill of
the claimed output, so it requires the weights and a GPU. **A typical end user cannot
verify their own receipt.** The mechanism is not self-verification; it is that a
receipt is checkable *by anyone* — a competing gateway, a watchtower service, a
researcher, the user's own infrastructure if they have it — so a gateway that
fabricates receipts is discoverable by whoever does check, and its identity is
public. Fraud has to be detectable, not detected by everyone. That is the same shape
as the miner-level argument in §5, one layer up.

**What a gateway still absorbs, and cannot be designed away:**

- **The prompt.** The user sends it to the gateway in the clear, and the gateway
  forwards it to a miner. Two parties now see it instead of one; §8's privacy limit
  gets worse, not better, under the deployment model users will actually use.
- **Selection.** The gateway chooses miners and could route to its own. The burn
  still prices that — routing to itself pays `β` like any other trade — but selection
  opacity remains its own.
- **Availability.** A gateway can refuse a user. The mitigation is that gateways are
  commodity and the protocol beneath them is open, which is real only if running one
  stays cheap.

**The property to protect is that gateways stay thin and replaceable.** A user of an
ordinary inference API cannot audit their provider at all, so they cannot compare
providers on honesty. A user holding receipts can — and can switch. Anything that
makes gateways sticky in ways receipts cannot audit erodes the only structural
advantage this design has over a conventional provider.

Sophisticated clients keep the direct path: run a client, hold an escrow, verify
locally, skip the gateway entirely. **The guarantee is available to everyone and the
gateway is a convenience** — which is the right ordering, and the opposite of the one
that emerges if receipts are omitted.

## 6. The consensus route: none at first, `%aip` later

An earlier version of this section proposed a new lock primitive as *the* route and
called it small because the verifier is already resident. That undersold it. The
count is right and the risk assessment was not.

**A lock primitive is a transaction-engine change.** `lock-primitive` is a `$%` in
[`hoon/common/tx-engine-1.hoon`](../../hoon/common/tx-engine-1.hoon) (`%pkh`, `%tim`,
`%hax`, `%brn`), mirrored in
[`crates/nockchain-types/src/tx_engine/v1/tx.rs`](../../crates/nockchain-types/src/tx_engine/v1/tx.rs).
A fifth variant touches the mold, `based`, `hashable`, `hash`, and `check` in **two**
call sites (`check-lock` and `check-multisig-lock`), plus Rust serialize, deserialize,
and leaf-hash — with exact Hoon/Rust agreement required, and respecting the ordering
constraint the `%brn` comment flags ("it's important that this be the default to break
a type loop in the compiler"). It also grows the **witness**, since `check` reads its
argument from `form` and a certificate is not a signature.

That is the most consensus-critical component in the repository. The AI-PoW puzzle is
an opt-in second puzzle; the transaction engine validates every transaction on the
chain.

**And the cost profile is a genuine regression, not just a fee question.**
`ai-pow-verify` is the *block puzzle* verifier: one certificate per block, at block
admission. Putting a STARK verification inside `check-lock`'s `levy` over spend
conditions runs it per primitive, per spend, per transaction — a different throughput
regime entirely. Setup-table residency is necessary and nowhere near sufficient. This
is the same class of hazard `SPOT_CHECKS_MAX` exists to bound, and the repository's own
comment there names it: a crafted block driving CPU-time DoS.

### What to do instead, for a first deployment

**Nothing.** The escrow needs no new primitive if the certificate is verified
*offchain*:

- 2-of-2 between client and miner, with a timeout that refunds the client. Plain
  multisig and timelock, both already expressible.
- On dispute the miner publishes its certificate. Anyone — the client, the scheduler,
  a competing miner — verifies it offchain with the machinery that already exists.
- A client that ignores a verifying certificate is publicly identifiable, and the
  miner declines to serve it again.

The residual weakness is bounded and cheap: a griefing client can take **one payment
per miner** before that miner stops serving it, and it burns its own reputation doing
so in public. For the request sizes this market starts at, that is a smaller cost than
a hard fork.

Where a client's downside genuinely warrants adjudication, a 2-of-3 with a
market-chosen adjudicator that verifies certificates offchain is also expressible
today. It reintroduces a trusted third party — which is exactly the trade, and it
should be the client's to make, not the protocol's.

### When `%aip` earns its hard fork

`%aip` removes the griefing window by making the certificate directly spendable. That
is a real improvement and it is an **optimization of a working market, not a
prerequisite for one.** Defer it until there is volume that justifies the fork, and
until the cost question above has an answer — a per-transaction verification bound, a
fee schedule reflecting it, and mempool admission policy that cannot be used to make
every node re-verify STARKs on demand.

If it is built, the versioned engine is the precedent to follow: `tx-engine-0` and
`tx-engine-1` both exist and `tx-engine.hoon` imports both, so a `tx-engine-2` is the
idiomatic path rather than mutating a deployed engine in place.

**Net for a first deployment: the consensus surface is zero.** Matching, manifest,
disputes, identities, and adjudication are all offchain; settlement uses `%pkh`,
`%tim`, and `%brn` exactly as they exist today. The kernel learns nothing about
inference, and nothing about this design requires a fork to start.

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
4. **Discovery round-trip latency** on the inference mesh, against the per-session
   amortization assumption — the claim that gossip cost disappears across a session
   holds only if session reuse is high in practice.
5. **Tier-A overhead on the serving path** — confirm the async tee is free rather
   than assuming it from bandwidth arithmetic.
6. **Per-transaction certificate verification cost and a mempool admission bound** —
   required *before* `%aip` is considered, not after. Until this has an answer, the
   design ships on existing primitives.
7. **Off-chain defection rate**, once a market exists. The only honest way to bound
   `β`, and the only gate that cannot be run before launch.

## 8. Limits

**No prompt privacy, and it is worse under the deployment model users will actually
use.** Miners see prompts and activations; mid-stack activations are substantially
invertible. The proof layer establishes correctness, not confidentiality — the
client's input is an input to the prover, not a secret from it. Through a gateway
(§5) both the operator and the miner see it.

**A typical user cannot verify their own receipt.** TOPLOC verification needs the
weights and a GPU. The guarantee is that receipts are checkable by anyone and forgery
is publicly discoverable, not that each user checks their own. Keeping first and last layers client-side narrows but does not close this.

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

**A miner can harvest prompts for the price of a ticket.** Claiming a job and never
serving costs one admission ticket and yields the prompt; escrow protects the client's
money but not its disclosure. Ticket difficulty prices both claim-abandon and prompt
harvesting, and the latter is harder to price because a prompt may be worth far more
than the compute a ticket represents (§5).

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
6. **Settlement and scheduler** — escrow on `%pkh`/`%tim`/`%brn` as they exist,
   auction, burn split, admission tickets, PoW/inference interleaving at the existing
   cancellation granularity.

**No stage in this plan changes consensus.** `%aip` (§6) is deliberately excluded: it
optimizes a working market rather than enabling one, and it is owed its own protocol
review, its own cost bound, and plausibly its own `tx-engine-2` — none of which should
gate shipping the rest.

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

**The route into consensus is that there is not one.** Settlement composes from
`%pkh`, `%tim`, and `%brn` as they exist; certificates are verified offchain, where
the machinery already runs. A `%aip` lock primitive would remove the residual
griefing window, but it is a transaction-engine change that puts a STARK verifier in
the transaction-validation loop — an optimization to earn later, not a prerequisite.

The honest limits are that only one layer is sound and it samples the integer linear
path, that end-to-end coverage rests on adversarial robustness nobody has tested with
money on the line, that miners see prompts, and that forfeiture deters profit-seeking
cheats without preventing griefing.

## References

[1] Jack Min Ong, Matthew Di Ferrante, Aaron Pazdera, Ryan Garner, Sami Jaghouar,
Manveer Basra, Max Ryabinin, Johannes Hagemann. *TOPLOC: A Locality Sensitive Hashing
Scheme for Trustless Verifiable Inference.* arXiv:2501.16007, January 2025 (rev. May
2025); ICML 2025. <https://arxiv.org/abs/2501.16007>
