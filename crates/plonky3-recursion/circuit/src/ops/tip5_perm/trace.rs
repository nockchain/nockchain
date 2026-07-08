//! Tip5 trace types and trace generation (C2.3).
//!
//! Mirrors `poseidon1_perm::trace` reduced to the single deployed
//! Goldilocks D=1 width-16 configuration. `Tip5CircuitRow` is defined
//! in `p3-tip5-circuit-air` (alongside the wrapper AIR that consumes
//! it) and re-exported here, exactly as `poseidon1_perm::trace`
//! re-exports `p3_poseidon1_circuit_air::Poseidon1CircuitRow`.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;

use p3_field::{ExtensionField, Field, PrimeCharacteristicRing, PrimeField};

use crate::CircuitError;
use crate::ops::NpoTypeId;
use crate::ops::tip5_perm::config::Tip5Config;
use crate::ops::tip5_perm::state::Tip5ExecutionState;
use crate::tables::NonPrimitiveTrace;

/// Tip5 configuration parameters for a field type. Mirrors
/// `Poseidon1Params`, Tip5-shaped (D always 1, width 16, rate 10).
pub trait Tip5Params {
    /// The base prime field the Tip5 permutation operates over.
    type BaseField: PrimeField + PrimeCharacteristicRing;
    /// The single deployed Tip5 configuration.
    const CONFIG: Tip5Config;
    /// Extension degree (always 1 — Tip5 is base-field only).
    const D: usize = Self::CONFIG.d();
    /// State width in base-field elements (16).
    const WIDTH: usize = Self::CONFIG.width();
    /// Rate in base-field elements (10).
    const RATE: usize = Self::CONFIG.rate();
}

/// Goldilocks D=1 Width=16 Rate=10 — the only deployed Tip5 config.
pub struct Tip5Goldilocks;

impl Tip5Params for Tip5Goldilocks {
    type BaseField = p3_goldilocks::Goldilocks;
    const CONFIG: Tip5Config = Tip5Config::GOLDILOCKS_W16;
}

pub use p3_tip5_circuit_air::Tip5CircuitRow;

/// Tip5 trace for all Tip5 permutation operations in the circuit.
#[derive(Debug, Clone)]
pub struct Tip5Trace<F> {
    /// Operation type for this Tip5 trace.
    pub op_type: NpoTypeId,
    /// All Tip5 operations (permutation rows) in this trace.
    pub operations: Vec<Tip5CircuitRow<F>>,
}

impl<F> Tip5Trace<F> {
    pub const fn total_rows(&self) -> usize {
        self.operations.len()
    }
}

impl<TraceF: Clone + Send + Sync + 'static, CF> NonPrimitiveTrace<CF> for Tip5Trace<TraceF> {
    fn op_type(&self) -> NpoTypeId {
        self.op_type.clone()
    }

    fn rows(&self) -> usize {
        self.total_rows()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn boxed_clone(&self) -> Box<dyn NonPrimitiveTrace<CF>> {
        Box::new(self.clone())
    }
}

/// Generate the Tip5 trace from execution state, converting the
/// circuit field's embedded-base coefficients to base-field rows.
///
/// Mirrors `generate_poseidon1_trace` for the D=1 path: Tip5 is D=1,
/// so each state slot is one base element (the constant coefficient
/// when the circuit field is an extension of `BaseField`).
pub fn generate_tip5_trace<F: Field + ExtensionField<Config::BaseField>, Config: Tip5Params>(
    op_states: &crate::ops::OpStateMap,
) -> Result<Option<Box<dyn NonPrimitiveTrace<F>>>, CircuitError> {
    let op_type = NpoTypeId::tip5_perm(Config::CONFIG);
    let Some(state) = op_states
        .get(&op_type)
        .and_then(|s| s.downcast_ref::<Tip5ExecutionState<F>>())
    else {
        return Ok(None);
    };

    if state.rows.is_empty() {
        return Ok(None);
    }

    let width = Config::WIDTH;

    let operations: Vec<Tip5CircuitRow<Config::BaseField>> = state
        .rows
        .iter()
        .map(|row| {
            debug_assert_eq!(
                row.input_values.len(),
                width,
                "Tip5 execution row must have WIDTH (16) input elements"
            );
            let mut input_values = vec![Config::BaseField::ZERO; width];
            for (slot, ext_val) in input_values.iter_mut().zip(&row.input_values) {
                // D=1: take the constant basis coefficient.
                *slot = ext_val.as_basis_coefficients_slice()[0];
            }
            Tip5CircuitRow {
                new_start: row.new_start,
                input_values,
                in_ctl: row.in_ctl.clone(),
                input_indices: row.input_indices.clone(),
                out_ctl: row.out_ctl.clone(),
                output_indices: row.output_indices.clone(),
                mmcs_bit_ctl: row.mmcs_bit_ctl,
                mmcs_bit_index: row.mmcs_bit_index,
                mmcs_bit: row.mmcs_bit,
            }
        })
        .collect();

    Ok(Some(Box::new(Tip5Trace {
        op_type,
        operations,
    })))
}
