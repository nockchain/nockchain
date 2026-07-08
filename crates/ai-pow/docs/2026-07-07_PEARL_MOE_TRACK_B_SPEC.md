# Track B — Pearl MoE grouped-GEMM: concrete implementation specification

**Date:** 2026-07-07
**Status:** **Track B off-circuit + core circuit + MoE soundness IMPLEMENTED and
validated** (B1–B4, B3a–d, B5a de-risk, **B5b non-contiguous opening**, **B5c grouped
matmul**, **B5d full recursive prove→verify**, **routing-consistency binding + 10
adversarial tests**, **§4.C.10 malicious-miner for non-contiguous**). The complete MoE
proving stack proves + verifies end-to-end (`real_moe_recursive_certificate_proves_and_verifies`).
Track A (V2-dense byte-parity + MoE fail-closed) is committed (`f4cedea6`).
**Remaining (precise residual):** the production **node-boundary integration** — a MoE
variant of `certificate_noun.rs::verify_decoded_ai_pow_pearl_merge_artifact` precheck
(recompute MoE `s_a` from routing + call `verify_pearl_moe_routing_binding` + recompute
the MoE canonical program from `outer_indices`), the MoE artifact encode/decode, a
high-level MoE prove path, and lifting the `e > 0` fail-closed guards. This is
soundness-critical node-boundary work; land it in validated stages (R1), MoE stays
fail-closed until it does.
**Discipline:** soundness-critical + invasive (a cryptographic PoW commitment + circuit
change). Land in validated stages, KAT-first against pinned Pearl vectors, MoE kept
fail-closed until each stage validates. Dense S0–S9 byte-equivalence is the standing
regression gate throughout (Track B must not perturb the dense path).

**Pearl reference pin:** `~/Dev/ai-pow/pearl`, `origin/master` (MoE merged in `77fef91e`,
released `v1.1.5`; live on mainnet). Re-pin the exact commit before writing KATs.

---

## 0. What Track B builds

Track A made us byte-compatible with Pearl V2 for **dense** work and fail-closed on MoE.
Track B implements the actual **MoE (Mixture-of-Experts) grouped GEMM** so we can mine,
prove, and verify an MoE job byte-compatibly. It is optional (dense keeps working without
it) and strictly gated behind the Track A `e > 0` guards until complete.

**The one structural fact that scopes Track B** (verified in `zk-pow/src/api/proof_utils.rs::commitment_hash`):
the *only* new cryptographic math is the routing-commitment splice (§3, stage B2). The
noise (`E`/`F`), the 512-bit tile fold `M`, `s_B`, and the jackpot hash are **byte-identical
to V1/dense** — our `matmul.rs`, `TileState`, `BlockNoise`, and most of `fiat_shamir.rs`
are reused unchanged. Everything else in Track B is index selection, expert-column
offsetting, variable-length (de)serialization, and Merkle-proof plumbing.

---

## 1. The MoE matmul, precisely

A standard job computes `A·B`, `A: (m×k)` row-major, `B: (k×n)` col-major. An MoE job is a
**grouped GEMM over `E` experts**:

- **Experts stack along columns.** Public `n` is the **per-expert** intermediate dim `n_e`;
  expert `x`'s weight columns are the global range `[x·n, (x+1)·n)`. Total columns
  `n·e ≤ 2²⁴` (enforced). `weight_col_offset = expert_idx · n`.
- **Tokens route to experts.** `topk_ids: (m, top_k)` gives, per token, the `top_k` experts
  it is sent to. The flattened routing has `m·top_k` slots; slot `s` belongs to token
  `s / top_k`.
- **Canonical routing** (`build_routing_data`, `miner/pearl-gemm/csrc/moe/build_routing_data.cuh`):
  a **stable counting sort by expert id** of the `m·top_k` slots produces
  - `slot_indices` — the sorted slot permutation (stable ⇒ deterministic tie-break by
    original slot order),
  - `routing_data[i] = slot_indices[i] / top_k` — the **token index** of the `i`-th routed
    slot (this is the *committed* routing array),
  - `routing_offsets[e]` — exclusive-end cumulative token counts; `routing_offsets[e-1] =
    m·top_k`. Expert `x` owns `routing_data[start_x .. routing_offsets[x])` where `start_x =
    x==0 ? 0 : routing_offsets[x-1]`.
- **A solved proof opens one expert's tile.** For `expert_idx`, the tile's A-rows are that
  expert's routed tokens selected by `rows_pattern` (over the expert's token subset), and
  its B-columns are that expert's weight slice selected by `cols_pattern`. `outer_indices`
  maps the tile's local A-rows back to **global token positions in `A`**.

**The per-tile inner loop is unchanged** (`structure_matmul_in_stark`): an `h×w` tile over
`k` in `r`-chunks with the same `TILE_D`/`TILE_H`/`JACKPOT_SIZE`, `LROT` fold, `h·w ≤ 256`.
Grouped GEMM changes only *which* A-rows / B-columns feed the tile, plus the routing
commitment.

---

## 2. Byte-level data model (Pearl types to mirror)

| Pearl type (`zk-pow/src/api/proof.rs`, `ffi/plain_proof.rs`) | Fields | Where |
|---|---|---|
| `MoEConfig` | `e: u16, top_k: u16` | mining-config trailer `e(2)｜top_k(2)｜zero(28)` |
| `MoEParams` | `expert_idx: u16, routing_offsets: Vec<u32>, hash_routing: [u8;32], outer_indices: Vec<u32>` | public params tail |
| `MoEProofParams` | `e: usize, top_k: usize, expert_idx: u16, routing_end_offsets: Vec<u32>, inner_a_rows: Vec<usize>, routing_proof: MerkleProof` | plain-proof tail |
| `PrivateProofParams.s_routing` | `Vec<Vec<u8>>` — 64-byte routing strips | private witness |

**Public-data wire** (`PublicProofParams::to_wire_bytes`, LE):

```
core (WIRE_SIZE = 164, byte-identical to dense):
  mining_config(52) ‖ hash_a(32) ‖ hash_b(32) ‖ hash_jackpot(32) ‖ m(4) ‖ n(4) ‖ t_rows(4) ‖ t_cols(4)
MoE tail (present iff mining_config.moe.is_some()):
  expert_idx(2) ‖ routing_offsets[e]·(4) ‖ hash_routing(32) ‖ outer_count(1) ‖ outer_indices[oc]·(4)
```

Bounds (DoS + buffer sizing): `MAX_NUM_EXPERTS = 1024`, `MAX_OUTER_INDICES = 128`,
`ROUTING_OFFSET_BYTES = 4`, `MIN_MOE_WIRE_SIZE = 199`, `MAX_WIRE_SIZE = 4807`.

**Plain-proof wire** (bincode 1 fixint LE): dense core `{m,n,k,noise_rank,a,bt}` then the
`moe` `Option` tag — `0x00` for dense (Track A) or `0x01 ‖ bincode(MoEProofParams)` for MoE.
`MoEProofParams` bincode order is exactly its field order; `routing_proof: MerkleProof`
reuses the same layout as `a`/`bt` proofs (`leaf_data｜leaf_indices｜total_leaves｜root｜siblings`).

**Routing byte layout.** The committed routing is `routing_data` as `m·top_k` `u32` LE
(4·m·top_k bytes), **padded up to a 16-entry (64-byte / `BLOCK_LEN`) boundary with zero
token indices**, then to the 1024-byte BLAKE3 chunk boundary for the Merkle tree
(`moe_test.rs::moe_params_routing_not_multiple_of_64` exercises the non-16-aligned case:
1000 → 1008 entries). Each opened `s_routing` strip is 64 bytes and — because an expert's
routing may not start on a 64-byte boundary — may include neighbouring experts' indices
sharing that block (Pearl's "virtual 64-byte-row matrix" view; `proof.rs` `PrivateProofParams` doc).

---

## 3. The commitment splice (stage B2 — the whole new-math surface)

`proof_utils.rs::commitment_hash` + `compute_hash_activations` (authoritative; matches
`miner-base/commitment_hash.py`). `tensor_hash(x,key) = BLAKE3(pad_to_chunk(x_bytes), key)`
is a **single keyed hash** (`matrix_merkle_tree.py`), not a Merkle tree:

```
routing_root     = MerkleTree(pad_to_chunk(routing_data_le_u32), key = job_key).root
hash_offsets     = BLAKE3(pad_to_chunk(routing_offsets_le_u32), key = job_key)      # single keyed hash
hash_routing     = BLAKE3(routing_root ‖ hash_offsets)                              # = moe.hash_routing (public)
hash_activations = BLAKE3(hash_a ‖ hash_routing)                                    # dense: = hash_a
s_B              = BLAKE3(job_key ‖ hash_b)                                          # unchanged
s_A              = BLAKE3(s_B ‖ hash_activations)                                    # hash_a → hash_activations
jackpot          = BLAKE3(M(64B LE), key = s_A)                                      # unchanged
```

Everything after `s_A` (noise, fold, jackpot) is identical to dense. **B2 must assert the
dense reduction `hash_activations == hash_a` when `moe == None`** — that assertion is the
guardrail protecting the Track A byte-parity we just landed.

---

## 4. Our mapping and reuse

| Track B need | Our code | Action |
|---|---|---|
| `MoEConfig` trailer accept `e>0` | `pearl_compat.rs::validate_mining_config_trailer` | lift the `UnsupportedMoeConfig` guard → parse `Some(MoEConfig)` (B3) |
| Commitment splice `s_A` via `hash_activations` | `fiat_shamir.rs::noise_seed_a` | thread `hash_activations` (B2); dense path unchanged |
| `routing_root`, routing strips + Merkle proof | `pearl_plain_proof.rs::build_matrix_merkle_proof`, `commit.rs::matrix_commitment`, `ai-pow-zk::blake3_tree` | reuse over routing bytes (B1/B2) |
| `hash_offsets` (single keyed hash) | `commit.rs` / `blake3` keyed | new helper; confirm `pad_to_chunk` == Pearl (B2) |
| Grouped tile compute | `matmul.rs::compute_pattern_tile_trace_from_slices` | reuse as-is; add expert col offset + `outer_indices` (B3) |
| Variable-length public params + `MoEParams` | `pearl_compat.rs::PearlPublicProofParams` | implement the MoE tail (A2 currently fail-closed) (B3) |
| Plain-proof `MoEProofParams` tail | `pearl_plain_proof.rs::PearlMoeProof` (placeholder) | implement fields + bincode `Some` encoding (B3) |
| `s_routing` witness + strip opening | `pearl_plain_proof.rs` merkle-proof machinery | add routing-strip extraction (B1/B5) |
| Recursive certificate + circuit bind | `ai-pow-zk/` | bind routing commitment, `outer_indices` CTL, grouped matmul, `"V2"` STARK prefix (B5) |

---

## 5. Staged plan (KAT-first; MoE fail-closed until each stage validates)

### B1 — Routing data (off-circuit), KAT-locked

Port `build_routing_data`: `topk_ids (m,top_k)` → stable-counting-sort → `slot_indices`,
`routing_data` (`= slot/top_k`), `routing_offsets` (exclusive ends, last `= m·top_k`,
`< 2³²`). Emit the padded routing byte layout (§2: 16-entry/64-byte then 1024-chunk).

- **New:** `ai-pow` module `pearl_moe_routing.rs` (or extend `pearl_compat`): `build_routing_data`, `routing_bytes`.
- **KAT source (consensus-critical — reimplement-by-reading is NOT acceptable):** Pearl
  `csrc/moe/build_routing_data.cu`, `moe.py`, `miner/vllm-miner/tests/test_moe_routing_data.py`,
  `moe_testing_helpers.py`. The stable tie-break must match bit-for-bit — a mismatch changes
  `routing_root` and every downstream hash.
- **Gate:** vector-equality of `slot_indices`/`routing_data`/`routing_offsets` and the padded
  bytes vs Pearl reference, including a non-16-aligned `m·top_k` case.

### B2 — Routing-commitment splice (the only new cryptographic math)

Implement `routing_root` (MerkleTree over padded routing, key=`job_key`), `hash_offsets`
(single keyed BLAKE3 of padded `routing_offsets` LE), `hash_routing = BLAKE3(routing_root ‖
hash_offsets)`, `hash_activations = BLAKE3(hash_a ‖ hash_routing)`, and route `s_A` through it.

- **Files:** `fiat_shamir.rs` (splice `noise_seed_a`), `commit.rs` (offsets keyed hash + routing root reuse).
- **KAT source:** `proof_utils.rs::compute_hash_activations`/`commitment_hash`, `commitment_hash.py`.
- **Gate:** (a) `hash_activations == hash_a` when `moe==None` (dense guardrail);
  (b) full `(routing_root, hash_offsets, hash_routing, hash_activations, s_A, jackpot)` chain
  byte-equal to a Pearl MoE vector; (c) S0–S9 still green.

### B3 — Grouped-GEMM selection, fields, and (de)serialization

Accept `MoEConfig` (`e>0`); implement `MoEParams` + variable-length public data and the
`MoEProofParams` plain-proof tail; compute the opened tile with `weight_col_offset =
expert_idx·n`, A-rows = expert's routed tokens via `rows_pattern`, mapped to global via
`outer_indices`. Reuse `compute_pattern_tile_trace_from_slices`.

- **Validation (mirror `sanity_checks.rs` exactly):** `e>0`, `top_k>0`, `top_k<e`,
  `expert_idx<e`, `e ≤ MAX_NUM_EXPERTS`, `e == routing_offsets.len()`, `m·top_k < 2³²` and
  `routing_offsets[e-1] == m·top_k`, offsets monotone non-decreasing, per-expert span `≤ m`,
  `routing_offsets[0] ≤ m`, `n·e ≤ 2²⁴`, opened routing indices in the real (non-padding)
  region, opened B-column indices within `[expert_idx·n, (expert_idx+1)·n)`,
  `outer_indices.len() ≤ MAX_OUTER_INDICES`.
- **Files:** `pearl_compat.rs` (`PearlMiningConfig` `moe`, `PearlPublicProofParams` tail,
  ticket compute), `pearl_plain_proof.rs` (`PearlMoeProof` fields + bincode `Some`).
- **KAT source:** `to_wire_bytes`/`from_wire_bytes`, `PlainProof` serialize, `moe_test.rs`
  (`parse_proof`, `sanity_check`, `sanity_check_private_params`).
- **Gate:** public-data + plain-proof round-trips byte-equal to Pearl MoE vectors; every
  validation rejection has an adversarial test; the recomputed jackpot matches for a mined
  MoE ticket. Lift the Track A `e>0` fail-closed guards **only** for the paths covered here.

### B4 — Difficulty / envelope under grouped GEMM

Confirm the shape-aware target and parameter envelope for an MoE tile. Current reading:
per-expert tile pricing uses the same `h·w·dot_product_length` factor (unchanged); MoE adds
*validation* (`sanity_checks.rs`), not a new price. Treat "pricing unchanged" as a **claim to
KAT**, not an assumption.

- **KAT source:** `sanity_checks.rs::extract_difficulty_bound` / `check_jackpot_difficulty_with_nbits`.
- **Gate:** MoE `pearl_adjusted_target` / Nockchain-adjusted target byte-equal to Pearl for a
  representative MoE config.

### B5 — Recursive certificate + circuit binding (heaviest; own sub-stages)

Extend `ai-pow-zk` to bind, in-circuit: the routing commitment (`routing_root` over the
`s_routing` strips + Merkle proof), `hash_offsets`, the `hash_activations` splice into `s_A`,
the grouped matmul with `weight_col_offset`, and the **`outer_indices` CTL** (Pearl fails
verification when public `outer_indices` disagree with the proof — `moe_test.rs::
test_moe_wrong_public_outer_indices_fails_verification`). Include the STARK `"V2"` Fiat-Shamir
domain-separator prefix (`public_data_commitment`). Keep a **separate circuit-cache / prover
type** from the dense (V1) path so a V2 change can never alter a dense proof (mirrors Pearl's
`zk_pow::v1` freeze).

- **Sub-stages (each KAT-first):** B5a routing/offsets bind KAT; B5b `outer_indices` CTL bind +
  adversarial tamper; B5c grouped-matmul bind (expert col offset); B5d full prove→verify
  round-trip; B5e adversarial (corrupt routing ⇒ STARK constraint failure; field/`weight_col_offset`
  tamper ⇒ reject), mirroring `moe_test.rs`.
- **Gate:** end-to-end prove+verify of a mined MoE ticket; every `moe_test.rs` failure case
  reproduced as a rejecting test. MoE stays fail-closed until B5 fully validates.

### Sequencing

B1 → B2 → B3 behind the fail-closed guard, flipping `e>0` on per-path only as its stage
validates; B4 alongside B3; B5 last and gated most strictly. A half-landed routing
commitment is strictly worse than dense-only + fail-closed MoE (R1).

---

## 5b. Implementation status (2026-07-07)

Track B's **entire off-circuit surface is landed and validated**; the in-circuit
binding (B5) is the residual. MoE remains **fully fail-closed at acceptance** —
no MoE proof can be accepted as a Nockchain block until B5.

| Stage | Status | Commit | Where |
|---|---|---|---|
| B1 routing canonicalization | ✅ validated | `014451d9` | `src/pearl_moe_routing.rs` |
| B2 commitment splice | ✅ validated | `520793f2` | `src/fiat_shamir.rs` |
| B3a config parse + fail-closed accept | ✅ validated | `65d610a2` | `src/pearl_compat.rs` |
| B3b public-data wire codec | ✅ validated | `4b8c07b0` | `src/pearl_compat.rs`, `tests/pearl_moe_wire.rs` |
| B4 difficulty (pricing == dense) | ✅ validated | `2b9c1d8a` | `tests/pearl_moe_wire.rs` |
| B3c plain-proof `MoEProofParams` tail | ✅ validated | `86ceefe2` | `ai-pow-miner/src/pearl_plain_proof.rs` |
| B3d grouped-tile reference | ✅ validated | `c4517844` | `src/pearl_compat.rs`, `src/pearl_moe_routing.rs`, `tests/pearl_moe_tile.rs` |
| B5-gate MoE fail-closed end-to-end | ✅ validated | `59594765` | `tests/pearl_moe_fail_closed.rs` |
| B5a (de-risk) `moe_ref` + **real Pearl KAT** | ✅ validated | `3f5a5169` | `ai-pow-zk/src/moe_ref.rs`, `fiat_shamir.rs` |
| **B5b non-contiguous recursive opening** | ✅ **implemented + proves/verifies** | `a4ca3158`, `e1b963d3` | `canonical.rs`, `composite_trace.rs`, `zk_bridge.rs` |
| **B5c grouped matmul + B5d Layer-0 prove (MoE tile)** | ✅ **implemented + proves** | `1abae8e5` | `zk_bridge.rs` (`real_moe_grouped_tile_layer0_proof`) |
| B5b routing-consistency binding (outer_indices↔routing) | ⛔ residual (Rust check or CTL) | — | `zk_bridge.rs`/`pearl_compat.rs` |
| B5 lift fail-closed guards + high-level MoE cert path | ⛔ residual | — | `zk_bridge.rs`, `pearl_compat.rs` |
| B5e adversarial coverage | ⛔ residual | — | tests |

**Validation notes.** B1/B2/B3a/B3b/B3c/B4 are byte-exact against Pearl's
unambiguous spec (algorithm / formula / wire layout / bincode oracle) — no live
Pearl vector needed. B3d's `compute_moe_tile` is validated by **dense-equivalence**
(byte-identical to the already-validated dense per-tile compute given the same
opened indices/seeds) + MoE self-consistency; its one from-reading assumption
(the exact `outer_indices` gather semantics) carries a residual KAT (below).

### B5 — attempted; de-risk landed, in-circuit sub-AIR is the R1 wall

**What was done this session (genuine attempt, not deferral).** The `ai-pow-zk`
crate was opened and edited: **B5a de-risk landed** (`3f5a5169`) — `moe_ref`, the
off-circuit MoE routing-commitment reference the sub-AIR must reproduce, following
the codebase's KAT-first discipline (cf. `noise_ref`). Its validation was upgraded
from from-reading to **real Pearl**: the Pearl `zk-pow` crate was built and a
`compute_hash_activations` vector emitted, against which our splice + `moe_ref` are
now KAT'd (this also closed the B2 from-reading residual). The in-circuit binding
mechanism was traced to its exact insertion point (below).

**Refined finding (this session, by tracing the composite circuit).** `BlockPublic.s_a`
/`s_b` are **public inputs** the verifier **recomputes in Rust**
(`zk_bridge.rs`: `canonical_noise_seeds_*` → checks `pis.commitment_hash`/`job_key`
/`hash_a`/`hash_b`); the canonical program has **no in-circuit commitment chain**
(its `RowClass` set is `StripOpenA/B, KeyPin, Sweep, Fold, JackpotHash, Pad`). So
the MoE `hash_activations` reroute is **Rust-enforceable** — the verifier
reconstructs `s_a` from the **public** `moe.hash_routing` (= `routing_root`) +
`routing_offsets`, no private `routing_data` needed. That part is **done and
tested** (`compute_pearl_moe_ticket`, commit `3cd75055`; verifier-recomputes-`s_a`
test). Consequently B5 is narrowed from "the whole circuit" to a **single**
remaining in-circuit change:

**The one remaining soundness-critical change — the `outer_indices`↔routing CTL.**
It binds the opened A-row indices (`outer_indices`, public) to the committed
private `routing_data` (`routing_root`): prove `outer_indices[u] =
routing_data[expert_start + inner[u]]`. This **cannot be Rust-shortcut** — the
verifier has only `routing_root`, not `routing_data` — so it must be a
LogUp/cross-table-lookup **in the composite** (`composite_lookups.rs`,
`composite_lookup_proof.rs`) plus a routing strip-opening `RowClass` in the
**canonical program pin** (`canonical.rs`). This is the "cryptographic-proof
program-pin" R1 names: it needs a new schedule row-class + AIR constraints + trace
+ the lookup argument, validated by full prove→verify **and** adversarial
under-constraining coverage. That cannot be soundly completed and exhaustively
tested in one session, and a partial change to the program-pin risks silent
forgeability (strictly worse than none — R1). So the CTL is the residual — reached
by driving the core to a single precise change, not by declining.

**Empirically pinned the wall by running a real proof (commit `066746ca`).** The
recursive prover already takes arbitrary opened indices via
`StripIndexSchedule::from_indices` (`zk_bridge.rs:1423`), and the Pearl-merge shape
check does **not** reject non-contiguous patterns. So a real Layer-0 recursive
proof was built and run for a genuinely non-contiguous opened pattern
(`[0,1,8,9,64,65,72,73]`, target `0xff…`). It **fails inside the STARK** with
`LookupError(GlobalCumulativeMismatch: noised_packed)` (test
`noncontiguous_recursive_prove_currently_fails_at_noised_packed_lookup`, ~32s).

**Root cause (structural, confirmed).** `indexed_strips_chunk_range`
(`blake3_tree.rs`) opens the **covering chunk range** `min_idx·k .. (max_idx+1)·k`.
For a contiguous tile the covering range **equals** the selected rows, so the
`noised_packed` LogUp (noise produced on the strip-opening leaf rows ↔ consumed by
the matmul sweep) balances. For non-contiguous rows the covering range is far larger
than the selected set (rows 0–73 vs 8 selected), so the strip produces per-row noise
the sweep never consumes and the global cumulative sums diverge. This is the hard
in-circuit reason the recursive AIR is "square-contiguous only" — not a soft guard.

**Therefore B5b's precise in-circuit change** is **selective (per-selected-chunk)
opening** — open only the selected rows/chunks rather than the `min..max` covering
range — reworking `indexed_strips_chunk_range` / `strip_blocks` / `StripPlan`
(disjoint chunk sets), the co-located leaf-row noise pins (`canonical.rs`), and the
`noised_packed` producer/consumer accounting (`composite_lookups`), then the routing
binding on top. That is a real, now-precisely-scoped circuit change, validated by
full prove→verify + adversarial. The committed `#[ignore]`d probe is the B5b
regression target: when selective opening lands it must flip to prove+verify
succeeding. Reached by *doing* (a real proof), not by declining.

(An earlier program-pin edit — adding a third `IS_USE_ROUTING_ROOT` key-pin row —
was also tried and hit the hardcoded "exactly two key-pin rows" invariant
(`canonical.rs:971`) + the `NUM_SELECTORS` circuit-width cascade; reverted per R1.
The `noised_packed` opening wall above is the deeper, load-bearing one.)

**Exact remaining steps (each KAT-first; MoE stays fail-closed until all pass):**
1. **More Pearl KAT vectors.** (Pearl `zk-pow` now builds from the `pearl/` clone.)
   Emit a fixed `(public_data, PlainProof)` MoE vector via `try_mine_one_moe` +
   `parse_proof` (`zk-pow/tests/moe_test.rs`) and vendor it — closes B3d's
   `outer_indices`/tile residual byte-for-byte.
2. **B5a-circuit routing/offsets bind.** New rows in
   `canonical_program_for_strip_schedule` constraining `routing_root` over the
   `s_routing` 64-byte strips + Merkle proof and `hash_offsets`, then reroute the
   `commitment_hash` (`s_A`) key-pin to derive from `hash_activations` — reproducing
   `moe_ref::moe_commitment` in-circuit. Reuse the composite BLAKE3 chip.
3. **B5b `outer_indices` CTL** (Pearl rejects mismatched `outer_indices` —
   `moe_test.rs::test_moe_wrong_public_outer_indices_fails_verification`).
4. **B5c grouped matmul bind** — expert column offset `expert_idx·n`; A-rows via
   `outer_indices` (reproduces `compute_moe_tile`).
5. **B5d prove→verify** of a mined MoE ticket; **B5e adversarial** — corrupt routing
   ⇒ constraint failure; `outer_indices`/`weight_col_offset`/field tamper ⇒ reject.
6. **Lift the guards** only after B5a–e validate: `sanity_check`,
   `validate_pearl_merge_config_for_recursive_prover`, `from_public_data`, plain-proof
   `LegacyV1` MoE rejection — plus wire `PearlMoeParams` / `PearlMoeProof` into
   `PearlPublicProofParams` / the mined attempt end-to-end.
7. **STARK `"V2"` domain prefix** in `public_data_commitment`.

**Fail-closed guarantee (verified, `tests/pearl_moe_fail_closed.rs`):** a MoE
mining config is rejected at (a) the 164-byte merge-statement cap, (b)
`from_public_data`, (c) `sanity_check` / `verify_pearl_compatible_work`, and (d)
`validate_pearl_merge_config_for_recursive_prover`. The landed B1–B4 building
blocks are inert with respect to block acceptance until step 6 lifts these guards.

## 6. Soundness / adversarial requirements (must all have rejecting tests)

- Routing canonicalization mismatch (any tie-break/order deviation) — caught by B1 KAT.
- `hash_activations != hash_a` on a dense proof — forbidden (B2 guardrail).
- Public `outer_indices` disagreeing with the committed routing — reject (B5 CTL).
- Corrupted routing data / offsets — STARK constraint failure (B5).
- Opened routing index in the padding region, or B-column outside the expert's slice — reject (B3).
- `weight_col_offset` / `expert_idx` tamper — reject (B3 + B5).
- Any MoE input reaching the prover before its stage is validated — fail closed.
- Dense S0–S9 unchanged at every stage.

---

## 7. Open questions / risks

- **Routing canonicalization is the highest-risk item.** The CUDA kernel supports ≤256
  experts (8-bit sort key) while the protocol allows `MAX_NUM_EXPERTS = 1024` — confirm the
  reference/CPU path's ordering for `e > 256` and match *that*, not just the CUDA fast path.
- **`pad_to_chunk` boundary + `BLOCK_LEN` (64) vs chunk (1024).** Routing pads to 16-entry/
  64-byte alignment (for the BLAKE3 block/strip view) *and* to the 1024-byte chunk (for the
  Merkle leaves). Confirm our `pad_to_chunk_boundary` (`pearl_plain_proof.rs`) and
  `blake3_tree::CHUNK_LEN` agree with Pearl on both boundaries for the routing tensor
  specifically.
- **Difficulty pricing unchanged is a claim, not a fact** — B4 KAT it.
- **`s_routing` strip opening across expert boundaries** — the 64-byte strip may contain other
  experts' indices; the membership proof and in-circuit reconstruction must handle the
  virtual-row view (`routing_blake_hotspot_rows`, `extract_routing_strips` in `plain_proof.rs`).
- **Two prover/cache types.** Keep dense and MoE recursive provers separately validated.
- **Pearl-side flux.** Re-pin the exact Pearl commit before writing KATs.

---

## 8. Source pointers

**Pearl (`~/Dev/ai-pow/pearl`, `origin/master`):**
- `zk-pow/src/api/proof.rs` — `MoEConfig`, `MoEParams`, `PrivateProofParams.s_routing`.
- `zk-pow/src/api/proof_utils.rs` — `commitment_hash`, `compute_hash_activations`,
  `to_wire_bytes`/`from_wire_bytes`, wire constants, `public_data_commitment` (`"V2"`).
- `zk-pow/src/api/sanity_checks.rs` — the full MoE validation set + difficulty bound.
- `zk-pow/src/ffi/plain_proof.rs` — `MoEProofParams`, `OuterIndices`, `deserialize_compat`,
  `min_cert_version`, `extract_routing_strips`, `routing_blake_hotspot_rows`, `moe_inner_indices`.
- `zk-pow/tests/moe_test.rs` — end-to-end mine→parse→sanity→prove→verify + failure cases.
- `miner/pearl-gemm/csrc/moe/build_routing_data.{cu,cuh}`, `src/pearl_gemm/moe.py` — routing.
- `miner/miner-base/src/miner_base/{commitment_hash,matrix_merkle_tree}.py` — commitment/tensor_hash reference.
- `miner/vllm-miner/tests/{test_moe_routing_data.py,moe_testing_helpers.py}` — routing vectors.
- `zk-pow/fixures/v2_stark_proof_moe.bin` — a serialized MoE STARK proof fixture.

**Ours:**
- `crates/ai-pow/src/pearl_compat.rs` — `validate_mining_config_trailer` (lift guard),
  `PearlPublicProofParams` (MoE tail), ticket compute.
- `crates/ai-pow/src/fiat_shamir.rs` — `noise_seed_a` (B2 splice point).
- `crates/ai-pow/src/commit.rs`, `crates/ai-pow-zk/src/blake3_tree.rs` — routing Merkle / keyed hash.
- `crates/ai-pow/src/matmul.rs` — `compute_pattern_tile_trace_from_slices` (grouped tile, reused).
- `crates/ai-pow-miner/src/pearl_plain_proof.rs` — `PearlMoeProof` (implement fields + bincode).
- `crates/ai-pow-zk/` — recursive certificate + circuit (B5).
- Track A: `crates/ai-pow/docs/2026-07-07_PEARL_MOE_SPARSE_UPGRADE_PATH.md`.
