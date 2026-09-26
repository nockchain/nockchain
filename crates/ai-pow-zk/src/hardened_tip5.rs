//! Height-selected Tip5 bindings for recursive verification.
//!
//! Proving, setup reconstruction, recursive verification, and native compact
//! verification use the same state bindings and row constraints.

use hashbrown::HashMap;
use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_batch_stark::{StarkGenericConfig, Val};
use p3_circuit::ops::{NpoTypeId, Op, Tip5Config};
use p3_circuit::tables::Traces;
use p3_circuit::{Circuit, CircuitError, WitnessId};
use p3_circuit_prover::common::{CircuitTableAir, NpoAirBuilder};
use p3_circuit_prover::config::StarkField;
use p3_circuit_prover::{
    BatchAir, BatchStarkProver, BatchTableInstance, ConstraintProfile, DynamicAirEntry,
    NonPrimitiveTableEntry, TablePacking, TableProver, Tip5AirBuilder, Tip5Prover,
};
use p3_field::extension::BinomialExtensionField;
use p3_field::{Algebra, Field, PrimeCharacteristicRing};
use p3_lookup::InteractionBuilder;
use p3_matrix::dense::RowMajorMatrix;
use p3_tip5_circuit_air::{Tip5CircuitAir, TIP5_CIRCUIT_PREP_WIDTH};
use p3_uni_stark::{SymbolicExpression, SymbolicExpressionExt};

use crate::proof_rules::ProofRules;

const WIDTH: usize = 16;
const DIGEST: usize = 5;

/// Bind carried state before preprocessing so the committed lookup
/// multiplicities include its wires. Operation IDs are preserved.
pub(crate) fn bind_state<F: Field>(circuit: &mut Circuit<F>) -> Result<(), CircuitError> {
    let allocate = |count: &mut u32| -> Result<WitnessId, CircuitError> {
        let id = WitnessId(*count);
        *count = count
            .checked_add(1)
            .ok_or(CircuitError::InvalidPreprocessedValues)?;
        Ok(id)
    };
    let zero = allocate(&mut circuit.witness_count)?;
    circuit.ops.insert(
        0,
        Op::Const {
            out: zero,
            val: F::ZERO,
        },
    );
    let mut previous: HashMap<NpoTypeId, [Option<[WitnessId; WIDTH]>; 2]> = HashMap::new();
    for op in &mut circuit.ops {
        let Op::NonPrimitiveOpWithExecutor {
            inputs,
            outputs,
            executor,
            ..
        } = op
        else {
            continue;
        };
        let Some(mode) = executor.tip5_terminal_mode() else {
            continue;
        };
        if *executor.op_type() != NpoTypeId::tip5_perm(Tip5Config::GOLDILOCKS_W16)
            || !matches!(inputs.len(), 16 | 18)
            || !matches!(outputs.len(), 10 | 16)
            || inputs.iter().any(|slot| slot.len() > 1)
            || outputs.iter().any(|slot| slot.len() > 1)
        {
            return Err(CircuitError::InvalidPreprocessedValues);
        }
        let chains = previous
            .entry(executor.op_type().clone())
            .or_insert([None, None]);
        let chain = &mut chains[usize::from(mode.merkle_path)];
        if !mode.new_start && chain.is_none() {
            return Err(CircuitError::InvalidPreprocessedValues);
        }
        for i in 0..WIDTH {
            if !inputs[i].is_empty() {
                continue;
            }
            // Merkle sibling lanes are authenticated by the path.
            if mode.merkle_path && (DIGEST..2 * DIGEST).contains(&i) {
                continue;
            }
            let value = if mode.new_start || (mode.merkle_path && i >= 2 * DIGEST) {
                zero
            } else {
                chain.ok_or(CircuitError::InvalidPreprocessedValues)?[i]
            };
            inputs[i].push(value);
        }
        outputs.resize_with(WIDTH, Vec::new);
        let mut next = [zero; WIDTH];
        for (i, output) in outputs.iter_mut().enumerate() {
            if output.is_empty() {
                output.push(allocate(&mut circuit.witness_count)?);
            }
            next[i] = output[0];
        }
        *chain = Some(next);
    }
    Ok(())
}

#[derive(Clone)]
struct HardenedTip5Air<F, const D: usize>(Tip5CircuitAir<F, D>);

impl<F: Field, const D: usize> BaseAir<F> for HardenedTip5Air<F, D> {
    fn width(&self) -> usize {
        self.0.width()
    }
    fn preprocessed_width(&self) -> usize {
        self.0.preprocessed_width()
    }
    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<F>> {
        self.0.preprocessed_trace()
    }
    fn main_next_row_columns(&self) -> Vec<usize> {
        self.0.main_next_row_columns()
    }
    fn max_constraint_degree(&self) -> Option<usize> {
        None
    }
}

impl<AB, const D: usize> Air<AB> for HardenedTip5Air<AB::F, D>
where
    AB: AirBuilder + InteractionBuilder,
    AB::F: Field,
{
    fn eval(&self, builder: &mut AB) {
        self.0.eval(builder);
        let main = builder.main();
        let prep = builder.preprocessed();
        let row = main.current_slice();
        let fixed = prep.current_slice();
        // Layout of the pinned Tip5 AIR: P_IS_ROUND = 3; the last
        // preprocessed pair is mmcs_bit_ctl/mmcs_bit_idx.
        let kind: AB::Expr = fixed[3].into();
        let merkle: AB::Expr = fixed[TIP5_CIRCUIT_PREP_WIDTH - 2].into();
        let direction: AB::Expr = row[self.width() - 1].into();
        builder.assert_zero(kind * (AB::Expr::ONE - merkle) * direction);
    }
}

impl<SC, const D: usize> BatchAir<SC> for HardenedTip5Air<Val<SC>, D>
where
    SC: StarkGenericConfig + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
}

fn entry<SC>(prep: Vec<Val<SC>>, min_height: usize, d: u32) -> Option<DynamicAirEntry<SC>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    match d {
        1 => Some(DynamicAirEntry::new(Box::new(HardenedTip5Air(
            Tip5CircuitAir::<Val<SC>, 1>::new_with_preprocessed(prep, min_height),
        )))),
        2 => Some(DynamicAirEntry::new(Box::new(HardenedTip5Air(
            Tip5CircuitAir::<Val<SC>, 2>::new_with_preprocessed(prep, min_height),
        )))),
        _ => None,
    }
}

struct HardenedTip5Builder;

impl<SC> NpoAirBuilder<SC, 2> for HardenedTip5Builder
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn try_build(
        &self,
        op: &NpoTypeId,
        prep: &[Val<SC>],
        min_height: usize,
        lanes: usize,
        profile: ConstraintProfile,
    ) -> Option<(CircuitTableAir<SC, 2>, usize)> {
        let (_, degree): (CircuitTableAir<SC, 2>, usize) =
            Tip5AirBuilder::<2>.try_build(op, prep, min_height, lanes, profile)?;
        Some((
            CircuitTableAir::Dynamic(entry::<SC>(prep.to_vec(), min_height, 2)?),
            degree,
        ))
    }
}

pub(crate) fn air_builders<SC>(rules: ProofRules) -> Vec<Box<dyn NpoAirBuilder<SC, 2>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    match rules {
        ProofRules::Legacy => p3_circuit_prover::tip5_air_builders::<SC, 2>(),
        ProofRules::Hardened => vec![Box::new(HardenedTip5Builder)],
    }
}

struct HardenedTip5Prover(Tip5Prover);

macro_rules! delegate_instance {
    ($name:ident, $field:ty, $d:expr) => {
        fn $name(
            &self,
            config: &SC,
            packing: &TablePacking,
            traces: &Traces<$field>,
        ) -> Option<BatchTableInstance<SC>> {
            let mut instance = self.0.$name(config, packing, traces)?;
            let prep = instance.air.preprocessed_trace()?.values;
            instance.air = entry::<SC>(prep, packing.min_trace_height(), $d)?;
            Some(instance)
        }
    };
}

impl<SC> TableProver<SC> for HardenedTip5Prover
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn op_type(&self) -> NpoTypeId {
        NpoTypeId::tip5_perm(Tip5Config::GOLDILOCKS_W16)
    }
    delegate_instance!(batch_instance_d1, Val<SC>, 1);
    delegate_instance!(batch_instance_d2, BinomialExtensionField<Val<SC>, 2>, 2);
    delegate_instance!(batch_instance_d4, BinomialExtensionField<Val<SC>, 4>, 4);
    delegate_instance!(batch_instance_d6, BinomialExtensionField<Val<SC>, 6>, 6);
    delegate_instance!(batch_instance_d8, BinomialExtensionField<Val<SC>, 8>, 8);

    fn batch_air_from_table_entry(
        &self,
        _config: &SC,
        _degree: usize,
        d: u32,
        _table: &NonPrimitiveTableEntry<SC>,
    ) -> Result<DynamicAirEntry<SC>, String> {
        entry::<SC>(Vec::new(), 1, d).ok_or_else(|| format!("unsupported Tip5 dimension {d}"))
    }

    fn air_with_committed_preprocessed(
        &self,
        prep: Vec<Val<SC>>,
        min_height: usize,
        _lanes: usize,
        d: u32,
    ) -> Option<DynamicAirEntry<SC>> {
        entry::<SC>(prep, min_height, d)
    }
}

pub(crate) fn table_prover<SC>(rules: ProofRules) -> Box<dyn TableProver<SC>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    let inner = Tip5Prover::new(Tip5Config::GOLDILOCKS_W16, ConstraintProfile::Standard);
    match rules {
        ProofRules::Legacy => Box::new(inner),
        ProofRules::Hardened => Box::new(HardenedTip5Prover(inner)),
    }
}

pub(crate) fn register<SC>(prover: &mut BatchStarkProver<SC>, rules: ProofRules)
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    prover.register_table_prover(table_prover::<SC>(rules));
}
