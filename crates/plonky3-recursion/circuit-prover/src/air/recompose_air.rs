//! [`RecomposeAir`] defines the AIR for the recompose NPO table.
//!
//! Each row packs D base-field witnesses into one extension-field witness.
//! Multiple operations can be packed side-by-side as independent lanes.
//! There are zero local constraints — correctness is enforced entirely
//! by the output cross-table lookup on the WitnessChecks bus.
//!
//! Circuits use two logical tables when extension degree can differ from a base-width Poseidon2:
//! - **`recompose`**: EF output receive only (narrow preprocessed row).
//! - **`recompose/coeff`**: per-coefficient receives for D=1-style readers (plus the EF output receive).
//!
//! # Column layout (per lane)
//!
//! **Main columns** (D per lane): `v_0, v_1, ..., v_{D-1}` — the base-field coefficient values.
//!
//! **Preprocessed columns** per lane:
//! - Always: `output_idx`, `out_mult` (2 columns).
//! - On `recompose/coeff` only: `coeff_i_idx`, `coeff_i_mult` for each `i` (2×D extra).
//!
//! # CTL lookups (per lane per row)
//!
//! **Receive** `[output_idx, v_0, ..., v_{D-1}]` with multiplicity `out_mult`
//!
//! **Receive (coeff)** `[coeff_i_idx, v_i, 0, ..., 0]` with multiplicity `coeff_i_mult` (×D)

use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::{Field, PrimeCharacteristicRing};
use p3_lookup::builder::InteractionBuilder;
use p3_matrix::dense::RowMajorMatrix;
use tracing::instrument;

use super::recompose_columns::{RECOMPOSE_PREP_LANE_COL_MAP, RECOMPOSE_PREP_LANE_WIDTH};

/// AIR for the recompose (BF→EF packing) table.
///
/// Zero local constraints — all correctness is via CTL bus.
#[derive(Debug, Clone)]
pub struct RecomposeAir<F, const D: usize> {
    pub(crate) lanes: usize,
    pub(crate) preprocessed: Vec<F>,
    pub(crate) min_height: usize,
    /// When true, D additional per-coefficient Receive lookups are registered per lane so
    /// that NPOs consuming raw BF coefficients (e.g. a D=1 Poseidon2 challenger inside a
    /// D>1 circuit) find them on the WitnessChecks bus.  When false, only the single EF
    /// output lookup is registered, matching the pre-decoupling layout.
    pub(crate) coeff_lookups: bool,
    _phantom: PhantomData<F>,
}

impl<F: Field + PrimeCharacteristicRing, const D: usize> RecomposeAir<F, D> {
    /// Main trace width per lane: D columns (one per BF coefficient).
    pub const fn lane_width() -> usize {
        D
    }

    /// Preprocessed width per lane.
    ///
    /// Without coefficient lookups: `[output_idx, out_mult]` = 2 columns.
    /// With coefficient lookups: adds `D × (coeff_idx, coeff_mult)`.
    pub const fn preprocessed_lane_width_for(coeff_lookups: bool) -> usize {
        if coeff_lookups {
            RECOMPOSE_PREP_LANE_WIDTH + 2 * D
        } else {
            RECOMPOSE_PREP_LANE_WIDTH
        }
    }

    /// Preprocessed width per lane for this AIR instance.
    pub const fn preprocessed_lane_width(&self) -> usize {
        Self::preprocessed_lane_width_for(self.coeff_lookups)
    }

    /// Create a new `RecomposeAir` with the given preprocessed data and lane count.
    pub fn new_with_preprocessed(
        lanes: usize,
        preprocessed: Vec<F>,
        min_height: usize,
        coeff_lookups: bool,
    ) -> Self {
        Self {
            lanes: lanes.max(1),
            preprocessed,
            min_height,
            coeff_lookups,
            _phantom: PhantomData,
        }
    }

    /// Build the main trace matrix from recompose circuit rows with lane packing.
    #[instrument(skip_all, name = "RecomposeAir::build_trace")]
    pub fn trace_to_matrix(
        rows: &[p3_circuit::ops::recompose::RecomposeCircuitRow<F>],
        lanes: usize,
    ) -> RowMajorMatrix<F> {
        let lane_w = Self::lane_width();
        let row_width = lanes * lane_w;
        let num_ops = rows.len();
        let num_rows = num_ops.div_ceil(lanes).max(1);

        let mut values = F::zero_vec(num_rows * row_width);

        for (op_idx, row) in rows.iter().enumerate() {
            let r = op_idx / lanes;
            let l = op_idx % lanes;
            let base = r * row_width + l * lane_w;
            for (j, &val) in row.values.iter().enumerate() {
                values[base + j] = val;
            }
        }

        let mut mat = RowMajorMatrix::new(values, row_width);
        mat.pad_to_power_of_two_height(F::ZERO);
        mat
    }
}

impl<F: Field, const D: usize> BaseAir<F> for RecomposeAir<F, D> {
    fn width(&self) -> usize {
        self.lanes * Self::lane_width()
    }

    fn preprocessed_width(&self) -> usize {
        self.lanes * self.preprocessed_lane_width()
    }

    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<F>> {
        let width = self.lanes * self.preprocessed_lane_width();
        let mut mat = RowMajorMatrix::from_flat_padded(self.preprocessed.to_vec(), width, F::ZERO);
        mat.pad_to_min_power_of_two_height(self.min_height, F::ZERO);
        Some(mat)
    }

    fn main_next_row_columns(&self) -> Vec<usize> {
        vec![]
    }

    fn preprocessed_next_row_columns(&self) -> Vec<usize> {
        vec![]
    }
}

impl<AB: AirBuilder + InteractionBuilder, const D: usize> Air<AB> for RecomposeAir<AB::F, D>
where
    AB::F: Field,
{
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();
        let main_local = main.current_slice();
        let prep = builder.preprocessed().clone();
        let prep_local = prep.current_slice();

        let lane_w = Self::lane_width();
        let prep_lane_w = self.preprocessed_lane_width();

        for lane in 0..self.lanes {
            let main_off = lane * lane_w;
            let prep_off = lane * prep_lane_w;

            let output_idx: AB::Expr =
                prep_local[prep_off + RECOMPOSE_PREP_LANE_COL_MAP.output_idx].into();
            let out_mult: AB::Expr =
                prep_local[prep_off + RECOMPOSE_PREP_LANE_COL_MAP.out_mult].into();

            let mut values: Vec<AB::Expr> = Vec::with_capacity(1 + D);
            values.push(output_idx);
            for j in 0..D {
                values.push(main_local[main_off + j].into());
            }
            builder.push_interaction("WitnessChecks", values, out_mult, 1);

            // Coefficient Receive lookups: only when the circuit hosts a Poseidon2 permutation
            // whose D differs from the circuit extension degree.
            if self.coeff_lookups {
                for i in 0..D {
                    let coeff_idx: AB::Expr =
                        prep_local[prep_off + RECOMPOSE_PREP_LANE_WIDTH + i * 2].into();
                    let coeff_mult: AB::Expr =
                        prep_local[prep_off + RECOMPOSE_PREP_LANE_WIDTH + i * 2 + 1].into();

                    let mut coeff_values: Vec<AB::Expr> = Vec::with_capacity(1 + D);
                    coeff_values.push(coeff_idx);
                    coeff_values.push(main_local[main_off + i].into());
                    for _ in 1..D {
                        coeff_values.push(AB::Expr::ZERO);
                    }
                    builder.push_interaction("WitnessChecks", coeff_values, coeff_mult, 1);
                }
            }
        }
    }
}
