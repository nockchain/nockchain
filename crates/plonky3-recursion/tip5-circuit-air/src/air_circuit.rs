//! Circuit-prover wrapper AIR for the lookup-table 5-round Tip5
//! permutation (C2.3 / M-S4).
//!
//! This is the **faithful mechanical mirror** of the Poseidon1 D=1
//! circuit-AIR → batch-STARK bridge, specialised to recursive Tip5
//! sponge geometry (Goldilocks, D=1, width 16, rate 10, capacity 6,
//! digest 5, 5 rounds) and to the *existing*
//! [`crate::Tip5PermLookupAir`] constraint system. Nothing here alters
//! the degree-2-proven `tip5_l` LogUp bus, the algebraic constraints,
//! or the verifier-fixed 256-row L-table — that AIR's `eval` is reused
//! **verbatim** via composition.
//!
//! What this module adds is the cross-table `WitnessChecks` CTL layer
//! and one wrapper-owned MMCS direction-bit column. The CTL layer
//! connects the permutation's
//! `IN[0..16]` (consumed) and `ROUT[NUM_ROUNDS-1][0..10]` rate outputs (produced)
//! to the rest of the circuit's witness bus, exactly as
//! `p3-poseidon1-circuit-air`'s `eval_interactions` does for the
//! compact D=1 path: per-input-limb **sends** with multiplicity
//! `-in_ctl`, per-rate-output **receives** with multiplicity
//! `out_ctl`, both gated by the row-kind selector so the L-table /
//! padding rows contribute nothing to the bus. CTL metadata is placed
//! on the verifier-fixed final-round row of each permutation so
//! `TIN` is the permutation input and `ROUT` is the final output.
//!
//! Trace shape is [`crate::generate_lookup_trace`] plus one ignored-by-inner
//! `mmcs_bit` column: rows `[0,256)` = L-table, rows `[256,256+P)` = one full
//! Tip5 evaluation each, the rest inert padding. The matching preprocessed
//! trace carries the verifier-fixed L-table/round-selector columns
//! `[IS_TABLE, TIN, TOUT, IS_ROUND, ROUND[0..5]]` *and* the per-perm-row CTL
//! columns `[in_ctl[16], in_idx[16], out_idx[10], out_ctl[10],
//! mmcs_bit_ctl, mmcs_bit_idx]`.

use alloc::vec::Vec;

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::{Field, PrimeCharacteristicRing};
use p3_goldilocks::Goldilocks;
use p3_lookup::builder::InteractionBuilder;
use p3_matrix::{Matrix, dense::RowMajorMatrix};

use crate::air_lookup::{
    PREP_WIDTH as L_PREP_WIDTH, TABLE_ROWS, Tip5PermLookupAir, tip5_in_col,
    tip5_lookup_air_width, tip5_out_col,
};
use crate::generation_lookup::generate_lookup_trace;
use crate::tip5_spec::{NUM_ROUNDS, STATE_SIZE};

/// Sponge rate (squeezed/CTL-exposed output lanes) of recursive
/// Tip5: `PaddingFreeSponge<_,16,10,5>` / `DuplexChallenger<_,_,16,10>`.
pub const TIP5_RATE: usize = 10;
/// Tip5 state width in base-field elements.
pub const TIP5_WIDTH: usize = STATE_SIZE; // 16
/// Tip5 Merkle digest width in base-field elements.
pub const TIP5_DIGEST: usize = 5;

/// Per-perm-row CTL preprocessed columns appended after the lookup AIR's
/// L-table/round-selector columns: `in_ctl[16] | in_idx[16] |
/// out_idx[10] | out_ctl[10] | mmcs_bit_ctl | mmcs_bit_idx`.
/// (Mirrors the Poseidon1 compact-D1 header/idx/ctl columns,
/// Tip5-shaped: no merkle/chaining selectors — Tip5 sponge chaining is
/// realised by the executor carrying the previous full state into
/// `IN`, not by per-limb preprocessed chain selectors.)
pub const TIP5_CTL_PREP_COLS: usize = TIP5_WIDTH + TIP5_WIDTH + TIP5_RATE + TIP5_RATE + 2; // 54

/// Total preprocessed width: lookup AIR columns + CTL columns.
pub const TIP5_CIRCUIT_PREP_WIDTH: usize = L_PREP_WIDTH + TIP5_CTL_PREP_COLS; // 9 + 54 = 63

/// Extra main-trace columns appended after the validated lookup AIR's
/// columns. The lookup AIR ignores these; the wrapper AIR uses them
/// only for MMCS-direction binding and direction-aware input CTL value
/// selection.
pub const TIP5_CIRCUIT_EXTRA_MAIN_COLS: usize = 1;

// CTL column offsets *within* the appended block (after `L_PREP_WIDTH`).
const CTL_IN_CTL: usize = 0;
const CTL_IN_IDX: usize = CTL_IN_CTL + TIP5_WIDTH;
const CTL_OUT_IDX: usize = CTL_IN_IDX + TIP5_WIDTH;
const CTL_OUT_CTL: usize = CTL_OUT_IDX + TIP5_RATE;
const CTL_MMCS_BIT_CTL: usize = CTL_OUT_CTL + TIP5_RATE;
const CTL_MMCS_BIT_IDX: usize = CTL_MMCS_BIT_CTL + 1;
/// Verifier-fixed permutation-row selector in the embedded lookup AIR
/// preprocessed columns. Mirrors `air_lookup::P_IS_ROUND`.
const L_PREP_IS_ROUND: usize = 3;

/// Per-permutation row description captured by the Tip5 NPO executor
/// and consumed by trace + preprocessed generation.
///
/// Mirrors `p3_poseidon1_circuit_air::Poseidon1CircuitRow`, reduced to
/// the Tip5 D=1 / non-merkle fields actually used.
#[derive(Clone, Debug)]
pub struct Tip5CircuitRow<F> {
    /// True if this permutation begins a fresh sponge chain (no
    /// previous-state carry). Recorded for parity with the Poseidon1
    /// mirror / debugging; the actual state carry is performed by the
    /// executor, so the lookup-AIR trace already contains the resolved
    /// `IN`.
    pub new_start: bool,
    /// Flattened 16-element Tip5 permutation input state (post chain
    /// resolution + CTL overwrite), exactly as fed to the permutation.
    pub input_values: Vec<F>,
    /// Per-input-limb CTL exposure flags (length 16).
    pub in_ctl: Vec<bool>,
    /// Per-input-limb CTL witness indices (length 16).
    pub input_indices: Vec<u32>,
    /// Per-rate-output-limb CTL exposure flags (length 10).
    pub out_ctl: Vec<bool>,
    /// Per-rate-output-limb CTL witness indices (length 10).
    pub output_indices: Vec<u32>,
    /// True when the MMCS direction bit is an explicit circuit input
    /// for this Tip5 row.
    pub mmcs_bit_ctl: bool,
    /// MMCS direction bit witness index when `mmcs_bit_ctl` is true.
    pub mmcs_bit_index: u32,
    /// Resolved MMCS direction bit value used by the executor. Stored
    /// in an extra main-trace column and bound to `mmcs_bit_index` on
    /// the `WitnessChecks` bus when `mmcs_bit_ctl` is true.
    pub mmcs_bit: bool,
}

/// Build the `[u64;16]` permutation inputs for
/// [`generate_lookup_trace`] from captured circuit rows.
pub fn tip5_inputs_from_rows<F: Field + p3_field::PrimeField64>(
    rows: &[Tip5CircuitRow<F>],
) -> Vec<[u64; STATE_SIZE]> {
    rows.iter()
        .map(|r| {
            debug_assert_eq!(r.input_values.len(), STATE_SIZE);
            core::array::from_fn(|i| r.input_values[i].as_canonical_u64())
        })
        .collect()
}

/// Build the full circuit main trace: the validated lookup AIR trace
/// plus one wrapper-owned `mmcs_bit` column.
pub fn build_tip5_circuit_main_with_mmcs_bits<F>(
    lookup_main: RowMajorMatrix<Goldilocks>,
    rows: &[Tip5CircuitRow<F>],
) -> RowMajorMatrix<Goldilocks> {
    let lookup_width = lookup_main.width();
    let height = lookup_main.height();
    let width = lookup_width + TIP5_CIRCUIT_EXTRA_MAIN_COLS;
    let mut values = Goldilocks::zero_vec(height * width);

    for r in 0..height {
        let src = r * lookup_width;
        let dst = r * width;
        values[dst..dst + lookup_width].copy_from_slice(&lookup_main.values[src..src + lookup_width]);
    }

    let mmcs_col = lookup_width;
    for (pi, row) in rows.iter().enumerate() {
        let trace_row = TABLE_ROWS + pi * NUM_ROUNDS + (NUM_ROUNDS - 1);
        values[trace_row * width + mmcs_col] = Goldilocks::from_bool(row.mmcs_bit);
    }

    RowMajorMatrix::new(values, width)
}

/// Build the full circuit preprocessed flat vector (row-major,
/// `TIP5_CIRCUIT_PREP_WIDTH` wide, height = padded lookup-trace
/// height): the verifier-fixed L-table on rows `[0,256)` and the
/// per-perm-row CTL columns on each permutation's final round row.
///
/// `idx_scale` is the witness-bus extension scale (always 1 for the
/// base-field Tip5 circuit; kept for mirror parity with Poseidon1's
/// `d`). `out_mult`/`in` reader accounting is applied later by
/// [`Tip5CircuitPreprocessing`] using `ext_reads`; this only lays out
/// the raw indices/flags and the L-table.
pub fn build_tip5_circuit_preprocessed<F: Field>(
    l_table_preprocessed: &[Goldilocks],
    rows: &[Tip5CircuitRow<F>],
    height: usize,
    idx_scale: u32,
) -> Vec<F> {
    let width = TIP5_CIRCUIT_PREP_WIDTH;
    let mut prep = F::zero_vec(height * width);

    // Lookup-AIR preprocessed columns [0,L_PREP_WIDTH) on every row:
    // table rows, round selectors, and padding. The lookup-AIR's own
    // preprocessed is Goldilocks; transmute-free copy through the
    // field's canonical u64 (Goldilocks -> F is the identity here since
    // the Tip5 circuit field is Goldilocks, but we go through the field
    // API to keep this generic and lint-clean).
    debug_assert!(l_table_preprocessed.len() >= height * L_PREP_WIDTH);
    for r in 0..height {
        let src = r * L_PREP_WIDTH;
        let dst = r * width;
        for c in 0..L_PREP_WIDTH {
            let v = <Goldilocks as p3_field::PrimeField64>::as_canonical_u64(
                &l_table_preprocessed[src + c],
            );
            prep[dst + c] = F::from_u64(v);
        }
    }

    // Per-perm-row CTL columns on each permutation's final round row.
    // The input `TIN` is carried across all five rows, but `ROUT` is
    // the full permutation output only on the final round.
    for (pi, row) in rows.iter().enumerate() {
        debug_assert_eq!(row.in_ctl.len(), TIP5_WIDTH);
        debug_assert_eq!(row.input_indices.len(), TIP5_WIDTH);
        debug_assert_eq!(row.out_ctl.len(), TIP5_RATE);
        debug_assert_eq!(row.output_indices.len(), TIP5_RATE);
        let trace_row = TABLE_ROWS + pi * NUM_ROUNDS + (NUM_ROUNDS - 1);
        let base = trace_row * width + L_PREP_WIDTH;
        for i in 0..TIP5_WIDTH {
            prep[base + CTL_IN_CTL + i] = F::from_bool(row.in_ctl[i]);
            prep[base + CTL_IN_IDX + i] = F::from_u32(row.input_indices[i] * idx_scale);
        }
        for i in 0..TIP5_RATE {
            prep[base + CTL_OUT_IDX + i] = F::from_u32(row.output_indices[i] * idx_scale);
            prep[base + CTL_OUT_CTL + i] = F::from_bool(row.out_ctl[i]);
        }
        prep[base + CTL_MMCS_BIT_CTL] = F::from_bool(row.mmcs_bit_ctl);
        prep[base + CTL_MMCS_BIT_IDX] = F::from_u32(row.mmcs_bit_index * idx_scale);
    }

    prep
}

/// Build the matching main trace via the *unmodified*
/// [`generate_lookup_trace`] and return both it and the L-table
/// preprocessed (Goldilocks). The circuit preprocessed is then built
/// by [`build_tip5_circuit_preprocessed`] so the heights agree.
pub fn generate_tip5_circuit_main(
    inputs: &[[u64; STATE_SIZE]],
) -> (RowMajorMatrix<Goldilocks>, Vec<Goldilocks>) {
    generate_lookup_trace(inputs)
}

/// The circuit-prover Tip5 AIR: composes [`Tip5PermLookupAir`] (its
/// `tip5_l` LogUp bus + algebraic constraints + verifier-fixed L-table,
/// reused verbatim) with the `WitnessChecks` cross-table CTL on the
/// permutation rows.
/// `WITNESS_EXT_D` is the **circuit's** witness-bus extension degree
/// (1 for the standalone base-field Tip5 circuit; 2 for the D=2
/// Goldilocks-challenge Layer-0 recursion verifier circuit; 5 kept
/// for mirror parity with Poseidon1's quintic witness-bus). Tip5 is
/// an intrinsically D=1 (base-Goldilocks) permutation, so each CTL
/// limb's value is a *single* base element; the `WitnessChecks` bus
/// tuple is D-padded to `[idx, value, ZERO×(WITNESS_EXT_D − 1)]` so
/// it matches the recompose/witness-bus producers in a D≥2 circuit —
/// a faithful mirror of `p3_poseidon1_circuit_air`'s
/// `eval_interactions<…, const WITNESS_EXT_D>` D-padding (there each
/// limb pushes `D` value coordinates then `WITNESS_EXT_D − D` zeros;
/// for Tip5 the perm `D == 1`, so the value contributes exactly one
/// coordinate and the pad is `WITNESS_EXT_D − 1`). The default `= 1`
/// keeps every existing `Tip5CircuitAir<F>` reference byte-identical
/// (the pad loop runs zero times ⇒ the emitted tuple is exactly the
/// previous `[idx, value]`).
#[derive(Debug, Clone)]
pub struct Tip5CircuitAir<F, const WITNESS_EXT_D: usize = 1> {
    inner: Tip5PermLookupAir<F>,
    /// Full preprocessed (L-table ++ CTL), row-major
    /// `TIP5_CIRCUIT_PREP_WIDTH` wide.
    preprocessed: Vec<F>,
    min_height: usize,
}

impl<F: Field, const WITNESS_EXT_D: usize> Tip5CircuitAir<F, WITNESS_EXT_D> {
    /// Construct from the full circuit preprocessed flat vector.
    ///
    /// The first `L_PREP_WIDTH` columns of every row are the L-table
    /// columns; they are sliced back out and handed to the inner
    /// [`Tip5PermLookupAir`] so its constraint code sees exactly the
    /// preprocessed shape it was validated against.
    pub fn new_with_preprocessed(preprocessed: Vec<F>, min_height: usize) -> Self {
        let l_only = Self::slice_l_table(&preprocessed);
        Self {
            inner: Tip5PermLookupAir::new(l_only),
            preprocessed,
            min_height: min_height.max(1),
        }
    }

    fn slice_l_table(full: &[F]) -> Vec<F> {
        if full.is_empty() {
            return Vec::new();
        }
        debug_assert!(full.len().is_multiple_of(TIP5_CIRCUIT_PREP_WIDTH));
        let rows = full.len() / TIP5_CIRCUIT_PREP_WIDTH;
        let mut l = F::zero_vec(rows * L_PREP_WIDTH);
        for r in 0..rows {
            let src = r * TIP5_CIRCUIT_PREP_WIDTH;
            let dst = r * L_PREP_WIDTH;
            l[dst..dst + L_PREP_WIDTH].copy_from_slice(&full[src..src + L_PREP_WIDTH]);
        }
        l
    }

    /// Padded power-of-two height honouring `min_height`, for the
    /// full preprocessed matrix.
    fn padded_height(&self) -> usize {
        let rows = if self.preprocessed.is_empty() {
            0
        } else {
            self.preprocessed.len() / TIP5_CIRCUIT_PREP_WIDTH
        };
        rows.max(1)
            .next_power_of_two()
            .max(self.min_height.next_power_of_two())
    }
}

impl<F: Field, const WITNESS_EXT_D: usize> BaseAir<F> for Tip5CircuitAir<F, WITNESS_EXT_D> {
    fn width(&self) -> usize {
        tip5_lookup_air_width() + TIP5_CIRCUIT_EXTRA_MAIN_COLS
    }

    fn preprocessed_width(&self) -> usize {
        TIP5_CIRCUIT_PREP_WIDTH
    }

    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<F>> {
        let width = TIP5_CIRCUIT_PREP_WIDTH;
        let padded_h = self.padded_height();
        let mut data = self.preprocessed.clone();
        data.resize(padded_h * width, F::ZERO);
        Some(RowMajorMatrix::new(data, width))
    }

    fn max_constraint_degree(&self) -> Option<usize> {
        // Return `None` so `p3-batch-stark` computes the *true*
        // symbolic degree of the composed AIR (inner degree-4
        // algebraic + inner `tip5_l` LogUp + the added `WitnessChecks`
        // CTL whose multiplicity is `in_ctl·kind`). Hard-coding the
        // inner's `Some(4)` hint omitted the wrapper's CTL LogUp
        // degree, so the prover committed a quotient of the wrong
        // degree while the verifier recomputed a larger one ⇒
        // `OodEvaluationMismatch`. Letting batch-stark infer it keeps
        // prover and verifier on the identical quotient degree.
        None
    }

    fn main_next_row_columns(&self) -> Vec<usize> {
        // The composed `Tip5PermLookupAir` links each permutation round
        // to the next row through `main.next_slice()`. Batch STARK currently
        // treats this vector as a boolean and opens the full next row when it
        // is non-empty, so expose every column here.
        (0..self.width()).collect()
    }

    fn preprocessed_next_row_columns(&self) -> Vec<usize> {
        // The lookup table selectors and witness-check controls are read
        // only from the current preprocessed row.
        Vec::new()
    }
}

impl<AB, const WITNESS_EXT_D: usize> Air<AB> for Tip5CircuitAir<AB::F, WITNESS_EXT_D>
where
    AB: AirBuilder + InteractionBuilder,
    AB::F: Field,
{
    fn eval(&self, builder: &mut AB) {
        // ---- 1. reuse the validated lookup-AIR constraints + bus ----
        // The inner AIR reads `builder.preprocessed()` columns
        // `[IS_TABLE, TIN, TOUT]` (the first `L_PREP_WIDTH`). Our
        // preprocessed matrix has those as its first columns, so the
        // inner `eval` sees exactly the validated shape; the extra CTL
        // columns are simply ignored by it.
        Air::<AB>::eval(&self.inner, builder);

        // ---- 2. add the WitnessChecks cross-table CTL layer ----
        // (Re-borrow the windows; the inner eval already released them.)
        let main = builder.main();
        let prep = builder.preprocessed().clone();
        let local = main.current_slice();
        let pre = prep.current_slice();

        let kind: AB::Expr = pre[L_PREP_IS_ROUND].into();
        let cbase = L_PREP_WIDTH;
        let mmcs_bit_ctl: AB::Expr = pre[cbase + CTL_MMCS_BIT_CTL].into();
        let mmcs_bit_idx: AB::Expr = pre[cbase + CTL_MMCS_BIT_IDX].into();
        let mmcs_bit: AB::Expr = local[tip5_lookup_air_width()].into();
        let active_mmcs_bit = mmcs_bit_ctl.clone() * mmcs_bit.clone();

        builder.assert_zero(
            kind.clone()
                * mmcs_bit_ctl.clone()
                * mmcs_bit.clone()
                * (mmcs_bit.clone() - AB::Expr::ONE),
        );

        let mut bit_lookup: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
        bit_lookup.push(mmcs_bit_idx);
        bit_lookup.push(mmcs_bit.clone());
        for _ in 0..(WITNESS_EXT_D - 1) {
            bit_lookup.push(AB::Expr::ZERO);
        }
        builder.push_interaction(
            "WitnessChecks",
            bit_lookup,
            -(mmcs_bit_ctl.clone() * kind.clone()),
            1,
        );

        // Input limb SENDS: `[idx, value, ZERO×(WITNESS_EXT_D − 1)]`,
        // multiplicity `-(in_ctl * kind)`. `kind` (boolean, asserted by
        // the inner AIR's verifier-fixed preprocessed selectors) zeroes
        // the bus contribution on L-table / padding rows. The tuple is
        // D-padded to `WITNESS_EXT_D + 1` exactly as
        // `p3_poseidon1_circuit_air::eval_interactions`
        // (input limb sends): `push(idx)`, then the perm's `D` value
        // coordinates, then `WITNESS_EXT_D − D` zeros. Tip5's perm
        // `D == 1`, so the value contributes a single coordinate and
        // the pad count is `WITNESS_EXT_D − 1`. At `WITNESS_EXT_D == 1`
        // the pad loop runs zero times ⇒ the emitted tuple is
        // byte-identical to the previous `[idx, value]`.
        for i in 0..TIP5_WIDTH {
            let idx: AB::Expr = pre[cbase + CTL_IN_IDX + i].into();
            let in_ctl: AB::Expr = pre[cbase + CTL_IN_CTL + i].into();
            let mut value: AB::Expr = local[tip5_in_col(i)].into();
            if i < 2 * TIP5_DIGEST {
                let swap_i = if i < TIP5_DIGEST {
                    i + TIP5_DIGEST
                } else {
                    i - TIP5_DIGEST
                };
                let swapped: AB::Expr = local[tip5_in_col(swap_i)].into();
                value = value.clone() + active_mmcs_bit.clone() * (swapped - value);
            }
            let mult = in_ctl * kind.clone();
            let mut input_idx_limb: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
            input_idx_limb.push(idx);
            input_idx_limb.push(value);
            for _ in 0..(WITNESS_EXT_D - 1) {
                input_idx_limb.push(AB::Expr::ZERO);
            }
            builder.push_interaction("WitnessChecks", input_idx_limb, -mult, 1);
        }

        // Rate output limb RECEIVES: `[idx, value, ZERO×(WITNESS_EXT_D
        // − 1)]`, multiplicity `out_ctl * kind` (the resolved
        // per-witness read count is baked into `out_ctl` by the
        // preprocessor; `kind` gates rows). Same D-padding as the
        // poseidon1 output-limb receives (`push(idx)`, `D` value
        // coords, `WITNESS_EXT_D − D` zeros; Tip5 perm `D == 1`).
        for i in 0..TIP5_RATE {
            let idx: AB::Expr = pre[cbase + CTL_OUT_IDX + i].into();
            let out_ctl: AB::Expr = pre[cbase + CTL_OUT_CTL + i].into();
            let value: AB::Expr = local[tip5_out_col(i)].into();
            let mult = out_ctl * kind.clone();
            let mut output_idx_limb: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
            output_idx_limb.push(idx);
            output_idx_limb.push(value);
            for _ in 0..(WITNESS_EXT_D - 1) {
                output_idx_limb.push(AB::Expr::ZERO);
            }
            builder.push_interaction("WitnessChecks", output_idx_limb, mult, 1);
        }
    }
}
