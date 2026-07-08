//! Puzzle parameters.
//!
//! Matches Pearl Whitepaper §4.1 (mining configuration) for the in-crate
//! synthetic-`A,B` setting: the protocol multiplies `(A + E) * (B + F)`
//! tile-by-tile, with `E = E_L · E_R` and `F = F_L · F_R` of rank
//! `noise_rank = r` (Pearl §4.4 Alg. 3). The noise rank is also the
//! accumulator stripe width for the Pearl iterative tile-state update
//! (Pearl §4.5 Alg. 4), so `r | k` is required.
//!
//! Difficulty is expressed in log-bits via `difficulty_bits = b` so the
//! hardness condition matches Pearl §4.5:
//!   BLAKE3(M, key = pow_key_for_nonce(s_a, nonce))  <=  2^(256 - b) * r * t_m * t_n
//! (with square tiles, `t_m = t_n = tile`).
//! `s_a` is nonce-bound before noise generation and matmul; changing the nonce
//! changes the commitments, noise, and tile states before this final hash.
//!
//! # Pearl §4.8 envelope (the Pearl-faithful PROD path, "γ")
//!
//! The Pearl whitepaper §4.8 ("Supported PoW Parameters") *caps* the
//! mining parameters so that **one opened tile's proof always fits a
//! single STARK** — Pearl deliberately never segments (see
//! `crates/ai-pow-zk/docs/2026-05-17_M_S2_PEARL_EVALUATION.md`). We adopt that
//! envelope here, split into two layers:
//!
//! * [`MatmulParams::validate`] enforces the **universal** Pearl §4.8
//!   trace bound `k·(h+w) ≤ 2²²` (the verifier's restriction; with
//!   square tiles `h = w = tile`, this is `k·2·tile ≤ 2²²`). This is
//!   the *one-tile-one-STARK* guarantee and holds for **every**
//!   accepted puzzle, test or production — it is why no segmentation
//!   (G3) is needed within the envelope. Called by `verifier::verify`
//!   and `prover` already.
//! * [`MatmulParams::validate_prod_envelope`] additionally enforces
//!   the §4.8 **security** caps (`16r ≤ k ≤ 4r²`, `r ∈ {2⁵..2¹⁰}`,
//!   `64 | k`, `m,n ≤ 2²⁴`, `h·w ≥ 32`). This is the **consensus
//!   admission rule** — a real protocol puzzle MUST satisfy it.
//!   Small in-crate test profiles (e.g. [`MatmulParams::TEST_SMALL`],
//!   `r = 4`) are intentionally *below* this envelope: they exercise
//!   the circuit machinery fast and are **not** consensus-valid by
//!   design. The future consensus/block-admission layer (M-C1) calls
//!   `validate_prod_envelope`.

use thiserror::Error;

/// Pearl §4.8 *whitepaper* trace proxy `k·(h+w) ≤ 2²²` (square
/// tiles ⇒ `h = w = tile`). NOTE: this is the whitepaper's stated
/// bound, but it is NOT the quantity that bounds the Layer-0 trace —
/// Pearl's reference `expected_num_rows` scales as `h·w·(k/r)`, not
/// `k·(h+w)`. The cap that actually keeps one tile in one STARK is
/// the reference prover's per-tile `h·w ≤ 256` ([`PEARL_HW_MAX`]).
pub const PEARL_TRACE_BOUND: u64 = 1 << 22;
/// Pearl §4.8 common-dimension cap `k ≤ 2¹⁶`.
pub const PEARL_K_MAX: u32 = 1 << 16;
/// Pearl §4.8 noise-rank range `r ∈ {2⁵, …, 2¹⁰}` (32 ≤ r ≤ 1024).
pub const PEARL_R_MIN: u32 = 1 << 5;
pub const PEARL_R_MAX: u32 = 1 << 10;
/// Pearl §4.8 matrix-dimension cap `m, n ≤ 2²⁴`.
pub const PEARL_MN_MAX: u32 = 1 << 24;
/// Pearl §4.8 entropy floor `h·w ≥ 32` (sufficient entropy in `M`).
pub const PEARL_HW_MIN: u64 = 32;
/// Pearl per-tile cap `h·w ≤ 256`. NOT in the whitepaper §4.8 text,
/// but hard-enforced by Pearl's reference prover
/// (`structure_matmul_in_stark` in `pearl/zk-pow/src/circuit/
/// pearl_program.rs`: `ensure!(h * w <= 256)`). Because the real
/// Layer-0 trace scales as `h·w·(k/r)`, this — not the whitepaper's
/// `k·(h+w)` proxy — is the cap that actually keeps one opened tile
/// in one STARK. Square tiles ⇒ `tile² ≤ 256`, i.e. `tile ≤ 16`.
pub const PEARL_HW_MAX: u64 = 256;

/// H2 (DoS audit) — universal cap on `spot_checks`. `verifier::verify`
/// iterates `spot_checks` times, each iteration re-hashing an
/// up-to-2-MiB strip + a Merkle path. Without a cap, a crafted block
/// with a huge `spot_checks` drives a CPU-time DoS. Production Pearl
/// uses `sigma = 80`; **256** is ~3× headroom — generous for any
/// realistic protocol choice and small enough to keep a single
/// `verify` call bounded (at most ~5–10 s under the worst-case
/// `validate()`-allowed `t·k`; sub-second on the production envelope).
pub const SPOT_CHECKS_MAX: u32 = 256;

/// §6(b) in-circuit matmul-sweep stripe capacity. This mirrors
/// `ai-pow-zk::composite_layout::STRIPE_MAX`, but lives here so the
/// `ai-pow` consensus parameter envelope does not require the optional
/// `zk` dependency.
pub const STRIPE_MAX: usize = 64;

/// Parameters of a Pearl-style matmul PoW puzzle.
///
/// Matmul shape is `(m, k) * (k, n) = (m, n)`. Tiles are square `tile x tile`.
/// `noise_rank` is the rank `r` of the low-rank noise factors **and** the
/// inner-accumulator stripe width. `difficulty_bits` is Pearl's `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatmulParams {
    pub m: u32,
    pub k: u32,
    pub n: u32,
    /// Noise rank `r`. Also the accumulator stripe width. The lenient
    /// [`validate`](Self::validate) requires `2 <= r <= k`, `r | k`,
    /// and `r` a power of two (so small test profiles stay valid).
    /// The Pearl §4.8 security band (`r ∈ {2⁵..2¹⁰}`,
    /// `16r <= k <= 4r²`) is enforced by
    /// [`validate_prod_envelope`](Self::validate_prod_envelope) — the
    /// consensus admission rule.
    pub noise_rank: u32,
    pub tile: u32,
    pub spot_checks: u32,
    /// Logarithmic difficulty `b` (Pearl §4.5). A tile is accepted when
    /// `BLAKE3(M, key = pow_key_for_nonce(s_a, nonce)) <= 2^(256 - b) * r * t^2`.
    /// The `s_a` seed is already nonce-bound through the per-attempt
    /// commitment hash. `b = 0` accepts every tile; values above 256 reject
    /// everything.
    pub difficulty_bits: u32,
}

impl MatmulParams {
    /// Default test profile — small enough to run end-to-end in milliseconds.
    /// Picks `r = 4` so `16r = 64 = k` (Pearl-recommended lower bound).
    pub const TEST_SMALL: Self = Self {
        m: 64,
        k: 64,
        n: 64,
        noise_rank: 4,
        tile: 8,
        spot_checks: 8,
        difficulty_bits: 0,
    };

    /// Production profile: 4096^3 INT8 matmul, 8-tile, 80 spot checks.
    /// `r = 64` so `16r = 1024 <= k = 4096 <= 4r^2 = 16384` (Pearl §4.8 OK).
    /// `tile = 8` ⇒ `h·w = 64` — Pearl's reference `default_mining_config`
    /// tile, well inside the `h·w ≤ 256` cap ([`PEARL_HW_MAX`]); the prior
    /// `tile = 128` (`h·w = 16384`) was 256× over what Pearl mines per
    /// block. `tile = 16` (`h·w = 256`, the cap) is also envelope-valid —
    /// either config is supported; 8 is the lower-latency default.
    pub const PROD: Self = Self {
        m: 4096,
        k: 4096,
        n: 4096,
        noise_rank: 64,
        tile: 8,
        spot_checks: 80,
        difficulty_bits: 0,
    };

    /// Gemma 4 31B FFN gate / up matmul: `(B=4096, hidden=5376, intermediate=21504)`.
    /// `r = 128` (`128 | 5376`; `num_stripes = k/r = 42 ≤ STRIPE_MAX`
    /// ⇒ the §6(b) matmul is proven in-circuit).
    pub const GEMMA_4_31B_FFN: Self = Self {
        m: 4096,
        k: 5376,
        n: 21504,
        noise_rank: 128,
        tile: 8,
        spot_checks: 80,
        difficulty_bits: 0,
    };

    /// Qwen 3.6 27B FFN gate / up matmul: `(B=4096, hidden=5120, intermediate=17408)`.
    pub const QWEN_3_6_27B_FFN: Self = Self {
        m: 4096,
        k: 5120,
        n: 17408,
        noise_rank: 128,
        tile: 8,
        spot_checks: 80,
        difficulty_bits: 0,
    };

    /// **Real shipped Pearl-certified model** —
    /// `pearl-ai/Llama-3.1-8B-Instruct-pearl` (run via the Pearl
    /// vLLM mining plugin; `~/Dev/Llama-3.1-8B-Instruct-pearl`).
    /// `hidden_size = 4096`, `intermediate_size = 14336`, 32
    /// layers; `quant_method = "pearl"` (group_1 weights **7-bit
    /// int**, activations 7-bit per-token ⇒ Pearl §4.1's
    /// `[−64,64]` int regime). These are the two binding FFN
    /// GEMMs the miner actually proves (the largest committed
    /// weights, ≈58.7M params = 57 344 BLAKE3 chunks each — far
    /// past any synthetic preset; this is the real P-B.2
    /// motivation). Both satisfy [`validate_prod_envelope`] with
    /// `r = 64`. (`q/o_proj` k=n=4096, `kv_proj` n=1024 are
    /// strictly smaller and also in-envelope.)
    pub const LLAMA_3_1_8B_GATE_UP: Self = Self {
        m: 4096,
        k: 4096,  // hidden_size
        n: 14336, // intermediate_size
        noise_rank: 64,
        tile: 8,
        spot_checks: 80,
        difficulty_bits: 0,
    };

    /// `Llama-3.1-8B-Instruct-pearl` `down_proj` **shape**
    /// (`k = intermediate_size = 14336`, the largest-`k` GEMM).
    ///
    /// ⚠️ **NOT consensus-mineable for this model.** Per the
    /// verified `config.json`, `down_proj` is in **group_0
    /// (FP8 block[128,128])** — Pearl §4.1 type-0 is INT only and
    /// the FP PoUW is unshipped (§1.1), so this layer is **out of
    /// production scope** (Phase B / B3; the
    /// [`LlamaFfnLayer::DownProj`] guard rejects it via
    /// [`ParamError::Fp8LayerNotMineable`]). This preset is
    /// retained **only as a §4.8 trace-bound *sizing reference***
    /// (the binding largest-`k` envelope-math case: `16r ≤ k ≤
    /// 4r²` with `r = 256` ⇒ `4096 ≤ 14336 ≤ 262144` ✓; `256 |
    /// 14336` ✓; `num_stripes = k/r = 56 ≤ STRIPE_MAX` ✓;
    /// `k·2·tile = 229 376 ≤ 2²²` ✓) — it is a valid
    /// `validate_prod_envelope` *shape*, but mining it for this
    /// model would prove an FP8 layer, which production must not
    /// do. The mineable group_1 INT7 GEMMs are
    /// [`LLAMA_3_1_8B_GATE_UP`](Self::LLAMA_3_1_8B_GATE_UP) +
    /// `o_proj`/late-`qkv` (see [`LlamaFfnLayer`]).
    pub const LLAMA_3_1_8B_DOWN: Self = Self {
        m: 4096,
        k: 14336, // intermediate_size
        n: 4096,  // hidden_size
        noise_rank: 256,
        tile: 8,
        spot_checks: 80,
        difficulty_bits: 0,
    };

    /// Generic LLM-FFN profile builder. `batch_seq` is the M dimension (the
    /// product of mini-batch and sequence length the GEMM kernel sees);
    /// `hidden` and `intermediate` are the two model dimensions for the FFN
    /// gate / up matmul. Picks `tile = 8`, `r = 64`, `sigma = 80`.
    pub const fn llm_ffn(hidden: u32, intermediate: u32, batch_seq: u32) -> Self {
        Self {
            m: batch_seq,
            k: hidden,
            n: intermediate,
            noise_rank: 64,
            tile: 8,
            spot_checks: 80,
            difficulty_bits: 0,
        }
    }

    pub fn validate(&self) -> Result<(), ParamError> {
        if self.tile == 0 {
            return Err(ParamError::ZeroTile);
        }
        if self.m % self.tile != 0 || self.n % self.tile != 0 {
            return Err(ParamError::TileDoesNotDivide);
        }
        let row_tiles = self.m / self.tile;
        let col_tiles = self.n / self.tile;
        let total = (row_tiles as u64) * (col_tiles as u64);
        if total == 0 {
            return Err(ParamError::ZeroTiles);
        }
        // Tile indices (`found_idx`, the spot-check challenge indices)
        // are u32-addressed throughout the proof path. `total` is
        // computed in u64 — so this check itself cannot overflow — but
        // `num_tiles()` returning a value past `u32::MAX` could not be
        // addressed by a proof. Reject such a puzzle outright.
        if total > u32::MAX as u64 {
            return Err(ParamError::TooManyTiles);
        }
        // Pearl §4.8 caps k at 2^16. With `|A| <= 64`, `|E| <= 63`, the per-multiply
        // bound is (64+63)^2 = 16129 < 2^14, so `k * 16129 < 2^31` holds well past
        // Pearl's cap. Use Pearl's cap directly.
        if self.k == 0 || self.k > PEARL_K_MAX {
            return Err(ParamError::KOutOfRange);
        }
        // Pearl §4.8 **universal trace bound** `k·(h+w) ≤ 2²²`
        // (square tiles ⇒ `h = w = tile`). This is THE
        // one-tile-one-STARK guarantee: it bounds the Layer-0 trace
        // so a single opened tile always proves in one STARK — the
        // Pearl-faithful reason segmentation (G3) is unnecessary.
        // Holds for every accepted puzzle (test and production); the
        // §4.8 *security* caps are layered on in
        // `validate_prod_envelope`. See `2026-05-17_M_S2_PEARL_EVALUATION.md`.
        if self.pearl_trace_bound() > PEARL_TRACE_BOUND {
            return Err(ParamError::TraceBoundExceeded);
        }
        // Noise rank requirements: 1 <= r <= k and r | k.
        if self.noise_rank == 0 || self.noise_rank > self.k {
            return Err(ParamError::NoiseRankOutOfRange);
        }
        if self.k % self.noise_rank != 0 {
            return Err(ParamError::NoiseRankDoesNotDivideK);
        }
        // Pearl §4.4: each column of E_R has one +1 and one -1 at two
        // *distinct* positions; requires r >= 2.
        if self.noise_rank < 2 {
            return Err(ParamError::NoiseRankTooSmall);
        }
        // Pearl's permutation generator uses `rank_mask = r - 1` as a bitmask;
        // this is only well-formed for `r` a power of two
        // (`Pearl zk-pow pearl_noise.rs:107`).
        if !self.noise_rank.is_power_of_two() {
            return Err(ParamError::NoiseRankNotPowerOfTwo);
        }
        if self.spot_checks == 0 {
            return Err(ParamError::ZeroSpotChecks);
        }
        // H2 (DoS audit): hard cap on `spot_checks`. `verify` iterates
        // `spot_checks` times, each iteration re-hashing an up-to-2-MiB
        // strip — uncapped `spot_checks` ⇒ time-DoS on a crafted block.
        // Checked BEFORE the count check so the cap-violation is
        // reported precisely (rather than masked by `TooManySpotChecks`
        // on a small-tile-grid test config).
        if self.spot_checks > SPOT_CHECKS_MAX {
            return Err(ParamError::SpotChecksAboveDosCap);
        }
        if (self.spot_checks as u64) > total {
            return Err(ParamError::TooManySpotChecks);
        }
        Ok(())
    }

    /// Pearl §4.8 trace proxy `k·(h+w)`. Square tiles ⇒ `h = w =
    /// tile`, so this is `k·2·tile`. The Pearl whitepaper's verifier
    /// restricts this to `≤ 2²²` ([`PEARL_TRACE_BOUND`]); within that
    /// bound one opened tile always proves in a single STARK.
    pub fn pearl_trace_bound(&self) -> u64 {
        (self.k as u64) * (self.tile as u64 + self.tile as u64)
    }

    /// Pearl §4.8 **consensus admission rule** — the full Supported
    /// PoW Parameters envelope. A real protocol puzzle MUST satisfy
    /// this; in-crate sub-envelope test profiles
    /// ([`MatmulParams::TEST_SMALL`]) intentionally do not (they use
    /// the lenient [`validate`](Self::validate) for fast circuit
    /// tests and are not consensus-valid by design).
    ///
    /// Enforces, on top of [`validate`](Self::validate) (which
    /// already covers `k ≤ 2¹⁶`, `r | k`, `r` a power of two `≥ 2`,
    /// and the universal `k·(h+w) ≤ 2²²` trace bound):
    /// * `m, n ≤ 2²⁴`
    /// * `r ∈ {2⁵, …, 2¹⁰}` (32 ≤ r ≤ 1024)
    /// * `16r ≤ k ≤ 4r²` (the §4.8 security band)
    /// * `64 | k` (commitment-hash alignment)
    /// * `h·w ≥ 32` (entropy in `M`; square tiles ⇒ `tile² ≥ 32`)
    /// * `h·w ≤ 256` (Pearl reference-prover per-tile cap ⇒ `tile ≤ 16`)
    /// * `num_stripes = k/noise_rank ≤ STRIPE_MAX` (so the §6(b)
    ///   matmul sweep is proven in-circuit, not the off-circuit fallback)
    ///
    /// Within this envelope Pearl proves one opened tile in a single
    /// STARK — which is exactly why the Pearl-faithful PROD path
    /// needs no segmentation (`2026-05-17_M_S2_PEARL_EVALUATION.md`).
    pub fn validate_prod_envelope(&self) -> Result<(), ParamError> {
        self.validate()?;
        if self.m > PEARL_MN_MAX || self.n > PEARL_MN_MAX {
            return Err(ParamError::MatrixDimTooLarge);
        }
        if self.noise_rank < PEARL_R_MIN || self.noise_rank > PEARL_R_MAX {
            return Err(ParamError::NoiseRankOutOfEnvelope);
        }
        // 16r ≤ k ≤ 4r² (u64 throughout: r ≤ 1024 ⇒ 4r² ≤ 2²²).
        let r = self.noise_rank as u64;
        let k = self.k as u64;
        if k < 16 * r || k > 4 * r * r {
            return Err(ParamError::KOutOfSecurityBand);
        }
        if self.k % 64 != 0 {
            return Err(ParamError::KNotAlignedTo64);
        }
        // h·w ≥ 32 with square tiles (h = w = tile).
        if (self.tile as u64) * (self.tile as u64) < PEARL_HW_MIN {
            return Err(ParamError::TileEntropyTooLow);
        }
        // h·w ≤ 256 — Pearl's reference prover (`structure_matmul_in_stark`)
        // hard-rejects `h·w > 256`. The whitepaper §4.8 omits this cap, so
        // the prior envelope (whitepaper `k·(h+w) ≤ 2²²` only) wrongly
        // admitted `tile = 64` (`h·w = 4096`) — inflating the Layer-0
        // trace ~16× over what Pearl proves per block. See `PEARL_HW_MAX`.
        if (self.tile as u64) * (self.tile as u64) > PEARL_HW_MAX {
            return Err(ParamError::TileTooLarge);
        }
        // num_stripes = k / noise_rank must fit the §6(b) in-circuit
        // matmul-sweep capacity (`STRIPE_MAX` SX-register lanes). A
        // config with num_stripes > STRIPE_MAX falls back to the
        // off-circuit `compute_tile_trace` path where `sx_bound` is
        // false and the matmul→fold keystone is gated off — the
        // matmul would NOT be proven in-circuit. The consensus
        // envelope rejects such configs outright (raise noise_rank
        // so k/r <= STRIPE_MAX). The real Llama mineable GEMMs have
        // k/r = 4096/64 = 64 = STRIPE_MAX ⇒ in-circuit.
        if (self.num_stripes() as usize) > STRIPE_MAX {
            return Err(ParamError::TooManyStripes);
        }
        Ok(())
    }

    pub fn row_tiles(&self) -> u32 {
        self.m / self.tile
    }
    pub fn col_tiles(&self) -> u32 {
        self.n / self.tile
    }
    /// Total tile count. Returns `u64`: `row_tiles · col_tiles` can
    /// exceed `u32::MAX` for large in-§4.8-envelope matrices
    /// (`m, n ≤ 2²⁴`), so a `u32` product would silently overflow
    /// (release) or panic (debug). Computed in `u64`.
    pub fn num_tiles(&self) -> u64 {
        (self.row_tiles() as u64) * (self.col_tiles() as u64)
    }
    /// Number of leaves in the padded Merkle tree (next power of two of
    /// `num_tiles`). `u64` for the same reason as [`num_tiles`].
    pub fn num_tiles_padded(&self) -> u64 {
        self.num_tiles().next_power_of_two()
    }
    pub fn tile_index(&self, i: u32, j: u32) -> u64 {
        (i as u64) * (self.col_tiles() as u64) + (j as u64)
    }
    pub fn tile_coords(&self, idx: u64) -> (u32, u32) {
        let cols = self.col_tiles() as u64;
        ((idx / cols) as u32, (idx % cols) as u32)
    }
    /// Number of accumulator stripes per tile (`⌊k / r⌋`). Each stripe folds
    /// one update into the 512-bit `M` state.
    pub fn num_stripes(&self) -> u32 {
        self.k / self.noise_rank
    }
}

// ───────────────────────────────────────────────────────────────
//  Phase B / B3 — INT-only production scoping (machine-enforced)
//
//  `pearl-ai/Llama-3.1-8B-Instruct-pearl` `config.json`
//  `quantization_config` (verified 2026-05-18) has two
//  `config_groups`:
//
//    group_1  int-quantized, 7-bit, weights per-CHANNEL /
//             activations per-TOKEN — targets:
//               re:.*self_attn\.o_proj$
//               re:.*\.gate_proj$
//               re:.*\.up_proj$
//               re:model\.layers\.(1[6-9]|2[0-9]|3[01])\.self_attn\.[qkv]_proj$
//    group_0  float-quantized, FP8 block[128,128] — targets:
//               re:.*\.down_proj$
//               re:model\.layers\.([0-9]|1[0-5])\.self_attn\.[qkv]_proj$
//
//  Pearl whitepaper §4.1 fixes matmul-accumulate **type-0 = INT
//  only** (`[−64,64]`, int32 accumulate); §1.1 defers an FP PoUW
//  to an UNSHIPPED upgrade. ⇒ production mines group_1's INT7
//  GEMMs ONLY; group_0 (FP8) is a documented production
//  limitation, machine-enforced here (DB-3(a) /
//  `2026-05-18_PHASE_B_DESIGN.md` §3/§7). This is the in-repo admission
//  guard mirroring `validate_prod_envelope`; the vLLM plugin
//  (Phase D, external) is the operational filter on top.
// ───────────────────────────────────────────────────────────────

/// The model's `quantization_config` group for a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantGroup {
    /// group_1 — INT7 (Pearl §4.1 type-0 `[−64,64]` int regime).
    /// **Mineable.**
    Int7Mined,
    /// group_0 — FP8 block-quantized. **Not mineable** until
    /// Pearl ships its FP PoUW (§1.1, unshipped).
    Fp8Deferred,
}

/// The Llama-3.1-8B mining-relevant linear layers, classified by
/// the verified `config.json` regex targets. `layer_idx ∈ 0..32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlamaFfnLayer {
    /// `mlp.gate_proj` — group_1 INT7 (mined). [K=4096, N=14336].
    GateProj,
    /// `mlp.up_proj` — group_1 INT7 (mined). [K=4096, N=14336].
    UpProj,
    /// `self_attn.o_proj` — group_1 INT7 (mined). [K=4096, N=4096].
    OProj,
    /// `self_attn.{q,k,v}_proj` at `layer_idx` — group depends on
    /// the layer index (16..=31 ⇒ group_1 INT7 mined; 0..=15 ⇒
    /// group_0 FP8 deferred), per the config regexes.
    AttnQkv { layer_idx: u32, which: QkvProj },
    /// `mlp.down_proj` — **group_0 FP8 (deferred — NOT mined)**.
    /// [K=14336, N=4096]. The `LLAMA_3_1_8B_DOWN` preset is this
    /// layer's *shape* (kept only for §4.8 trace-bound sizing
    /// math); it is **not** consensus-mineable for this model.
    DownProj,
}

/// `q`/`k`/`v` projection selector (for `AttnQkv`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QkvProj {
    Q,
    K,
    V,
}

impl LlamaFfnLayer {
    /// The `quantization_config` group, faithful to the verified
    /// `config.json` regex targets. `gate/up/o_proj` are always
    /// group_1; `down_proj` always group_0; attention `qkv` is
    /// group_1 iff `layer_idx ∈ 16..=31` else group_0.
    pub fn quant_group(&self) -> QuantGroup {
        match self {
            LlamaFfnLayer::GateProj | LlamaFfnLayer::UpProj | LlamaFfnLayer::OProj => {
                QuantGroup::Int7Mined
            }
            LlamaFfnLayer::DownProj => QuantGroup::Fp8Deferred,
            LlamaFfnLayer::AttnQkv { layer_idx, .. } => {
                if (16..=31).contains(layer_idx) {
                    QuantGroup::Int7Mined
                } else {
                    QuantGroup::Fp8Deferred
                }
            }
        }
    }

    /// `true` iff this layer is mined in production (group_1 INT7).
    pub fn is_mineable(&self) -> bool {
        self.quant_group() == QuantGroup::Int7Mined
    }

    /// The mineable [`MatmulParams`] for this layer, or
    /// [`ParamError::Fp8LayerNotMineable`] if it is group_0 (FP8).
    /// **This is the B3 machine guard:** a caller that tries to
    /// mine an FP8 layer is rejected here, in-repo, before any
    /// proving. Shapes: `hidden=4096`, `intermediate=14336`,
    /// GQA `q=4096`, `k=v=1024` (8 KV heads × 128); `M` =
    /// `batch_seq` (the GEMM's batched-token dimension). Returned
    /// params are `validate_prod_envelope`-valid.
    pub fn mineable_matmul_params(&self, batch_seq: u32) -> Result<MatmulParams, ParamError> {
        if self.quant_group() == QuantGroup::Fp8Deferred {
            return Err(ParamError::Fp8LayerNotMineable);
        }
        // group_1 INT7 mined GEMMs (hidden=4096, intermediate=14336).
        let (k, n) = match self {
            LlamaFfnLayer::GateProj | LlamaFfnLayer::UpProj => (4096, 14336),
            LlamaFfnLayer::OProj => (4096, 4096),
            LlamaFfnLayer::AttnQkv { which, .. } => match which {
                QkvProj::Q => (4096, 4096),
                QkvProj::K | QkvProj::V => (4096, 1024),
            },
            // Unreachable: DownProj is Fp8Deferred (rejected above).
            LlamaFfnLayer::DownProj => unreachable!("down_proj is FP8"),
        };
        let p = MatmulParams {
            m: batch_seq,
            k,
            n,
            noise_rank: 64,
            tile: 8,
            spot_checks: 80,
            difficulty_bits: 0,
        };
        p.validate_prod_envelope()?;
        Ok(p)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParamError {
    #[error("tile size must be > 0")]
    ZeroTile,
    #[error("tile must divide m and n")]
    TileDoesNotDivide,
    #[error("(m/t)*(n/t) must be > 0")]
    ZeroTiles,
    #[error("k must be in 1..=2^16 (Pearl §4.8)")]
    KOutOfRange,
    #[error("k·(h+w) must be <= 2^22 (Pearl §4.8 universal trace bound — one-tile-one-STARK)")]
    TraceBoundExceeded,
    #[error("m and n must be <= 2^24 (Pearl §4.8 envelope)")]
    MatrixDimTooLarge,
    #[error("noise_rank must be in {{2^5..=2^10}} = 32..=1024 (Pearl §4.8 envelope)")]
    NoiseRankOutOfEnvelope,
    #[error("k must satisfy 16r <= k <= 4r^2 (Pearl §4.8 security band)")]
    KOutOfSecurityBand,
    #[error("k must be a multiple of 64 (Pearl §4.8 commitment-hash alignment)")]
    KNotAlignedTo64,
    #[error("tile^2 (= h·w) must be >= 32 (Pearl §4.8 entropy floor for M)")]
    TileEntropyTooLow,
    #[error("tile^2 (= h·w) must be <= 256 (Pearl reference-prover per-tile cap)")]
    TileTooLarge,
    #[error(
        "num_stripes (= k / noise_rank) must be <= STRIPE_MAX — the §6(b) \
         in-circuit matmul-sweep capacity; a larger value forces the \
         unsound off-circuit fallback (matmul not proven in-circuit). \
         Raise noise_rank so k/r <= STRIPE_MAX."
    )]
    TooManyStripes,
    #[error("noise_rank must be in 1..=k")]
    NoiseRankOutOfRange,
    #[error("noise_rank must divide k")]
    NoiseRankDoesNotDivideK,
    #[error("noise_rank must be >= 2 (Pearl §4.4 ChoiceMatrix requires two distinct positions)")]
    NoiseRankTooSmall,
    #[error("noise_rank must be a power of two (Pearl permutation bitmask requirement)")]
    NoiseRankNotPowerOfTwo,
    #[error("spot_checks must be > 0")]
    ZeroSpotChecks,
    #[error("spot_checks must be <= number of tiles")]
    TooManySpotChecks,
    #[error(
        "spot_checks exceeds the verifier DoS cap (SPOT_CHECKS_MAX = 256) — \
         a larger value would let a crafted block drive a CPU-time DoS \
         in `verifier::verify`'s per-opening loop"
    )]
    SpotChecksAboveDosCap,
    #[error(
        "tile count (m/t)·(n/t) must be <= u32::MAX — tile indices \
         (found_idx, spot-check challenge indices) are u32-addressed \
         throughout the proof path"
    )]
    TooManyTiles,
    #[error(
        "layer is in the FP8 quant group (Pearl §4.1 type-0 is INT only; \
         the FP PoUW is unshipped — Pearl whitepaper §1.1): not mineable"
    )]
    Fp8LayerNotMineable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        MatmulParams::TEST_SMALL.validate().unwrap();
        MatmulParams::PROD.validate().unwrap();
        MatmulParams::GEMMA_4_31B_FFN.validate().unwrap();
        MatmulParams::QWEN_3_6_27B_FFN.validate().unwrap();
    }

    #[test]
    fn rejects_bad_params() {
        let mut p = MatmulParams::TEST_SMALL;
        p.tile = 0;
        assert_eq!(p.validate(), Err(ParamError::ZeroTile));

        p = MatmulParams::TEST_SMALL;
        p.tile = 7;
        assert_eq!(p.validate(), Err(ParamError::TileDoesNotDivide));

        p = MatmulParams::TEST_SMALL;
        p.k = (1 << 16) + 1;
        assert_eq!(p.validate(), Err(ParamError::KOutOfRange));

        p = MatmulParams::TEST_SMALL;
        p.spot_checks = 0;
        assert_eq!(p.validate(), Err(ParamError::ZeroSpotChecks));

        p = MatmulParams::TEST_SMALL;
        p.spot_checks = (p.num_tiles() + 1) as u32;
        assert_eq!(p.validate(), Err(ParamError::TooManySpotChecks));

        // Noise rank must divide k.
        p = MatmulParams::TEST_SMALL;
        p.noise_rank = 5; // 5 does not divide 64
        assert_eq!(p.validate(), Err(ParamError::NoiseRankDoesNotDivideK));

        // Noise rank cannot be 1 (ChoiceMatrix needs two distinct positions).
        p = MatmulParams::TEST_SMALL;
        p.noise_rank = 1;
        assert_eq!(p.validate(), Err(ParamError::NoiseRankTooSmall));
    }

    #[test]
    fn coord_round_trip() {
        let p = MatmulParams::TEST_SMALL;
        for idx in 0..p.num_tiles() {
            let (i, j) = p.tile_coords(idx);
            assert_eq!(p.tile_index(i, j), idx);
        }
    }

    #[test]
    fn rectangular_non_pow2_validates() {
        // (m/t, n/t) = (8, 12) -> 96 tiles; not a power of two.
        // r = 4 divides k = 64.
        let p = MatmulParams {
            m: 64,
            k: 64,
            n: 96,
            noise_rank: 4,
            tile: 8,
            spot_checks: 8,
            difficulty_bits: 0,
        };
        p.validate().unwrap();
        assert_eq!(p.num_tiles(), 96);
        assert_eq!(p.num_tiles_padded(), 128);
    }

    /// H1 (DoS/overflow audit): `num_tiles()` must compute the tile
    /// count in `u64`. For an in-§4.8-envelope matrix (`m, n = 2²⁴`)
    /// the product `row_tiles · col_tiles` is `2⁴²` — a `u32` multiply
    /// would panic (debug) or silently wrap to a wrong, smaller count
    /// (release). `validate()` must in turn *reject* such a puzzle:
    /// tile indices (`found_idx`, the challenge indices) are
    /// u32-addressed end-to-end, so a count past `u32::MAX` is
    /// unrepresentable.
    #[test]
    fn num_tiles_no_overflow_then_rejected() {
        // Envelope-max square matrix; tile = 8 (smallest entropy-valid).
        let mut p = MatmulParams::TEST_SMALL;
        p.m = 1 << 24;
        p.n = 1 << 24;
        p.tile = 8;
        // row_tiles = col_tiles = 2²¹ ⇒ num_tiles = 2⁴² (exact, u64).
        assert_eq!(p.row_tiles(), 1 << 21);
        assert_eq!(p.col_tiles(), 1 << 21);
        assert_eq!(p.num_tiles(), 1u64 << 42);
        // tile_index over the whole grid must not overflow either.
        assert_eq!(
            p.tile_index(p.row_tiles() - 1, p.col_tiles() - 1),
            (1u64 << 42) - 1,
        );
        // A puzzle with > u32::MAX tiles is unaddressable ⇒ rejected.
        assert_eq!(p.validate(), Err(ParamError::TooManyTiles));

        // Boundary: exactly u32::MAX + 1 tiles ⇒ rejected.
        let mut at_boundary = MatmulParams::TEST_SMALL;
        at_boundary.m = 1 << 16;
        at_boundary.n = 1 << 16;
        at_boundary.tile = 1;
        assert_eq!(at_boundary.num_tiles(), 1u64 << 32);
        assert_eq!(at_boundary.validate(), Err(ParamError::TooManyTiles));

        // Just under the cap: the count gate must NOT fire (later
        // structural checks may still reject — only the count gate is
        // under test here).
        let mut under = MatmulParams::TEST_SMALL;
        under.m = 1 << 16;
        under.n = (1 << 16) - 1;
        under.tile = 1;
        assert!(under.num_tiles() <= u64::from(u32::MAX));
        assert_ne!(under.validate(), Err(ParamError::TooManyTiles));
    }

    /// H2 (DoS audit): `validate()` rejects `spot_checks` past the
    /// hard cap (`SPOT_CHECKS_MAX = 256`). `verifier::verify` iterates
    /// `spot_checks` times re-hashing up-to-2-MiB strips — uncapped
    /// `spot_checks` ⇒ time-DoS on a crafted block.
    #[test]
    fn validate_rejects_spot_checks_above_dos_cap() {
        let mut p = MatmulParams::TEST_SMALL;
        p.spot_checks = SPOT_CHECKS_MAX + 1;
        assert_eq!(p.validate(), Err(ParamError::SpotChecksAboveDosCap));

        // At the cap: the DoS gate must NOT fire (the count-vs-tiles
        // check may still bite when num_tiles is small — that is fine,
        // it's not the DoS gate).
        let mut at_cap = MatmulParams::TEST_SMALL;
        at_cap.spot_checks = SPOT_CHECKS_MAX;
        assert_ne!(at_cap.validate(), Err(ParamError::SpotChecksAboveDosCap));
    }

    #[test]
    fn llm_profiles_have_padded_merkle() {
        let p = MatmulParams::GEMMA_4_31B_FFN;
        assert_eq!(p.row_tiles(), 512); // 4096 / 8
        assert_eq!(p.col_tiles(), 2688); // 21504 / 8
        assert_eq!(p.num_tiles(), 512 * 2688);
        assert!(!p.num_tiles().is_power_of_two());
        assert_eq!(p.num_tiles_padded(), p.num_tiles().next_power_of_two());
    }

    #[test]
    fn num_stripes_matches_k_over_r() {
        let p = MatmulParams::TEST_SMALL;
        assert_eq!(p.num_stripes(), p.k / p.noise_rank);
        assert_eq!(MatmulParams::PROD.num_stripes(), 4096 / 64);
    }

    // ─────────────── P-A: Pearl §4.8 envelope (γ path) ───────────────

    /// Every production / LLM preset satisfies the full Pearl §4.8
    /// consensus envelope.
    #[test]
    fn prod_presets_satisfy_envelope() {
        for p in [
            MatmulParams::PROD,
            MatmulParams::GEMMA_4_31B_FFN,
            MatmulParams::QWEN_3_6_27B_FFN,
            MatmulParams::llm_ffn(4096, 11008, 4096),
            // Real shipped Pearl-certified model (the production
            // target, not a synthetic guess).
            MatmulParams::LLAMA_3_1_8B_GATE_UP,
            MatmulParams::LLAMA_3_1_8B_DOWN,
        ] {
            p.validate_prod_envelope()
                .unwrap_or_else(|e| panic!("{p:?} not in §4.8 envelope: {e}"));
            // …and therefore trivially within the one-STARK bound.
            assert!(p.pearl_trace_bound() <= PEARL_TRACE_BOUND);
        }
    }

    /// TEST_SMALL is intentionally a *sub-envelope* circuit-test
    /// profile: it passes the lenient structural `validate()` (and
    /// the universal trace bound) but is NOT consensus-valid (`r =
    /// 4 < 32`). This split is by design (see module docs).
    #[test]
    fn test_small_is_below_consensus_envelope() {
        MatmulParams::TEST_SMALL.validate().unwrap();
        assert!(MatmulParams::TEST_SMALL.pearl_trace_bound() <= PEARL_TRACE_BOUND);
        assert_eq!(
            MatmulParams::TEST_SMALL.validate_prod_envelope(),
            Err(ParamError::NoiseRankOutOfEnvelope),
        );
    }

    /// The universal `k·(h+w) ≤ 2²²` trace bound is enforced by the
    /// plain `validate()` (so `verifier::verify`/`prover` already
    /// reject un-provable puzzles) — the one-tile-one-STARK
    /// guarantee, holding for test and prod alike.
    #[test]
    fn universal_trace_bound_enforced_by_validate() {
        // k = 2^16, tile = 64 ⇒ k·2·tile = 2^23 > 2^22.
        let p = MatmulParams {
            m: 64,
            k: 1 << 16,
            n: 64,
            noise_rank: 64,
            tile: 64,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        assert_eq!(p.pearl_trace_bound(), 1 << 23);
        assert_eq!(p.validate(), Err(ParamError::TraceBoundExceeded));
        // Halving the tile brings it back to exactly the bound.
        let ok = MatmulParams {
            tile: 32,
            m: 64,
            n: 64,
            ..p
        };
        assert_eq!(ok.pearl_trace_bound(), PEARL_TRACE_BOUND);
        ok.validate().unwrap();
    }

    /// **Envelope ⇒ one-STARK theorem.** Any params accepted by
    /// `validate_prod_envelope` necessarily satisfies the universal
    /// `k·(h+w) ≤ 2²²` bound (it is checked inside the `validate`
    /// the envelope delegates to). Swept over the whole §4.8
    /// security band, the strongest in-envelope load still proves in
    /// one STARK — which is exactly why the Pearl-faithful path
    /// needs no segmentation.
    #[test]
    fn envelope_implies_one_stark_bound() {
        let mut checked = 0u32;
        for r_log in 5..=10u32 {
            let r = 1u32 << r_log; // 32..=1024
            for &kf in &[16u64, 32, 64, 256, 1024] {
                let k64 = kf * r as u64;
                if k64 == 0 || k64 > PEARL_K_MAX as u64 || k64 > 4 * (r as u64) * (r as u64) {
                    continue;
                }
                let k = k64 as u32;
                if k % 64 != 0 || k % r != 0 {
                    continue;
                }
                // Largest tile that still respects the trace bound,
                // rounded to divide m=n; tile≥6 for the entropy floor.
                for &tile in &[8u32, 16, 32, 64, 128] {
                    let p = MatmulParams {
                        m: tile * 4,
                        k,
                        n: tile * 4,
                        noise_rank: r,
                        tile,
                        spot_checks: 1,
                        difficulty_bits: 0,
                    };
                    match p.validate_prod_envelope() {
                        Ok(()) => {
                            assert!(
                                p.pearl_trace_bound() <= PEARL_TRACE_BOUND,
                                "{p:?} in envelope but trace {} > 2^22",
                                p.pearl_trace_bound()
                            );
                            checked += 1;
                        }
                        // Out-of-envelope combos are fine to skip; the
                        // theorem is only about the Ok arm.
                        Err(_) => {}
                    }
                }
            }
        }
        assert!(checked > 0, "swept no in-envelope params — sweep bug");
    }

    /// Each §4.8 *security* cap rejects with its specific error
    /// (built by perturbing the known-good PROD preset minimally).
    #[test]
    fn envelope_rejects_each_security_violation() {
        // m too large (still tile-aligned: 2^24 and 128 are % 16 == 0).
        let p = MatmulParams {
            m: (1 << 24) + 128,
            ..MatmulParams::PROD
        };
        assert_eq!(
            p.validate_prod_envelope(),
            Err(ParamError::MatrixDimTooLarge)
        );

        // r below the {2^5..2^10} band (16 | 4096, pow2, but < 32).
        let p = MatmulParams {
            noise_rank: 16,
            ..MatmulParams::PROD
        };
        assert_eq!(
            p.validate_prod_envelope(),
            Err(ParamError::NoiseRankOutOfEnvelope)
        );

        // k outside the 16r..=4r² band (r=64 ⇒ band [1024, 16384];
        // k=512 is below it; 512%64==0, 64|512, trace ok).
        let p = MatmulParams {
            k: 512,
            ..MatmulParams::PROD
        };
        assert_eq!(
            p.validate_prod_envelope(),
            Err(ParamError::KOutOfSecurityBand)
        );

        // k not aligned to 64 (r=32, k=544: 32|544, in band [512,4096],
        // 544%64==32≠0).
        let p = MatmulParams {
            noise_rank: 32,
            k: 544,
            ..MatmulParams::PROD
        };
        assert_eq!(p.validate_prod_envelope(), Err(ParamError::KNotAlignedTo64));

        // tile entropy floor: tile²<32 (tile=4 ⇒ 16<32; m,n%4==0,
        // k=512,r=32 in band, 512%64==0).
        let p = MatmulParams {
            tile: 4,
            m: 512,
            n: 512,
            k: 512,
            noise_rank: 32,
            ..MatmulParams::PROD
        };
        assert_eq!(
            p.validate_prod_envelope(),
            Err(ParamError::TileEntropyTooLow)
        );
    }

    /// `validate_prod_envelope` is strictly stronger than
    /// `validate`: anything it accepts, `validate` accepts. All
    /// production presets have `num_stripes = k/r ≤ STRIPE_MAX`
    /// (PROD/Llama r=64⇒64; GEMMA r=128⇒42; QWEN r=128⇒40) ⇒ the
    /// §6(b) matmul is proven in-circuit for every one.
    #[test]
    fn envelope_implies_validate() {
        for p in [
            MatmulParams::PROD,
            MatmulParams::GEMMA_4_31B_FFN,
            MatmulParams::QWEN_3_6_27B_FFN,
        ] {
            assert!(p.validate_prod_envelope().is_ok());
            assert!(p.validate().is_ok());
        }
    }

    // ── Phase B / B3 — INT-only production scoping ──────────────

    /// Quant-group classification is faithful to the verified
    /// `config.json` regex targets (incl. the layer-16 boundary).
    #[test]
    fn b3_quant_group_classification_matches_config() {
        use QkvProj::*;
        // group_1 INT7 (always mined): gate/up/o_proj.
        for l in [LlamaFfnLayer::GateProj, LlamaFfnLayer::UpProj, LlamaFfnLayer::OProj] {
            assert_eq!(l.quant_group(), QuantGroup::Int7Mined);
            assert!(l.is_mineable());
        }
        // group_0 FP8 (never mined): down_proj.
        assert_eq!(
            LlamaFfnLayer::DownProj.quant_group(),
            QuantGroup::Fp8Deferred
        );
        assert!(!LlamaFfnLayer::DownProj.is_mineable());
        // Attention qkv: 0..=15 ⇒ FP8 (group_0); 16..=31 ⇒ INT7
        // (group_1). Spot-check the exact regex boundary.
        for (idx, want) in [
            (0u32, QuantGroup::Fp8Deferred),
            (15, QuantGroup::Fp8Deferred),
            (16, QuantGroup::Int7Mined),
            (31, QuantGroup::Int7Mined),
        ] {
            for which in [Q, K, V] {
                let l = LlamaFfnLayer::AttnQkv {
                    layer_idx: idx,
                    which,
                };
                assert_eq!(l.quant_group(), want, "layer {idx} {which:?} group");
            }
        }
    }

    /// **B3 machine guard (adversarial):** mining an FP8 (group_0)
    /// layer is rejected in-repo before any proving.
    #[test]
    fn b3_fp8_layers_are_rejected_by_the_guard() {
        use QkvProj::*;
        assert_eq!(
            LlamaFfnLayer::DownProj.mineable_matmul_params(4096),
            Err(ParamError::Fp8LayerNotMineable),
        );
        for idx in [0u32, 7, 15] {
            for which in [Q, K, V] {
                assert_eq!(
                    LlamaFfnLayer::AttnQkv {
                        layer_idx: idx,
                        which
                    }
                    .mineable_matmul_params(4096),
                    Err(ParamError::Fp8LayerNotMineable),
                    "early-layer {idx} {which:?} qkv is FP8 ⇒ rejected"
                );
            }
        }
    }

    /// Mined group_1 layers yield `validate_prod_envelope`-valid
    /// params; gate/up match the `LLAMA_3_1_8B_GATE_UP` shape.
    #[test]
    fn b3_mined_layers_are_envelope_valid() {
        use QkvProj::*;
        let bs = 4096;
        for l in [
            LlamaFfnLayer::GateProj,
            LlamaFfnLayer::UpProj,
            LlamaFfnLayer::OProj,
            LlamaFfnLayer::AttnQkv {
                layer_idx: 16,
                which: Q,
            },
            LlamaFfnLayer::AttnQkv {
                layer_idx: 31,
                which: K,
            },
            LlamaFfnLayer::AttnQkv {
                layer_idx: 20,
                which: V,
            },
        ] {
            let p = l
                .mineable_matmul_params(bs)
                .unwrap_or_else(|e| panic!("{l:?} must mine: {e}"));
            assert!(
                p.validate_prod_envelope().is_ok(),
                "{l:?} params must be envelope-valid"
            );
        }
        let gu = LlamaFfnLayer::GateProj.mineable_matmul_params(bs).unwrap();
        assert_eq!(
            (gu.k, gu.n),
            (
                MatmulParams::LLAMA_3_1_8B_GATE_UP.k,
                MatmulParams::LLAMA_3_1_8B_GATE_UP.n
            ),
            "gate_proj shape == LLAMA_3_1_8B_GATE_UP"
        );
        // down_proj's SHAPE is still a valid §4.8 sizing reference
        // (the doc'd retained use) even though it is not mineable.
        assert!(
            MatmulParams::LLAMA_3_1_8B_DOWN
                .validate_prod_envelope()
                .is_ok(),
            "DOWN preset stays a valid envelope-sizing shape"
        );
    }
}
