# Pearl MoE ("sparse matmul") upgrade: situation analysis and byte-compatibility path

**Date:** 2026-07-07
**Status:** analysis / design. No `ai-pow` or `ai-pow-zk` code changed by this document.
**Scope:** what Pearl changed at the matmul-spec level, how it maps onto our code, and
the staged path to regain exact byte-compatibility.
**Discipline:** soundness-critical + invasive — land in validated stages, KAT-first, no
rushing and no fake completion (standing rule R1). This is the design/de-risk artifact
that precedes those stages.

**Pearl reference pin:** `~/Dev/ai-pow/pearl`, `origin/master`, MoE merged in
`77fef91e` ("Moe fork changes (#173)", raskin1, 2026-06-11). Re-pin the exact commit
before writing KATs; do not trust the earlier `feat/moe_hardfork` placeholder constants.

---

## 0. TL;DR

- The "sparse matrices" upgrade is Pearl's **MoE (Mixture-of-Experts) hard fork**. It
  generalizes the PoUW matmul from one **dense** GEMM to an optional **grouped GEMM**:
  the weight matrix `B` is column-partitioned into `E` experts, each token is
  data-dependently **routed** to `top_k` of the `E` experts, and only the selected
  (token, expert) products are computed. The sparsity is the routing.
- It is merged on Pearl `master` (released in `v1.1.5`) and **already active on mainnet**.
  Fork heights (`node/chaincfg/params.go`, set 2026-06-11 in `77fef91e`): **mainnet
  71,935**, testnet **1** (from genesis), testnet2 54,869. Mainnet tip was **83,480** on
  2026-07-07 — **~11,500 blocks past activation.** At/after the fork, **every** block must
  carry a **V2 certificate** — *including blocks that mine the old dense models*. Crucially,
  the strict cutover (`CheckCertificateVersion`) binds the **block certificate the
  gateway/pool builds from our share**, not the plain-proof share we submit. A post-fork V2
  gateway still accepts our dense share via `PlainProof::deserialize_compat`, and its V2
  prover certifies it — so **dense merge mining keeps working across the fork; nothing of
  ours is rejected.** Track A is about *exact* byte-parity + clean MoE fail-closed, not
  restoring broken dense mining.
- **The entire soundness-critical math delta is one line.** V2 recomputes A's noise seed
  as `s_A = H(s_B ‖ hash_activations)` where `hash_activations = hash_a` for dense and
  `= H(hash_a ‖ H(routing_root ‖ H(pad₁₀₂₄(offsets_le), key=job_key)))` for MoE. The
  **noise generation (E/F), the tile fold `M`, `s_B`, and the jackpot hash are
  byte-identical to V1.** Verified against `zk-pow/src/api/proof_utils.rs::commitment_hash`.
- Consequence: **V2 dense is byte-identical to V1** (all-zero MoE trailer, 164-byte public
  data, unchanged math). Pearl froze the V1 circuit as `zk_pow::v1`; the V2 prover accepts
  both dense and MoE plain proofs.
- **Our `ai-pow` matches Pearl V1 dense byte-for-byte** (fixtures S0–S9); our recursive
  prover covers the square-contiguous tile subset. We implement **none** of the V2 surface:
  grouped GEMM, routing commitment/proof, `MoEConfig`/`MoEParams`, the V2 certificate
  wire format, or the version-2 proof-commitment prefix.
- **Two incompatibilities bite today**, both in `src/pearl_compat.rs`: (1) we reject any
  nonzero mining-config trailer (`NonzeroReserved`) — that trailer now holds `MoEConfig`;
  (2) we hardcode the V1 164-byte public-params size and V1-shaped merge statement.
- **Path — two tracks.** **Track A** (time-critical, before the fork height): emit/verify
  the **V2 certificate envelope for dense work** — mostly serialization + one
  commitment-prefix change; math untouched. **Track B** (optional, to mine MoE): add the
  grouped GEMM + routing commitment. Because the fold/noise/jackpot are reusable, **Track
  B's only new cryptographic math is the routing-commitment splice (B2);** the rest is
  index selection, expert-column offsetting, and Merkle-proof plumbing.

---

## 1. Baseline: the V1 (dense) spec we already match

Pearl V1 mines one tiled INT7×INT7→INT32 matmul `(A + E)(B + F)` with low-rank noise, an
iterative 512-bit tile state `M` folded per `r`-wide stripe, and a keyed-BLAKE3 jackpot
hash. A miner commits to a `MiningConfiguration`, then proves one opened tile ("ticket")
selected by periodic row/column patterns with offsets `t_rows`/`t_cols`.

V1 commitment chain (our names bracketed; `src/fiat_shamir.rs`, `src/pearl_compat.rs`):

```
job_key   = blake3(incomplete_header ‖ mining_config)        [κ]
hash_a    = MerkleRoot(A,   key = job_key)                    [H_A]
hash_b    = MerkleRoot(B^T, key = job_key)                    [H_B]
s_B       = blake3(job_key ‖ hash_b)                          [s_B]
s_A       = blake3(s_B ‖ hash_a)                              [s_A]
jackpot   = blake3(M(64 bytes LE), key = s_A)                 [hash_jackpot]
```

Wire facts we match: `MiningConfiguration` is **52 bytes** ending in a **32-byte trailer
required to be all-zero** in V1; `PublicProofParams` public data is **164 bytes**; the V1
certificate is `Version(4) ‖ HeaderHash(32) ‖ PublicData(164) ‖ ProofLen(4) ‖ ProofData`
with proof commitment `double_sha256(cert_version_le32(=1) ‖ public_data)`. Byte
equivalence is locked by `tests/fixtures/pearl.rs` + `tests/pearl_compat_fixtures.rs`
(S0–S9).

---

## 2. What Pearl changed: the MoE grouped-GEMM spec

### 2.1 The matmul, precisely

A standard job computes `A·B` with `A: (m×k)`, `B: (k×n)`. An MoE job reinterprets this as
a **grouped GEMM over `E` experts**:

- `MoEConfig { e: u16, top_k: u16 }` selects the mode. **`e == 0` ⇒ standard job** (`moe
  == None`); `e > 0` ⇒ grouped GEMM. `e` and `top_k` live in the 52-byte mining config's
  trailer as `e(2 LE) ‖ top_k(2 LE) ‖ zero(28)`, so they are committed inside `job_key`.
- The weight matrices of the `E` experts are **stacked along columns**: `n` in the public
  params is the **per-expert** intermediate dim `n_e`; expert `x`'s columns are the global
  range `[x·n, (x+1)·n)`. Total columns `n·e ≤ 2²⁴` (enforced).
- Each of the `m` tokens is routed to `top_k` experts (`topk_ids: (m, top_k)`). The flat
  routing array has `m·top_k` `u32` token indices, grouped per expert; `routing_offsets[i]`
  is the exclusive-end cumulative token count for experts `0..=i`, with
  `routing_offsets[e-1] = m·top_k` (`< 2³²` enforced). Per expert count `≤ m`; offsets
  monotone non-decreasing.
- A solved proof opens **one expert's tile**: `expert_idx`, the tile's A-rows are that
  expert's routed tokens (mapped to global token positions via **`outer_indices`**), and
  its B-columns are that expert's weight slice.

The **per-tile inner loop is unchanged from V1**: `structure_matmul_in_stark`
(`zk-pow/src/circuit/pearl_program.rs`) still walks an `h×w` tile over `k` in `r`-chunks
with the same `TILE_D`/`TILE_H`/`JACKPOT_SIZE`, `LROT_PER_TILE` fold, and `h·w ≤ 256`
cap. Grouped GEMM changes only *which* A-rows and B-columns feed the tile, and adds the
routing commitment.

### 2.2 The commitment delta (the whole soundness-critical change)

`proof_utils.rs::commitment_hash`:

```rust
let hash_activations = match &self.moe {
    Some(moe) => compute_hash_activations(&hash_a, &moe.hash_routing, &moe.routing_offsets, &job_key),
    None      => hash_a,                       //  ← dense: identical to V1
};
let s_B = blake3(job_key ‖ hash_b);            //  unchanged
let s_A = blake3(s_B ‖ hash_activations);      //  hash_a → hash_activations
```

with (`compute_hash_activations`, verified byte layout):

```
hash_offsets     = blake3(pad_to_chunk_boundary(routing_offsets as LE u32 bytes), key = job_key)  # pad to 1024
hash_routing_mix = blake3(routing_root ‖ hash_offsets)          # routing_root = moe.hash_routing = MerkleRoot(routing, key=job_key)
hash_activations = blake3(hash_a ‖ hash_routing_mix)
```

Everything downstream (`s_B`, `s_A`, noise `E`/`F`, tile fold, `compute_jackpot_hash =
blake3(M 64 bytes LE, key=s_A)`) is **identical to V1**. For dense, `hash_activations =
hash_a`, so `s_A` — and thus every subsequent byte — is bit-identical to V1.

### 2.3 New public/private fields

`PublicProofParams` gains `moe: Option<MoEParams>`:

```
MoEParams { expert_idx: u16,
            routing_offsets: Vec<u32>,   # e entries, last = m*top_k
            hash_routing: Hash256,       # MerkleRoot(routing, key=job_key)
            outer_indices: Vec<u32> }    # opened tile's local rows → global token positions in A
```

`PrivateProofParams` gains **`s_routing: Vec<Vec<u8>>`** — 64-byte strips of raw routing
data with a Merkle membership proof. Subtlety (verbatim from Pearl): routing is a list of
`u32` token indices, not a 64-byte-aligned matrix, so an expert's routing may not start on
a BLAKE3 block boundary. Pearl "virtually" treats routing as a matrix of 64-byte rows;
each strip is 64 bytes and may include indices of other experts sharing that block. Under
this view routing membership proofs behave like matrix membership proofs.

### 2.4 Exact V2 `public_data` wire layout

```
core (WIRE_SIZE = 164, byte-identical to V1):
  mining_config(52) ‖ hash_a(32) ‖ hash_b(32) ‖ hash_jackpot(32) ‖ m(4) ‖ n(4) ‖ t_rows(4) ‖ t_cols(4)
MoE tail (present iff mining_config.moe.is_some()):
  expert_idx(2 LE) ‖ routing_offsets[e]·(4 LE) ‖ hash_routing(32) ‖ outer_count(1) ‖ outer_indices[oc]·(4 LE)
```

A **dense** V2 `public_data` is exactly the 164-byte core — byte-identical to V1's public
data. Bounds (relevant to our decode-DoS posture, cf. our `SPOT_CHECKS_MAX`/`decode_dos`):
`MIN_MOE_WIRE_SIZE = 199`, `MAX_NUM_EXPERTS = 1024`, `MAX_OUTER_INDICES = 128`,
`ROUTING_OFFSET_BYTES = 4`, `MAX_WIRE_SIZE = 199 + 128·4 + 1024·4 = 4807`.

### 2.5 Certificate envelope + STARK binding

- **V2 certificate wire** (`node/wire/certificate.go`, `certificate_v2.go`):
  ```
  V1: Version(4) ‖ HeaderHash(32) ‖ PublicData(164)          ‖ ProofLen(4) ‖ ProofData
  V2: Version(4) ‖ HeaderHash(32) ‖ PublicDataLen(4) ‖ PublicData(N) ‖ ProofLen(4) ‖ ProofData
  ```
  Proof commitment: `double_sha256(cert_version_le32 ‖ public_data)` with the version
  prefix now **`2`** (was always `1`). Public data is **variable-length** in V2.
- **STARK Fiat-Shamir** now binds a `"V2"` domain-separator prefix:
  `public_data_commitment = blake3("V2" ‖ header ‖ public_data ‖ pow_bits ‖ rate_bits)`.
  This is inside the plonky2 STARK challenger, not the outer certificate — it matters for
  the recursive-proof track (B5), not for Track A.

### 2.6 Consensus mechanics

`CertificateVersionV2 = 2` alongside `V1 = 1`; `Params.MoEForkHeight` +
`IsMoEForkActive` + `RequiredCertVersion(height)`; `getblocktemplate` returns
`requiredcertversion` (1 pre-fork, 2 post-fork); strict cutover in
`blockchain/validate.go::CheckCertificateVersion` (exact required version only). Miner
`min_cert_version()` = V1 for dense / V2 for MoE; `check_cert_version_eligible` rejects an
MoE share before the fork. Authoritative summary: `docs/moe-fork-upgrade-guide.md`.

---

## 3. Our implementation vs. V2 — the reuse map

| Pearl V2 element | Our code | Reusable as-is? |
|---|---|---|
| Noise `E = E_L·E_R`, `F = F_L·F_R` | `matmul.rs::BlockNoise`, `prng.rs` | **Yes** — identical |
| Tile fold `M[step%16]=rotl13⊕x`, jackpot hash | `matmul.rs::TileState`, `keyed_hash` | **Yes** — identical |
| `s_B = blake3(job_key‖hash_b)` | `fiat_shamir.rs::noise_seed_b` | **Yes** — identical |
| `job_key`, `hash_a`, `hash_b` | `fiat_shamir.rs`, `commit.rs` | **Yes** — identical |
| `s_A = blake3(s_B‖hash_activations)` | `fiat_shamir.rs::noise_seed_a` | **Dense: yes. MoE: splice `hash_activations`** (B2) |
| Periodic patterns, `t_rows`/`t_cols`, difficulty pricing | `pearl_compat.rs` | **Yes** for shape; recursive prover square-contiguous only |
| `MoEConfig` trailer parse | `PearlMiningConfig` (rejects nonzero trailer) | **No** — must add (A1/B3) |
| `MoEParams`, `outer_indices`, variable public data | `PearlPublicProofParams` (fixed 164) | **No** — must add (A3/B3) |
| Grouped-GEMM row/col selection, expert column offset | — | **No** — must add (B3) |
| Routing data + `hash_routing` + `s_routing` Merkle proof | — | **No** — must add (B1/B2) |
| Pearl plain-proof share (`{m,n,k,r,a,bt}` bincode) | `pearl_plain_proof.rs` | **Dense: works via gateway compat; +1 `0x00` tag for exact V2** (A3) |
| V2 certificate wire + version-2 commitment prefix | (not parsed) | **Not needed** — we don't parse Pearl's `MsgCertificate`; binding is version-agnostic aux-inclusion |

**Active incompatibilities today** (`src/pearl_compat.rs`):
1. **Trailer rejection.** `PEARL_MINING_CONFIG_RESERVED_SIZE = 32`; `to_bytes`/`from_bytes`
   return `PearlCompatError::NonzeroReserved` for any nonzero trailer. A **dense** V2
   config (`e=0`, all-zero trailer) still decodes; any **MoE** config (`e>0`) is rejected
   here — the exact splice point for MoE trailer parsing.
2. **Fixed V1 sizes.** `PEARL_PUBLIC_PROOF_PARAMS_SIZE = 164` and
   `PEARL_MERGE_PUBLIC_STATEMENT_*` assume V1's fixed public data; V2's variable-length
   body (with `MoEParams`) has no representation.
3. **Plain-proof share is legacy-format (works, not exact).** Our produced Pearl share
   (`pearl_plain_proof.rs::encode_bincode1`) omits the trailing `moe: Option<_>` tag, so it
   is the V1/legacy encoding — one `0x00` short of native V2 dense. A post-fork gateway
   accepts it via `deserialize_compat`, so dense mining is **not broken**; it just isn't
   *exact* byte-parity and leans on the legacy fallback (A3 closes this).

**Matches V1 dense byte-for-byte:** commitment chain, PRNG/noise, tile loop, jackpot,
difficulty target (S0–S9); periodic-pattern ticket model at the precheck layer; recursive
prover for square-contiguous tickets (fail-closed otherwise).

---

## 4. Upgrade path

Two independent tracks, both staged per R1: KAT-first against the pinned Pearl vectors,
per-stage full-regression + adversarial gates, **S0–S9 kept bit-identical throughout**
(that invariant is the primary regression gate — any drift means we broke dense
byte-equivalence), commit per validated stage.

### Track A — regain exact V2-dense byte-compatibility (concrete staged plan)

**Scope correction (verified against our integration code).** Our system does **not** parse
Pearl's `MsgCertificate` wire wrapper and does **not** bind Pearl's version-2 proof
commitment `double_sha256(cert_version_le32 ‖ public_data)`. Our merge binding is the
**version-agnostic aux-inclusion path** (`verify_pearl_aux_inclusion`: nock commitment →
Pearl coinbase → Pearl header `merkle_root`), which MoE does not change. So Pearl's V2
*certificate envelope* is **out of scope** for us. Track A is exactly two byte-surfaces:

**Surface 1 — CONSUME/VERIFY** (`crates/ai-pow/src/pearl_compat.rs`): decode Pearl's 76-byte
`IncompleteBlockHeader` + 164-byte `public_data`, recompute κ→s_B→s_A→jackpot, check targets.
For **dense V2 these bytes are byte-identical to V1** (all-zero MoE trailer), so this path
**already verifies dense V2 correctly** — the only gap is trailer *semantics* + explicit KATs.

**Surface 2 — PRODUCE** (`crates/ai-pow-miner/src/pearl_plain_proof.rs`, `run.rs`): emit the
Pearl gateway plain-proof share. We hand-roll bincode-1 fixint/LE of `{m,n,k,noise_rank,a,bt}`
in `encode_bincode1` — this is the **legacy V1 format**, exactly one `0x00` option-tag short
of Pearl V2's native dense encoding (V2 `PlainProof` appends `moe: Option<_>`). A V2 gateway's
`deserialize_compat` still accepts our legacy bytes (it retries with a `0x00` appended), so
dense **functionally works today**; it is not *exact* byte-parity and relies on that fallback.

Standing gate for the whole track: **S0–S9 stay bit-identical** (no matmul/noise/jackpot
edits), and **all MoE (`e>0`) inputs fail closed** until Track B.

- **A0. KAT harness (Pearl V2 dense vectors).** Extend the `gen_fixtures`-style generator to
  emit, from the Pearl `zk-pow` crate at the pinned commit: (i) `MiningConfiguration.to_bytes()`
  dense (52B), (ii) `PublicProofParams.to_wire_bytes()` dense (164B) + its `commitment_hash`,
  (iii) `bincode::options().with_fixint_encoding().serialize(&PlainProof{moe:None,..})`. These
  become the byte oracles for A1–A4. *Source:* `zk-pow/src/api/proof_utils.rs`,
  `zk-pow/src/ffi/plain_proof.rs`.
- **A1. Trailer → `MoEConfig`, fail-closed (consume).** In `PearlMiningConfig` reinterpret the
  32-byte trailer (`reserved`) as `e(2 LE) ‖ top_k(2 LE) ‖ zero(28)`; expose `moe:
  Option<{e,top_k}>` (`e==0 ⇒ None`). `from_bytes`: require the 28-byte pad zero; on `e>0`
  return a new `PearlCompatError::UnsupportedMoeConfig` (replacing the misleading
  `NonzeroReserved` for that case). `to_bytes` for `e==0` must reproduce today's all-zero
  trailer **byte-for-byte**. *Gate:* existing pearl fixtures + S0–S9 green; new round-trip KAT
  (dense trailer == A0 vector).
- **A2. Dense public-data + reject MoE tail (consume).** Keep `PearlPublicProofParams`
  `from_public_data` at the 164-byte dense core (matches V2 `WIRE_SIZE`), but make an
  over-length (MoE) `public_data` fail closed with a clear `UnsupportedMoePublicParams` rather
  than the generic `BadPublicParamsLen`. *Gate:* decode A0's 164-byte V2-dense vector →
  identical `PearlPublicProofParams`; recomputed commitments/jackpot match A0's `commitment_hash`.
- **A3. Emit native V2 dense plain proof (produce).** In `encode_bincode1`, append the trailing
  bincode `Option::None` tag (`0x00`) after `bt` (equivalently add a `moe: Option<…>` field,
  `None` for dense). *Gate:* our `to_base64_bincode1` bytes == A0's Pearl-serialized dense
  `PlainProof` vector, **byte-for-byte**; and Pearl `PlainProof::deserialize_compat(our_bytes)`
  yields `min_cert_version == ZkDense`.
- **A4. Version-gate the emission (produce).** Only send the new-format (trailing-`0x00`) share
  when the job is V2 (mainnet is already post-fork ⇒ default V2); retain the legacy encoding
  path for any pre-fork gateway. No RPC-envelope change — `submitPlainProof` carries only
  `plain_proof` + `mining_job{incomplete_header_bytes,target}`. *Gate:* both encodings decode
  via Pearl `deserialize_compat`; miner run-loop test covers the selection.
- **A5. Fail-closed MoE end-to-end + trust-model note.** Assert an MoE mining config / share is
  rejected before any proving on both surfaces; document the analysis above (no Pearl
  certificate-wire parsing needed) and leave a fail-closed guard if certificate parsing is
  ever added. *Gate:* adversarial tests for `e>0` on consume + produce.

Net Track A change is small and byte-local: a trailer reinterpretation + a distinct
fail-closed error (consume), and a single appended `0x00` option tag (produce). No
matmul/noise/jackpot/certificate-wire code is touched.

#### Status — implemented + validated (2026-07-07)

All A gates are landed and tested; the standing dense-byte-parity gate (S0–S9) stays green.

- **A1** — `ai-pow/src/pearl_compat.rs`: `validate_mining_config_trailer` parses
  `e(2)|top_k(2)|zero(28)`, mirroring Pearl's checks; `to_bytes`/`from_bytes` fail closed
  with `UnsupportedMoeConfig{e,top_k}` (e>0), `MoeTopKWithoutExperts` (top_k≠0, e=0), or
  `NonzeroReserved` (pad). Dense (all-zero) round-trips byte-identically.
- **A2** — `PearlPublicProofParams::from_public_data` peeks the trailer discriminant and
  surfaces `UnsupportedMoeConfig` for any MoE public data (164-core or with the variable
  tail), while a genuinely-wrong dense length still reports `BadPublicParamsLen`.
- **A3** — `ai-pow-miner/src/pearl_plain_proof.rs`: added `moe: Option<PearlMoeProof>` +
  `PlainProofWireFormat`; `encode_bincode1` now emits native V2 (trailing bincode
  `Option::None` `0x00`) by default; `run.rs` submits V2 unchanged. Legacy V1 stays
  reachable via `encode_bincode1_with_format(.., LegacyV1)`.
- **A4** — both encodings validated against the exact `bincode 1` fixint config Pearl
  uses; a test reproduces Pearl's `deserialize_compat` (strict-decode V2 → `moe==None`;
  legacy fails strict, recovers after appending `0x00`).
- **A5** — MoE share generation fails closed (`MoeNotSupported`) on every encode path.

Tests: `ai-pow` full suite green incl. S0–S9 (11/11) and new `tests/pearl_moe_fail_closed.rs`
(8); `ai-pow-miner` `pearl_plain_proof` (6, incl. the V2/legacy/deserialize-compat/
fail-closed cases) + full `node` suite (119 lib + 9 bin) green.

**A0 residual (optional hardening, not blocking).** The produce-side byte oracle is a
faithful serde-mirror of Pearl's `PlainProof` serialized by the *same* `bincode 1.3.3`
crate Pearl uses, plus a hand-reproduction of Pearl's `deserialize_compat` fallback — it
is not a live link against the Pearl `zk-pow` crate. This matches the repo's existing
KAT pattern (S0–S9 vendored, not linked). A stronger cross-check would add an
`#[ignore]`d generator that links Pearl `zk-pow` to emit `to_wire_bytes` / `PlainProof`
vectors directly; deferred as heavy-dependency hardening.

### Track B — MoE grouped GEMM (large, soundness-critical, optional)

Only after Track A. Because fold/noise/jackpot/`s_B` are reusable, the **only new
cryptographic math is B2**; the rest is selection/offset/Merkle plumbing. Stage strictly,
each KAT-first.

- **B1. Routing data (off-circuit).** Port `build_routing_data`: `topk_ids (m,top_k)` →
  canonical `slot_indices`, `routing_data`, per-expert exclusive-end `routing_offsets`
  (last `= m·top_k`, `< 2³²`). **KAT vs Pearl CUDA/reference** (`moe.py`,
  `csrc/moe/build_routing_data.cu`) — the tie-break/stable ordering is consensus-critical;
  reimplement-by-reading is not acceptable here.
- **B2. Routing-commitment splice (the one math delta).** Implement `routing_root`
  (`MerkleRoot(routing, key=job_key)` under the 64-byte virtual-row view), `hash_offsets`
  (`blake3(pad₁₀₂₄(routing_offsets_le), key=job_key)`), `hash_routing_mix = blake3(routing_root
  ‖ hash_offsets)`, `hash_activations = blake3(hash_a ‖ hash_routing_mix)`, and route `s_A`
  through it. **Assert the dense reduction `hash_activations == hash_a` when `moe==None`**
  (guards §2.2's backward-compat guarantee).
- **B3. Grouped-GEMM selection + fields.** Expert column offset `expert_idx·n`; A-rows =
  routed tokens mapped through `outer_indices`; parse/validate `MoEConfig`/`MoEParams`
  (the full §2 constraint set: `top_k<e`, `expert_idx<e`, `e≤1024`, `n·e≤2²⁴`, monotone
  offsets, per-expert `≤ m`, routing indices in the real region, `outer_indices≤128`).
  Lift the A1 `e>0` fail-closed guard. Reuse `compute_pattern_tile_trace_from_slices` for
  the tile itself.
- **B4. Difficulty/envelope under grouped GEMM.** Confirm shape-aware target and parameter
  envelope (per-expert `h·w`, dot-product length) match Pearl V2 pricing.
- **B5. Recursive certificate + circuit.** Extend `ai-pow-zk` to bind the routing
  commitment + `s_routing` Merkle proof and the grouped matmul, incl. the STARK `"V2"`
  domain prefix. Heaviest, most soundness-critical stage — own validated sub-stages
  (noise-ref KAT, routing-bind KAT, full round-trip, adversarial tamper on every new
  opening/field). MoE stays fail-closed until B1–B5 each validate.

### Sequencing & gates

1. Land **Track A** first (dense stays mineable at the fork). Gates: full regression +
   S0–S9 + adversarial green; new V2-dense KATs green.
2. Land **Track B** stage-by-stage behind the `e>0` fail-closed guard; flip on only when
   B1–B5 each validate. A half-landed grouped-GEMM commitment is strictly worse than
   dense-only + fail-closed MoE (R1).

---

## 5. Open questions / risks

- **The fork is already active, but dense mining is not broken.** Mainnet `MoEForkHeight =
  71,935`; tip 83,480 on 2026-07-07 (≈11.5k blocks past); testnet active from genesis. Pearl
  mainnet rejects V1 *certificates*, but that binds the gateway/pool's certificate, not our
  plain-proof share, which the V2 gateway still accepts (compat). So Track A is prudent
  hardening (exact byte-parity + MoE fail-closed), not an outage fix. It *would* become
  urgent if a target gateway drops legacy-compat, or if we ever submit Pearl block
  certificates ourselves. (Actual block rate ran ≈2.6× the 194s target — read the tip, don't
  estimate activation from spacing.)
- **Routing canonicalization is consensus-load-bearing.** A tie-break mismatch in
  `build_routing_data` changes `routing_root` → every downstream hash. Strictly KAT-first
  (B1), not reimplement-by-reading.
- **`hash_offsets` padding detail.** `pad_to_chunk_boundary` pads the LE-`u32` offsets to a
  1024-byte boundary before the keyed hash. Confirm our chunk-Merkle/`tensor_hash` agrees
  byte-for-byte on the offsets tensor specifically before reusing it.
- **Two circuit caches.** Mirror Pearl's frozen-V1 + V2 split in the recursive prover so a
  V2 change can never silently alter a V1 (dense) proof.
- **Decode-DoS on the MoE tail.** Enforce Pearl's bounds on our side up front
  (`MAX_NUM_EXPERTS=1024`, `MAX_OUTER_INDICES=128`, `MAX_WIRE_SIZE=4807`, `routing_offsets`
  length `= e`), consistent with our existing `decode_dos`/`SPOT_CHECKS_MAX` posture.
- **Pearl-side flux.** V2 constants were placeholders on `feat/moe_hardfork` and finalized
  on master (#173). Re-pin the exact commit before writing KATs; ignore stale placeholders
  (e.g. old `PublicDataSizeV2=164` "mirrors V1", `MaxProofSizeV2=60000`).

---

## 6. Source pointers

**Pearl (`~/Dev/ai-pow/pearl`, `origin/master`):**
- `docs/moe-fork-upgrade-guide.md` — authoritative miner/pool guide.
- `zk-pow/src/api/proof.rs` — `MoEConfig`, `MoEParams`, `PrivateProofParams::s_routing`,
  `PublicProofParams`, commitment doc comments.
- `zk-pow/src/api/proof_utils.rs` — `commitment_hash` (the one-line delta),
  `compute_hash_activations`, `to_wire_bytes`/`from_wire_bytes`, wire constants,
  `public_data_commitment` (`"V2"` prefix).
- `zk-pow/src/api/sanity_checks.rs` — MoE validation constraint set + difficulty bound.
- `zk-pow/src/ffi/plain_proof.rs` — `CertificateVersion {ZkDense=1, ZkMoe=2}`,
  `OuterIndices`, `deserialize_compat`, `min_cert_version`, routing-strip extraction.
- `zk-pow/src/v1/…` — frozen V1 dense circuit.
- `miner/pearl-gemm/src/pearl_gemm/moe.py`, `csrc/moe/build_routing_data.{cu,cuh}` —
  routing canonicalization.
- `miner/miner-base/src/miner_base/commitment_hash.py` — router/activation commitment (ref).
- `node/wire/certificate*.go`, `node/chaincfg/params.go`, `node/blockchain/validate.go` —
  V2 envelope + fork height + strict cutover.

**Ours (`crates/ai-pow`, `crates/ai-pow-zk`):**
- `src/pearl_compat.rs` — `PearlMiningConfig` (trailer/`NonzeroReserved`),
  `PearlPublicProofParams` (164), `PEARL_MERGE_PUBLIC_STATEMENT_*`, periodic patterns.
- `src/fiat_shamir.rs` — `κ → s_B → s_A` chain; `noise_seed_a` is the B2 splice point.
- `src/matmul.rs` — `BlockNoise`, `TileState` fold, `compute_pattern_tile_trace_from_slices`
  (reused for the grouped tile).
- `tests/pearl_compat_fixtures.rs`, `tests/fixtures/pearl.rs` — S0–S9 byte-equivalence.
- `crates/ai-pow-zk/` — recursive certificate + circuit (B5).
