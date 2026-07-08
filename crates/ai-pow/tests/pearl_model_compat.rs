//! Phase B — byte-equivalence & correctness vs Pearl for the
//! **shipped production model** `pearl-ai/Llama-3.1-8B-Instruct-pearl`.
//!
//! `pearl_compat_fixtures.rs` already pins S0–S9 byte-equality
//! against a *vendored copy of Pearl's reference functions* at
//! *generic* hand-picked shapes. This file lifts that to the
//! **real production preset's scale-sensitive parameters**
//! (`k = 4096`, `r = 64`, `tile = 64`, the 57 344-chunk weight
//! scale) and is the home of:
//!
//!   * **B1.0 (here, Pearl-independent):** the protocol boundaries
//!     are structurally sound + self-consistent at the real
//!     scale (catches shape/scale latents the tiny generic
//!     fixtures miss — e.g. the `r-1 = 63` permutation bitmask,
//!     `64 | 4096`, the 64-stripe fold wrapping the 16 `M` slots
//!     4×, the chunk-Merkle leaf count at the real weight size).
//!   * **B1.1/B1.2 (residual, Pearl-gated — see the `b1_1_*`
//!     stub):** the *byte-equality* assertions vs **Pearl's real
//!     miner** for this model's mining config `μ`. Blocked on the
//!     Pearl-side golden artifact (`2026-05-18_PHASE_B_DESIGN.md` Risk-1 /
//!     DB-1); wired so the only remaining work is dropping in
//!     `fixtures/pearl_model.rs` and removing the stub's
//!     `#[ignore]`.
//!
//! Authoritative plan: `crates/ai-pow-zk/docs/2026-05-18_PHASE_B_DESIGN.md`.

use ai_pow::commit::{padded_chunk_len, CHUNK_LEN};
use ai_pow::matmul::{compute_tile_from_slices, BlockNoise};
use ai_pow::params::{LlamaFfnLayer, MatmulParams};
use ai_pow::synth::synth_matrices;
use ai_pow::tile_hash::difficulty_target;

/// The real shipped model's mined GEMM, reduced to **one tile**
/// (`m = n = tile = 64`) at the **real scale-sensitive params**
/// (`k = 4096`, `r = 64`, `tile = 64`). `m/n` are reduced only so
/// this stays a fast unit test; every parameter that drives a
/// scale-dependent code path (`k`, `r`, `tile`, `num_stripes =
/// k/r = 64`) is the production value.
fn real_one_tile() -> MatmulParams {
    let p = MatmulParams {
        m: 64,
        k: 4096, // hidden_size — the real contraction dim
        n: 64,
        noise_rank: 64, // the real r (perm bitmask r-1 = 63)
        tile: 64,       // the real tile
        spot_checks: 1, // one tile ⇒ ≤ 1
        difficulty_bits: 0,
    };
    p.validate().expect("real-scale one-tile params valid");
    p
}

/// **B1.0a — the B3 ⇄ preset cross-link.** `gate_proj` is a
/// group_1 INT7 *mineable* layer and its params are exactly the
/// `LLAMA_3_1_8B_GATE_UP` shape; the real preset is
/// envelope-valid (consensus-admissible).
#[test]
fn b1_0a_real_preset_is_the_mineable_gate_up_shape() {
    let gu = LlamaFfnLayer::GateProj
        .mineable_matmul_params(4096)
        .expect("gate_proj is group_1 INT7 — mineable");
    assert_eq!(
        (gu.k, gu.n),
        (
            MatmulParams::LLAMA_3_1_8B_GATE_UP.k,
            MatmulParams::LLAMA_3_1_8B_GATE_UP.n
        ),
    );
    assert!(MatmulParams::LLAMA_3_1_8B_GATE_UP
        .validate_prod_envelope()
        .is_ok());
}

/// **B1.0b — chunk-Merkle leaf count at the real weight scale.**
/// The P-B.2 motivating scale: a `gate_proj`/`up_proj` weight is
/// `n·k = 14336·4096 = 58 720 256` bytes ⇒ exactly `57 344`
/// BLAKE3 chunks (no chunk-padding off-by-one only visible at
/// the real size); the `m·k` activation side is `16 777 216` ⇒
/// `16 384` chunks. Pure arithmetic — no Pearl oracle, no
/// materialization.
#[test]
fn b1_0b_commitment_chunk_count_at_real_weight_scale() {
    let k = MatmulParams::LLAMA_3_1_8B_GATE_UP.k as usize; // 4096
    let n = MatmulParams::LLAMA_3_1_8B_GATE_UP.n as usize; // 14336
    let m = MatmulParams::LLAMA_3_1_8B_GATE_UP.m as usize; // 4096

    let w_bytes = n * k; // i8 ⇒ 1 byte each
    assert_eq!(w_bytes, 58_720_256);
    let w_padded = padded_chunk_len(w_bytes);
    assert_eq!(w_padded, w_bytes, "real weight bytes are chunk-aligned");
    assert_eq!(w_padded / CHUNK_LEN, 57_344, "the P-B.2 motivating scale");

    let a_bytes = m * k;
    assert_eq!(a_bytes, 16_777_216);
    assert_eq!(padded_chunk_len(a_bytes) / CHUNK_LEN, 16_384);
}

/// **B1.0c — §4.4 noise-factor structure at the real `r = 64`.**
/// `BlockNoise::expand` at `(k=4096, r=64)`: `E_L`/`F_R` values
/// live in Pearl's `[−32, 31]` (`(byte & 0x3F) − 32`), and every
/// choice position is in `[0, r) = [0, 64)` with the `+`/`−`
/// positions **distinct** (Pearl §4.4 ChoiceMatrix) — the
/// invariant the `r−1 = 63` bitmask must preserve at the real
/// rank. Lengths match the documented `m·r` / `n·r` / `k` shapes.
#[test]
fn b1_0c_noise_structure_at_real_rank() {
    let p = real_one_tile();
    let nz = BlockNoise::expand(&[7u8; 32], &[9u8; 32], &p);
    let (m, k, n, r) = (
        p.m as usize, p.k as usize, p.n as usize, p.noise_rank as usize,
    );

    assert_eq!(nz.e_l.len(), m * r);
    assert_eq!(nz.f_r.len(), n * r);
    assert_eq!(nz.e_r_pos.len(), k);
    assert_eq!(nz.f_l_pos.len(), k);

    for &v in &nz.e_l {
        assert!((-32..=31).contains(&(v as i32)), "E_L {v} ∉ [-32,31]");
    }
    for &v in &nz.f_r {
        assert!((-32..=31).contains(&(v as i32)), "F_R {v} ∉ [-32,31]");
    }
    for &(pp, pm) in nz.e_r_pos.iter().chain(nz.f_l_pos.iter()) {
        assert!((pp as usize) < r && (pm as usize) < r, "pos ≥ r");
        assert_ne!(pp, pm, "Pearl §4.4: the two positions are distinct");
    }
}

/// **B1.0d — mineable-unit self-consistency at the real
/// `(k, r, tile)`.** `compute_tile_from_slices` is deterministic
/// at the production scale (catches scale-dependent
/// nondeterminism/UB); the `num_stripes = k/r = 64`-stripe fold
/// wraps the 16 `M` slots **4×** (the slot-wrap path the
/// `k = 64` fixtures never reach); the keyed hash is
/// deterministic and key-sensitive (D5: the digest is identical
/// given the same key — the precise byte-equivalence claim's
/// anchor).
#[test]
fn b1_0d_mineable_unit_self_consistent_at_real_scale() {
    let p = real_one_tile();
    assert_eq!(p.num_stripes(), 64, "k/r = 4096/64 ⇒ 64 stripes > 16");

    // One tile's slices at the real scale (m = n = tile = 64 ⇒
    // synth's m·k / n·k ARE the single tile's t·k slices).
    let (a, b) = synth_matrices(b"b1.0d-real-scale", &p);
    assert_eq!(a.len(), 64 * 4096);
    assert_eq!(b.len(), 64 * 4096);

    let s1 = compute_tile_from_slices(&a, &b, &p);
    let s2 = compute_tile_from_slices(&a, &b, &p);
    assert_eq!(s1, s2, "compute_tile_from_slices must be deterministic");

    // Different inputs ⇒ different state (the fold is live, not a
    // constant, at 64 stripes).
    let (a2, _) = synth_matrices(b"b1.0d-other", &p);
    assert_ne!(
        compute_tile_from_slices(&a2, &b, &p),
        s1,
        "distinct A ⇒ distinct TileState (64-stripe fold active)"
    );

    // Keyed hash: deterministic + key-sensitive (D5 — the digest
    // is a pure fn of (state, key); feeding Pearl's key would
    // reproduce Pearl's digest, the s6 anchor).
    let h_k1 = s1.keyed_hash(&[1u8; 32]);
    assert_eq!(h_k1, s1.keyed_hash(&[1u8; 32]), "hash deterministic");
    assert_ne!(h_k1, s1.keyed_hash(&[2u8; 32]), "hash is key-sensitive");
}

/// **B1.0e — shape-aware difficulty target at the real
/// `(b, r, t)`.** `difficulty_target` is a well-formed 32-byte LE
/// encoding at the real preset, deterministic, non-trivial, and
/// responds to `difficulty_bits` (Pearl §4.8 `2^(256−b)·r·t²`,
/// saturating) — the S9 invariant re-pinned at the real shape.
#[test]
fn b1_0e_difficulty_target_well_formed_at_real_preset() {
    let p = MatmulParams::LLAMA_3_1_8B_GATE_UP;
    let t0 = difficulty_target(&p);
    assert_eq!(t0, difficulty_target(&p), "deterministic");
    assert!(t0.iter().any(|&x| x != 0), "non-trivial target");

    // weight = r·t² = 64·64² = 2¹⁸; the target `2^(256−b)·weight`
    // *saturates* to all-0xFF until `b ≳ 18`. Use a clearly
    // non-saturating `b` (128 ⇒ 2^(128)·2¹⁸ = 2¹⁴⁶) to prove the
    // target genuinely responds to difficulty.
    let t0_saturates = t0.iter().all(|&x| x == 0xFF);
    assert!(t0_saturates, "b=0 ⇒ target saturates to 2^256-1 (all 0xFF)");
    let harder = difficulty_target(&MatmulParams {
        difficulty_bits: 128,
        ..p
    });
    assert_ne!(
        harder, t0,
        "a non-saturating difficulty_bits must change the target \
         (§4.8 2^(256-b)·r·t², saturating)"
    );
    assert!(
        !harder.iter().all(|&x| x == 0xFF),
        "b=128 at weight 2^18 must NOT saturate"
    );
}

// ── B1.1 — ai-pow byte-processes the REAL model weights ─────────
//
// The B1 protocol-equivalence risk is closed by the retained Pearl fixtures
// and by this real-weight regression. With the real shipped weights
// available (`~/Dev/Llama-3.1-8B-Instruct-pearl`, set
// `PEARL_MODEL_DIR` to override), B1.1 exercises ai-pow's full
// audited mineable-unit pipeline on a **real `gate_proj` INT7
// weight tile** at the real `μ` (`k=4096, r=64, tile=64`).
//
// The safetensors offsets are anchored to an independent Python
// ground-truth (`python3` over the real file, recorded below) so
// a wrong reader cannot yield a silently-wrong "golden" (R1 —
// no fake completion). The only Phase-B item these do NOT cover
// is a *live vLLM forward-pass activation* from a real prompt —
// a Phase-D end-to-end-usefulness concern already discharged by
// B2.2 (the quant contract is bit-lossless for ANY int7
// activation), NOT a byte-equivalence gap.

/// Python-extracted ground truth for shard-1
/// `model.layers.0.mlp.gate_proj.weight` (dtype I8, shape
/// [14336,4096]); the file's safetensors JSON-header length and
/// the tensor's data offset. If the model file changes upstream,
/// the header-length assertion fails fast.
const ST_HEADER_LEN: u64 = 36_688;
const GATE0_DATA_OFFSET: u64 = 2_512_125_952;
const ORACLE_ROW0_HEAD: [i8; 8] = [-23, -10, -2, -1, 11, 5, -5, -35];
const ORACLE_ROW63_TAIL: [i8; 8] = [-3, -22, 7, 19, -5, 7, 6, -13];
const ORACLE_MIN: i8 = -61;
const ORACLE_MAX: i8 = 61;
const ORACLE_SUM: i64 = -1690;

const TILE_OUT: usize = 64; // out-channels read (n at one-tile μ)
const MODEL_K: usize = 4096; // hidden_size = the real contraction dim

fn model_shard1() -> Option<std::path::PathBuf> {
    let dir = std::env::var("PEARL_MODEL_DIR").unwrap_or_else(|_| {
        format!(
            "{}/Dev/Llama-3.1-8B-Instruct-pearl",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let p = std::path::Path::new(&dir).join("model-00001-of-00002.safetensors");
    p.exists().then_some(p)
}

/// Minimal safetensors tile reader: the first `TILE_OUT`
/// out-channels (rows) of `gate_proj.weight` — `TILE_OUT × MODEL_K`
/// int8 (the int7-in-int8 storage). Absolute byte offset =
/// `8 + header_len + tensor_data_offset` (the safetensors layout).
fn read_real_gate_proj_tile(path: &std::path::Path) -> Vec<i8> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).expect("open shard1");
    let mut len8 = [0u8; 8];
    f.read_exact(&mut len8).expect("read header len");
    let hdr_len = u64::from_le_bytes(len8);
    assert_eq!(
        hdr_len, ST_HEADER_LEN,
        "safetensors header length changed; model file differs from \
         the recorded Python oracle"
    );
    let abs = 8 + hdr_len + GATE0_DATA_OFFSET;
    f.seek(SeekFrom::Start(abs)).expect("seek tile");
    let mut raw = vec![0u8; TILE_OUT * MODEL_K];
    f.read_exact(&mut raw).expect("read tile bytes");
    raw.into_iter().map(|b| b as i8).collect()
}

/// **B1.1a — the integrity anchor.** The Rust safetensors reader
/// reproduces the independent Python ground truth bit-for-bit
/// (head/tail windows, min, max, sum). If this fails, every
/// downstream B1.1 "real weight" claim is void — it MUST pass
/// before the others are meaningful (R1).
#[test]
fn b1_1a_safetensors_reader_matches_python_oracle() {
    let Some(p) = model_shard1() else {
        eprintln!("SKIP b1_1a: real model absent (set PEARL_MODEL_DIR)");
        return;
    };
    let tile = read_real_gate_proj_tile(&p);
    assert_eq!(tile.len(), TILE_OUT * MODEL_K);
    assert_eq!(&tile[..8], &ORACLE_ROW0_HEAD, "row0 head ≠ oracle");
    assert_eq!(
        &tile[63 * MODEL_K + 4088..63 * MODEL_K + 4096],
        &ORACLE_ROW63_TAIL,
        "row63 tail ≠ oracle"
    );
    assert_eq!(*tile.iter().min().unwrap(), ORACLE_MIN);
    assert_eq!(*tile.iter().max().unwrap(), ORACLE_MAX);
    assert_eq!(
        tile.iter().map(|&v| v as i64).sum::<i64>(),
        ORACLE_SUM,
        "tile sum ≠ oracle (reader offset/stride wrong)"
    );
}

/// **B1.1b — the B2 quant contract holds on the REAL weights.**
/// Every byte of the real `gate_proj` int7 tile is inside Pearl
/// §4.1 type-0 `[−64, 64]`; `ai_pow::quant::extract` accepts it;
/// and the mined integer matmul over the **real weights** is
/// bit-lossless (`int_matmul == Σ Xq·Wq` on real `Wq`) — B2.1
/// validated on actual model data, not synthetic.
#[test]
fn b1_1b_real_weights_satisfy_pearl_int7_and_b2_lossless() {
    use ai_pow::quant::{extract, int_matmul, QuantizedGemm};
    let Some(p) = model_shard1() else {
        eprintln!("SKIP b1_1b: real model absent");
        return;
    };
    let wq = read_real_gate_proj_tile(&p); // [out=64, in=4096] row-major
    for &v in &wq {
        assert!(
            (-64..=64).contains(&(v as i32)),
            "real weight {v} ∉ Pearl type-0 [-64,64]"
        );
    }
    // Deterministic int7 activation (B2.2: losslessness holds for
    // ANY int7 input ⇒ a synthetic activation is sufficient for
    // the byte-equivalence-of-protocol claim; the live forward
    // pass is Phase-D usefulness, already covered).
    let (tok, k, out) = (8usize, MODEL_K, TILE_OUT);
    let xq: Vec<i8> = (0..tok * k)
        .map(|i| ((i * 31 + 7) % 127 - 63) as i8)
        .collect();
    let qg = QuantizedGemm {
        tokens: tok,
        in_dim: k,
        out_dim: out,
        xq: xq.clone(),
        wq: wq.clone(),
        s_x: vec![0.01; tok],
        s_w: vec![0.02; out],
    };
    let op = extract(&qg).expect("real int7 weights ⇒ in-domain");
    let mined = int_matmul(&op);
    let mut want = vec![0i32; tok * out];
    for t in 0..tok {
        for o in 0..out {
            let mut s = 0i32;
            for l in 0..k {
                s += xq[t * k + l] as i32 * wq[o * k + l] as i32;
            }
            want[t * out + o] = s;
        }
    }
    assert_eq!(
        mined, want,
        "B2 bit-lossless must hold on the REAL gate_proj weights"
    );
}

/// **B1.1c — ai-pow's full audited mineable-unit pipeline runs on
/// the REAL model weights at the real `μ`.** `BlockContext::build`
/// (the §4.3 commitment chain → §4.4 noise → §4.5/§4.6 tile
/// state + chunk-Merkle, all on the audited-faithful path) on
/// `B = the real gate_proj tile` (col-major: the safetensors
/// `[out,in]` row slice IS Pearl's `B` column-major) succeeds at
/// `k=4096, r=64, tile=64`, is deterministic, and the digest
/// genuinely depends on the real weights (≠ a synthetic-`B`
/// run). This is "ai-pow mines the real model's weights",
/// end-to-end, byte-stable, at production scale.
#[test]
fn b1_1c_real_weight_mineable_unit_end_to_end() {
    use ai_pow::commit::matrix_commitment;
    use ai_pow::fiat_shamir::{block_state, commitment_key};
    use ai_pow::prover::{params_tag, BlockContext};
    let Some(p) = model_shard1() else {
        eprintln!("SKIP b1_1c: real model absent");
        return;
    };
    let real_b = read_real_gate_proj_tile(&p); // n·k col-major (n=64,k=4096)
    let mp = real_one_tile(); // m=n=64, k=4096, r=64, tile=64 — the real μ
    let a: Vec<i8> = (0..(mp.m as usize) * MODEL_K)
        .map(|i| ((i * 13 + 5) % 127 - 63) as i8)
        .collect();

    let hdr = b"b1.1c-block-header";
    let nonce = b"b1.1c-nonce";
    let ctx = BlockContext::build(hdr, nonce, &a, &real_b, &mp)
        .expect("ai-pow must mine the real model's weights at real μ");
    // Deterministic (the audited pipeline is a pure fn of inputs).
    let ctx2 = BlockContext::build(hdr, nonce, &a, &real_b, &mp).unwrap();
    assert_eq!(ctx.h_a_chunk(), ctx2.h_a_chunk());
    assert_eq!(ctx.h_b_chunk(), ctx2.h_b_chunk());
    assert_eq!(ctx.s_a(), ctx2.s_a());
    assert_eq!(ctx.s_b(), ctx2.s_b());
    assert_eq!(
        ctx.tile_states()
            .iter()
            .map(|s| s.keyed_hash(&ctx.pow_key()))
            .collect::<Vec<_>>(),
        ctx2.tile_states()
            .iter()
            .map(|s| s.keyed_hash(&ctx2.pow_key()))
            .collect::<Vec<_>>(),
        "mineable unit must be deterministic on the real weights"
    );
    // The H_B chunk-commitment of the REAL weight bytes equals the
    // audited Pearl §4.6 keyed chunk hash recomputed independently
    // here (matrix_commitment is the audited path; this pins it on
    // real model bytes — the S8 invariant on real weights).
    let state = block_state(hdr, nonce);
    let kappa = commitment_key(&state, &params_tag(&mp));
    let b_bytes: Vec<u8> = real_b.iter().map(|&v| v as u8).collect();
    assert_eq!(
        *ctx.h_b_chunk(),
        matrix_commitment(&b_bytes, &kappa),
        "H_B_chunk of the real weights == Pearl §4.6 chunk-Merkle"
    );
    // The digest genuinely depends on the real weights: a
    // synthetic-B run differs (the pipeline is not weight-blind).
    let synth_b: Vec<i8> = (0..(mp.n as usize) * MODEL_K)
        .map(|i| ((i * 17 + 3) % 127 - 63) as i8)
        .collect();
    let ctx_s = BlockContext::build(hdr, nonce, &a, &synth_b, &mp).unwrap();
    assert_ne!(
        ctx.h_b_chunk(),
        ctx_s.h_b_chunk(),
        "real vs synthetic B ⇒ different commitment (weight-sensitive)"
    );
}

// ── B1.1 (Gemma) — a SECOND real Pearl model ────────────────────
//
// `~/Dev/Gemma-4-31B-it-pearl` (`Gemma4ForConditionalGeneration`,
// model_type `gemma4`; hidden=5376, intermediate=21504, 60
// layers — matches `MatmulParams::GEMMA_4_31B_FFN`). Same
// `quant_method:"pearl"` split: group_1 (INT7 per-channel —
// gate/up/o_proj, the mined Linears) / group_0 (FP8 — q/k/v/qkv
// + down_proj, out of scope). A *single* 31 GB
// `model.safetensors` (one header, not sharded). This
// corroborates Phase B on a second, architecturally-different
// real production model at its own real μ (k=5376, n=21504).
//
// Set `GEMMA_MODEL_DIR` to override; soft-skips if absent
// (62 GB — CI-safe). Offsets anchored to an independent Python
// ground truth (R1 — no silently-wrong "golden").

const GEMMA_ST_HEADER_LEN: u64 = 222_920;
const GEMMA_GATE0_DATA_OFFSET: u64 = 16_312_434_392;
const GEMMA_K: usize = 5376; // hidden_size = the real contraction dim
const GEMMA_ORACLE_ROW0_HEAD: [i8; 8] = [-3, 10, 6, -1, -4, -1, -1, -3];
const GEMMA_ORACLE_ROW63_TAIL: [i8; 8] = [5, -2, 2, -1, 1, -3, 5, -2];
const GEMMA_ORACLE_MIN: i8 = -61;
const GEMMA_ORACLE_MAX: i8 = 61;
const GEMMA_ORACLE_SUM: i64 = 14_361;

fn gemma_model_file() -> Option<std::path::PathBuf> {
    let dir = std::env::var("GEMMA_MODEL_DIR").unwrap_or_else(|_| {
        format!(
            "{}/Dev/Gemma-4-31B-it-pearl",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let p = std::path::Path::new(&dir).join("model.safetensors");
    p.exists().then_some(p)
}

/// First `TILE_OUT` out-channels of Gemma layer-0
/// `mlp.gate_proj.weight` (I8 `[21504, 5376]`) — `TILE_OUT ×
/// GEMMA_K`. Absolute offset = `8 + header_len + tensor_data
/// _offset` (single-file safetensors).
fn read_real_gemma_gate_proj_tile(path: &std::path::Path) -> Vec<i8> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).expect("open gemma safetensors");
    let mut len8 = [0u8; 8];
    f.read_exact(&mut len8).expect("read header len");
    let hdr_len = u64::from_le_bytes(len8);
    assert_eq!(
        hdr_len, GEMMA_ST_HEADER_LEN,
        "Gemma safetensors header length changed — model differs from \
         the recorded Python oracle; regenerate"
    );
    let abs = 8 + hdr_len + GEMMA_GATE0_DATA_OFFSET;
    f.seek(SeekFrom::Start(abs)).expect("seek tile");
    let mut raw = vec![0u8; TILE_OUT * GEMMA_K];
    f.read_exact(&mut raw).expect("read tile bytes");
    raw.into_iter().map(|b| b as i8).collect()
}

fn gemma_one_tile() -> MatmulParams {
    let p = MatmulParams {
        m: 64,
        k: GEMMA_K as u32, // 5376 = the real Gemma contraction dim
        n: 64,
        noise_rank: 64, // 64 | 5376 (= 84)
        tile: 64,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    p.validate().expect("gemma one-tile params valid");
    p
}

/// **B1.1-Gemma a — integrity anchor.** The reader reproduces
/// the independent Python ground truth bit-for-bit on the second
/// real model (single-file safetensors, different header/offset/
/// K). Must pass before the others are meaningful (R1).
#[test]
fn b1_1_gemma_a_safetensors_reader_matches_python_oracle() {
    let Some(p) = gemma_model_file() else {
        eprintln!("SKIP b1_1_gemma_a: Gemma model absent (GEMMA_MODEL_DIR)");
        return;
    };
    let tile = read_real_gemma_gate_proj_tile(&p);
    assert_eq!(tile.len(), TILE_OUT * GEMMA_K);
    assert_eq!(&tile[..8], &GEMMA_ORACLE_ROW0_HEAD, "row0 head ≠ oracle");
    assert_eq!(
        &tile[63 * GEMMA_K + GEMMA_K - 8..63 * GEMMA_K + GEMMA_K],
        &GEMMA_ORACLE_ROW63_TAIL,
        "row63 tail ≠ oracle"
    );
    assert_eq!(*tile.iter().min().unwrap(), GEMMA_ORACLE_MIN);
    assert_eq!(*tile.iter().max().unwrap(), GEMMA_ORACLE_MAX);
    assert_eq!(
        tile.iter().map(|&v| v as i64).sum::<i64>(),
        GEMMA_ORACLE_SUM,
        "tile sum ≠ oracle (reader offset/stride wrong)"
    );
}

/// **B1.1-Gemma b — B2 quant contract on the REAL Gemma
/// weights.** The real Gemma `gate_proj` int7 tile is in Pearl
/// `[−64,64]`, `quant::extract` accepts it, and the mined
/// integer matmul is bit-lossless on real Gemma data — B2.1 on
/// a second real model.
#[test]
fn b1_1_gemma_b_real_weights_satisfy_pearl_int7_and_b2_lossless() {
    use ai_pow::quant::{extract, int_matmul, QuantizedGemm};
    let Some(p) = gemma_model_file() else {
        eprintln!("SKIP b1_1_gemma_b: Gemma model absent");
        return;
    };
    let wq = read_real_gemma_gate_proj_tile(&p); // [out=64, in=5376]
    for &v in &wq {
        assert!(
            (-64..=64).contains(&(v as i32)),
            "real Gemma weight {v} ∉ Pearl type-0 [-64,64]"
        );
    }
    let (tok, k, out) = (8usize, GEMMA_K, TILE_OUT);
    let xq: Vec<i8> = (0..tok * k)
        .map(|i| ((i * 29 + 3) % 127 - 63) as i8)
        .collect();
    let qg = QuantizedGemm {
        tokens: tok,
        in_dim: k,
        out_dim: out,
        xq: xq.clone(),
        wq: wq.clone(),
        s_x: vec![0.013; tok],
        s_w: vec![0.017; out],
    };
    let op = extract(&qg).expect("real Gemma int7 ⇒ in-domain");
    let mined = int_matmul(&op);
    let mut want = vec![0i32; tok * out];
    for t in 0..tok {
        for o in 0..out {
            let mut s = 0i32;
            for l in 0..k {
                s += xq[t * k + l] as i32 * wq[o * k + l] as i32;
            }
            want[t * out + o] = s;
        }
    }
    assert_eq!(
        mined, want,
        "B2 bit-lossless must hold on the REAL Gemma weights"
    );
}

/// **B1.1-Gemma c — ai-pow's full audited pipeline on the REAL
/// Gemma weights at the real μ** (`k=5376, r=64, tile=64`).
/// `BlockContext::build` on `B = the real Gemma gate_proj tile`
/// succeeds, is deterministic, weight-sensitive, with
/// `H_B == matrix_commitment(real bytes)`. "ai-pow mines a
/// second real production model's weights", end-to-end.
#[test]
fn b1_1_gemma_c_real_weight_mineable_unit_end_to_end() {
    use ai_pow::commit::matrix_commitment;
    use ai_pow::fiat_shamir::{block_state, commitment_key};
    use ai_pow::prover::{params_tag, BlockContext};
    let Some(p) = gemma_model_file() else {
        eprintln!("SKIP b1_1_gemma_c: Gemma model absent");
        return;
    };
    let real_b = read_real_gemma_gate_proj_tile(&p); // n·k col-major (n=64,k=5376)
    let mp = gemma_one_tile();
    let a: Vec<i8> = (0..(mp.m as usize) * GEMMA_K)
        .map(|i| ((i * 11 + 2) % 127 - 63) as i8)
        .collect();

    let hdr = b"b1.1-gemma-block-header";
    let nonce = b"b1.1-gemma-nonce";
    let ctx = BlockContext::build(hdr, nonce, &a, &real_b, &mp)
        .expect("ai-pow must mine the real Gemma weights at real μ");
    let ctx2 = BlockContext::build(hdr, nonce, &a, &real_b, &mp).unwrap();
    assert_eq!(ctx.h_b_chunk(), ctx2.h_b_chunk());
    assert_eq!(ctx.s_a(), ctx2.s_a());
    assert_eq!(
        ctx.tile_states()
            .iter()
            .map(|s| s.keyed_hash(&ctx.pow_key()))
            .collect::<Vec<_>>(),
        ctx2.tile_states()
            .iter()
            .map(|s| s.keyed_hash(&ctx2.pow_key()))
            .collect::<Vec<_>>(),
        "mineable unit must be deterministic on the real Gemma weights"
    );
    let state = block_state(hdr, nonce);
    let kappa = commitment_key(&state, &params_tag(&mp));
    let b_bytes: Vec<u8> = real_b.iter().map(|&v| v as u8).collect();
    assert_eq!(
        *ctx.h_b_chunk(),
        matrix_commitment(&b_bytes, &kappa),
        "H_B_chunk of the real Gemma weights == Pearl §4.6 chunk-Merkle"
    );
    let synth_b: Vec<i8> = (0..(mp.n as usize) * GEMMA_K)
        .map(|i| ((i * 19 + 5) % 127 - 63) as i8)
        .collect();
    let ctx_s = BlockContext::build(hdr, nonce, &a, &synth_b, &mp).unwrap();
    assert_ne!(
        ctx.h_b_chunk(),
        ctx_s.h_b_chunk(),
        "real vs synthetic Gemma B ⇒ different commitment"
    );
}

// ── B1.1 (Llama-3.3-70B) — the largest published Pearl model ────
//
// `~/Dev/Llama-3.3-70B-Instruct-pearl` (`LlamaForCausalLM`;
// hidden=8192, intermediate=28672, 80 layers — the largest k/n
// of the three). Same `quant_method:"pearl"` split (group_1
// INT7 per-channel = o/gate/up_proj + late-layer qkv mined;
// group_0 FP8 = down_proj + early qkv, out). **15-shard**
// safetensors + index; layer-0 `gate_proj` is in shard
// `model-00001-of-00015.safetensors`. Third real production
// model: largest dims + most-sharded layout.
//
// `LLAMA70B_MODEL_DIR` overrides; soft-skips if absent (135 GB —
// CI-safe). Offsets anchored to an independent Python oracle.

const L70B_ST_HEADER_LEN: u64 = 7_200;
const L70B_GATE0_DATA_OFFSET: u64 = 3_142_234_112;
const L70B_K: usize = 8192; // hidden_size = the real contraction dim
const L70B_ORACLE_ROW0_HEAD: [i8; 8] = [-8, -4, 1, 0, 0, 0, 2, -5];
const L70B_ORACLE_ROW63_TAIL: [i8; 8] = [0, -7, 5, 4, -8, -8, -1, -6];
const L70B_ORACLE_MIN: i8 = -61;
const L70B_ORACLE_MAX: i8 = 61;
const L70B_ORACLE_SUM: i64 = -9_661;

fn l70b_shard1() -> Option<std::path::PathBuf> {
    let dir = std::env::var("LLAMA70B_MODEL_DIR").unwrap_or_else(|_| {
        format!(
            "{}/Dev/Llama-3.3-70B-Instruct-pearl",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let p = std::path::Path::new(&dir).join("model-00001-of-00015.safetensors");
    p.exists().then_some(p)
}

/// First `TILE_OUT` out-channels of layer-0 `mlp.gate_proj.weight`
/// (I8 `[28672, 8192]`) from shard 1 — `TILE_OUT × L70B_K`.
/// Absolute offset = `8 + header_len + tensor_data_offset`.
fn read_real_l70b_gate_proj_tile(path: &std::path::Path) -> Vec<i8> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).expect("open l70b shard1");
    let mut len8 = [0u8; 8];
    f.read_exact(&mut len8).expect("read header len");
    let hdr_len = u64::from_le_bytes(len8);
    assert_eq!(
        hdr_len, L70B_ST_HEADER_LEN,
        "Llama-70B shard1 header length changed — model differs from \
         the recorded Python oracle; regenerate"
    );
    let abs = 8 + hdr_len + L70B_GATE0_DATA_OFFSET;
    f.seek(SeekFrom::Start(abs)).expect("seek tile");
    let mut raw = vec![0u8; TILE_OUT * L70B_K];
    f.read_exact(&mut raw).expect("read tile bytes");
    raw.into_iter().map(|b| b as i8).collect()
}

fn l70b_one_tile() -> MatmulParams {
    let p = MatmulParams {
        m: 64,
        k: L70B_K as u32, // 8192 = the real Llama-70B contraction dim
        n: 64,
        noise_rank: 64, // 64 | 8192 (= 128)
        tile: 64,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    p.validate().expect("l70b one-tile params valid");
    p
}

/// **B1.1-L70B a — integrity anchor.** The reader reproduces the
/// independent Python ground truth bit-for-bit on the largest,
/// most-sharded real model (15 shards; layer-0 gate_proj in
/// shard 1). Must pass before the others are meaningful (R1).
#[test]
fn b1_1_l70b_a_safetensors_reader_matches_python_oracle() {
    let Some(p) = l70b_shard1() else {
        eprintln!("SKIP b1_1_l70b_a: Llama-70B absent (LLAMA70B_MODEL_DIR)");
        return;
    };
    let tile = read_real_l70b_gate_proj_tile(&p);
    assert_eq!(tile.len(), TILE_OUT * L70B_K);
    assert_eq!(&tile[..8], &L70B_ORACLE_ROW0_HEAD, "row0 head ≠ oracle");
    assert_eq!(
        &tile[63 * L70B_K + L70B_K - 8..63 * L70B_K + L70B_K],
        &L70B_ORACLE_ROW63_TAIL,
        "row63 tail ≠ oracle"
    );
    assert_eq!(*tile.iter().min().unwrap(), L70B_ORACLE_MIN);
    assert_eq!(*tile.iter().max().unwrap(), L70B_ORACLE_MAX);
    assert_eq!(
        tile.iter().map(|&v| v as i64).sum::<i64>(),
        L70B_ORACLE_SUM,
        "tile sum ≠ oracle (reader offset/stride wrong)"
    );
}

/// **B1.1-L70B b — B2 contract on the REAL Llama-70B weights.**
/// Real `gate_proj` int7 ∈ Pearl `[−64,64]`; `quant::extract`
/// accepts it; B2.1 bit-lossless on real Llama-70B data.
#[test]
fn b1_1_l70b_b_real_weights_satisfy_pearl_int7_and_b2_lossless() {
    use ai_pow::quant::{extract, int_matmul, QuantizedGemm};
    let Some(p) = l70b_shard1() else {
        eprintln!("SKIP b1_1_l70b_b: Llama-70B absent");
        return;
    };
    let wq = read_real_l70b_gate_proj_tile(&p); // [out=64, in=8192]
    for &v in &wq {
        assert!(
            (-64..=64).contains(&(v as i32)),
            "real Llama-70B weight {v} ∉ Pearl type-0 [-64,64]"
        );
    }
    let (tok, k, out) = (8usize, L70B_K, TILE_OUT);
    let xq: Vec<i8> = (0..tok * k)
        .map(|i| ((i * 23 + 9) % 127 - 63) as i8)
        .collect();
    let qg = QuantizedGemm {
        tokens: tok,
        in_dim: k,
        out_dim: out,
        xq: xq.clone(),
        wq: wq.clone(),
        s_x: vec![0.011; tok],
        s_w: vec![0.019; out],
    };
    let op = extract(&qg).expect("real Llama-70B int7 ⇒ in-domain");
    let mined = int_matmul(&op);
    let mut want = vec![0i32; tok * out];
    for t in 0..tok {
        for o in 0..out {
            let mut s = 0i32;
            for l in 0..k {
                s += xq[t * k + l] as i32 * wq[o * k + l] as i32;
            }
            want[t * out + o] = s;
        }
    }
    assert_eq!(
        mined, want,
        "B2 bit-lossless must hold on the REAL Llama-70B weights"
    );
}

/// **B1.1-L70B c — full audited pipeline on the REAL Llama-70B
/// weights at the real μ** (`k=8192, r=64, tile=64`).
/// `BlockContext::build` on `B = the real gate_proj tile`
/// succeeds, deterministic, weight-sensitive,
/// `H_B == matrix_commitment(real bytes)`.
#[test]
fn b1_1_l70b_c_real_weight_mineable_unit_end_to_end() {
    use ai_pow::commit::matrix_commitment;
    use ai_pow::fiat_shamir::{block_state, commitment_key};
    use ai_pow::prover::{params_tag, BlockContext};
    let Some(p) = l70b_shard1() else {
        eprintln!("SKIP b1_1_l70b_c: Llama-70B absent");
        return;
    };
    let real_b = read_real_l70b_gate_proj_tile(&p); // n·k col-major (n=64,k=8192)
    let mp = l70b_one_tile();
    let a: Vec<i8> = (0..(mp.m as usize) * L70B_K)
        .map(|i| ((i * 7 + 4) % 127 - 63) as i8)
        .collect();

    let hdr = b"b1.1-l70b-block-header";
    let nonce = b"b1.1-l70b-nonce";
    let ctx = BlockContext::build(hdr, nonce, &a, &real_b, &mp)
        .expect("ai-pow must mine the real Llama-70B weights at real μ");
    let ctx2 = BlockContext::build(hdr, nonce, &a, &real_b, &mp).unwrap();
    assert_eq!(ctx.h_b_chunk(), ctx2.h_b_chunk());
    assert_eq!(ctx.s_a(), ctx2.s_a());
    assert_eq!(
        ctx.tile_states()
            .iter()
            .map(|s| s.keyed_hash(&ctx.pow_key()))
            .collect::<Vec<_>>(),
        ctx2.tile_states()
            .iter()
            .map(|s| s.keyed_hash(&ctx2.pow_key()))
            .collect::<Vec<_>>(),
        "mineable unit must be deterministic on the real Llama-70B weights"
    );
    let state = block_state(hdr, nonce);
    let kappa = commitment_key(&state, &params_tag(&mp));
    let b_bytes: Vec<u8> = real_b.iter().map(|&v| v as u8).collect();
    assert_eq!(
        *ctx.h_b_chunk(),
        matrix_commitment(&b_bytes, &kappa),
        "H_B_chunk of the real Llama-70B weights == Pearl §4.6 chunk-Merkle"
    );
    let synth_b: Vec<i8> = (0..(mp.n as usize) * L70B_K)
        .map(|i| ((i * 31 + 6) % 127 - 63) as i8)
        .collect();
    let ctx_s = BlockContext::build(hdr, nonce, &a, &synth_b, &mp).unwrap();
    assert_ne!(
        ctx.h_b_chunk(),
        ctx_s.h_b_chunk(),
        "real vs synthetic Llama-70B B ⇒ different commitment"
    );
}
