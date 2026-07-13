# zk-pow Production Puzzle vs. Pearl — Discrepancy Audit

**Date:** 2026-07-13
**Scope:** Comprehensive comparison of Nockchain's AI-PoW production puzzle
(`crates/ai-pow`, `crates/ai-pow-zk`, `crates/ai-pow-miner`) against the vendored
Pearl reference (`pearl/zk-pow`, `pearl/node`, `pearl/miner`), looking for
divergences in **(a) the units of work the two puzzles support** and **(b) the
commitments each makes**.
**Method:** Five independent file:line-grounded readings (Pearl config space,
Pearl commitments, Pearl node/miner consensus, our supported units + admission,
our commitments + node precheck), each verified against a first-hand re-read of
the load-bearing code. Every claim below is traceable to a cited line.

> **Update 2026-07-13 (post-audit actions):** **D3 is FIXED** (the `h·w ≤ 256`
> cap is now enforced on the merge path, with a consensus-boundary trace
> backstop and tests). **D5 is CLOSED** as an intentional design divergence.
> **D2 is EVALUATED** with a recommendation to document the narrowed envelope
> and defer the circuit work. See the annotated D2/D3/D5 entries and Part F.

> **Naming convention used throughout:** Pearl's `job_key` = our `kappa` (κ);
> Pearl's `IncompleteBlockHeader` = `sigma` (σ); Pearl's `MiningConfiguration` =
> `mu` (μ). "Merge path" = the production Pearl-compatible recursive path
> (`validate_pearl_merge_config_for_recursive_prover` +
> `certificate_noun.rs` precheck). "Native path" = the diagnostic
> explicit-nonce square-tile path.
>
> **Certificate formats (important — these are proof artifacts, not saved work):**
> - **diagnostic L1 certificate** — this doc's name for the code's
>   **`checkpoint`** certificate (`AiPowRecursiveCertificate`). The recursion is
>   stopped at the intermediate L1 outer proof and emitted as a *large*
>   artifact that **embeds the L0 proof and L0 program**. It is a
>   diagnostic/regression format ("too large for the wire",
>   `recursion.rs:1995`; hidden from rustdoc), **not** production and **not**
>   partially-completed/saved progress. Grep the code for `checkpoint`.
> - **compact certificate** — the *production* artifact
>   (`AiPowCompactBatchRecursiveCertificate`): the recursion continues one more
>   layer (L2) and is pruned to a small body (≈ 122–124 KB). This is what
>   consensus would actually verify.
>
> The format axis (diagnostic-L1 vs compact) is **orthogonal** to the puzzle
> axis (dense vs MoE). Dense is validated on **both** formats (production =
> compact); MoE is validated **only** on the diagnostic-L1 format and is not yet
> implemented on compact — see D1 and Part G.

---

## 0. One-paragraph verdict

At the **byte level of the core mineable primitive**, the two puzzles are
**faithfully matched**: the commitment chain (κ → H_A/H_B → s_B → s_A →
jackpot), the keyed-BLAKE3 chunk-Merkle matrix commitments, the low-rank noise,
the 16-word XOR-rotate tile fold, the in-circuit jackpot digest, the
little-endian target with Pearl's `h·w·dot_product_length` pricing, and the 3-D
`PeriodicPattern` ticket language are all reproduced and, where testable,
KAT-verified against Pearl vectors. The discrepancies are **at the envelope and
integration layers**, and there are six that matter: **(D1)** Pearl supports MoE
(GROUPED_GEMM) end-to-end in live consensus (mainnet fork height 71 935) while we
are fail-closed on MoE across every production path; **(D2)** our in-circuit
`STRIPE_MAX = 64` categorically rejects a large band of Pearl-valid configs
(`k/r ∈ (64, 512]`); **(D3, now fixed)** our merge-path config validation omitted
Pearl's `h·w ≤ 256` per-tile cap; **(D4)** Pearl's node cryptographically verifies in
consensus today, whereas our Hoon `%ai-pow` surface is fail-closed ("verifier not
wired"); **(D5, closed as intentional)** the proof systems differ (Pearl Plonky2
≤ 60 KB vs. our Plonky3 compact certificate ≈ 122–124 KB); and **(D6)** on our
compact production route the opened-schedule/program binding is weaker than on the
diagnostic L1 route. After the 2026-07-13 actions (D3 fixed, D5 closed, D2 evaluated
→ document/defer), **D1 and D6 are the ones to resolve before any Hoon↔Rust
verifier wiring is enabled.**

---

## Part A — Units of work

### A.1 Puzzle configuration space (side-by-side)

| Knob | Pearl (`api/proof.rs`, `api/sanity_checks.rs`) | Ours (`pearl_compat.rs`, `params.rs`) | Match? |
|---|---|---|---|
| `common_dim` (k) | u32; `1024 ≤ k ≤ 2¹⁶`, `64\|k`, `16r ≤ k ≤ 4r²` | same (`sanity_check` `pearl_compat.rs:941-957`) | ✅ |
| `rank` (r) | u16; pow2, `32 ≤ r ≤ 1024`, `16\|r` (`TILE_D`) | same | ✅ |
| `mma_type` | enum, **only** `Int7xInt7ToInt32 = 0` (`proof.rs:23`, rejects others `proof_utils.rs:465`) | only `PEARL_MMA_INT7XINT7_TO_INT32 = 0` (`pearl_compat.rs:559`); FP8 layers also machine-rejected (`params.rs:488`) | ✅ (we are ⊆ Pearl) |
| `rows_pattern` / `cols_pattern` | `PeriodicPattern` = 3-D generalized arithmetic progression, 6 bytes, canonical | `PearlPeriodicPattern`, `NUM_DIMS=3`, identical encoding + canonicity (`pearl_compat.rs:265-486`) | ✅ |
| MoE trailer | `Option<MoEConfig{e,top_k}>` in 32-byte trailer; `e>0` ⇒ GROUPED_GEMM (`proof.rs:48-71`) | parsed identically (`parse_mining_config_trailer` `pearl_compat.rs:516`) but **fail-closed for proving** — see D1 | ⚠️ parse-yes, prove-no |
| `m`, `n` | `≤ 2²⁴`, and `n·e ≤ 2²⁴` in MoE | `≤ MAX_PERIOD = 2²⁴` (`pearl_compat.rs:272`) | ✅ |
| tile `h·w` | `2\|h`, `2\|w`, **`32 ≤ h·w ≤ 256`** | `2\|h`, `2\|w`, `32 ≤ h·w ≤ 256` — upper bound **added 2026-07-13** (D3 fix, `pearl_compat.rs sanity_check`) | ✅ (was ⚠️) |
| worker input | `(h+w)·dot_product_length ≤ 2²²` | same (`PEARL_WORKER_INPUT_MAX`, `pearl_compat.rs:954`) | ✅ |
| `k/r` (num_stripes) | **unbounded** beyond the `16r ≤ k ≤ 4r²` band (so up to 512) | **`k/r ≤ STRIPE_MAX = 64`** (`pearl_compat.rs:1095`) — see D2 | ❌ |
| `difficulty_bits` / target | `nbits` (Bitcoin compact), or `nbits_override` for pool shares | `difficulty_bits == 0`; target is verifier-supplied (`pearl_compat.rs:1084`) | ⚠️ deliberate (merge-mining) |
| tickets per certificate | exactly one `(t_rows, t_cols)` tile per proof | exactly one; `spot_checks == 1`, `num_tiles == 1` (`pearl_compat.rs:1089`) | ✅ |

**`dot_product_length` = `common_dim − common_dim % rank`** in both
(`proof_utils.rs:491` / `pearl_compat.rs` `dot_product_length`). Truncates k down
to a multiple of r; identical.

### A.2 Ticket / tile-shape space

Both express a ticket as the **cross-product of an opened row set and an opened
column set**, each generated by a 3-D `PeriodicPattern` shifted by `t_rows` /
`t_cols`. This admits **contiguous, strided, scattered, and rectangular**
openings on both sides. Our Pearl-**merge** recursive prover binds the *exact*
pattern indices as an explicit strip schedule and is explicitly forbidden from
rewriting them to a square tile (`validate_pearl_merge_config_for_recursive_prover`
doc, `pearl_compat.rs:1062-1069`; `from_indices` binding `zk_bridge.rs:1434`,
`certificate_noun.rs:2072`). Non-contiguous Pearl patterns
(e.g. `[0,1,8,9,64,65,72,73]`) are exercised
(`pearl_mining.rs:423`). The only circuit-imposed shape rule is **even h and even
w** (the `TILE_H = 2` sub-block engine, `zk_bridge.rs:2283`), which is exactly
Pearl's `2|h, 2|w`. **No discrepancy here** — except the missing `h·w ≤ 256`
ceiling (D3) and the `k/r ≤ 64` capacity limit (D2).

> The **native** square-contiguous path (`from_tile`,
> `prove_ai_pow_compact_recursive_certificate`) *does* narrow to square tiles,
> but it is the diagnostic/native route, not the Pearl-merge route. This is not a
> Pearl discrepancy; it is an internal path distinction.

### A.3 Noise / rank

Byte-identical model: rank-r factorization `E = E_L·E_R`, `F = F_L·F_R` with one
uniform factor (entries `[-32,31]`) and one sparse ±1 permutation factor (net
additive noise `[-63,63]`), seeded from `(s_B, s_A)`. Our packing base-129 /
`IRANGE7P1 = −64..=64` (`composite_layout.rs:136`) matches Pearl's noise range
exactly. Allowed ranks `{32,64,128,256,512,1024}` match. **No discrepancy.**

### A.4 Quantization / jackpot / target

- **Elements:** int7 `[-64,64]` for A,B,E,F; i32 accumulator; **wrapping**
  (no saturation) — identical both sides.
- **Tile state M:** 16 × u32, folded by `M[tid] = rotl₁₃(M[tid]) ⊕ xored_tile`
  over the `k/r` rank-chunks (`LROT_PER_TILE = 13`) — identical.
- **Jackpot digest:** `hash_jackpot = BLAKE3(M_16×u32_LE, key = s_A)` — proven
  **in-circuit** on both sides (Pearl `pearl_air.rs:105`; ours
  `composite_full_air.rs:567`).
- **Target:** `nbits_to_difficulty(nbits) · (h·w · dot_product_length)`,
  **little-endian** 256-bit comparison — identical formula and endianness
  (Pearl `sanity_checks.rs:164`; ours `pearl_compat.rs:973,1002`).
- **The `≤ target` inequality is checked natively (outside the circuit) on both
  sides**, over the in-circuit-bound `hash_jackpot`. Same trust model.

**No discrepancy** in the mineable primitive's arithmetic.

---

## Part B — Commitments

### B.1 Commitment chain (byte-parity confirmed)

| Step | Pearl | Ours | Match |
|---|---|---|---|
| κ / job_key | `BLAKE3(σ‖μ)` plain, σ=76 B, μ=52 B (`proof_utils.rs:347`) | `pearl_kappa(σ,μ) = BLAKE3(σ‖μ)` plain (`pearl_compat.rs:2274`) | ✅ |
| H_A | `BLAKE3(pad₁₀₂₄(A_row_major), key=κ)` | `matrix_commitment` keyed-BLAKE3 chunk-Merkle root (`commit.rs:59`), KAT-equal | ✅ |
| H_B | `BLAKE3(pad₁₀₂₄(Bᵀ), key=κ)` | same, B column-major | ✅ |
| s_B | `BLAKE3(κ ‖ H_B)` plain (`proof_utils.rs:363`) | `noise_seed_b` (`fiat_shamir.rs:64`) | ✅ |
| s_A | `BLAKE3(s_B ‖ hash_activations)` plain | `noise_seed_a` (`fiat_shamir.rs:74`); dense: `hash_activations = H_A` | ✅ |
| hash_jackpot | `BLAKE3(M, key=s_A)` | `pearl_jackpot_hash` (`pearl_compat.rs:2300`) | ✅ |

σ serialization matches to the byte, including the **byte-reversed** `prev_block`
/ `merkle_root` and the 76 + 52 = 128 = two-BLAKE3-block layout. **This chain is a
genuine byte-for-byte match**, which is the whole point of merge-mining
compatibility.

### B.2 MoE activation splice — implemented, KAT-matched, but inert

The MoE variant `hash_activations = BLAKE3(H_A ‖ BLAKE3(routing_root ‖
BLAKE3(pad₁₀₂₄(offsets_LE), key=κ)))` is implemented and KAT-matched against real
Pearl vectors (`fiat_shamir.rs:128,330`; Pearl `proof_utils.rs:33`). It is **only
reachable on the diagnostic L1 path**; the production merge path never
constructs it (see D1). Structurally correct, operationally fail-closed.

### B.3 Public inputs

Our Layer-0 STARK commits **60 public values** (`composite_public.rs:64`):
`cumsum[4] ‖ jackpot[16] ‖ hash_a[8] ‖ hash_b[8] ‖ job_key[8] ‖ commitment_hash[8]
‖ hash_jackpot[8]`. Pearl's STARK commits **6 hash words** (`job_key`,
`commitment_hash`, `hash_a`, `hash_b`, `hash_routing`, `hash_jackpot` — 48
values; `pearl_layout.rs:87`). Both bind the same load-bearing quantities;
ours additionally exposes the raw `cumsum`/`jackpot` tile state, and Pearl carries
a dedicated `HASH_ROUTING` slot (zero in dense) that we fold into
`commitment_hash` differently. **Semantically equivalent for the dense case.**

> **Slot dual-use to note:** our `COMMITMENT_HASH` public-input slot carries the
> nonce-derived pow-key on the native path but Pearl's `s_A` on the merge path
> (`composite_public.rs:110`). Documented; not a defect, but a known confusion
> surface (also flagged in the 2026-06-01 spec-match doc, action item 6).

### B.4 What the verifier binds vs. recomputes

Pearl's node holds the opened strips + witness and verifies the ZK proof, with
the program/opened-schedule pinned by connecting verifier-recomputed
**preprocessed columns** to the STARK's trace openings at ζ
(`starky/recursive_verifier.rs:44`). Our node precheck
(`precheck_pearl_merge_certificate_metadata`, `certificate_noun.rs:2003`) is
**stronger in one respect**: it holds the **full A and B** and independently
recomputes κ, H_A, H_B, s_A, s_B, the tile state, the jackpot hash, `found_idx`,
`trace_height`, and every public input, comparing each against the certificate —
trusting only the STARK proof bytes. **This is a faithful, arguably tighter,
verification model for the diagnostic L1 route.** The gap is on the compact route
(D6).

### B.5 Verifier-key / setup digest

Both bind their verifier setup. Pearl pins the recursion verifier keys via an
embedded circuit cache (`v2_cache.bin`) looked up by circuit params
(`pearl_circuit.rs:615`); a wrong cache ⇒ proof fails. Ours binds a Tip5
`verifier_key_digest` over route params + L2 metadata + FRI shape, checked equal
on both the verifier-owned context and the certificate
(`recursion.rs:357,1910`). Neither hashes the setup bytes into the block; both
rely on the setup being the canonical one. **Analogous; no discrepancy.**

---

## Part C — Discrepancy register (ranked)

### D1 — MoE (GROUPED_GEMM): Pearl supports it in production consensus; we are fail-closed everywhere — **MAJOR (supported units)**

- **Pearl:** MoE is a first-class production mode. The node verifies MoE proofs
  in consensus via `CertificateV2` behind the shipped `-tags zkpow` build
  (`node/zkpow/verify.go:36`; `Taskfile.yml:41`), gated by `MoEForkHeight`
  (**mainnet 71 935**, `chaincfg/params.go:319`) and enforced by exact
  `CheckCertificateVersion`. The miner runs real vLLM `FusedMoE` inference
  (`miner/vllm-miner/.../pearl_moe_method.py`). Full mine→verify→tamper tests
  pass (`node/zkpow/zkpow_test.go:205`).
- **Ours:** every production entry is fail-closed on `e>0` at **four independent
  guards** — `validate_pearl_merge_config_for_recursive_prover`
  (`pearl_compat.rs:1079`), `PearlPublicProofParams::from_public_data`
  (`pearl_compat.rs:844`), `sanity_check` (`pearl_compat.rs:928`), and the FP8
  layer guard (`params.rs:488`). The in-circuit `outer_indices ↔ routing` CTL is
  unimplemented; the compact MoE prove/verify path does not exist. MoE support
  lives only on the diagnostic **L1** path (`#[ignore]` test
  `zk_bridge.rs:3853`).
- **Consequence:** the largest divergence in *supported units of work*. Pearl's
  production puzzle space includes GROUPED_GEMM tickets (up to 1024 experts,
  top-k routing); ours categorically excludes them. Aligns with residual items
  M1–M7. Must land, staged and adversarially validated, before parity.

### D2 — `STRIPE_MAX = 64`: we reject a band of Pearl-valid dense configs — **MAJOR (supported units)** — *EVALUATED 2026-07-13: document + defer*

- Our in-circuit matmul sweep has 64 SX-register lanes (`STRIPE_MAX = 64`,
  `composite_layout.rs:482`; `STATE_LEN = 64`, `stripe_xor.rs:78`), and the
  off-circuit fallback was deleted (`zk_bridge.rs:249`). So `num_stripes = k/r >
  64` is **rejected outright**, both at config admission
  (`pearl_compat.rs:1095`) and node precheck (`certificate_noun.rs:2188`).
- Pearl imposes **no** `k/r` cap beyond `16r ≤ k ≤ 4r²` and `k ≤ 2¹⁶`. The
  Pearl-valid range of `k/r` is therefore up to **512** (e.g. r=128, k=65536),
  **256** (r=64, k=16384), or **128** (r=32, k=4096). Every such config is
  Pearl-mineable but **un-provable on our production path**.

**Evaluation (does this band need to be supported?)**

- **Pearl's own deployed configs are inside our envelope.** Pearl's default
  mainnet mining config is `common_dim = 1024, rank = 32` ⇒ `k/r = 32 ≤ 64`
  (`node/zkpow/miner.go` `defaultMiningConfigV1`; the stub echoes
  `DefaultNoiseRank = 32`, `zkpow_stub.go:16`). The real Llama-3.1 INT GEMMs we
  care about sit exactly at `k = 4096, r = 64 ⇒ k/r = 64` — the boundary, and
  in-circuit (`params.rs:376-377`).
- **The excluded band is the "low-rank, high-k" corner.** `k/r > 64` requires a
  small rank against a large common dim — e.g. `r = 32, k = 4096` (`k/r = 128`)
  or `r = 64, k = 16384` (`k/r = 256`). For any given large `k`, an operator can
  stay in-envelope by choosing `r ≥ k/64` (still Pearl-valid, since
  `16r ≤ k ≤ 4r²` leaves room). So the band is reachable but avoidable by config
  choice; it is not forced by any real workload we target.
- **The exclusion is one-way and not a soundness issue.** We are a strict subset
  of Pearl on this axis (we never accept a `k/r` Pearl rejects). A `k/r > 64`
  ticket is simply un-provable on our path — it fails closed, it is not
  mis-accepted. The only harm is merge-mining coverage: if a Pearl operator
  publishes a `mu` with `k/r > 64`, our chain cannot accept those tickets.
- **Cost to close is a circuit-level change, not a validation tweak.** Two
  routes: **(a)** widen the sweep lane count (`STRIPE_MAX`/`STATE_LEN`) — a wider
  trace, larger proof, and a full circuit re-validation; or **(b)** re-introduce
  segmented multi-STARK sweep (the "Pearl-faithful reason segmentation (G3)" that
  the current design deliberately drops because one in-envelope tile fits one
  STARK, `params.rs:263-267`) and fold the segments in the recursion. Both are
  substantial, soundness-critical (they touch the matmul constraint system and
  recursion) and must be staged per R1.
- **Recommendation — document + defer.** The `k/r > 64` band is **not required**
  for the primary merge-mining target (Pearl's default config and the real INT
  GEMMs are all `k/r ≤ 64`). The correct action now is to **document the narrowed
  envelope explicitly** so it is never mistaken for full Pearl parity, and to
  **gate any circuit work behind a demonstrated need** (a Pearl operator actually
  deploying `k/r > 64`). If that need materializes, prefer route (b)
  (segmentation) over (a) (wider lanes) to preserve the compact proof size, and
  land it staged + adversarially validated. Tracked in Part F item 3.

### D3 — Missing `h·w ≤ 256` per-tile cap on the merge path — **MEDIUM (spec-compat / validation completeness)** — ✅ **FIXED 2026-07-13**

> **Resolution (2026-07-13):** enforced Pearl's `h·w ≤ 256` cap directly.
> - Added `u64::from(h) * u64::from(w) > PEARL_HW_MAX` to
>   `PearlPublicProofParams::sanity_check` (`pearl_compat.rs`), mirroring Pearl's
>   `api/sanity_checks.rs:40` exactly; the pre-existing `< 32` lower bound now
>   uses the named `PEARL_HW_MIN` for symmetry.
> - Added a defense-in-depth backstop at the consensus boundary:
>   `precheck_pearl_merge_certificate_metadata` (`certificate_noun.rs`) now
>   rejects any certificate whose recomputed `trace_height > PEARL_TRACE_BOUND`
>   (`fits_one_stark`), so a future trace-formula change or upstream gap cannot
>   admit a tile that does not fit a single STARK.
> - Tests: `pearl_recursive_prover_config_rejects_tile_hw_above_256`
>   (`tests/pearl_merge_compat.rs`) pins `h·w = 324` → `PublicParamEnvelope` and
>   the `h·w = 256` boundary → accepted.
> - Validated: ai-pow lib **103/0**, `pearl_merge_compat` **51/0** (incl. new),
>   ai-pow-miner `--features node` **121/0** (6 ignored real-proving). No prior
>   test relied on `h·w > 256`.
>
> Analysis confirmed `h·w ≤ 256` is *sufficient* to guarantee the trace bound:
> `sweep = h·w·k/64 ≤ 2¹⁸` under the cap, and the existing `worker_input ≤ 2²²`
> gate bounds the `store`/`mhash` terms, so the total stays `≤ 2²²` — which is
> exactly why `params.rs:60-67` documents `PEARL_HW_MAX` as "the cap that keeps
> one opened tile in one STARK." The backstop makes that invariant explicit
> rather than transitive. **Original finding retained below for the record.**

- Pearl enforces `h·w ≤ 256` in **both** its verifier sanity check
  (`sanity_checks.rs:40`) and its prover (`structure_matmul_in_stark` hard-rejects
  `h·w > 256`, `pearl_program.rs:264`).
- Our merge-path admission enforces only the **lower** bound `h·w ≥ 32`
  (`pearl_compat.rs:950`). Neither `validate_pearl_merge_config_for_recursive_prover`,
  `validate_pearl_merge_recursive_params` (`certificate_noun.rs:2172` — checks
  only k, rank, `k/r ≤ STRIPE_MAX`), nor the metadata precheck imposes `h·w ≤ 256`
  or an explicit `trace_height ≤ 2²²` (`fits_one_stark`) ceiling. The only
  effective backstops are `worker_input = (h+w)·dot ≤ 2²²` and the caller-supplied
  `max_pattern_len`. The `h·w ≤ 256` cap **is** present on the native path
  (`validate_prod_envelope`, `tile² ≤ 256`, `params.rs:366`) — but the native path
  is not the Pearl-merge path.
- **Consequence:** a (contrived) Pearl config with `h·w > 256` — e.g. `h=2,
  w=256`, `k=1024`, `r=32` (worker_input = 258·1024 ≈ 2¹⁸ < 2²², `k/r=32 ≤ 64`) —
  would pass our merge admission but be **rejected by Pearl's own verifier**. This
  is a spec-compatibility divergence and a validation-completeness gap: we admit
  units Pearl declares out-of-envelope, relying on downstream trace limits rather
  than the explicit cap. Not an obvious economic exploit (difficulty credit and
  MAC-work both scale with `h·w`), but it must be closed for true byte-envelope
  parity. **Recommended fix:** add `h·w ≤ 256` to `PearlPublicProofParams::sanity_check`
  (one line, mirrors Pearl exactly) and assert `fits_one_stark()` on the merge path.

### D4 — Consensus wiring asymmetry: Pearl verifies in consensus; our Hoon path is fail-closed — **MAJOR (integration, not puzzle)**

- Pearl's shipping node cryptographically verifies every non-genesis block's ZK
  certificate in consensus (`validate.go:333 → zkpow.VerifyCertificate`).
- Our Hoon `%ai-pow` handler, after the activation-height gate
  (`ai-pow-activation-height = 95000`), emits `'do-pow: %ai-pow verifier not
  wired; rejected'` and rejects with no verification and no persistence
  (`hoon/apps/dumbnet/inner.hoon:1631-1644`; `wire.rs:15`). Every Rust verify
  entry point is `#[doc(hidden)]` "not wired from Hoon in the current milestone."
- **Consequence:** no AI-PoW block — dense or MoE — is accepted by our consensus
  today. This is the top production gate (residual S1). It is not a *puzzle*
  discrepancy, but it means the Rust-side parity below is latent until wiring
  lands — and D3/D6 must be resolved **before** it is enabled.

### D5 — Proof system and artifact size — **DESIGN DIVERGENCE (deliberate)** — ✅ **CLOSED 2026-07-13 (intentional)**

- Pearl: Plonky2 recursive proof, `MaxZKProofSize = 60 000` B, cert cap 65 000 B
  (`certificate.go:68`).
- Ours: Plonky3 compact recursive certificate ≈ **122–124 KB** (≤ 150 KB target,
  4 MiB jam DoS cap `certificate_noun.rs:155`), 60-bit relaxed FRI.
- **Consequence:** ~2× the wire size, an entirely different verifier substrate
  (Tip5 transcript, Goldilocks composite STARK).

> **Closed as intentional (2026-07-13).** This is a deliberate, load-bearing
> architecture choice, **not** a defect to reconcile toward Pearl. Nockchain does
> **not** adopt Pearl's Plonky2 stack: the canonical block artifact is a
> structured, Hoon-cue-able recursive-certificate **noun** verified by the
> local Plonky3-recursion substrate with a Tip5 transcript over Goldilocks — a
> requirement of embedding into Nockchain's Nock/Hoon consensus, which Pearl's
> opaque Plonky2 byte-blob cannot satisfy. The divergence is confined to the
> **proof-system / wire layer**; it does **not** touch the mineable computation
> (the commitment chain, tile, and jackpot remain byte-parity with Pearl, Part
> B). This was already the stated position in
> `docs/ai-pow-integration/2026-06-01_AI_POW_PEARL_SPEC_MATCH_AND_MINEABLE_UNIT.md`
> ("Proof System and Artifact"). The only residual obligations are operational,
> not architectural: keep the artifact within the ≤ 150 KB / ~30 s bar and
> account for its size in block limits (Part F item 6). **No parity action is
> owed.**

### D6 — Compact production route binds the opened schedule more weakly than the diagnostic L1 route — **MEDIUM (internal soundness surface)** — 🔎 **INVESTIGATED 2026-07-13 → see Part G**

> **Deep-dive result (Part G):** confirmed and sharpened. The compact
> `verifier_key_digest` is **shape-only**; the opened-schedule/selector binding
> lives entirely in the verifier-owned `context.circuit_prover_data`, and **no
> verifier-side builder derives that context from the canonical program** — every
> path uses the prover's own context. A safety contract was added at the node
> entry point; the fix (verifier-side canonical context derivation + adversarial
> cross-schedule test) is the M2/M3 residual, detailed in Part G. **Status:
> characterized, not fixed** — must be closed before D4 wiring.

- The strongest opened-schedule/program binding — `l0_program_matches` /
  `canonical_program_for_strip_schedule`, a full cell-by-cell equality of the
  preprocessed program to the verifier-recomputed canonical program — runs on the
  **L1** path (`recursion.rs:141`; `zk_bridge.rs:2116`). The **compact**
  production route (`verify_compact_batch_recursive_certificate_with_context`)
  binds the statement only via the Tip5 digest of the 60 public inputs +
  verifier-key digest; it does **not** re-run `canonical_program`
  (`recursion.rs:155-195`).
- The schedule binding on the compact route therefore rests transitively on the
  recursion having verified L0 against its pinned program, tied to the PI digest —
  not on a direct program recompute at the node.
- **Consequence:** a real trust-surface difference between the two routes on
  *our own* side. Moot today (D4 fail-closed) but **must be audited and, if
  needed, hardened before compact-path acceptance is wired** (residual M2/M3/M7).
  Related minor: the merge precheck leaves the `cumsum` PI slot unpinned
  (`certificate_noun.rs:2200`) — benign because `cumsum` is a derived intermediate
  of the same fold that yields the checked jackpot, but it is the one PI the node
  does not independently compare.

---

## Part D — Confirmed consistencies (not discrepancies)

These were checked and **do** match, and should be preserved:

1. **Commitment chain byte-parity** (κ, H_A, H_B, s_B, s_A, hash_jackpot) —
   KAT-verified.
2. **Matrix commitments**: keyed-BLAKE3 chunk-Merkle root over `pad₁₀₂₄`, A
   row-major / B column-major, selective chunk-SET opening that is byte-identical
   to the covering range for contiguous tiles.
3. **3-D `PeriodicPattern`** ticket language, canonicity, and offset validity.
4. **Rank/noise** model and ranges; base-129 packing.
5. **Jackpot fold** (16 × u32, rotl₁₃ ⊕), in-circuit jackpot digest.
6. **Target** formula (`nbits · h·w · dot_product_length`) and LE comparison.
7. **`mma_type` pinned to 0** (INT7×INT7→INT32) on both sides.
8. **One tile per certificate** — both prove a single `(t_rows, t_cols)` ticket;
   our `spot_checks==1` / `num_tiles==1` is *consistent* with Pearl, not a
   narrowing (a common misreading — Pearl also emits one proof per tile).
9. **`found_idx` semantics differ by design** (native: s_A-derived tile index;
   merge: Pearl `t_rows/t_cols` pattern offset) — the merge semantics correctly
   mirror Pearl's ticket-search model; not a discrepancy.
10. **Verifier holds full A/B and recomputes everything** on the diagnostic L1 certificate
    precheck — a faithful (tighter) verification model.

---

## Part F — Recommended residual (ties to existing tracking)

Status after the 2026-07-13 actions; each open stage KAT-first, full regression +
adversarial gates, commit per validated stage (per R1):

1. **D3 — ✅ DONE.** `h·w ≤ 256` enforced in `sanity_check` + `trace_height ≤
   PEARL_TRACE_BOUND` backstop in the node precheck, with rejection/boundary
   tests; validated (see D3 resolution box).
2. **D6 — OPEN.** Before wiring compact-path acceptance, either re-run
   `canonical_program`/`l0_program_matches` at the node for the compact route, or
   prove the PI-digest binding is equivalent; pin `cumsum`. (Residual M2/M3/M7.)
3. **D2 — EVALUATED → document + defer.** The `k/r > 64` band is not required for
   the primary merge-mining target (Pearl's default config and the real INT GEMMs
   are `k/r ≤ 64`). **Action:** document the narrowed envelope explicitly (this
   audit + the spec-match doc) so it is not mistaken for full parity; gate any
   circuit work (prefer segmentation over wider lanes) behind a demonstrated Pearl
   operator need. No code change now.
4. **D1 — OPEN.** Land the MoE compact prove/verify + node branch behind the
   guards, adversarially validated, then lift the guards last (residual M1–M7, M4
   last).
5. **D4 — OPEN.** Wire Hoon consensus → Rust verifier at the DoS-safe jam
   boundary (residual S1) — **only after D6 is closed** (D3 now done).
6. **D5 — ✅ CLOSED (intentional).** No parity action owed; operational only —
   keep the artifact within the ≤ 150 KB / ~30 s bar and account for its size in
   block limits.

---

## Appendix — Key file:line index

**Pearl:** config `pearl/zk-pow/src/api/proof.rs:10-160`; sanity
`api/sanity_checks.rs:12-180`; commitments `api/proof_utils.rs:33-495`; target
`sanity_checks.rs:145-180`; node verify `node/zkpow/verify.go:36-168`; MoE fork
`node/chaincfg/params.go:319`; cert sizes `node/wire/certificate.go:68`.

**Ours:** admission gate `pearl_compat.rs:1070-1123`; sanity_check
`pearl_compat.rs:924-970`; MoE fail-closed guards `pearl_compat.rs:1079,844,928`,
`params.rs:488`; commitment chain `pearl_compat.rs:2274-2347`, `commit.rs:59`,
`fiat_shamir.rs:54-188`; STRIPE_MAX `composite_layout.rs:482`,
`pearl_compat.rs:1095`; public inputs `composite_public.rs:56-119`; node precheck
`certificate_noun.rs:1646-2144`; compact cert `recursion.rs:155-195,357-388`;
Hoon fail-closed `hoon/apps/dumbnet/inner.hoon:1631-1644`.

---

## Part G — D6 deep-dive: compact-path opened-schedule binding (2026-07-13)

*Investigated as the chosen first work item. Status: **characterized, not fixed** —
the sound fix is the M2/M3 residual (below). A safety contract was added; no
soundness-logic change was landed, because landing the verifier-side derivation
half-validated would be worse than a clean residual (R1).*

### G.1 What the compact path actually binds — traced end to end

- The compact certificate carries only `verifier_key_digest` +
  `l2_compact_body` — **no `l0_program`, no L0 proof** (`recursion.rs:155-158`).
  So the diagnostic L1 path's binding `AiPowRecursiveCertificate::l0_program_matches`
  (cell-by-cell equality of the preprocessed program to the canonical one,
  `recursion.rs:141`; invoked at `zk_bridge.rs:1662-1669, 2117`) **cannot run on
  the compact path** — there is no program to compare.
- The node's compact verify (`certificate_noun.rs:1867-1890`) runs the *same*
  cheap precheck as the diagnostic L1 path (recomputes the ticket → rows/cols → all
  60 public inputs, incl. `hash_jackpot`; `certificate_noun.rs:1910`,
  `precheck_pearl_merge_certificate_metadata`), then calls
  `verify_compact_batch_recursive_certificate_with_context`
  (`recursion.rs:1910`).
- Inside that call, the L2 body is verified against a **verifier-owned
  `AiPowCompactBatchVerifierContext`** — specifically its `circuit_prover_data`
  and `metadata` (`recursion.rs:1954-1964`). The context's `circuit_prover_data`
  **encodes the specific L0 program**: it is built from
  `CompositeFullAirWithLookupsPinned::new_with(verified.program)` → the L1
  verifier circuit → L1/L2 prep (`recursion.rs:1790-1868`).
- `verifier_key_digest = digest(route_params, l2_metadata, fri_shape)`
  (`recursion.rs:357-388`). `l2_metadata` is
  `GoldilocksBlake3BatchStarkProofMetadata::from_proof` — `table_packing`,
  `public_binding_lanes`, `rows`, `alu_variant`, `ext_degree`, …, `stark_common`
  (`batch_stark_prover.rs:655-681`) — i.e. **circuit shape/config**, the same for
  every block of a given trace shape. It does **not** bind which rows/columns
  were opened.

**Conclusion:** on the compact path the entire opened-schedule + constraint-
selector binding lives in `context.circuit_prover_data`. The `verifier_key_digest`
the node checks is shape-only. Therefore the compact path is sound **iff** the
node builds `context` (and the expected digest) from the *canonical* program
re-derived from the public opened schedule.

### G.2 The gap — no verifier-side canonical context exists

- The **only** construction site for `AiPowCompactBatchVerifierContext` is
  prover-side, inside `prove_..._compact...` (`recursion.rs:1865`), built from
  `verified.program` (the prover's program).
- **Every** verify caller obtains the context via `run.verifier_context()` — the
  prover's own context (all `#[ignore]`d round-trip tests,
  e.g. `certificate_noun.rs:5476, 5567`; `recursion.rs:2262`). The expected
  digest is likewise taken from `run.verifier_key_digest()`
  (`certificate_noun.rs:5468-5471`).
- The design docs already flag this as required-but-unimplemented
  (`recursion.rs:133-140` and `184-195`: "Accepting those values from the prover
  would make this object unsound").

So the diagnostic L1 path's `l0_program_matches` binding has **no realized
equivalent** on the compact path. Existing compact tests cover wrong-PI and
wrong-/noncanonical-digest rejection (`recursion.rs:2267-2292`;
`certificate_noun.rs:5574-5607`) but **never** the program binding — a cert
proven over a *different opened schedule* is not tested for rejection, because
there is no canonical context to reject it against. Wiring the node to a
prover-supplied or generic context would let a malicious miner open a favorable
strip / disable a selector-gated constraint and still verify.

> This matches the residual's own caveat ("everything MoE above is validated on
> the diagnostic L1 certificate; the compact/node integration is residual" — M2/M3) and is
> a **dense** issue too: the dense compact path shares this binding model.

### G.3 Feasibility of a verifier-side builder, and the wall

- The L1 verifier circuit allocates the inner L0 proof as circuit **inputs**
  (`BatchStarkVerifierInputsBuilder::allocate`, `recursion.rs:686`), so the
  circuit structure — and thus `circuit_prover_data` — is **witness-independent**:
  a function of (program, proof *shape*, config), not proof values. Confirmed by
  the prover cache being reused across distinct proofs
  (`recursion.rs:1805-1813`, `ensure_compact_batch_l1_prep_matches_built`). So a
  verifier-side builder is possible **in principle**, and `common_data` is
  derivable from the canonical program (`logup_common_for(&cfg, &program, true)`,
  `recursion.rs:1791`).
- **The wall:** `build_composite_l1_verifier_circuit` still requires a
  `BatchProof` object of the correct *shape* (`recursion.rs:648, 686`), which the
  compact certificate deliberately omits for size. Building the context
  verifier-side therefore needs either (i) a shape-correct L0 proof synthesizer
  (construct a dimensionally-valid dummy `BatchProof` from params/`trace_height`),
  or (ii) a redesign that binds the canonical program's preprocessed commitment
  into `verifier_key_digest` / the statement so a shape-only digest is no longer
  accepted. Neither is wired today. This is a genuine subsystem-level effort, not
  a drop-in — hence the R1 residual outcome rather than a rushed change.

### G.4 What was landed this session (validated)

- **Safety contract** at the node compact-verify entry point: a `# Soundness`
  section on `verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits`
  (`certificate_noun.rs`) stating that `compact_context` /
  `expected_verifier_key_digest` MUST be derived from the canonical program (never
  prover-supplied, never a reused generic context), that `verifier_key_digest` is
  shape-only, and that block acceptance must not reach this path until the
  verifier-side builder lands. Compile + existing tests green.

### G.5 Precise residual (the fix — M2/M3 core)

1. **Verifier-side canonical context/digest derivation.** Add a builder that
   constructs `AiPowCompactBatchVerifierContext` (or at least the expected
   `verifier_key_digest` + the pinned `circuit_prover_data`) from the canonical
   program re-derived from the public opened schedule
   (`canonical_program_for_strip_schedule` + noise pins + the 60 PIs), via
   `logup_common_for` → `build_composite_l1_verifier_circuit` →
   `build_compact_batch_l1_prep` → `build_compact_batch_l2_over_l1_prep`. Resolve
   the shape-correct-proof requirement by either (i) a dummy shape-only
   `BatchProof` constructor, or (ii) binding the program's preprocessed commitment
   into `verifier_key_digest`/the statement digest.
2. **Node rewiring.** Change the compact node-verify path to build its own context
   via (1); forbid accepting a prover-supplied context.
3. **Adversarial validation gate (the missing test).** Prove two compact certs
   over two opened schedules of the *same shape* (e.g. two Pearl tickets with
   different `t_rows`/`t_cols`, same patterns). Assert: (a) they share
   `verifier_key_digest` (documents it is shape-only), and (b) `cert_A` verified
   against the canonical context for schedule B is **rejected** (proving the
   binding is realized). This is the compact-path analogue of the diagnostic L1 certificate
   `l0_program_matches` test and of residual M7/D4's "cert over a different opened
   set is rejected."
4. **Pin `cumsum`.** The merge precheck leaves the `cumsum` PI slot unpinned
   (`certificate_noun.rs:2200-2205`); include it once the above lands.

**Gate:** D6 must be closed (1–4 green) before D4 (Hoon↔Rust consensus wiring),
because the compact certificate is the production artifact for both dense and
MoE, and D1's production landing rides this same path.
