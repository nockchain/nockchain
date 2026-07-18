//! Poseidon1 trace types and trace generation.

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;

use p3_field::{ExtensionField, Field, PrimeCharacteristicRing, PrimeField};

use crate::CircuitError;
use crate::ops::NpoTypeId;
use crate::ops::poseidon1_perm::config::Poseidon1Config;
use crate::ops::poseidon1_perm::state::Poseidon1ExecutionState;
use crate::tables::NonPrimitiveTrace;
use crate::types::NonPrimitiveOpId;

/// Poseidon1 configuration parameters for a field type.
pub trait Poseidon1Params {
    type BaseField: PrimeField + PrimeCharacteristicRing;
    const CONFIG: Poseidon1Config;
    const D: usize = Self::CONFIG.d();
    const WIDTH: usize = Self::CONFIG.width();
    const RATE_EXT: usize = Self::CONFIG.rate_ext();
    const CAPACITY_EXT: usize = Self::CONFIG.capacity_ext();
    const CAPACITY_SIZE: usize = Self::CAPACITY_EXT * Self::D;
    const SBOX_DEGREE: u64 = Self::CONFIG.sbox_degree();
    const SBOX_REGISTERS: usize = Self::CONFIG.sbox_registers();
    const HALF_FULL_ROUNDS: usize = Self::CONFIG.half_full_rounds();
    const PARTIAL_ROUNDS: usize = Self::CONFIG.partial_rounds();
    const WIDTH_EXT: usize = Self::RATE_EXT + Self::CAPACITY_EXT;
}

/// BabyBear D=1 Width=16 configuration for base field challenges.
pub struct BabyBearD1Width16;

impl Poseidon1Params for BabyBearD1Width16 {
    type BaseField = p3_baby_bear::BabyBear;
    const CONFIG: Poseidon1Config = Poseidon1Config::BABY_BEAR_D1_W16;
}

/// KoalaBear D=1 Width=16 configuration for base field challenges.
pub struct KoalaBearD1Width16;

impl Poseidon1Params for KoalaBearD1Width16 {
    type BaseField = p3_koala_bear::KoalaBear;
    const CONFIG: Poseidon1Config = Poseidon1Config::KOALA_BEAR_D1_W16;
}

/// BabyBear D=4 Width=16 configuration (quartic challenge extension).
pub struct BabyBearD4Width16;

impl Poseidon1Params for BabyBearD4Width16 {
    type BaseField = p3_baby_bear::BabyBear;
    const CONFIG: Poseidon1Config = Poseidon1Config::BABY_BEAR_D4_W16;
}

/// KoalaBear D=4 Width=16 configuration (quartic challenge extension).
pub struct KoalaBearD4Width16;

impl Poseidon1Params for KoalaBearD4Width16 {
    type BaseField = p3_koala_bear::KoalaBear;
    const CONFIG: Poseidon1Config = Poseidon1Config::KOALA_BEAR_D4_W16;
}

/// Goldilocks D=2 Width=8 configuration (matches Poseidon1Goldilocks<8>).
pub struct GoldilocksD2Width8;

impl Poseidon1Params for GoldilocksD2Width8 {
    type BaseField = p3_goldilocks::Goldilocks;
    const CONFIG: Poseidon1Config = Poseidon1Config::GOLDILOCKS_D2_W8;
}

pub use p3_poseidon1_circuit_air::Poseidon1CircuitRow;

/// Poseidon1 trace for all hash operations in the circuit.
#[derive(Debug, Clone)]
pub struct Poseidon1Trace<F> {
    /// Operation type for this Poseidon1 trace.
    pub op_type: NpoTypeId,
    /// All Poseidon1 operations (permutation rows) in this trace.
    pub operations: Vec<Poseidon1CircuitRow<F>>,
}

impl<F> Poseidon1Trace<F> {
    pub const fn total_rows(&self) -> usize {
        self.operations.len()
    }
}

impl<TraceF: Clone + Send + Sync + 'static, CF> NonPrimitiveTrace<CF> for Poseidon1Trace<TraceF> {
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

/// Generate the Poseidon1 trace from execution state, converting extension-field
/// rows to base-field rows.
pub fn generate_poseidon1_trace<
    F: Field + ExtensionField<Config::BaseField>,
    Config: Poseidon1Params,
>(
    op_states: &crate::ops::OpStateMap,
) -> Result<Option<Box<dyn NonPrimitiveTrace<F>>>, CircuitError> {
    let op_type = NpoTypeId::poseidon1_perm(Config::CONFIG);
    let Some(state) = op_states
        .get(&op_type)
        .and_then(|s| s.downcast_ref::<Poseidon1ExecutionState<F>>())
    else {
        return Ok(None);
    };

    if state.rows.is_empty() {
        return Ok(None);
    }

    let d = Config::D;

    let operations: Vec<Poseidon1CircuitRow<Config::BaseField>> = state
        .rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| -> Result<_, CircuitError> {
            let limb_count = Config::WIDTH / d;
            assert_eq!(
                row.input_values.len(),
                limb_count,
                "Source row must have WIDTH/D input limbs"
            );
            let mut input_values = vec![Config::BaseField::ZERO; Config::WIDTH];
            assert_eq!(
                input_values.len(),
                Config::WIDTH,
                "Target row must have WIDTH input elements"
            );
            for (limb, ext_val) in row.input_values.iter().enumerate() {
                let coeffs = ext_val.as_basis_coefficients_slice();
                if d == 1 {
                    // D=1 AIR consumes one base element per state slot. When the circuit field is an
                    // extension of `BaseField`, embedded-base semantics use the constant coefficient.
                    input_values[limb] = coeffs[0];
                } else {
                    input_values[limb * d..(limb + 1) * d].copy_from_slice(coeffs);
                }
            }

            let mmcs_index_sum = row.mmcs_index_sum.as_base().ok_or_else(|| {
                CircuitError::IncorrectNonPrimitiveOpPrivateData {
                    op: op_type.clone(),
                    operation_index: NonPrimitiveOpId(row_index as u32),
                    expected: "base field mmcs_index_sum".to_string(),
                    got: "extension value".to_string(),
                }
            })?;

            Ok(Poseidon1CircuitRow {
                new_start: row.new_start,
                merkle_path: row.merkle_path,
                mmcs_bit: row.mmcs_bit,
                mmcs_index_sum,
                input_values,
                in_ctl: row.in_ctl.clone(),
                input_indices: row.input_indices.clone(),
                out_ctl: row.out_ctl.clone(),
                output_indices: row.output_indices.clone(),
                mmcs_index_sum_idx: row.mmcs_index_sum_idx,
                mmcs_ctl_enabled: row.mmcs_ctl_enabled,
            })
        })
        .collect::<Result<Vec<_>, CircuitError>>()?;

    Ok(Some(Box::new(Poseidon1Trace {
        op_type,
        operations,
    })))
}
