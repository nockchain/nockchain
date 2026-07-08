//! Phase B / B2 — the quant-extraction contract `Q`.
//!
//! The production model `pearl-ai/Llama-3.1-8B-Instruct-pearl`
//! serves its **group_1** linear layers as INT7 GEMMs
//! (`config.json` `quantization_config`, verified): weights
//! per-**output-channel** symmetric int7, activations per-**token**
//! symmetric int7. The served result is
//!
//! ```text
//!   Y_fp[t, o] = s_x[t] · s_w[o] · Σ_in Xq[t,in] · Wq[o,in]
//! ```
//!
//! where the **per-token** scale `s_x` and **per-channel** scale
//! `s_w` are applied *outside* the integer accumulate. Pearl §4.1
//! type-0 mines exactly an integer matmul over the int8
//! `[−64, 64]` domain (int32 accumulate). `Q` ([`extract`]) maps
//! the vLLM-side quantized operands to ai-pow's
//! `(A, B)` i8 layout (`A` row-major `m·k`, `B` col-major `n·k`,
//! the [`crate::prover::BlockContext`] / [`crate::matmul`]
//! convention), so the *mined integers are exactly the integers
//! vLLM already computed* — `Q` is **bit-lossless** (a pure
//! reindex + a domain check; no requantization). The scales are
//! carried separately as the dequant `μ` for the
//! usefulness-reconstruction check only; they are **not** part of
//! the proven integer relation.
//!
//! This module ships the **offline contract + its conformance
//! KAT** (Pearl-independent). The *live* extraction is Phase D's
//! vLLM plugin (external); the Pearl-digest-parity fixture
//! (B2-fixture) is the one Pearl-side-gated residual — see
//! `crates/ai-pow-zk/docs/2026-05-18_PHASE_B_DESIGN.md` §3/§7.
//!
//! **Soundness-adjacent (R1, KAT-first):** B2's "the mined
//! integers equal what Pearl mines" touches the *mined integer
//! operand*. The lossless KAT below is the gate; never weaken it
//! to a tolerance.

use thiserror::Error;

/// Pearl §4.1 type-0 integer domain (the committed operand range;
/// `|A| ≤ 64`, matmul.rs / `pearl_compat_fixtures` S4). Symmetric
/// int7 ⊂ `[−64, 64]` for either the `[−64,63]` or `[−63,63]`
/// convention, so a faithful extraction never clips.
pub const PEARL_INT_LO: i32 = -64;
pub const PEARL_INT_HI: i32 = 64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QuantError {
    #[error("operand value {value} at {what} is outside the Pearl type-0 int domain [-64, 64]")]
    OutOfDomain { what: &'static str, value: i32 },
    #[error("shape mismatch: {what} (expected {expected}, got {got})")]
    Shape {
        what: &'static str,
        expected: usize,
        got: usize,
    },
}

/// One served **group_1** INT7 GEMM as the vLLM plugin sees it
/// (the model-faithful representation; row-major).
///
/// `Y_fp[t,o] = s_x[t]·s_w[o]·Σ_in Xq[t,in]·Wq[o,in]`.
#[derive(Debug, Clone)]
pub struct QuantizedGemm {
    /// Tokens (the GEMM's batched-sequence dimension) = `m`.
    pub tokens: usize,
    /// Contraction dim (`hidden`) = `k`.
    pub in_dim: usize,
    /// Output channels (`intermediate`/`hidden`) = `n`.
    pub out_dim: usize,
    /// Activations, int7, **row-major** `tokens × in_dim`.
    pub xq: Vec<i8>,
    /// Weights, int7, **row-major** `out_dim × in_dim`.
    pub wq: Vec<i8>,
    /// Per-token activation scale, length `tokens`.
    pub s_x: Vec<f32>,
    /// Per-output-channel weight scale, length `out_dim`.
    pub s_w: Vec<f32>,
}

/// The extracted Pearl/ai-pow mineable operand + the dequant `μ`.
/// `a`/`b` are exactly the [`crate::prover::BlockContext::build`]
/// layout: `a` row-major `m·k`, `b` **column-major** `n·k`
/// (column `j` at `j*k..(j+1)*k`). The integer matmul ai-pow/
/// Pearl mines is `Σ_l a[i,l]·b[j-col,l]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PearlOperands {
    pub m: usize,
    pub k: usize,
    pub n: usize,
    /// `A`, i8, row-major `m·k` (= `Xq`).
    pub a: Vec<i8>,
    /// `B`, i8, column-major `n·k` (= `Wq` re-laid by channel).
    pub b: Vec<i8>,
    /// Dequant `μ`: per-token (`m`) and per-channel (`n`) scales.
    /// **Outside** the mined integer relation.
    pub s_x: Vec<f32>,
    pub s_w: Vec<f32>,
}

/// `Q` — the quant-extraction contract. Pure reindex
/// (`Xq → A` row-major, `Wq → B` column-major) + the Pearl
/// type-0 domain check + carrying `μ`. **Bit-lossless:** the
/// mined integers are exactly `Xq`/`Wq`; no requantization.
/// `Err` iff an operand leaves `[−64, 64]` (a contract
/// violation — a faithful int7 model never does) or a shape is
/// inconsistent.
pub fn extract(qg: &QuantizedGemm) -> Result<PearlOperands, QuantError> {
    let (m, k, n) = (qg.tokens, qg.in_dim, qg.out_dim);
    if qg.xq.len() != m * k {
        return Err(QuantError::Shape {
            what: "xq",
            expected: m * k,
            got: qg.xq.len(),
        });
    }
    if qg.wq.len() != n * k {
        return Err(QuantError::Shape {
            what: "wq",
            expected: n * k,
            got: qg.wq.len(),
        });
    }
    if qg.s_x.len() != m {
        return Err(QuantError::Shape {
            what: "s_x",
            expected: m,
            got: qg.s_x.len(),
        });
    }
    if qg.s_w.len() != n {
        return Err(QuantError::Shape {
            what: "s_w",
            expected: n,
            got: qg.s_w.len(),
        });
    }
    let check = |v: i8, what: &'static str| -> Result<i8, QuantError> {
        let x = v as i32;
        if x < PEARL_INT_LO || x > PEARL_INT_HI {
            Err(QuantError::OutOfDomain { what, value: x })
        } else {
            Ok(v)
        }
    };
    // A := Xq, row-major m×k (ai-pow row i at i*k..(i+1)*k).
    let mut a = vec![0i8; m * k];
    for (idx, &v) in qg.xq.iter().enumerate() {
        a[idx] = check(v, "xq")?;
    }
    // B := Wq re-laid column-major n×k: column `o` (output
    // channel) at o*k..(o+1)*k holds Wq[o, :]. Then
    // Σ_l a[t,l]·b[o-col,l] = Σ_in Xq[t,in]·Wq[o,in] = the GEMM
    // integer accumulate (= ai-pow/Pearl's mined Σ A·B).
    let mut b = vec![0i8; n * k];
    for o in 0..n {
        for l in 0..k {
            b[o * k + l] = check(qg.wq[o * k + l], "wq")?;
        }
    }
    Ok(PearlOperands {
        m,
        k,
        n,
        a,
        b,
        s_x: qg.s_x.clone(),
        s_w: qg.s_w.clone(),
    })
}

/// The mined integer accumulate (reference): `acc[t,o] =
/// Σ_l A[t,l]·B[o-col,l]`, int32 — exactly the relation
/// ai-pow's `matmul` / Pearl §4.1 type-0 proves (here without
/// noise; the noise `(E,F)` is added by `crate::matmul`).
/// Row-major `m·n`.
pub fn int_matmul(op: &PearlOperands) -> Vec<i32> {
    let (m, k, n) = (op.m, op.k, op.n);
    let mut acc = vec![0i32; m * n];
    for t in 0..m {
        for o in 0..n {
            let mut s = 0i32;
            for l in 0..k {
                s += op.a[t * k + l] as i32 * op.b[o * k + l] as i32;
            }
            acc[t * n + o] = s;
        }
    }
    acc
}

/// Usefulness reconstruction: `Y_fp[t,o] = s_x[t]·s_w[o]·acc[t,o]`
/// — the dequant that recovers the model's true GEMM output from
/// the *mined* integers + `μ`. Row-major `m·n`.
pub fn dequant(op: &PearlOperands, acc: &[i32]) -> Vec<f32> {
    let (m, n) = (op.m, op.n);
    let mut y = vec![0f32; m * n];
    for t in 0..m {
        for o in 0..n {
            y[t * n + o] = op.s_x[t] * op.s_w[o] * acc[t * n + o] as f32;
        }
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic synthetic int7 GEMM (stand-in for a real
    /// vLLM capture — the B2-fixture residual swaps in the real
    /// model's operands + Pearl's digest, see 2026-05-18_PHASE_B_DESIGN.md).
    fn synth_int7_gemm(m: usize, k: usize, n: usize, seed: u64) -> QuantizedGemm {
        let mut st = seed | 1;
        let mut nx = || {
            // xorshift64 → symmetric int7 in [-63, 63] (⊂ [-64,64]).
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            ((st % 127) as i64 - 63) as i8
        };
        let xq = (0..m * k).map(|_| nx()).collect();
        let wq = (0..n * k).map(|_| nx()).collect();
        let s_x = (0..m).map(|i| 0.01 + (i as f32) * 1e-4).collect();
        let s_w = (0..n).map(|j| 0.02 + (j as f32) * 2e-4).collect();
        QuantizedGemm {
            tokens: m,
            in_dim: k,
            out_dim: n,
            xq,
            wq,
            s_x,
            s_w,
        }
    }

    /// **B2.1 (the R1 soundness-adjacent gate): `Q` is
    /// bit-lossless.** The mined integers `int_matmul(extract(qg))`
    /// equal the reference integer GEMM `Σ Xq·Wq` **bit-for-bit**
    /// — extraction adds *zero* error to the mined operand (it is
    /// a pure reindex). This is the exact, checkable form of
    /// "we mine precisely the integers vLLM already computed".
    #[test]
    fn b2_1_extraction_is_bit_lossless_on_the_mined_integers() {
        for (m, k, n, seed) in [(4, 8, 6, 1), (16, 64, 16, 7), (8, 32, 24, 99), (1, 128, 1, 1234)] {
            let qg = synth_int7_gemm(m, k, n, seed);
            let op = extract(&qg).expect("faithful int7 ⇒ in-domain");
            let mined = int_matmul(&op);
            // Reference: the integer GEMM directly off (Xq, Wq).
            let mut want = vec![0i32; m * n];
            for t in 0..m {
                for o in 0..n {
                    let mut s = 0i32;
                    for l in 0..k {
                        s += qg.xq[t * k + l] as i32 * qg.wq[o * k + l] as i32;
                    }
                    want[t * n + o] = s;
                }
            }
            assert_eq!(
                mined, want,
                "extract ∘ int_matmul must equal Σ Xq·Wq bit-for-bit \
                 (m={m},k={k},n={n})"
            );
        }
    }

    /// **B2.2 — usefulness preserved.** `dequant(mined, μ)` equals
    /// the model's reference `Y_fp = s_x·s_w·ΣXqWq` **exactly**
    /// (bit-identical f32: same operation, same order — extraction
    /// introduces no additional numeric error; the only error is
    /// the model's pre-existing INT7 quant, which is the model's).
    #[test]
    fn b2_2_dequant_reconstructs_the_reference_gemm_exactly() {
        let (m, k, n) = (8, 32, 12);
        let qg = synth_int7_gemm(m, k, n, 42);
        let op = extract(&qg).unwrap();
        let got = dequant(&op, &int_matmul(&op));
        let mut want = vec![0f32; m * n];
        for t in 0..m {
            for o in 0..n {
                let mut s = 0i32;
                for l in 0..k {
                    s += qg.xq[t * k + l] as i32 * qg.wq[o * k + l] as i32;
                }
                want[t * n + o] = qg.s_x[t] * qg.s_w[o] * s as f32;
            }
        }
        assert_eq!(got, want, "dequant must reconstruct Y_fp exactly");
    }

    /// **B2.3 (adversarial) — the Pearl type-0 domain is
    /// enforced.** An operand outside `[−64, 64]` (a contract
    /// violation: a faithful int7 model never produces one) is
    /// rejected by `extract`, not silently clipped/mined.
    #[test]
    fn b2_3_out_of_domain_operand_is_rejected() {
        let mut qg = synth_int7_gemm(2, 4, 3, 5);
        qg.wq[5] = 100; // > PEARL_INT_HI
        assert_eq!(
            extract(&qg),
            Err(QuantError::OutOfDomain {
                what: "wq",
                value: 100
            }),
        );
        let mut qg2 = synth_int7_gemm(2, 4, 3, 6);
        qg2.xq[0] = -120; // < PEARL_INT_LO
        assert_eq!(
            extract(&qg2),
            Err(QuantError::OutOfDomain {
                what: "xq",
                value: -120
            }),
        );
        // Shape violations are caught too.
        let mut qg3 = synth_int7_gemm(2, 4, 3, 7);
        qg3.s_w.pop();
        assert!(matches!(
            extract(&qg3),
            Err(QuantError::Shape { what: "s_w", .. })
        ));
    }

    /// **B2.4 — drop-in for the Pearl-digest-parity fixture.**
    /// The extracted `(A, B)` are exactly the
    /// `BlockContext::build` layout, so the B2-fixture residual
    /// (Pearl-gated) is *only*: build a `BlockContext` from
    /// `extract(real_qg).{a,b}` and assert its digest == the real
    /// Pearl miner's. This test pins the layout contract that
    /// makes that a one-fixture swap (no further code).
    #[test]
    fn b2_4_extracted_operands_are_blockcontext_layout() {
        let (m, k, n) = (8, 16, 8);
        let qg = synth_int7_gemm(m, k, n, 2024);
        let op = extract(&qg).unwrap();
        assert_eq!(op.a.len(), m * k, "A is row-major m·k");
        assert_eq!(op.b.len(), n * k, "B is column-major n·k");
        // Column-major B: channel `o` lives contiguously at
        // o*k..(o+1)*k and equals Wq's row `o`.
        for o in 0..n {
            assert_eq!(
                &op.b[o * k..(o + 1) * k],
                &qg.wq[o * k..(o + 1) * k],
                "B column {o} == Wq row {o}"
            );
        }
    }
}
