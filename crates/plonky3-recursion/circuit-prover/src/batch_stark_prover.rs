//! Batch STARK prover and verifier that unifies all circuit tables
//! into a single batched STARK proof using `p3-batch-stark`.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

#[cfg(debug_assertions)]
use p3_air::DebugConstraintBuilder;
use p3_air::symbolic::AirLayout;
use p3_air::{Air, BaseAir};
use p3_batch_stark::common::{GlobalPreprocessed, PreprocessedInstanceMeta};
use p3_batch_stark::symbolic::get_log_num_quotient_chunks as get_batch_log_num_quotient_chunks;
use p3_batch_stark::{
    BatchProof, BatchTranscript, CommonData, Domain, ProverData, StarkGenericConfig, StarkInstance,
    Val,
};
use p3_challenger::{CanObserve, CanSampleBits, FieldChallenger, GrindingChallenger};
use p3_circuit::ops::{
    NonPrimitivePreprocessedMap, NpoTypeId, Poseidon1Config, Poseidon2Config, PrimitiveOpType,
    Tip5Config,
};
use p3_circuit::tables::Traces;
use p3_commit::{Mmcs, Pcs, PolynomialSpace};
use p3_field::extension::{BinomialExtensionField, BinomiallyExtendable};
use p3_field::{
    Algebra, BasedVectorSpace, ExtensionField, Field, PrimeCharacteristicRing, PrimeField,
    TwoAdicField,
};
use p3_goldilocks::Goldilocks;
use p3_lookup::Lookups;
use p3_lookup::folder::{ProverConstraintFolderWithLookups, VerifierConstraintFolderWithLookups};
use p3_lookup::logup::LogUpGadget;
use p3_lookup::symbolic::InteractionSymbolicBuilder;
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use p3_matrix::interpolation::Interpolate;
use p3_merkle_tree::{MerkleTreeMmcs, PrunedMerklePaths, PrunedPath};
use p3_symmetric::{PaddingFreeSponge, TruncatedPermutation};
use p3_tip5_circuit_air::Tip5Perm;
use p3_uni_stark::{SymbolicExpression, SymbolicExpressionExt, validate_degree_bits};
use p3_util::{checked_log_size_sum, log2_strict_usize, reverse_bits_len};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::instrument;

use crate::air::{AluAir, ConstAir, PublicAir};
use crate::batch_stark_prover::dynamic_air::transmute_traces;
use crate::batch_stark_prover::packing::{AirTableShape, TraceTablesLayout};
use crate::common::{CircuitTableAir, NpoAirBuilder, NpoPreprocessor};
use crate::config::{
    GOLDILOCKS_TIP5_RECURSIVE_CAP_HEIGHT, GOLDILOCKS_TIP5_RECURSIVE_COMMIT_POW_BITS,
    GOLDILOCKS_TIP5_RECURSIVE_LOG_BLOWUP, GOLDILOCKS_TIP5_RECURSIVE_LOG_FINAL_POLY_LEN,
    GOLDILOCKS_TIP5_RECURSIVE_MAX_LOG_ARITY, GOLDILOCKS_TIP5_RECURSIVE_NUM_QUERIES,
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_CAP_HEIGHT,
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS,
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_BLOWUP,
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_FINAL_POLY_LEN,
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_MAX_LOG_ARITY,
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_NUM_QUERIES,
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS, GOLDILOCKS_TIP5_RECURSIVE_QUERY_POW_BITS,
    GoldilocksBlake3Challenge, GoldilocksBlake3Config, GoldilocksBlake3ValMmcs,
    GoldilocksTipsConfig, StarkField, goldilocks_blake3_val_mmcs,
};
use crate::constraint_profile::ConstraintProfile;
use crate::field_params::ExtractBinomialW;

mod dynamic_air;
mod packing;
mod poseidon1;
mod poseidon2;
mod recompose;
mod tip5;

pub use dynamic_air::{
    BatchAir, BatchTableInstance, CloneableBatchAir, DynamicAirEntry, TableProver,
};
pub use packing::TablePacking;
pub use poseidon1::{
    Poseidon1AirBuilder, Poseidon1AirWrapperInner, Poseidon1Preprocessor, Poseidon1Prover,
    Poseidon1ProverD2, poseidon1_preprocessor, poseidon1_verifier_air_from_config,
};
pub use poseidon2::{
    Poseidon2AirBuilder, Poseidon2AirWrapperInner, Poseidon2Preprocessor, Poseidon2Prover,
    Poseidon2ProverD2, poseidon2_preprocessor, poseidon2_verifier_air_from_config,
};
pub use recompose::{RecomposeAirBuilder, RecomposePreprocessor, RecomposeProver};
pub use tip5::{
    Tip5AirBuilder, Tip5Preprocessor, Tip5Prover, tip5_air_builders, tip5_preprocessor,
    tip5_verifier_air_from_config,
};

/// Prime modulus of the BabyBear field (`2^31 - 2^27 + 1`).
pub const BABY_BEAR_MODULUS: u64 = 0x7800_0001;
/// Prime modulus of the KoalaBear field (`2^31 - 2^24 + 1`).
pub const KOALA_BEAR_MODULUS: u64 = 0x7f00_0001;

/// Opaque variant tag for a non-primitive AIR in a batch proof.
///
/// Each [`NonPrimitiveTableEntry`] has one tag. The **meaning** of the tag is
/// defined by that entry's `op_type`: the corresponding [`TableProver`] interprets
/// it when building the AIR in [`TableProver::batch_air_from_table_entry`].
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AirVariant {
    /// Baseline AIR for this op type (default behaviour).
    #[default]
    Baseline = 0,
    /// Recursion-optimized variant.
    Optimized = 1,
}

/// Metadata describing a non-primitive table inside a batch proof.
///
/// Every non-primitive dynamic plugin produces exactly one `NonPrimitiveTableEntry`
/// per batch instance. The entry is stored inside a `BatchStarkProof` and later provided
/// back to the plugin during verification through
/// [`TableProver::batch_air_from_table_entry`].
const fn default_npo_lanes() -> usize {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct NonPrimitiveTableEntry<SC>
where
    SC: StarkGenericConfig,
{
    /// Operation type (it should match `TableProver::op_type`).
    pub op_type: NpoTypeId,
    /// Number of logical operations (before lane packing) produced for this table.
    pub rows: usize,
    /// Number of operations packed per AIR row (lane count). Defaults to 1.
    #[serde(default = "default_npo_lanes")]
    pub lanes: usize,
    /// Public values exposed by this table (if any).
    pub public_values: Vec<Val<SC>>,
    /// AIR variant used for this non-primitive table.
    #[serde(default)]
    pub air_variant: AirVariant,
}

impl<SC: StarkGenericConfig> NonPrimitiveTableEntry<SC> {
    /// Re-check the lane-count invariant that constructors clamp, after deserialization.
    pub fn validate(&self) -> Result<(), ProofMetadataError> {
        if self.lanes == 0 {
            return Err(ProofMetadataError::ZeroNpoLanes(self.op_type.clone()));
        }
        Ok(())
    }
}

/// Compare the verifier-relevant preprocessed binding inside two
/// [`CommonData`] values.
///
/// `BatchStarkProof` serializes only this binding, and verifier code rebuilds
/// lookups from the reconstructed AIRs. Production callers that pin a circuit
/// identity should compare this binding against verifier-rebuilt common data
/// before calling [`BatchStarkProver::verify_all_tables_with_public_values`].
pub fn common_preprocessed_binding_eq<SC>(left: &CommonData<SC>, right: &CommonData<SC>) -> bool
where
    SC: StarkGenericConfig,
    <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: PartialEq,
{
    match (&left.preprocessed, &right.preprocessed) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.commitment == right.commitment
                && left.matrix_to_instance == right.matrix_to_instance
                && left.instances.len() == right.instances.len()
                && left
                    .instances
                    .iter()
                    .zip(&right.instances)
                    .all(|(left, right)| match (left, right) {
                        (None, None) => true,
                        (Some(left), Some(right)) => {
                            left.matrix_index == right.matrix_index
                                && left.width == right.width
                                && left.degree_bits == right.degree_bits
                        }
                        _ => false,
                    })
        }
        _ => false,
    }
}

/// Combined data for circuit proving, including STARK prover data and preprocessed columns.
///
/// This struct bundles the upstream [`ProverData`] with circuit-specific preprocessed data,
/// providing a cleaner API for `prove_all_tables`.
///
/// Preprocessed columns are stored as flat base-field vectors rather than a
/// [`PreprocessedColumns<F, D>`](p3_circuit::PreprocessedColumns) because `D` is only
/// determined at proving time (via `EF::DIMENSION`) while this struct is constructed
/// and stored beforehand. The `ext_reads` and `dup_npo_outputs` fields from
/// `PreprocessedColumns` are fully consumed during AIR construction in
/// [`get_airs_and_degrees_with_prep`](crate::common::get_airs_and_degrees_with_prep)
/// and are not needed here.
pub struct CircuitProverData<SC: StarkGenericConfig> {
    /// STARK prover data from p3_batch_stark.
    pub prover_data: ProverData<SC>,
    /// Preprocessed columns for primitive operations (Const, Public, ALU).
    pub primitive_columns: Vec<Vec<Val<SC>>>,
    /// Preprocessed columns for non-primitive operations.
    pub non_primitive_columns: NonPrimitivePreprocessedMap<Val<SC>>,
}

impl<SC: StarkGenericConfig> CircuitProverData<SC> {
    /// Create new circuit prover data from components.
    pub const fn new(
        prover_data: ProverData<SC>,
        primitive_columns: Vec<Vec<Val<SC>>>,
        non_primitive_columns: NonPrimitivePreprocessedMap<Val<SC>>,
    ) -> Self {
        Self {
            prover_data,
            primitive_columns,
            non_primitive_columns,
        }
    }

    /// Get a reference to the common data.
    pub const fn common_data(&self) -> &CommonData<SC> {
        &self.prover_data.common
    }
}

/// Convenience macro for deriving all degree-specific helpers from a single base
/// implementation.
///
/// Plugins usually implement a single `batch_instance_base` method that operates on
/// base-field traces. This macro reuses that method to provide the `batch_instance_d*`
/// variants by casting higher-degree traces back to the base field.
///
/// Users can invoke it inside their `TableProver` impl:
///
/// ```ignore
/// impl<SC> TableProver<SC> for MyPlugin {
///     fn op_type(&self) -> NpoTypeId {
///         NpoTypeId::Poseidon2Perm(Poseidon2Config::BABY_BEAR_D4_W16)
///     }
///
///     impl_table_prover_batch_instances_from_base!(batch_instance_base);
///
///     fn batch_air_from_table_entry(
///         &self,
///         config: &SC,
///         degree: usize,
///         circuit_extension_degree: u32,
///         table_entry: &NonPrimitiveTableEntry<SC>,
///     ) -> Result<DynamicAirEntry<SC>, String> {
///         Ok(DynamicAirEntry::new(Box::new(MyPluginAir::<Val<SC>>::new(config))))
///     }
/// }
/// ```
#[macro_export]
macro_rules! impl_table_prover_batch_instances_from_base {
    ($base:ident) => {
        fn batch_instance_d1(
            &self,
            config: &SC,
            packing: &TablePacking,
            traces: &p3_circuit::tables::Traces<p3_batch_stark::Val<SC>>,
        ) -> Option<BatchTableInstance<SC>> {
            self.$base::<SC>(config, packing, traces)
        }

        fn batch_instance_d2(
            &self,
            config: &SC,
            packing: &TablePacking,
            traces: &p3_circuit::tables::Traces<
                p3_field::extension::BinomialExtensionField<p3_batch_stark::Val<SC>, 2>,
            >,
        ) -> Option<BatchTableInstance<SC>> {
            let t: &p3_circuit::tables::Traces<p3_batch_stark::Val<SC>> =
                unsafe { transmute_traces(traces) };
            self.$base::<SC>(config, packing, t)
        }

        fn batch_instance_d4(
            &self,
            config: &SC,
            packing: &TablePacking,
            traces: &p3_circuit::tables::Traces<
                p3_field::extension::BinomialExtensionField<p3_batch_stark::Val<SC>, 4>,
            >,
        ) -> Option<BatchTableInstance<SC>> {
            let t: &p3_circuit::tables::Traces<p3_batch_stark::Val<SC>> =
                unsafe { transmute_traces(traces) };
            self.$base::<SC>(config, packing, t)
        }

        fn batch_instance_d6(
            &self,
            config: &SC,
            packing: &TablePacking,
            traces: &p3_circuit::tables::Traces<
                p3_field::extension::BinomialExtensionField<p3_batch_stark::Val<SC>, 6>,
            >,
        ) -> Option<BatchTableInstance<SC>> {
            let t: &p3_circuit::tables::Traces<p3_batch_stark::Val<SC>> =
                unsafe { transmute_traces(traces) };
            self.$base::<SC>(config, packing, t)
        }

        fn batch_instance_d8(
            &self,
            config: &SC,
            packing: &TablePacking,
            traces: &p3_circuit::tables::Traces<
                p3_field::extension::BinomialExtensionField<p3_batch_stark::Val<SC>, 8>,
            >,
        ) -> Option<BatchTableInstance<SC>> {
            let t: &p3_circuit::tables::Traces<p3_batch_stark::Val<SC>> =
                unsafe { transmute_traces(traces) };
            self.$base::<SC>(config, packing, t)
        }

        fn batch_instance_d5(
            &self,
            config: &SC,
            packing: &TablePacking,
            traces: &p3_circuit::tables::Traces<
                p3_field::extension::QuinticTrinomialExtensionField<p3_batch_stark::Val<SC>>,
            >,
        ) -> Option<BatchTableInstance<SC>> {
            let t: &p3_circuit::tables::Traces<p3_batch_stark::Val<SC>> =
                unsafe { transmute_traces(traces) };
            self.$base::<SC>(config, packing, t)
        }
    };
}

/// Type alias for the primitive operation table selector.
///
/// Used as an index into [`RowCounts`] and related per-table arrays.
pub type PrimitiveTable = PrimitiveOpType;

/// Number of primitive circuit tables included in the unified batch STARK proof.
pub const NUM_PRIMITIVE_TABLES: usize = PrimitiveTable::Alu as usize + 1;

/// Row counts wrapper with type-safe indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowCounts([usize; NUM_PRIMITIVE_TABLES]);

impl RowCounts {
    /// Creates a new RowCounts with the given row counts for each table.
    pub const fn new(rows: [usize; NUM_PRIMITIVE_TABLES]) -> Self {
        // Validate that all row counts are non-zero
        let mut i = 0;
        while i < rows.len() {
            assert!(rows[i] > 0);
            i += 1;
        }
        Self(rows)
    }

    /// Re-check the invariant [`RowCounts::new`] enforces, after deserialization.
    pub fn validate(&self) -> Result<(), ProofMetadataError> {
        if self.0.contains(&0) {
            return Err(ProofMetadataError::ZeroRowCount);
        }
        Ok(())
    }
}

impl core::ops::Index<PrimitiveTable> for RowCounts {
    type Output = usize;
    fn index(&self, table: PrimitiveTable) -> &Self::Output {
        &self.0[table as usize]
    }
}

/// Serializable mirror of [`PreprocessedInstanceMeta`].
///
/// Defined locally because the upstream type does not derive `Serialize`/`Deserialize`.
#[derive(Serialize, Deserialize)]
struct SerializedPreprocessedInstanceMeta {
    matrix_index: usize,
    width: usize,
    degree_bits: usize,
}

/// Serializable projection of [`CommonData::preprocessed`] used to bind the proof
/// to its prover-side common data across (de)serialization.
///
/// `lookups` are intentionally omitted: the verifier always rebuilds them from the
/// AIRs reconstructed from proof metadata, so they are not part of the binding.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
struct SerializedStarkCommon<SC: StarkGenericConfig> {
    commitment: <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment,
    instances: Vec<Option<SerializedPreprocessedInstanceMeta>>,
    matrix_to_instance: Vec<usize>,
}

impl<SC: StarkGenericConfig> SerializedStarkCommon<SC> {
    fn from_common(common: &CommonData<SC>) -> Option<Self> {
        common.preprocessed.as_ref().map(|gp| Self {
            commitment: gp.commitment.clone(),
            instances: gp
                .instances
                .iter()
                .map(|opt| {
                    opt.as_ref().map(|m| SerializedPreprocessedInstanceMeta {
                        matrix_index: m.matrix_index,
                        width: m.width,
                        degree_bits: m.degree_bits,
                    })
                })
                .collect(),
            matrix_to_instance: gp.matrix_to_instance.clone(),
        })
    }

    fn into_common(self) -> CommonData<SC> {
        CommonData::new(
            Some(GlobalPreprocessed {
                commitment: self.commitment,
                instances: self
                    .instances
                    .into_iter()
                    .map(|opt| {
                        opt.map(|m| PreprocessedInstanceMeta {
                            matrix_index: m.matrix_index,
                            width: m.width,
                            degree_bits: m.degree_bits,
                        })
                    })
                    .collect(),
                matrix_to_instance: self.matrix_to_instance,
            }),
            Vec::new(),
        )
    }
}

/// Clone a [`CommonData`] without requiring [`Clone`] on the upstream
/// [`GlobalPreprocessed`] / [`PreprocessedInstanceMeta`] types.
fn clone_common_data<SC: StarkGenericConfig>(common: &CommonData<SC>) -> CommonData<SC> {
    CommonData::new(
        common.preprocessed.as_ref().map(|gp| GlobalPreprocessed {
            commitment: gp.commitment.clone(),
            instances: gp
                .instances
                .iter()
                .map(|opt| {
                    opt.as_ref().map(|m| PreprocessedInstanceMeta {
                        matrix_index: m.matrix_index,
                        width: m.width,
                        degree_bits: m.degree_bits,
                    })
                })
                .collect(),
            matrix_to_instance: gp.matrix_to_instance.clone(),
        }),
        common.lookups.clone(),
    )
}

fn flatten_public_binding_values<SC, EF, const D: usize>(
    traces: &Traces<EF>,
    public_binding_lanes: usize,
) -> Result<Vec<Val<SC>>, BatchStarkProverError>
where
    SC: StarkGenericConfig,
    EF: Field + BasedVectorSpace<Val<SC>>,
{
    if public_binding_lanes == 0 {
        return Ok(Vec::new());
    }
    if public_binding_lanes > traces.public_trace.values.len() {
        return Err(BatchStarkProverError::Verify(format!(
            "public binding lanes ({public_binding_lanes}) exceed public trace values ({})",
            traces.public_trace.values.len()
        )));
    }

    let mut out = Vec::with_capacity(public_binding_lanes * D);
    for value in traces.public_trace.values.iter().take(public_binding_lanes) {
        let coeffs = value.as_basis_coefficients_slice();
        if coeffs.len() != D {
            return Err(BatchStarkProverError::Verify(format!(
                "public binding extension degree mismatch: expected {D}, got {}",
                coeffs.len()
            )));
        }
        out.extend_from_slice(coeffs);
    }
    Ok(out)
}

/// Custom (de)serialization for [`BatchStarkProof::stark_common`]. Persists only the
/// preprocessed binding (commitment + per-instance metadata): the part the verifier
/// needs to bind the proof to the [`CommonData`] it was generated against. `lookups`
/// are intentionally not serialized because the verifier always rebuilds them from
/// the AIRs reconstructed from proof metadata.
mod serde_stark_common {
    use alloc::vec::Vec;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{CommonData, SerializedStarkCommon, StarkGenericConfig};

    pub(super) fn serialize<S, SC>(value: &CommonData<SC>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        SC: StarkGenericConfig,
    {
        SerializedStarkCommon::from_common(value).serialize(serializer)
    }

    pub(super) fn deserialize<'de, D, SC>(deserializer: D) -> Result<CommonData<SC>, D::Error>
    where
        D: Deserializer<'de>,
        SC: StarkGenericConfig,
    {
        let parsed: Option<SerializedStarkCommon<SC>> = Option::deserialize(deserializer)?;
        Ok(parsed
            .map(SerializedStarkCommon::into_common)
            .unwrap_or_else(|| CommonData::new(None, Vec::new())))
    }
}

/// Proof bundle and metadata for the unified batch STARK proof across all circuit tables.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct BatchStarkProof<SC>
where
    SC: StarkGenericConfig,
{
    /// The core cryptographic proof generated by `p3-batch-stark`.
    pub proof: BatchProof<SC>,
    /// Packing configuration used for the Witness, Public, and unified ALU tables.
    pub table_packing: TablePacking,
    /// Number of leading Public table lanes exposed as STARK public values.
    ///
    /// This duplicates `table_packing.public_binding_lanes()` for cheap,
    /// explicit verifier checks after deserialization.
    #[serde(default)]
    pub public_binding_lanes: usize,
    /// The number of rows in each of the circuit tables.
    pub rows: RowCounts,
    /// Variant used for the primitive ALU table.
    pub alu_variant: AirVariant,
    /// The degree of the field extension (`D`) used for the proof.
    pub ext_degree: usize,
    /// The binomial coefficient `W` for extension field multiplication, if `ext_degree > 1`.
    pub w_binomial: Option<Val<SC>>,
    /// When `true` with `ext_degree == 5`, the ALU uses quintic trinomial reduction (`X^5+X^2-1`).
    #[serde(default)]
    pub alu_quintic_trinomial: bool,
    /// Manifest describing batched non-primitive tables defined at runtime.
    pub non_primitives: Vec<NonPrimitiveTableEntry<SC>>,
    /// Common data derived from the final table AIRs after trace construction.
    #[serde(with = "serde_stark_common")]
    pub stark_common: CommonData<SC>,
}

/// Verifier-owned metadata template for a Goldilocks/Tip5 batch STARK proof.
///
/// This is the part of [`BatchStarkProof`] that must be derived from trusted
/// verifier setup, circuit identity, and statement metadata when the wire proof
/// carries only a compact body. It intentionally omits [`BatchProof`].
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct GoldilocksTip5BatchStarkProofMetadata {
    pub table_packing: TablePacking,
    pub public_binding_lanes: usize,
    pub rows: RowCounts,
    pub alu_variant: AirVariant,
    pub ext_degree: usize,
    pub w_binomial: Option<Goldilocks>,
    pub alu_quintic_trinomial: bool,
    pub non_primitives: Vec<NonPrimitiveTableEntry<GoldilocksTipsConfig>>,
    #[serde(with = "serde_stark_common")]
    pub stark_common: CommonData<GoldilocksTipsConfig>,
}

impl GoldilocksTip5BatchStarkProofMetadata {
    /// Clone the verifier-relevant metadata from a full proof.
    ///
    /// This helper is mainly for diagnostics and tests. Production compact-body
    /// verification should rebuild this template from the canonical circuit
    /// setup and statement metadata rather than accepting it from the prover.
    pub fn from_proof(proof: &BatchStarkProof<GoldilocksTipsConfig>) -> Self {
        Self {
            table_packing: proof.table_packing.clone(),
            public_binding_lanes: proof.public_binding_lanes,
            rows: proof.rows,
            alu_variant: proof.alu_variant,
            ext_degree: proof.ext_degree,
            w_binomial: proof.w_binomial,
            alu_quintic_trinomial: proof.alu_quintic_trinomial,
            non_primitives: proof.non_primitives.clone(),
            stark_common: clone_common_data(&proof.stark_common),
        }
    }

    /// Re-check the structural invariants that compact-body verifiers rely on
    /// after this metadata has been rebuilt or deserialized from trusted
    /// verifier state.
    pub fn validate(&self) -> Result<(), ProofMetadataError> {
        match self.ext_degree {
            1 | 2 | 4 | 5 | 6 | 8 => {}
            d => return Err(ProofMetadataError::UnsupportedExtDegree(d)),
        }
        self.rows.validate()?;
        self.table_packing.validate()?;
        if self.public_binding_lanes != self.table_packing.public_binding_lanes() {
            return Err(ProofMetadataError::PublicBindingMismatch {
                proof_lanes: self.public_binding_lanes,
                packing_lanes: self.table_packing.public_binding_lanes(),
            });
        }
        for entry in &self.non_primitives {
            entry.validate()?;
        }
        Ok(())
    }

    /// Rehydrate a compact proof body with this verifier-owned metadata.
    pub fn into_proof_with_body(
        &self,
        proof: BatchProof<GoldilocksTipsConfig>,
    ) -> BatchStarkProof<GoldilocksTipsConfig> {
        BatchStarkProof {
            proof,
            table_packing: self.table_packing.clone(),
            public_binding_lanes: self.public_binding_lanes,
            rows: self.rows,
            alu_variant: self.alu_variant,
            ext_degree: self.ext_degree,
            w_binomial: self.w_binomial,
            alu_quintic_trinomial: self.alu_quintic_trinomial,
            non_primitives: self.non_primitives.clone(),
            stark_common: clone_common_data(&self.stark_common),
        }
    }
}

/// Verifier-owned metadata template for a native-final-layer Goldilocks/BLAKE3
/// batch STARK proof.
///
/// This mirrors [`GoldilocksTip5BatchStarkProofMetadata`] for the BLAKE3 final
/// layer. It is not a prover-trusted wire artifact: compact-body verifiers must
/// rebuild or pin this metadata from trusted verifier setup before rehydrating a
/// compact proof body.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct GoldilocksBlake3BatchStarkProofMetadata {
    pub table_packing: TablePacking,
    pub public_binding_lanes: usize,
    pub rows: RowCounts,
    pub alu_variant: AirVariant,
    pub ext_degree: usize,
    pub w_binomial: Option<Goldilocks>,
    pub alu_quintic_trinomial: bool,
    pub non_primitives: Vec<NonPrimitiveTableEntry<GoldilocksBlake3Config>>,
    #[serde(with = "serde_stark_common")]
    pub stark_common: CommonData<GoldilocksBlake3Config>,
}

impl GoldilocksBlake3BatchStarkProofMetadata {
    pub fn from_proof(proof: &BatchStarkProof<GoldilocksBlake3Config>) -> Self {
        Self {
            table_packing: proof.table_packing.clone(),
            public_binding_lanes: proof.public_binding_lanes,
            rows: proof.rows,
            alu_variant: proof.alu_variant,
            ext_degree: proof.ext_degree,
            w_binomial: proof.w_binomial,
            alu_quintic_trinomial: proof.alu_quintic_trinomial,
            non_primitives: proof.non_primitives.clone(),
            stark_common: clone_common_data(&proof.stark_common),
        }
    }

    pub fn validate(&self) -> Result<(), ProofMetadataError> {
        match self.ext_degree {
            1 | 2 | 4 | 5 | 6 | 8 => {}
            d => return Err(ProofMetadataError::UnsupportedExtDegree(d)),
        }
        self.rows.validate()?;
        self.table_packing.validate()?;
        if self.public_binding_lanes != self.table_packing.public_binding_lanes() {
            return Err(ProofMetadataError::PublicBindingMismatch {
                proof_lanes: self.public_binding_lanes,
                packing_lanes: self.table_packing.public_binding_lanes(),
            });
        }
        for entry in &self.non_primitives {
            entry.validate()?;
        }
        Ok(())
    }

    pub fn into_proof_with_body(
        &self,
        proof: BatchProof<GoldilocksBlake3Config>,
    ) -> BatchStarkProof<GoldilocksBlake3Config> {
        BatchStarkProof {
            proof,
            table_packing: self.table_packing.clone(),
            public_binding_lanes: self.public_binding_lanes,
            rows: self.rows,
            alu_variant: self.alu_variant,
            ext_degree: self.ext_degree,
            w_binomial: self.w_binomial,
            alu_quintic_trinomial: self.alu_quintic_trinomial,
            non_primitives: self.non_primitives.clone(),
            stark_common: clone_common_data(&self.stark_common),
        }
    }
}

/// Compact projection of [`BatchStarkProof`] that omits verifier-deterministic
/// out-of-domain openings for preprocessed columns.
///
/// This is **not** a standalone proof format. Verification must provide the
/// canonical [`CircuitProverData`] for the statement being verified. The verifier
/// first checks that the canonical setup's preprocessed commitment/metadata is
/// exactly the binding serialized in [`BatchStarkProof::stark_common`], then
/// recomputes the omitted `preprocessed_local` / `preprocessed_next` OOD values
/// from that setup before delegating to `p3-batch-stark` verification.
///
/// Soundness invariant: the omitted values are never trusted from the prover and
/// are never skipped in the Fiat-Shamir transcript. They are restored before the
/// upstream verifier observes PCS openings, so the transcript and AIR checks are
/// identical to a full proof with those openings serialized.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct PreprocessedOodCompactBatchStarkProof<SC>
where
    SC: StarkGenericConfig,
{
    /// The full batch proof metadata and cryptographic proof, with
    /// `proof.opened_values.instances[*].base_opened_values.preprocessed_*`
    /// omitted where the verifier can recompute them from canonical setup data.
    pub proof: BatchStarkProof<SC>,
}

impl<SC> PreprocessedOodCompactBatchStarkProof<SC>
where
    SC: StarkGenericConfig,
{
    /// Build a compact proof by consuming a full proof and clearing all
    /// serialized preprocessed OOD openings.
    pub fn from_full(mut proof: BatchStarkProof<SC>) -> Self {
        omit_preprocessed_ood_openings(&mut proof.proof);
        Self { proof }
    }

    /// Consume the compact wrapper and return its inner proof object.
    pub fn into_inner(self) -> BatchStarkProof<SC> {
        self.proof
    }
}

type GoldilocksTip5Challenge = BinomialExtensionField<Goldilocks, 2>;
type GoldilocksTip5Hash = PaddingFreeSponge<Tip5Perm, 16, 10, 5>;
type GoldilocksTip5Compress = TruncatedPermutation<Tip5Perm, 2, 5, 16>;
type GoldilocksTip5ValMmcs =
    MerkleTreeMmcs<Goldilocks, Goldilocks, GoldilocksTip5Hash, GoldilocksTip5Compress, 2, 5>;
type GoldilocksTip5MerkleDigest = [Goldilocks; 5];
type GoldilocksTip5MerkleProof = Vec<GoldilocksTip5MerkleDigest>;

/// Public FRI shape metadata needed to regenerate omitted Goldilocks/Tip5
/// preprocessed input-batch openings.
///
/// `TwoAdicFriPcs` intentionally keeps its FRI parameters private, so a compact
/// proof that omits verifier-deterministic MMCS openings must carry the shape
/// used for reconstruction. This shape is not trusted as a verifier policy:
/// [`BatchStarkProver`] still verifies the restored proof with its configured
/// PCS. A wrong shape can only make restoration fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldilocksTip5FriShape {
    pub log_blowup: usize,
    pub log_final_poly_len: usize,
    pub max_log_arity: usize,
    pub num_queries: usize,
    pub commit_pow_bits: usize,
    pub query_pow_bits: usize,
    pub cap_height: usize,
}

impl GoldilocksTip5FriShape {
    /// Current mixed query/PoW recursive Tip5 profile.
    pub const fn recursive_60bit() -> Self {
        Self {
            log_blowup: GOLDILOCKS_TIP5_RECURSIVE_LOG_BLOWUP,
            log_final_poly_len: GOLDILOCKS_TIP5_RECURSIVE_LOG_FINAL_POLY_LEN,
            max_log_arity: GOLDILOCKS_TIP5_RECURSIVE_MAX_LOG_ARITY,
            num_queries: GOLDILOCKS_TIP5_RECURSIVE_NUM_QUERIES,
            commit_pow_bits: GOLDILOCKS_TIP5_RECURSIVE_COMMIT_POW_BITS,
            query_pow_bits: GOLDILOCKS_TIP5_RECURSIVE_QUERY_POW_BITS,
            cap_height: GOLDILOCKS_TIP5_RECURSIVE_CAP_HEIGHT,
        }
    }

    /// Current pure-query recursive Tip5 production-candidate profile.
    pub const fn recursive_pure_query_60bit() -> Self {
        Self {
            log_blowup: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_BLOWUP,
            log_final_poly_len: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_FINAL_POLY_LEN,
            max_log_arity: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_MAX_LOG_ARITY,
            num_queries: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_NUM_QUERIES,
            commit_pow_bits: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS,
            query_pow_bits: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS,
            cap_height: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_CAP_HEIGHT,
        }
    }

    pub const fn johnson_bits(self) -> usize {
        self.log_blowup * self.num_queries + self.query_pow_bits
    }

    fn final_poly_len(self) -> Result<usize, BatchStarkProverError> {
        checked_power_of_two(self.log_final_poly_len, "FRI final polynomial length")
    }

    fn validate(self) -> Result<(), BatchStarkProverError> {
        if self.log_blowup == 0 {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 FRI shape must have non-zero log_blowup",
            )));
        }
        if self.max_log_arity == 0 {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 FRI shape must have non-zero max_log_arity",
            )));
        }
        if self.num_queries == 0 {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 FRI shape must have at least one query",
            )));
        }
        if self.cap_height > Goldilocks::TWO_ADICITY {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 FRI cap height {} exceeds Goldilocks two-adicity {}",
                self.cap_height,
                Goldilocks::TWO_ADICITY
            )));
        }
        let _ = self.final_poly_len()?;
        Ok(())
    }
}

/// Public FRI shape metadata needed to regenerate omitted Goldilocks/BLAKE3
/// preprocessed input-batch openings.
///
/// As with the Tip5 compact body, this shape is checked against a
/// verifier-owned context before restoration and upstream verification. It is
/// carried only because `TwoAdicFriPcs` keeps FRI parameters private.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldilocksBlake3FriShape {
    pub log_blowup: usize,
    pub log_final_poly_len: usize,
    pub max_log_arity: usize,
    pub num_queries: usize,
    pub commit_pow_bits: usize,
    pub query_pow_bits: usize,
    pub cap_height: usize,
}

impl GoldilocksBlake3FriShape {
    pub const fn johnson_bits(self) -> usize {
        self.log_blowup * self.num_queries + self.query_pow_bits
    }

    fn final_poly_len(self) -> Result<usize, BatchStarkProverError> {
        checked_power_of_two(self.log_final_poly_len, "FRI final polynomial length")
    }

    fn validate(self) -> Result<(), BatchStarkProverError> {
        if self.log_blowup == 0 {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 FRI shape must have non-zero log_blowup",
            )));
        }
        if self.max_log_arity == 0 {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 FRI shape must have non-zero max_log_arity",
            )));
        }
        if self.num_queries == 0 {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 FRI shape must have at least one query",
            )));
        }
        if self.cap_height > Goldilocks::TWO_ADICITY {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 FRI cap height {} exceeds Goldilocks two-adicity {}",
                self.cap_height,
                Goldilocks::TWO_ADICITY
            )));
        }
        let _ = self.final_poly_len()?;
        Ok(())
    }
}

/// Goldilocks/Tip5 compact projection that omits all verifier-deterministic
/// preprocessed openings currently available to the native verifier.
///
/// The wrapper removes both:
/// - preprocessed out-of-domain openings from `BatchProof.opened_values`; and
/// - the preprocessed commitment's per-query FRI input-batch openings.
///
/// Verification must provide the canonical [`CircuitProverData`] for the
/// statement. The verifier restores OOD openings, replays the transcript to
/// derive FRI query indices, regenerates the omitted preprocessed
/// [`Mmcs::open_batch`] results from the canonical preprocessed prover data, and
/// then delegates to the normal upstream `p3-batch-stark` verifier.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct GoldilocksTip5PreprocessedCompactBatchStarkProof {
    pub proof: BatchStarkProof<GoldilocksTipsConfig>,
    pub fri_shape: GoldilocksTip5FriShape,
}

impl GoldilocksTip5PreprocessedCompactBatchStarkProof {
    /// Build the compact projection from a full Goldilocks/Tip5 proof.
    pub fn try_from_full(
        mut proof: BatchStarkProof<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<Self, BatchStarkProverError> {
        fri_shape.validate()?;
        omit_preprocessed_ood_openings(&mut proof.proof);
        omit_goldilocks_tip5_preprocessed_fri_input_batches(&mut proof.proof, &proof.stark_common)?;
        Ok(Self { proof, fri_shape })
    }

    /// Consume the compact wrapper and return its inner proof object.
    pub fn into_inner(self) -> BatchStarkProof<GoldilocksTipsConfig> {
        self.proof
    }
}

/// Pruned binary Merkle authentication paths for the concrete Goldilocks/Tip5
/// MMCS used by the recursive production-candidate batch STARK.
///
/// `full_path_len` is verifier-bounded by the derived FRI global height before
/// restoration. The leaf indices are used only to reconstruct the canonical
/// full paths; upstream `p3-batch-stark` verification still derives and checks
/// the query indices from Fiat-Shamir.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoldilocksTip5PrunedMerklePaths {
    pub full_path_len: usize,
    pub paths: PrunedMerklePaths<Goldilocks, 5>,
}

/// Goldilocks/Tip5 compact projection that also prunes repeated Merkle
/// authentication path material across FRI queries.
///
/// The wrapper removes:
/// - verifier-deterministic preprocessed OOD openings;
/// - verifier-deterministic preprocessed input-batch openings; and
/// - Merkle authentication paths for the remaining input batches and FRI
///   commit-phase openings.
///
/// The verifier restores all omitted paths and preprocessed openings before
/// delegating to the normal upstream verifier. Opened values, FRI sibling
/// values, commitments, log arities, final polynomial coefficients, and PoW
/// witnesses remain in the inner proof and stay bound by the upstream
/// Fiat-Shamir transcript.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct GoldilocksTip5PathPrunedCompactBatchStarkProof {
    pub proof: BatchStarkProof<GoldilocksTipsConfig>,
    pub fri_shape: GoldilocksTip5FriShape,
    pub input_batch_paths: Vec<GoldilocksTip5PrunedMerklePaths>,
    pub commit_phase_paths: Vec<GoldilocksTip5PrunedMerklePaths>,
}

impl GoldilocksTip5PathPrunedCompactBatchStarkProof {
    /// Consume the compact wrapper and return its inner proof object.
    pub fn into_inner(self) -> BatchStarkProof<GoldilocksTipsConfig> {
        self.proof
    }

    /// Consume the wrapper and drop verifier-deterministic batch proof metadata.
    ///
    /// The returned body is only sound when verified with metadata rebuilt from
    /// the canonical verifier setup for the same statement.
    pub fn into_body(self) -> GoldilocksTip5PathPrunedCompactBatchStarkProofBody {
        GoldilocksTip5PathPrunedCompactBatchStarkProofBody {
            proof: self.proof.proof,
            fri_shape: self.fri_shape,
            input_batch_paths: self.input_batch_paths,
            commit_phase_paths: self.commit_phase_paths,
        }
    }
}

/// Compact Goldilocks/Tip5 proof body for the canonical-metadata wire path.
///
/// This carries only the cryptographic [`BatchProof`] plus the FRI shape and
/// pruned-path dictionaries needed to restore omitted verifier-deterministic
/// openings. It deliberately omits all [`BatchStarkProof`] metadata. Verification
/// must provide a trusted [`GoldilocksTip5BatchStarkProofMetadata`] template.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct GoldilocksTip5PathPrunedCompactBatchStarkProofBody {
    pub proof: BatchProof<GoldilocksTipsConfig>,
    pub fri_shape: GoldilocksTip5FriShape,
    pub input_batch_paths: Vec<GoldilocksTip5PrunedMerklePaths>,
    pub commit_phase_paths: Vec<GoldilocksTip5PrunedMerklePaths>,
}

impl GoldilocksTip5PathPrunedCompactBatchStarkProofBody {
    /// Rehydrate this compact body with verifier-owned metadata.
    pub fn into_wrapped(
        self,
        metadata: &GoldilocksTip5BatchStarkProofMetadata,
    ) -> GoldilocksTip5PathPrunedCompactBatchStarkProof {
        GoldilocksTip5PathPrunedCompactBatchStarkProof {
            proof: metadata.into_proof_with_body(self.proof),
            fri_shape: self.fri_shape,
            input_batch_paths: self.input_batch_paths,
            commit_phase_paths: self.commit_phase_paths,
        }
    }
}

/// Pruned binary Merkle authentication paths for the native-final-layer
/// Goldilocks/BLAKE3 MMCS.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoldilocksBlake3PrunedMerklePaths {
    pub full_path_len: usize,
    pub paths: PrunedMerklePaths<u8, 32>,
}

/// Goldilocks/BLAKE3 compact projection that prunes repeated Merkle
/// authentication path material and omits verifier-deterministic preprocessed
/// openings.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct GoldilocksBlake3PathPrunedCompactBatchStarkProof {
    pub proof: BatchStarkProof<GoldilocksBlake3Config>,
    pub fri_shape: GoldilocksBlake3FriShape,
    pub input_batch_paths: Vec<GoldilocksBlake3PrunedMerklePaths>,
    pub commit_phase_paths: Vec<GoldilocksBlake3PrunedMerklePaths>,
}

impl GoldilocksBlake3PathPrunedCompactBatchStarkProof {
    pub fn into_body(self) -> GoldilocksBlake3PathPrunedCompactBatchStarkProofBody {
        GoldilocksBlake3PathPrunedCompactBatchStarkProofBody {
            proof: self.proof.proof,
            fri_shape: self.fri_shape,
            input_batch_paths: self.input_batch_paths,
            commit_phase_paths: self.commit_phase_paths,
        }
    }
}

/// Compact Goldilocks/BLAKE3 proof body for the canonical-metadata wire path.
///
/// Verification must provide trusted [`GoldilocksBlake3BatchStarkProofMetadata`]
/// and canonical setup; accepting them from the prover would make the compact
/// body unsound.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct GoldilocksBlake3PathPrunedCompactBatchStarkProofBody {
    pub proof: BatchProof<GoldilocksBlake3Config>,
    pub fri_shape: GoldilocksBlake3FriShape,
    pub input_batch_paths: Vec<GoldilocksBlake3PrunedMerklePaths>,
    pub commit_phase_paths: Vec<GoldilocksBlake3PrunedMerklePaths>,
}

impl GoldilocksBlake3PathPrunedCompactBatchStarkProofBody {
    pub fn into_wrapped(
        self,
        metadata: &GoldilocksBlake3BatchStarkProofMetadata,
    ) -> GoldilocksBlake3PathPrunedCompactBatchStarkProof {
        GoldilocksBlake3PathPrunedCompactBatchStarkProof {
            proof: metadata.into_proof_with_body(self.proof),
            fri_shape: self.fri_shape,
            input_batch_paths: self.input_batch_paths,
            commit_phase_paths: self.commit_phase_paths,
        }
    }
}

/// Verifier-owned context for checking a canonical-metadata Goldilocks/BLAKE3
/// path-pruned compact body.
pub struct GoldilocksBlake3PathPrunedCompactVerifierContext<'a> {
    pub metadata: &'a GoldilocksBlake3BatchStarkProofMetadata,
    pub canonical_setup: &'a CircuitProverData<GoldilocksBlake3Config>,
    pub fri_shape: GoldilocksBlake3FriShape,
    pub public_values: &'a [Goldilocks],
}

impl<'a> GoldilocksBlake3PathPrunedCompactVerifierContext<'a> {
    pub const fn new(
        metadata: &'a GoldilocksBlake3BatchStarkProofMetadata,
        canonical_setup: &'a CircuitProverData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
        public_values: &'a [Goldilocks],
    ) -> Self {
        Self {
            metadata,
            canonical_setup,
            fri_shape,
            public_values,
        }
    }

    fn validate(&self) -> Result<(), BatchStarkProverError> {
        self.metadata.validate()?;
        self.fri_shape.validate()?;
        if !common_preprocessed_binding_eq(
            &self.metadata.stark_common,
            &self.canonical_setup.prover_data.common,
        ) {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 compact verifier context metadata/setup binding mismatch",
            )));
        }
        let expected_public_values = self.metadata.public_binding_lanes * self.metadata.ext_degree;
        if self.public_values.len() != expected_public_values {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 compact verifier context public values length mismatch: expected {expected_public_values}, got {}",
                self.public_values.len()
            )));
        }
        Ok(())
    }
}

/// Verifier-owned context for checking a canonical-metadata Goldilocks/Tip5
/// path-pruned compact body.
///
/// The compact body is not self-describing. A sound verifier must own all
/// context here: the proof metadata template, canonical setup used to restore
/// omitted preprocessed openings, expected FRI shape, and final public values.
/// None of these fields may be accepted from the prover as trusted context.
pub struct GoldilocksTip5PathPrunedCompactVerifierContext<'a> {
    pub metadata: &'a GoldilocksTip5BatchStarkProofMetadata,
    pub canonical_setup: &'a CircuitProverData<GoldilocksTipsConfig>,
    pub fri_shape: GoldilocksTip5FriShape,
    pub public_values: &'a [Goldilocks],
}

impl<'a> GoldilocksTip5PathPrunedCompactVerifierContext<'a> {
    pub const fn new(
        metadata: &'a GoldilocksTip5BatchStarkProofMetadata,
        canonical_setup: &'a CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
        public_values: &'a [Goldilocks],
    ) -> Self {
        Self {
            metadata,
            canonical_setup,
            fri_shape,
            public_values,
        }
    }

    fn validate(&self) -> Result<(), BatchStarkProverError> {
        self.metadata.validate()?;
        self.fri_shape.validate()?;
        if !common_preprocessed_binding_eq(
            &self.metadata.stark_common,
            &self.canonical_setup.prover_data.common,
        ) {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 compact verifier context metadata/setup binding mismatch",
            )));
        }
        let expected_public_values = self.metadata.public_binding_lanes * self.metadata.ext_degree;
        if self.public_values.len() != expected_public_values {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 compact verifier context public values length mismatch: expected {expected_public_values}, got {}",
                self.public_values.len()
            )));
        }
        Ok(())
    }
}

fn omit_preprocessed_ood_openings<SC>(proof: &mut BatchProof<SC>) -> usize
where
    SC: StarkGenericConfig,
{
    proof
        .opened_values
        .instances
        .iter_mut()
        .map(|inst| {
            let mut omitted = 0;
            if inst.base_opened_values.preprocessed_local.take().is_some() {
                omitted += 1;
            }
            if inst.base_opened_values.preprocessed_next.take().is_some() {
                omitted += 1;
            }
            omitted
        })
        .sum()
}

fn omit_goldilocks_tip5_preprocessed_fri_input_batches(
    proof: &mut BatchProof<GoldilocksTipsConfig>,
    common: &CommonData<GoldilocksTipsConfig>,
) -> Result<(), BatchStarkProverError> {
    if common.preprocessed.is_none() {
        return Ok(());
    }

    let preprocessed_idx = goldilocks_tip5_preprocessed_trace_idx();
    for (query, query_proof) in proof.opening_proof.query_proofs.iter_mut().enumerate() {
        if query_proof.input_proof.len() <= preprocessed_idx {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 compact proof query {query} lacks preprocessed FRI input batch"
            )));
        }
        query_proof.input_proof.remove(preprocessed_idx);
    }

    Ok(())
}

fn omit_goldilocks_blake3_preprocessed_fri_input_batches(
    proof: &mut BatchProof<GoldilocksBlake3Config>,
    common: &CommonData<GoldilocksBlake3Config>,
) -> Result<(), BatchStarkProverError> {
    if common.preprocessed.is_none() {
        return Ok(());
    }

    let preprocessed_idx = goldilocks_blake3_preprocessed_trace_idx();
    for (query, query_proof) in proof.opening_proof.query_proofs.iter_mut().enumerate() {
        if query_proof.input_proof.len() <= preprocessed_idx {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 compact proof query {query} lacks preprocessed FRI input batch"
            )));
        }
        query_proof.input_proof.remove(preprocessed_idx);
    }

    Ok(())
}

fn prune_goldilocks_tip5_merkle_paths(
    leaf_indices: &[usize],
    full_paths: &[GoldilocksTip5MerkleProof],
) -> Result<GoldilocksTip5PrunedMerklePaths, BatchStarkProverError> {
    if leaf_indices.len() != full_paths.len() {
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks/Tip5 path-pruned proof leaf/path count mismatch: {} leaves, {} paths",
            leaf_indices.len(),
            full_paths.len()
        )));
    }

    let full_path_len = full_paths.first().map_or(0, Vec::len);
    for (query, path) in full_paths.iter().enumerate() {
        if path.len() != full_path_len {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 query {query} Merkle path length mismatch: expected {full_path_len}, got {}",
                path.len()
            )));
        }
    }

    let mut sorted = leaf_indices
        .iter()
        .copied()
        .zip(full_paths.iter())
        .enumerate()
        .collect::<Vec<_>>();
    sorted.sort_by_key(|(_, (leaf_index, _))| *leaf_index);

    let mut original_order = vec![0_u32; full_paths.len()];
    let mut pruned_paths = Vec::new();
    let mut previous_leaf = None;
    let mut previous_full_path: Option<&GoldilocksTip5MerkleProof> = None;

    for (original_query, (leaf_index, full_path)) in sorted {
        if let Some(prev_leaf) = previous_leaf {
            if leaf_index == prev_leaf {
                let Some(prev_path) = previous_full_path else {
                    return Err(BatchStarkProverError::Verify(String::from(
                        "Goldilocks/Tip5 duplicate Merkle leaf without previous path",
                    )));
                };
                if full_path != prev_path {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 duplicate Merkle leaf {leaf_index} has inconsistent authentication paths",
                    )));
                }
                let sorted_index = pruned_paths.len().checked_sub(1).ok_or_else(|| {
                    BatchStarkProverError::Verify(String::from(
                        "Goldilocks/Tip5 duplicate Merkle leaf before first path",
                    ))
                })?;
                original_order[original_query] = u32::try_from(sorted_index).map_err(|_| {
                    BatchStarkProverError::Verify(String::from(
                        "Goldilocks/Tip5 pruned Merkle path index exceeds u32",
                    ))
                })?;
                continue;
            }
        }

        let keep_len = previous_leaf.map_or(full_path_len, |prev_leaf| {
            goldilocks_tip5_binary_unique_prefix_len(prev_leaf, leaf_index, full_path_len)
        });
        pruned_paths.push(PrunedPath {
            leaf_index,
            siblings: full_path[..keep_len].to_vec(),
        });
        original_order[original_query] = u32::try_from(pruned_paths.len() - 1).map_err(|_| {
            BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 pruned Merkle path index exceeds u32",
            ))
        })?;
        previous_leaf = Some(leaf_index);
        previous_full_path = Some(full_path);
    }

    Ok(GoldilocksTip5PrunedMerklePaths {
        full_path_len,
        paths: PrunedMerklePaths {
            original_order,
            paths: pruned_paths,
        },
    })
}

fn restore_goldilocks_tip5_merkle_paths(
    pruned: &GoldilocksTip5PrunedMerklePaths,
    expected_paths: usize,
    expected_leaf_indices: &[usize],
    max_path_len: usize,
    label: &str,
) -> Result<Vec<GoldilocksTip5MerkleProof>, BatchStarkProverError> {
    if pruned.full_path_len > max_path_len {
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks/Tip5 {label} pruned Merkle path length {} exceeds derived maximum {max_path_len}",
            pruned.full_path_len,
        )));
    }
    if pruned.paths.original_order.len() != expected_paths {
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks/Tip5 {label} pruned Merkle original-order length mismatch: expected {expected_paths}, got {}",
            pruned.paths.original_order.len()
        )));
    }
    if expected_leaf_indices.len() != expected_paths {
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks/Tip5 {label} expected leaf-index length mismatch: expected {expected_paths}, got {}",
            expected_leaf_indices.len()
        )));
    }
    if expected_paths == 0 {
        if pruned.paths.paths.is_empty() {
            return Ok(Vec::new());
        }
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks/Tip5 {label} pruned Merkle paths present for empty query set"
        )));
    }
    if pruned.paths.paths.is_empty() {
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks/Tip5 {label} pruned Merkle proof has no paths"
        )));
    }

    let mut restored_sorted: Vec<GoldilocksTip5MerkleProof> =
        Vec::with_capacity(pruned.paths.paths.len());
    let mut previous_leaf = None;
    for (sorted_index, path) in pruned.paths.paths.iter().enumerate() {
        let keep_len = if let Some(prev_leaf) = previous_leaf {
            if path.leaf_index <= prev_leaf {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 {label} pruned Merkle leaves are not strictly increasing",
                )));
            }
            goldilocks_tip5_binary_unique_prefix_len(
                prev_leaf,
                path.leaf_index,
                pruned.full_path_len,
            )
        } else {
            pruned.full_path_len
        };

        if path.siblings.len() != keep_len {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 {label} pruned Merkle path {sorted_index} sibling length mismatch: expected {keep_len}, got {}",
                path.siblings.len()
            )));
        }

        let mut restored = path.siblings.clone();
        if let Some(previous_path) = restored_sorted.last() {
            restored.extend_from_slice(&previous_path[keep_len..]);
        }
        if restored.len() != pruned.full_path_len {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 {label} restored Merkle path {sorted_index} length mismatch: expected {}, got {}",
                pruned.full_path_len,
                restored.len()
            )));
        }

        restored_sorted.push(restored);
        previous_leaf = Some(path.leaf_index);
    }

    for (query, (&sorted_index, &expected_leaf_index)) in pruned
        .paths
        .original_order
        .iter()
        .zip(expected_leaf_indices.iter())
        .enumerate()
    {
        let sorted_index = sorted_index as usize;
        let Some(path) = pruned.paths.paths.get(sorted_index) else {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 {label} pruned Merkle query {query} references missing sorted path {sorted_index}",
            )));
        };
        if path.leaf_index != expected_leaf_index {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 {label} query {query} leaf index mismatch: expected {expected_leaf_index}, got {}",
                path.leaf_index
            )));
        }
    }

    pruned
        .paths
        .original_order
        .iter()
        .enumerate()
        .map(|(query, &sorted_index)| {
            let sorted_index = sorted_index as usize;
            restored_sorted
                .get(sorted_index)
                .cloned()
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 {label} pruned Merkle query {query} references missing sorted path {sorted_index}",
                    ))
                })
        })
        .collect()
}

fn prune_merkle_paths_generic<D, const DIGEST_ELEMS: usize>(
    leaf_indices: &[usize],
    full_paths: &[Vec<[D; DIGEST_ELEMS]>],
    label: &str,
) -> Result<(usize, PrunedMerklePaths<D, DIGEST_ELEMS>), BatchStarkProverError>
where
    D: Clone + Eq,
{
    if leaf_indices.len() != full_paths.len() {
        return Err(BatchStarkProverError::Verify(format!(
            "{label} path-pruned proof leaf/path count mismatch: {} leaves, {} paths",
            leaf_indices.len(),
            full_paths.len()
        )));
    }

    let full_path_len = full_paths.first().map_or(0, Vec::len);
    for (query, path) in full_paths.iter().enumerate() {
        if path.len() != full_path_len {
            return Err(BatchStarkProverError::Verify(format!(
                "{label} query {query} Merkle path length mismatch: expected {full_path_len}, got {}",
                path.len()
            )));
        }
    }

    let mut sorted = leaf_indices
        .iter()
        .copied()
        .zip(full_paths.iter())
        .enumerate()
        .collect::<Vec<_>>();
    sorted.sort_by_key(|(_, (leaf_index, _))| *leaf_index);

    let mut original_order = vec![0_u32; full_paths.len()];
    let mut pruned_paths = Vec::new();
    let mut previous_leaf = None;
    let mut previous_full_path: Option<&Vec<[D; DIGEST_ELEMS]>> = None;

    for (original_query, (leaf_index, full_path)) in sorted {
        if let Some(prev_leaf) = previous_leaf
            && leaf_index == prev_leaf
        {
            let Some(prev_path) = previous_full_path else {
                return Err(BatchStarkProverError::Verify(format!(
                    "{label} duplicate Merkle leaf without previous path",
                )));
            };
            if full_path != prev_path {
                return Err(BatchStarkProverError::Verify(format!(
                    "{label} duplicate Merkle leaf {leaf_index} has inconsistent authentication paths",
                )));
            }
            let sorted_index = pruned_paths.len().checked_sub(1).ok_or_else(|| {
                BatchStarkProverError::Verify(format!(
                    "{label} duplicate Merkle leaf before first path",
                ))
            })?;
            original_order[original_query] = u32::try_from(sorted_index).map_err(|_| {
                BatchStarkProverError::Verify(format!(
                    "{label} pruned Merkle path index exceeds u32",
                ))
            })?;
            continue;
        }

        let keep_len = previous_leaf.map_or(full_path_len, |prev_leaf| {
            goldilocks_tip5_binary_unique_prefix_len(prev_leaf, leaf_index, full_path_len)
        });
        pruned_paths.push(PrunedPath {
            leaf_index,
            siblings: full_path[..keep_len].to_vec(),
        });
        original_order[original_query] = u32::try_from(pruned_paths.len() - 1).map_err(|_| {
            BatchStarkProverError::Verify(format!("{label} pruned Merkle path index exceeds u32"))
        })?;
        previous_leaf = Some(leaf_index);
        previous_full_path = Some(full_path);
    }

    Ok((
        full_path_len,
        PrunedMerklePaths {
            original_order,
            paths: pruned_paths,
        },
    ))
}

fn restore_merkle_paths_generic<D, const DIGEST_ELEMS: usize>(
    full_path_len: usize,
    paths: &PrunedMerklePaths<D, DIGEST_ELEMS>,
    expected_paths: usize,
    expected_leaf_indices: &[usize],
    max_path_len: usize,
    label: &str,
) -> Result<Vec<Vec<[D; DIGEST_ELEMS]>>, BatchStarkProverError>
where
    D: Clone + Eq,
{
    if full_path_len > max_path_len {
        return Err(BatchStarkProverError::Verify(format!(
            "{label} pruned Merkle path length {full_path_len} exceeds derived maximum {max_path_len}",
        )));
    }
    if paths.original_order.len() != expected_paths {
        return Err(BatchStarkProverError::Verify(format!(
            "{label} pruned Merkle original-order length mismatch: expected {expected_paths}, got {}",
            paths.original_order.len()
        )));
    }
    if expected_leaf_indices.len() != expected_paths {
        return Err(BatchStarkProverError::Verify(format!(
            "{label} expected leaf-index length mismatch: expected {expected_paths}, got {}",
            expected_leaf_indices.len()
        )));
    }
    if expected_paths == 0 {
        if paths.paths.is_empty() {
            return Ok(Vec::new());
        }
        return Err(BatchStarkProverError::Verify(format!(
            "{label} pruned Merkle paths present for empty query set"
        )));
    }
    if paths.paths.is_empty() {
        return Err(BatchStarkProverError::Verify(format!(
            "{label} pruned Merkle proof has no paths"
        )));
    }

    let mut restored_sorted: Vec<Vec<[D; DIGEST_ELEMS]>> = Vec::with_capacity(paths.paths.len());
    let mut previous_leaf = None;
    for (sorted_index, path) in paths.paths.iter().enumerate() {
        let keep_len = if let Some(prev_leaf) = previous_leaf {
            if path.leaf_index <= prev_leaf {
                return Err(BatchStarkProverError::Verify(format!(
                    "{label} pruned Merkle leaves are not strictly increasing",
                )));
            }
            goldilocks_tip5_binary_unique_prefix_len(prev_leaf, path.leaf_index, full_path_len)
        } else {
            full_path_len
        };

        if path.siblings.len() != keep_len {
            return Err(BatchStarkProverError::Verify(format!(
                "{label} pruned Merkle path {sorted_index} sibling length mismatch: expected {keep_len}, got {}",
                path.siblings.len()
            )));
        }

        let mut restored = path.siblings.clone();
        if let Some(previous_path) = restored_sorted.last() {
            restored.extend_from_slice(&previous_path[keep_len..]);
        }
        if restored.len() != full_path_len {
            return Err(BatchStarkProverError::Verify(format!(
                "{label} restored Merkle path {sorted_index} length mismatch: expected {full_path_len}, got {}",
                restored.len()
            )));
        }

        restored_sorted.push(restored);
        previous_leaf = Some(path.leaf_index);
    }

    for (query, (&sorted_index, &expected_leaf_index)) in paths
        .original_order
        .iter()
        .zip(expected_leaf_indices.iter())
        .enumerate()
    {
        let sorted_index = sorted_index as usize;
        let Some(path) = paths.paths.get(sorted_index) else {
            return Err(BatchStarkProverError::Verify(format!(
                "{label} pruned Merkle query {query} references missing sorted path {sorted_index}",
            )));
        };
        if path.leaf_index != expected_leaf_index {
            return Err(BatchStarkProverError::Verify(format!(
                "{label} query {query} leaf index mismatch: expected {expected_leaf_index}, got {}",
                path.leaf_index
            )));
        }
    }

    paths
        .original_order
        .iter()
        .enumerate()
        .map(|(query, &sorted_index)| {
            let sorted_index = sorted_index as usize;
            restored_sorted.get(sorted_index).cloned().ok_or_else(|| {
                BatchStarkProverError::Verify(format!(
                    "{label} pruned Merkle query {query} references missing sorted path {sorted_index}",
                ))
            })
        })
        .collect()
}

fn goldilocks_tip5_input_batch_leaf_indices(
    query_indices: &[usize],
    log_global_max_height: usize,
    full_path_len: usize,
    cap_height: usize,
) -> Result<Vec<usize>, BatchStarkProverError> {
    if full_path_len > log_global_max_height {
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks/Tip5 input Merkle path length {full_path_len} exceeds FRI global height log {log_global_max_height}",
        )));
    }
    let log_batch_height = full_path_len
        .saturating_add(cap_height)
        .min(log_global_max_height);
    Ok(query_indices
        .iter()
        .map(|index| index >> (log_global_max_height - log_batch_height))
        .collect())
}

fn goldilocks_tip5_binary_unique_prefix_len(
    previous_leaf: usize,
    leaf_index: usize,
    full_path_len: usize,
) -> usize {
    let differing_bits = (previous_leaf ^ leaf_index).ilog2() as usize + 1;
    differing_bits.min(full_path_len)
}

fn goldilocks_tip5_val_mmcs(cap_height: usize) -> GoldilocksTip5ValMmcs {
    let perm = Tip5Perm;
    let hash = GoldilocksTip5Hash::new(perm);
    let compress = GoldilocksTip5Compress::new(perm);
    MerkleTreeMmcs::new(hash, compress, cap_height)
}

fn evaluate_goldilocks_bit_reversed_lde_at<M>(
    lde_matrix: &M,
    log_blowup: usize,
    point: BinomialExtensionField<Goldilocks, 2>,
) -> Result<Vec<BinomialExtensionField<Goldilocks, 2>>, BatchStarkProverError>
where
    M: Matrix<Goldilocks>,
{
    let log_lde_height = log2_strict_usize(lde_matrix.height());
    if log_blowup > log_lde_height {
        return Err(BatchStarkProverError::Verify(format!(
            "Goldilocks log_blowup {log_blowup} exceeds LDE height log {log_lde_height}",
        )));
    }
    let log_low_height = log_lde_height - log_blowup;
    let low_height = checked_power_of_two(log_low_height, "Goldilocks/Tip5 low coset height")?;
    let width = lde_matrix.width();
    let mut natural_values = Vec::with_capacity(low_height * width);
    for row in 0..low_height {
        let bit_reversed_row = reverse_bits_len(row, log_low_height);
        for col in 0..width {
            natural_values.push(
                lde_matrix
                    .get(bit_reversed_row, col)
                    .expect("row/column are in bounds by construction"),
            );
        }
    }
    let natural_matrix = RowMajorMatrix::new(natural_values, width);
    Ok(natural_matrix.interpolate_coset(Goldilocks::GENERATOR, point))
}

fn goldilocks_tip5_preprocessed_trace_idx() -> usize {
    <<GoldilocksTipsConfig as StarkGenericConfig>::Pcs as Pcs<
        GoldilocksTip5Challenge,
        <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
    >>::PREPROCESSED_TRACE_IDX
}

fn goldilocks_blake3_preprocessed_trace_idx() -> usize {
    <<GoldilocksBlake3Config as StarkGenericConfig>::Pcs as Pcs<
        GoldilocksBlake3Challenge,
        <GoldilocksBlake3Config as StarkGenericConfig>::Challenger,
    >>::PREPROCESSED_TRACE_IDX
}

fn checked_power_of_two(log: usize, label: &str) -> Result<usize, BatchStarkProverError> {
    let shift = u32::try_from(log).map_err(|_| {
        BatchStarkProverError::Verify(format!("{label} log {log} does not fit in u32"))
    })?;
    1usize.checked_shl(shift).ok_or_else(|| {
        BatchStarkProverError::Verify(format!("{label} log {log} does not fit in usize"))
    })
}

impl<SC> core::fmt::Debug for BatchStarkProof<SC>
where
    SC: StarkGenericConfig,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let stark_common_summary = self.stark_common.preprocessed.as_ref().map(|gp| {
            (
                gp.instances.len(),
                gp.matrix_to_instance.len(),
                self.stark_common.lookups.len(),
            )
        });
        f.debug_struct("BatchStarkProof")
            .field("table_packing", &self.table_packing)
            .field("public_binding_lanes", &self.public_binding_lanes)
            .field("rows", &self.rows)
            .field("ext_degree", &self.ext_degree)
            .field("w_binomial", &self.w_binomial)
            .field("alu_quintic_trinomial", &self.alu_quintic_trinomial)
            .field(
                "stark_common(instances, matrices, lookups)",
                &stark_common_summary,
            )
            .finish()
    }
}

impl<SC> BatchStarkProof<SC>
where
    SC: StarkGenericConfig,
{
    /// Re-check the structural invariants that the prover enforces but
    /// `#[derive(Deserialize)]` can bypass.
    pub fn validate(&self) -> Result<(), ProofMetadataError> {
        match self.ext_degree {
            1 | 2 | 4 | 5 | 6 | 8 => {}
            d => return Err(ProofMetadataError::UnsupportedExtDegree(d)),
        }
        self.rows.validate()?;
        self.table_packing.validate()?;
        if self.public_binding_lanes != self.table_packing.public_binding_lanes() {
            return Err(ProofMetadataError::PublicBindingMismatch {
                proof_lanes: self.public_binding_lanes,
                packing_lanes: self.table_packing.public_binding_lanes(),
            });
        }
        for entry in &self.non_primitives {
            entry.validate()?;
        }
        Ok(())
    }
}

/// Produces a single batch STARK proof covering all circuit tables.
pub struct BatchStarkProver<SC>
where
    SC: StarkGenericConfig + 'static,
{
    config: SC,
    table_packing: TablePacking,
    /// Variant used for the primitive ALU AIR.
    alu_variant: AirVariant,
    /// Registered dynamic non-primitive table provers.
    non_primitive_provers: Vec<Box<dyn TableProver<SC>>>,
    /// When true, run the lookup debugger before proving to report imbalanced multisets.
    debug_lookups: bool,
}

/// Errors raised when proof metadata fails the structural invariants that the
/// type constructors enforce but `#[derive(Deserialize)]` can bypass.
///
/// Validated via [`BatchStarkProof::validate`] before native and recursive
/// verification so malformed serialized metadata is rejected up front.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProofMetadataError {
    /// A primitive table row count is zero (constructors require non-zero).
    #[error("primitive table row count must be non-zero")]
    ZeroRowCount,

    /// A primitive lane count is zero (`new`/`with_*` clamp to at least 1).
    #[error("`{0}` lane count must be at least 1")]
    ZeroLanes(&'static str),

    /// A non-primitive table lane count is zero (defaults/clamps to at least 1).
    #[error("non-primitive table `{0:?}` lane count must be at least 1")]
    ZeroNpoLanes(NpoTypeId),

    /// `min_trace_height` is not a non-zero power of two.
    #[error("minimum trace height must be a non-zero power of two (got {0})")]
    BadMinTraceHeight(usize),

    /// `horner_packed_steps` is less than 2.
    #[error("horner_packed_steps must be at least 2 (got {0})")]
    BadHornerPackedSteps(usize),

    /// The public table cannot bind more lanes than it packs into the first row.
    #[error("public binding lanes ({binding_lanes}) exceed public lanes ({public_lanes})")]
    PublicBindingExceedsLanes {
        binding_lanes: usize,
        public_lanes: usize,
    },

    /// Serialized proof metadata disagrees with the table packing's public binding lanes.
    #[error(
        "public binding lane metadata mismatch: proof has {proof_lanes}, packing has {packing_lanes}"
    )]
    PublicBindingMismatch {
        proof_lanes: usize,
        packing_lanes: usize,
    },

    /// `ext_degree` is not one of the supported values.
    #[error("unsupported extension degree {0} (supported: 1,2,4,5,6,8)")]
    UnsupportedExtDegree(usize),
}

/// Errors for the batch STARK table prover.
#[derive(Debug, Error)]
pub enum BatchStarkProverError {
    /// The extension field degree is not one of the supported values (1, 2, 4, 6, 8).
    #[error("unsupported extension degree: {0} (supported: 1,2,4,5,6,8)")]
    UnsupportedDegree(usize),

    /// An extension field with degree > 1 was requested but the binomial parameter `W` was not provided.
    #[error("missing binomial parameter W for extension-field multiplication")]
    MissingWForExtension,

    /// The batch STARK verifier rejected the proof.
    #[error("verification failed: {0}")]
    Verify(String),

    /// A non-primitive table entry references an op type for which no [`TableProver`] was registered.
    #[error("missing table prover for non-primitive op `{0:?}`")]
    MissingTableProver(NpoTypeId),

    /// Proof metadata failed structural validation before verification.
    #[error("invalid proof metadata: {0}")]
    InvalidMetadata(#[from] ProofMetadataError),
}

impl<SC, const D: usize> BaseAir<Val<SC>> for CircuitTableAir<SC, D>
where
    SC: StarkGenericConfig,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SymbolicExpression<Val<SC>>>,
{
    fn width(&self) -> usize {
        match self {
            Self::Const(a) => a.width(),
            Self::Public(a) => a.width(),
            Self::Alu(a) => a.width(),
            Self::Dynamic(a) => <dyn CloneableBatchAir<SC> as BaseAir<Val<SC>>>::width(a.air()),
        }
    }

    fn preprocessed_width(&self) -> usize {
        match self {
            Self::Const(a) => BaseAir::<Val<SC>>::preprocessed_width(a),
            Self::Public(a) => BaseAir::<Val<SC>>::preprocessed_width(a),
            Self::Alu(a) => BaseAir::<Val<SC>>::preprocessed_width(a),
            Self::Dynamic(a) => {
                <dyn CloneableBatchAir<SC> as BaseAir<Val<SC>>>::preprocessed_width(a.air())
            }
        }
    }

    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<Val<SC>>> {
        match self {
            Self::Const(a) => a.preprocessed_trace(),
            Self::Public(a) => a.preprocessed_trace(),
            Self::Alu(a) => a.preprocessed_trace(),
            Self::Dynamic(a) => {
                <dyn CloneableBatchAir<SC> as BaseAir<Val<SC>>>::preprocessed_trace(a.air())
            }
        }
    }

    fn main_next_row_columns(&self) -> Vec<usize> {
        match self {
            Self::Const(a) => a.main_next_row_columns(),
            Self::Public(a) => a.main_next_row_columns(),
            Self::Alu(a) => a.main_next_row_columns(),
            Self::Dynamic(a) => a.main_next_row_columns(),
        }
    }

    fn preprocessed_next_row_columns(&self) -> Vec<usize> {
        match self {
            Self::Const(a) => a.preprocessed_next_row_columns(),
            Self::Public(a) => a.preprocessed_next_row_columns(),
            Self::Alu(a) => a.preprocessed_next_row_columns(),
            Self::Dynamic(a) => a.preprocessed_next_row_columns(),
        }
    }

    fn num_constraints(&self) -> Option<usize> {
        match self {
            Self::Const(a) => a.num_constraints(),
            Self::Public(a) => a.num_constraints(),
            Self::Alu(a) => a.num_constraints(),
            Self::Dynamic(a) => a.num_constraints(),
        }
    }

    fn max_constraint_degree(&self) -> Option<usize> {
        match self {
            Self::Const(a) => a.max_constraint_degree(),
            Self::Public(a) => a.max_constraint_degree(),
            Self::Alu(a) => a.max_constraint_degree(),
            Self::Dynamic(a) => a.max_constraint_degree(),
        }
    }

    fn num_public_values(&self) -> usize {
        match self {
            Self::Const(a) => a.num_public_values(),
            Self::Public(a) => a.num_public_values(),
            Self::Alu(a) => a.num_public_values(),
            Self::Dynamic(a) => {
                <dyn CloneableBatchAir<SC> as BaseAir<Val<SC>>>::num_public_values(a.air())
            }
        }
    }
}

macro_rules! impl_circuit_table_air_for_builder {
    ($builder_ty:ty) => {
        fn eval(&self, builder: &mut $builder_ty) {
            match self {
                Self::Const(a) => Air::<$builder_ty>::eval(a, builder),
                Self::Public(a) => Air::<$builder_ty>::eval(a, builder),
                Self::Alu(a) => Air::<$builder_ty>::eval(a, builder),
                Self::Dynamic(a) => Air::<$builder_ty>::eval(a, builder),
            }
        }
    };
}

impl<SC, const D: usize> Air<InteractionSymbolicBuilder<Val<SC>, SC::Challenge>>
    for CircuitTableAir<SC, D>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    impl_circuit_table_air_for_builder!(InteractionSymbolicBuilder<Val<SC>, SC::Challenge>);
}

#[cfg(debug_assertions)]
impl<'a, SC, const D: usize> Air<DebugConstraintBuilder<'a, Val<SC>, SC::Challenge>>
    for CircuitTableAir<SC, D>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SymbolicExpression<Val<SC>>>,
{
    impl_circuit_table_air_for_builder!(DebugConstraintBuilder<'a, Val<SC>, SC::Challenge>);
}

impl<'a, SC, const D: usize> Air<ProverConstraintFolderWithLookups<'a, SC>>
    for CircuitTableAir<SC, D>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SymbolicExpression<Val<SC>>>,
{
    impl_circuit_table_air_for_builder!(ProverConstraintFolderWithLookups<'a, SC>);
}

impl<'a, SC, const D: usize> Air<VerifierConstraintFolderWithLookups<'a, SC>>
    for CircuitTableAir<SC, D>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SymbolicExpression<Val<SC>>>,
{
    impl_circuit_table_air_for_builder!(VerifierConstraintFolderWithLookups<'a, SC>);
}

/// Extract the lookups for a `CircuitTableAir` by symbolic evaluation. The dispatch by
/// inner variant is needed to satisfy the AIR trait bound on the matched arms.
pub(crate) fn lookups_for_circuit_table_air<SC, const D: usize>(
    air: &CircuitTableAir<SC, D>,
) -> Lookups<Val<SC>>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    match air {
        CircuitTableAir::Const(a) => Lookups::from_air::<SC::Challenge, _>(a),
        CircuitTableAir::Public(a) => {
            let mut lookup_air = a.clone();
            lookup_air.public_binding_lanes = 0;
            Lookups::from_air::<SC::Challenge, _>(&lookup_air)
        }
        CircuitTableAir::Alu(a) => Lookups::from_air::<SC::Challenge, _>(a),
        CircuitTableAir::Dynamic(a) => Lookups::from_air::<SC::Challenge, _>(a),
    }
}

pub fn strip_public_binding_for_lookup_metadata<SC, const D: usize>(
    air: &CircuitTableAir<SC, D>,
) -> CircuitTableAir<SC, D>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    match air {
        CircuitTableAir::Public(a) => {
            let mut lookup_air = a.clone();
            lookup_air.public_binding_lanes = 0;
            CircuitTableAir::Public(lookup_air)
        }
        _ => air.clone(),
    }
}

fn lookup_shapes_eq<F: Field>(left: &Lookups<F>, right: &Lookups<F>) -> bool {
    left.len() == right.len()
        && left.iter().zip(right.iter()).all(|(left, right)| {
            left.kind == right.kind
                && left.column == right.column
                && left.elements.len() == right.elements.len()
                && left.multiplicities.len() == right.multiplicities.len()
                && left
                    .elements
                    .iter()
                    .zip(&right.elements)
                    .all(|(left, right)| left.len() == right.len())
        })
}

fn cached_preprocessed_metadata_matches_runtime_shape<SC, const D: usize>(
    common: &CommonData<SC>,
    airs: &[CircuitTableAir<SC, D>],
    trace_ext_degree_bits: &[usize],
    is_zk: usize,
) -> bool
where
    SC: StarkGenericConfig,
{
    if airs.len() != trace_ext_degree_bits.len() {
        return false;
    }

    let mut expected_instances = Vec::with_capacity(airs.len());
    let mut expected_matrix_to_instance = Vec::new();
    for (i, (air, &ext_db)) in airs.iter().zip(trace_ext_degree_bits).enumerate() {
        let Some(base_db) = ext_db.checked_sub(is_zk) else {
            return false;
        };
        let Some(preprocessed) = air.preprocessed_trace() else {
            expected_instances.push(None);
            continue;
        };
        let width = preprocessed.width();
        if width == 0 {
            expected_instances.push(None);
            continue;
        }
        if preprocessed.height() != (1usize << base_db) {
            return false;
        }
        let matrix_index = expected_matrix_to_instance.len();
        expected_matrix_to_instance.push(i);
        expected_instances.push(Some((matrix_index, width, ext_db)));
    }

    match &common.preprocessed {
        None => expected_matrix_to_instance.is_empty(),
        Some(cached) => {
            cached.matrix_to_instance == expected_matrix_to_instance
                && cached.instances.len() == expected_instances.len()
                && cached
                    .instances
                    .iter()
                    .zip(expected_instances)
                    .all(|(cached, expected)| match (cached, expected) {
                        (None, None) => true,
                        (Some(cached), Some((matrix_index, width, degree_bits))) => {
                            cached.matrix_index == matrix_index
                                && cached.width == width
                                && cached.degree_bits == degree_bits
                        }
                        _ => false,
                    })
        }
    }
}

fn cached_prover_data_matches_runtime_shape<SC, const D: usize>(
    cached: &ProverData<SC>,
    airs: &[CircuitTableAir<SC, D>],
    trace_ext_degree_bits: &[usize],
    is_zk: usize,
) -> bool
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    // This is a prover-side cache compatibility guard, not a verifier
    // soundness check. Verification still rebuilds AIRs/lookups from proof
    // metadata and binds preprocessed commitments through `CommonData`.
    if !cached_preprocessed_metadata_matches_runtime_shape(
        &cached.common,
        airs,
        trace_ext_degree_bits,
        is_zk,
    ) {
        return false;
    }
    if cached.common.preprocessed.is_some() != cached.prover_only.preprocessed_prover_data.is_some()
    {
        return false;
    }
    if cached.common.lookups.len() != airs.len() {
        return false;
    }
    cached.common.lookups.iter().zip(airs).all(|(cached, air)| {
        let expected = lookups_for_circuit_table_air::<SC, D>(air);
        lookup_shapes_eq(cached, &expected)
    })
}

/// Const-generic dispatch for [`BatchStarkProver::register_poseidon2_table`]: only the chosen
/// extension degree's `BinomiallyExtendable` bound is required on `Val<SC>`.
#[doc(hidden)]
pub trait RegisterPoseidon2ForExt<const D: usize, SC>
where
    SC: StarkGenericConfig + 'static,
{
    fn register_poseidon2(prover: &mut BatchStarkProver<SC>, config: Poseidon2Config);
}

impl<SC> RegisterPoseidon2ForExt<2, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<2>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_poseidon2(prover: &mut BatchStarkProver<SC>, config: Poseidon2Config) {
        prover.register_table_prover(Box::new(Poseidon2ProverD2::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

impl<SC> RegisterPoseidon2ForExt<4, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<4>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_poseidon2(prover: &mut BatchStarkProver<SC>, config: Poseidon2Config) {
        prover.register_table_prover(Box::new(Poseidon2Prover::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

impl<SC> RegisterPoseidon2ForExt<5, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<4>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_poseidon2(prover: &mut BatchStarkProver<SC>, config: Poseidon2Config) {
        prover.register_table_prover(Box::new(Poseidon2Prover::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

/// Const-generic dispatch for [`BatchStarkProver::register_poseidon1_table`]: only the chosen
/// extension degree's `BinomiallyExtendable` bound is required on `Val<SC>`.
#[doc(hidden)]
pub trait RegisterPoseidon1ForExt<const D: usize, SC>
where
    SC: StarkGenericConfig + 'static,
{
    fn register_poseidon1(prover: &mut BatchStarkProver<SC>, config: Poseidon1Config);
}

impl<SC> RegisterPoseidon1ForExt<2, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<2>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_poseidon1(prover: &mut BatchStarkProver<SC>, config: Poseidon1Config) {
        prover.register_table_prover(Box::new(Poseidon1ProverD2::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

impl<SC> RegisterPoseidon1ForExt<4, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<4>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_poseidon1(prover: &mut BatchStarkProver<SC>, config: Poseidon1Config) {
        prover.register_table_prover(Box::new(Poseidon1Prover::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

impl<SC> RegisterPoseidon1ForExt<5, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<4>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_poseidon1(prover: &mut BatchStarkProver<SC>, config: Poseidon1Config) {
        prover.register_table_prover(Box::new(Poseidon1Prover::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

/// Const-generic dispatch for [`BatchStarkProver::register_tip5_table`].
///
/// Tip5 is the deployed Goldilocks **base-field** (`d == 1`) permutation,
/// but the recursion verifier circuit operates over the STARK *challenge*
/// field (`BinomialExtensionField<Goldilocks, 2>`), so it is packed in a
/// `D == 2` circuit (the ai-pow-zk Layer-0 case — mirror of the Poseidon1
/// D=1-in-D≥2 path, `RegisterPoseidon1ForExt<{1,2,4,5}>`).
///
/// `D == 1` is the standalone base-field Tip5 (`test_tip5_lookups`);
/// `D == 2` is the Layer-0 recursion verifier circuit. Both register the
/// *same* [`tip5::Tip5Prover`]: its `TableProver` impl already provides
/// `batch_instance_d2` (via `impl_table_prover_batch_instances_from_base!`,
/// which transmutes the D=2 traces back to base-field and delegates to the
/// validated `batch_instance_base`), and the committed-preprocessed
/// override carries the D-correct witness-index scaling from the
/// (already-present) `Tip5Preprocessor` D=2 arm. No constraint / `tip5_l`
/// bus / single-row design is touched.
#[doc(hidden)]
pub trait RegisterTip5ForExt<const D: usize, SC>
where
    SC: StarkGenericConfig + 'static,
{
    fn register_tip5(prover: &mut BatchStarkProver<SC>, config: Tip5Config);
}

impl<SC> RegisterTip5ForExt<1, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_tip5(prover: &mut BatchStarkProver<SC>, config: Tip5Config) {
        prover.register_table_prover(Box::new(tip5::Tip5Prover::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

impl<SC> RegisterTip5ForExt<2, SC> for ()
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    fn register_tip5(prover: &mut BatchStarkProver<SC>, config: Tip5Config) {
        prover.register_table_prover(Box::new(tip5::Tip5Prover::new(
            config,
            ConstraintProfile::Standard,
        )));
    }
}

impl<SC> BatchStarkProver<SC>
where
    SC: StarkGenericConfig + 'static,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    /// Create a new prover with the given STARK config and default table packing.
    pub fn new(config: SC) -> Self {
        Self {
            config,
            table_packing: TablePacking::default(),
            alu_variant: AirVariant::Optimized,
            non_primitive_provers: Vec::new(),
            debug_lookups: false,
        }
    }

    /// Override the default [`TablePacking`] configuration (builder-style).
    #[must_use]
    pub fn with_table_packing(mut self, table_packing: TablePacking) -> Self {
        self.table_packing = table_packing;
        self
    }

    /// Enable the lookup debugger. When set, `prove_all_tables` will run
    /// `check_lookups` on the constructed traces before generating the proof,
    /// panicking with a detailed message on any multiset imbalance.
    #[must_use]
    pub const fn with_debug_lookups(mut self) -> Self {
        self.debug_lookups = true;
        self
    }

    /// Register a dynamic non-primitive table prover.
    pub fn register_table_prover(&mut self, prover: Box<dyn TableProver<SC>>) {
        self.non_primitive_provers.push(prover);
    }

    /// Builder-style registration for a dynamic non-primitive table prover.
    #[must_use]
    pub fn with_table_prover(mut self, prover: Box<dyn TableProver<SC>>) -> Self {
        self.register_table_prover(prover);
        self
    }

    /// Register the non-primitive Poseidon2 table prover for extension degree `D` (`2` or `4`).
    pub fn register_poseidon2_table<const D: usize>(&mut self, config: Poseidon2Config)
    where
        SC: Send + Sync,
        (): RegisterPoseidon2ForExt<D, SC>,
    {
        <() as RegisterPoseidon2ForExt<D, SC>>::register_poseidon2(self, config);
    }

    /// Register the non-primitive Poseidon1 table prover for extension degree `D` (`2`, `4` or `5`).
    pub fn register_poseidon1_table<const D: usize>(&mut self, config: Poseidon1Config)
    where
        SC: Send + Sync,
        (): RegisterPoseidon1ForExt<D, SC>,
    {
        <() as RegisterPoseidon1ForExt<D, SC>>::register_poseidon1(self, config);
    }

    /// Register the non-primitive Tip5 table prover for extension
    /// degree `D` (only `1` — the deployed Goldilocks base-field Tip5).
    pub fn register_tip5_table<const D: usize>(&mut self, config: Tip5Config)
    where
        SC: Send + Sync,
        (): RegisterTip5ForExt<D, SC>,
    {
        <() as RegisterTip5ForExt<D, SC>>::register_tip5(self, config);
    }

    /// Register the recompose (BF→EF packing) table prover(s) for extension degree `D`.
    ///
    /// Set `split_coeff_tables` to `true` when the Poseidon2 permutation degree can differ
    /// from the circuit extension degree `D` (e.g. D=1 Poseidon2 in a D=5 circuit). That
    /// registers both the standard `recompose` table and `recompose/coeff` (per-coefficient
    /// WitnessChecks receives only where the circuit uses them).
    pub fn register_recompose_table<const D: usize>(&mut self, split_coeff_tables: bool)
    where
        SC: Send + Sync,
    {
        for prover in recompose_table_provers::<SC, D>(1, split_coeff_tables) {
            self.register_table_prover(prover);
        }
    }

    /// Builder-style registration for the recompose table prover.
    #[must_use]
    pub fn with_recompose_table<const D: usize>(mut self, split_coeff_tables: bool) -> Self
    where
        SC: Send + Sync,
    {
        self.register_recompose_table::<D>(split_coeff_tables);
        self
    }

    /// Return the current [`TablePacking`] configuration.
    #[inline]
    pub const fn table_packing(&self) -> &TablePacking {
        &self.table_packing
    }

    /// Select which ALU AIR variant to use for primitive tables.
    #[must_use]
    pub const fn with_alu_variant(mut self, variant: AirVariant) -> Self {
        self.alu_variant = variant;
        self
    }

    /// Generate a unified batch STARK proof for all circuit tables.
    #[instrument(skip_all)]
    pub fn prove_all_tables<EF>(
        &self,
        traces: &Traces<EF>,
        circuit_prover_data: &CircuitProverData<SC>,
    ) -> Result<BatchStarkProof<SC>, BatchStarkProverError>
    where
        EF: Field + BasedVectorSpace<Val<SC>> + ExtractBinomialW<Val<SC>>,
        SC::Pcs: Sync,
        Val<SC>: Send + Sync,
        SC::Challenge: Send + Sync,
        Domain<SC>: Send + Sync,
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::ProverData: Sync,
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: Sync,
        CircuitTableAir<SC, 1>: Sync,
        CircuitTableAir<SC, 2>: Sync,
        CircuitTableAir<SC, 4>: Sync,
        CircuitTableAir<SC, 5>: Sync,
        CircuitTableAir<SC, 6>: Sync,
        CircuitTableAir<SC, 8>: Sync,
        SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SymbolicExpression<Val<SC>>>,
    {
        let w_opt = EF::extract_w();
        match EF::DIMENSION {
            1 => self.prove::<EF, 1>(traces, None, circuit_prover_data),
            2 => self.prove::<EF, 2>(traces, w_opt, circuit_prover_data),
            4 => self.prove::<EF, 4>(traces, w_opt, circuit_prover_data),
            5 => self.prove::<EF, 5>(traces, w_opt, circuit_prover_data),
            6 => self.prove::<EF, 6>(traces, w_opt, circuit_prover_data),
            8 => self.prove::<EF, 8>(traces, w_opt, circuit_prover_data),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    /// Verify the unified batch STARK proof against all tables.
    pub fn verify_all_tables(
        &self,
        proof: &BatchStarkProof<SC>,
    ) -> Result<(), BatchStarkProverError> {
        self.verify_all_tables_with_public_values(proof, &[])
    }

    /// Verify the unified batch STARK proof while binding the leading Public
    /// table lanes to caller-supplied STARK public values.
    pub fn verify_all_tables_with_public_values(
        &self,
        proof: &BatchStarkProof<SC>,
        public_values: &[Val<SC>],
    ) -> Result<(), BatchStarkProverError> {
        proof.validate()?;
        let common = &proof.stark_common;
        match proof.ext_degree {
            1 => self.verify::<1>(proof, None, common, public_values),
            2 => self.verify::<2>(proof, proof.w_binomial, common, public_values),
            4 => self.verify::<4>(proof, proof.w_binomial, common, public_values),
            5 => self.verify::<5>(proof, proof.w_binomial, common, public_values),
            6 => self.verify::<6>(proof, proof.w_binomial, common, public_values),
            8 => self.verify::<8>(proof, proof.w_binomial, common, public_values),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    /// Verify a compact proof whose preprocessed OOD openings were omitted.
    ///
    /// `canonical_setup` must be the setup data for the exact statement being
    /// verified. Its preprocessed commitment and metadata are compared to the
    /// proof's serialized `stark_common` before any omitted value is restored.
    pub fn verify_compact_preprocessed_ood(
        &self,
        proof: PreprocessedOodCompactBatchStarkProof<SC>,
        canonical_setup: &CircuitProverData<SC>,
    ) -> Result<(), BatchStarkProverError>
    where
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: PartialEq,
    {
        self.verify_compact_preprocessed_ood_with_public_values(proof, &[], canonical_setup)
    }

    /// Verify a compact proof while binding leading Public-table lanes.
    ///
    /// This restores omitted preprocessed OOD openings from `canonical_setup`
    /// and then runs the same `p3-batch-stark` verifier used for full proofs.
    pub fn verify_compact_preprocessed_ood_with_public_values(
        &self,
        proof: PreprocessedOodCompactBatchStarkProof<SC>,
        public_values: &[Val<SC>],
        canonical_setup: &CircuitProverData<SC>,
    ) -> Result<(), BatchStarkProverError>
    where
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: PartialEq,
    {
        proof.proof.validate()?;
        if !common_preprocessed_binding_eq(
            &proof.proof.stark_common,
            &canonical_setup.prover_data.common,
        ) {
            return Err(BatchStarkProverError::Verify(String::from(
                "compact preprocessed OOD proof does not match canonical setup binding",
            )));
        }

        let ext_degree = proof.proof.ext_degree;
        let w_binomial = proof.proof.w_binomial;
        match ext_degree {
            1 => self.verify_compact::<1>(proof, None, public_values, canonical_setup),
            2 => self.verify_compact::<2>(proof, w_binomial, public_values, canonical_setup),
            4 => self.verify_compact::<4>(proof, w_binomial, public_values, canonical_setup),
            5 => self.verify_compact::<5>(proof, w_binomial, public_values, canonical_setup),
            6 => self.verify_compact::<6>(proof, w_binomial, public_values, canonical_setup),
            8 => self.verify_compact::<8>(proof, w_binomial, public_values, canonical_setup),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    /// Generate a batch STARK proof for a specific extension field degree.
    ///
    /// This is the core proving logic that handles all circuit tables for a given
    /// extension field dimension. It constructs AIRs, converts traces to matrices,
    /// and generates the unified proof.
    fn prove<EF, const D: usize>(
        &self,
        traces: &Traces<EF>,
        w_binomial: Option<Val<SC>>,
        circuit_prover_data: &CircuitProverData<SC>,
    ) -> Result<BatchStarkProof<SC>, BatchStarkProverError>
    where
        EF: Field + BasedVectorSpace<Val<SC>> + ExtractBinomialW<Val<SC>>,
        SC::Pcs: Sync,
        Val<SC>: Send + Sync,
        SC::Challenge: Send + Sync,
        Domain<SC>: Send + Sync,
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::ProverData: Sync,
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: Sync,
        CircuitTableAir<SC, D>: Sync,
    {
        let primitive = &circuit_prover_data.primitive_columns;
        let non_primitive = &circuit_prover_data.non_primitive_columns;

        // One lookup per NpoTypeId instead of repeated `op_type()` (clones inner id string).
        let prover_index_by_type: BTreeMap<NpoTypeId, usize> = self
            .non_primitive_provers
            .iter()
            .enumerate()
            .map(|(i, p)| (p.op_type(), i))
            .collect();

        // Build matrices and AIRs per table.
        let packing = &self.table_packing;
        let min_height = packing.min_trace_height();

        // Check if Alu table has only dummy operations (trace length <= 1).
        // The table implementation adds a dummy row when empty, so we check for <= 1.
        // Using lanes > 1 with only dummy operations causes issues in recursive verification
        // due to a bug in how multi-lane padding interacts with lookup constraints.
        // We automatically reduce lanes to 1 in these cases with a warning.
        let alu_trace_only_dummy = traces.alu_trace.op_kind.len() <= 1;

        let alu_lanes = if alu_trace_only_dummy && packing.alu_lanes() > 1 {
            tracing::warn!(
                "ALu table has only dummy operations but alu_lanes={} > 1. Reducing to \
                 alu_lanes=1 to avoid recursive verification issues. Consider using \
                 alu_lanes=1 when no additions are expected.",
                packing.alu_lanes()
            );
            1
        } else {
            packing.alu_lanes()
        };

        // Const — preprocessed is already in [ext_mult, index] 2-col format.
        let const_rows = traces.const_trace.values.len();
        let const_prep = primitive[PrimitiveOpType::Const as usize].clone();
        let const_air = ConstAir::<Val<SC>, D>::new_with_preprocessed(const_rows, const_prep)
            .with_min_height(min_height);
        let const_matrix: RowMajorMatrix<Val<SC>> =
            ConstAir::<Val<SC>, D>::trace_to_matrix(&traces.const_trace, min_height);

        // Public — reduce lanes to 1 if the table has only dummy operations.
        let public_trace_only_dummy = traces.public_trace.values.len() <= 1;
        let public_lanes = if public_trace_only_dummy && packing.public_lanes() > 1 {
            tracing::warn!(
                "Public table has only dummy operations but public_lanes={} > 1. Reducing to \
                 public_lanes=1 to avoid recursive verification issues. Consider using \
                 public_lanes=1 when few public inputs are expected.",
                packing.public_lanes()
            );
            1
        } else {
            packing.public_lanes()
        };

        // Preprocessed is already in [ext_mult, index] 2-col format.
        let public_rows = traces.public_trace.values.len();
        let public_prep = primitive[PrimitiveOpType::Public as usize].clone();
        let public_air =
            PublicAir::<Val<SC>, D>::new_with_preprocessed(public_rows, public_lanes, public_prep)
                .with_public_binding_lanes(packing.public_binding_lanes())
                .with_min_height(min_height);
        let public_matrix: RowMajorMatrix<Val<SC>> = PublicAir::<Val<SC>, D>::trace_to_matrix(
            &traces.public_trace,
            public_lanes,
            min_height,
        );
        let public_binding_values =
            flatten_public_binding_values::<SC, EF, D>(traces, packing.public_binding_lanes())?;

        // ALU — preprocessed is already in 10-col format (with multiplicities) from
        // get_airs_and_degrees_with_prep. When the trace is empty, a dummy row is included.
        let alu_rows = traces.alu_trace.values.len();
        let alu_prep = primitive[PrimitiveOpType::Alu as usize].clone();
        let alu_num_ops = alu_prep.len() / AluAir::<Val<SC>, D>::preprocessed_lane_width();
        let horner_k = packing.horner_packed_steps();
        let alu_quintic = D == 5 && EF::alu_is_quintic_trinomial();
        let alu_air: AluAir<Val<SC>, D> = if D == 1 {
            AluAir::<Val<SC>, D>::new_with_preprocessed(alu_num_ops, alu_lanes, alu_prep, horner_k)
                .with_min_height(min_height)
        } else if alu_quintic {
            AluAir::<Val<SC>, D>::new_quintic_trinomial_with_preprocessed(
                alu_num_ops,
                alu_lanes,
                alu_prep,
                horner_k,
            )
            .with_min_height(min_height)
        } else {
            let w = w_binomial.ok_or(BatchStarkProverError::MissingWForExtension)?;
            AluAir::<Val<SC>, D>::new_binomial_with_preprocessed(
                alu_num_ops,
                alu_lanes,
                w,
                alu_prep,
                horner_k,
            )
            .with_min_height(min_height)
        };
        let alu_matrix: RowMajorMatrix<Val<SC>> =
            alu_air.trace_to_matrix(&traces.alu_trace, min_height);
        let alu_scheduled_entries = alu_air.scheduled_entry_count();

        // We first handle all non-primitive tables dynamically, which will then be batched alongside primitive ones.
        // Each trace must have a corresponding registered prover for it to be provable.
        for (op_type, trace) in &traces.non_primitive_traces {
            if trace.rows() == 0 {
                continue;
            }
            if !prover_index_by_type.contains_key(op_type) {
                return Err(BatchStarkProverError::MissingTableProver(op_type.clone()));
            }
        }

        let mut dynamic_instances: Vec<BatchTableInstance<SC>> =
            Vec::with_capacity(self.non_primitive_provers.len());
        if D == 1 {
            let t: &Traces<Val<SC>> = unsafe { transmute_traces(traces) };
            for p in &self.non_primitive_provers {
                if let Some(instance) = p.batch_instance_d1(&self.config, packing, t) {
                    dynamic_instances.push(instance);
                }
            }
        } else if D == 2 {
            type EF2<F> = BinomialExtensionField<F, 2>;
            let t: &Traces<EF2<Val<SC>>> = unsafe { transmute_traces(traces) };
            for p in &self.non_primitive_provers {
                if let Some(instance) = p.batch_instance_d2(&self.config, packing, t) {
                    dynamic_instances.push(instance);
                }
            }
        } else if D == 4 {
            type EF4<F> = BinomialExtensionField<F, 4>;
            let t: &Traces<EF4<Val<SC>>> = unsafe { transmute_traces(traces) };
            for p in &self.non_primitive_provers {
                if let Some(instance) = p.batch_instance_d4(&self.config, packing, t) {
                    dynamic_instances.push(instance);
                }
            }
        } else if D == 6 {
            type EF6<F> = BinomialExtensionField<F, 6>;
            let t: &Traces<EF6<Val<SC>>> = unsafe { transmute_traces(traces) };
            for p in &self.non_primitive_provers {
                if let Some(instance) = p.batch_instance_d6(&self.config, packing, t) {
                    dynamic_instances.push(instance);
                }
            }
        } else if D == 8 {
            type EF8<F> = BinomialExtensionField<F, 8>;
            let t: &Traces<EF8<Val<SC>>> = unsafe { transmute_traces(traces) };
            for p in &self.non_primitive_provers {
                if let Some(instance) = p.batch_instance_d8(&self.config, packing, t) {
                    dynamic_instances.push(instance);
                }
            }
        } else if D == 5 {
            type EF5<F> = p3_field::extension::QuinticTrinomialExtensionField<F>;
            let t: &Traces<EF5<Val<SC>>> = unsafe { transmute_traces(traces) };
            for p in &self.non_primitive_provers {
                if let Some(instance) = p.batch_instance_d5(&self.config, packing, t) {
                    dynamic_instances.push(instance);
                }
            }
        }

        // The `batch_instance_dN` methods regenerate Poseidon2 preprocessed data from
        // runtime ops using `extract_preprocessed_from_operations`.
        //
        // Hence, we override here with the committed preprocessed data so the debug
        // lookup check is consistent with the committed preprocessed trace.
        for instance in &mut dynamic_instances {
            if let Some(committed_prep) = non_primitive.get(&instance.op_type)
                && let Some(&pi) = prover_index_by_type.get(&instance.op_type)
            {
                let p = &self.non_primitive_provers[pi];
                if let Some(new_air) = p.air_with_committed_preprocessed(
                    committed_prep.clone(),
                    min_height,
                    instance.lanes,
                    D as u32,
                ) {
                    instance.air = new_air;
                }
            }
        }

        TraceTablesLayout {
            const_: AirTableShape {
                main_cols: BaseAir::width(&const_air),
                prep_cols: ConstAir::<Val<SC>, D>::preprocessed_width(),
                rows: const_rows,
                lanes: 1,
            },
            public: AirTableShape {
                main_cols: BaseAir::width(&public_air),
                prep_cols: public_air.preprocessed_width(),
                rows: public_rows.div_ceil(public_lanes),
                lanes: public_lanes,
            },
            alu: AirTableShape {
                main_cols: BaseAir::width(&alu_air),
                prep_cols: alu_air.preprocessed_width(),
                rows: alu_scheduled_entries.div_ceil(alu_lanes),
                lanes: alu_lanes,
            },
            non_primitives: dynamic_instances
                .iter()
                .map(|inst| {
                    let prep_cols = BaseAir::preprocessed_trace(&inst.air)
                        .map(|m| m.width())
                        .unwrap_or(0);
                    let rows = traces
                        .non_primitive_traces
                        .get(&inst.op_type)
                        .map(|t| t.rows())
                        .unwrap_or(inst.rows);
                    (
                        inst.op_type.clone(),
                        AirTableShape {
                            main_cols: inst.trace.width(),
                            prep_cols,
                            rows: rows / inst.lanes,
                            lanes: inst.lanes,
                        },
                    )
                })
                .collect(),
        }
        .log();

        // Wrap AIRs in enum for heterogeneous batching and build instances in fixed order.
        let mut air_storage: Vec<CircuitTableAir<SC, D>> =
            Vec::with_capacity(NUM_PRIMITIVE_TABLES + dynamic_instances.len());
        let mut trace_storage: Vec<RowMajorMatrix<Val<SC>>> =
            Vec::with_capacity(NUM_PRIMITIVE_TABLES + dynamic_instances.len());
        let mut public_storage: Vec<Vec<Val<SC>>> =
            Vec::with_capacity(NUM_PRIMITIVE_TABLES + dynamic_instances.len());
        let mut non_primitive_meta: Vec<(NpoTypeId, usize, usize, AirVariant)> =
            Vec::with_capacity(dynamic_instances.len());

        // Pad all trace matrices to at least min_height (for FRI compatibility)
        air_storage.push(CircuitTableAir::Const(const_air));
        trace_storage.push(const_matrix);
        public_storage.push(Vec::new());

        air_storage.push(CircuitTableAir::Public(public_air));
        trace_storage.push(public_matrix);
        public_storage.push(public_binding_values);

        air_storage.push(CircuitTableAir::Alu(alu_air));
        trace_storage.push(alu_matrix);
        public_storage.push(Vec::new());

        for instance in dynamic_instances {
            let BatchTableInstance {
                op_type,
                air,
                mut trace,
                public_values,
                lanes,
                rows,
            } = instance;
            air_storage.push(CircuitTableAir::Dynamic(air));
            trace.pad_to_min_power_of_two_height(min_height, Val::<SC>::ZERO);
            trace_storage.push(trace);
            public_storage.push(public_values);
            non_primitive_meta.push((op_type, rows, lanes, AirVariant::Baseline));
        }

        // The circuit setup data is an upper-bound/static description. The
        // actual runner output may reduce lanes for dummy primitive tables or
        // use trace heights that differ from the static degree hints. Build the
        // proving common data from the exact AIRs and matrix heights committed
        // below so native `verify_all_tables` reconstructs the same statement.
        let trace_ext_degree_bits: Vec<usize> = trace_storage
            .iter()
            .map(|m| log2_strict_usize(m.height()) + self.config.is_zk())
            .collect();
        let lookup_metadata_airs: Vec<CircuitTableAir<SC, D>> = air_storage
            .iter()
            .map(strip_public_binding_for_lookup_metadata)
            .collect();
        let rebuilt_prover_data;
        let effective_prover_data = if cached_prover_data_matches_runtime_shape::<SC, D>(
            &circuit_prover_data.prover_data,
            &lookup_metadata_airs,
            &trace_ext_degree_bits,
            self.config.is_zk(),
        ) {
            &circuit_prover_data.prover_data
        } else {
            rebuilt_prover_data = ProverData::from_airs_and_degrees(
                &self.config,
                &lookup_metadata_airs,
                &trace_ext_degree_bits,
            );
            &rebuilt_prover_data
        };

        let proof = {
            let trace_refs: Vec<&RowMajorMatrix<Val<SC>>> = trace_storage.iter().collect();
            let instances: Vec<StarkInstance<'_, SC, CircuitTableAir<SC, D>>> =
                StarkInstance::new_multiple(&air_storage, &trace_refs, &public_storage);

            if self.debug_lookups {
                use p3_lookup::debug_util::{LookupDebugInstance, check_lookups};

                let mut preprocessed_traces: Vec<Option<RowMajorMatrix<Val<SC>>>> = instances
                    .iter()
                    .map(|inst| inst.air.preprocessed_trace())
                    .collect();

                for (j, (op_type, _, lanes, _)) in non_primitive_meta.iter().enumerate() {
                    if let Some(committed_prep) = non_primitive.get(op_type) {
                        let prover = self
                            .non_primitive_provers
                            .iter()
                            .find(|p| TableProver::op_type(p.as_ref()) == *op_type);
                        if let Some(prover) = prover
                            && let Some(air) = prover.air_with_committed_preprocessed(
                                committed_prep.clone(),
                                min_height,
                                *lanes,
                                D as u32,
                            )
                            && let Some(trace) = air.preprocessed_trace()
                        {
                            preprocessed_traces[NUM_PRIMITIVE_TABLES + j] = Some(trace);
                        }
                    }
                }

                let debug_instance_lookups: Vec<Lookups<Val<SC>>> = instances
                    .iter()
                    .map(|inst| lookups_for_circuit_table_air::<SC, D>(inst.air))
                    .collect();
                let debug_instances: Vec<LookupDebugInstance<'_, Val<SC>>> = instances
                    .iter()
                    .zip(preprocessed_traces.iter())
                    .zip(debug_instance_lookups.iter())
                    .map(|((inst, prep), lookups)| LookupDebugInstance {
                        main_trace: inst.trace,
                        preprocessed_trace: prep,
                        public_values: &inst.public_values,
                        lookups,
                        permutation_challenges: &[],
                    })
                    .collect();
                check_lookups(&debug_instances);
            }

            p3_batch_stark::prove_batch(&self.config, &instances, &effective_prover_data)
        };

        let dynamic_public_values = public_storage.drain(NUM_PRIMITIVE_TABLES..);
        let non_primitives: Vec<NonPrimitiveTableEntry<SC>> = non_primitive_meta
            .into_iter()
            .zip(dynamic_public_values)
            .map(
                |((op_type, rows, lanes, air_variant), public_values)| NonPrimitiveTableEntry {
                    op_type,
                    rows,
                    lanes,
                    public_values,
                    air_variant,
                },
            )
            .collect();

        // Ensure all primitive table row counts are at least 1
        // RowCounts::new requires non-zero counts, so pad zeros to 1
        let const_rows_padded = const_rows.max(1);
        let public_rows_padded = public_rows.max(1);
        let alu_rows_padded = alu_rows.max(1);

        // Store the effective packing (reduced lanes if applicable) so the verifier matches
        // proving. Clone full config so `horner_packed_steps`, NPO lane overrides, etc. are preserved.
        let effective_packing = self
            .table_packing
            .clone()
            .with_public_alu_lanes(public_lanes, alu_lanes);

        // Populate `stark_common` so the proof is self-binding to the exact
        // preprocessed metadata used for this proof.
        let stark_common = clone_common_data(&effective_prover_data.common);

        Ok(BatchStarkProof {
            proof,
            table_packing: effective_packing,
            public_binding_lanes: packing.public_binding_lanes(),
            rows: RowCounts::new([const_rows_padded, public_rows_padded, alu_rows_padded]),
            alu_variant: self.alu_variant,
            ext_degree: D,
            w_binomial: if D > 1 { w_binomial } else { None },
            alu_quintic_trinomial: alu_quintic,
            non_primitives,
            stark_common,
        })
    }

    /// Verify a batch STARK proof for a specific extension field degree.
    ///
    /// This reconstructs the AIRs from the proof metadata and verifies the proof
    /// against all circuit tables. The AIRs are reconstructed using the same
    /// configuration that was used during proof generation.
    fn verify<const D: usize>(
        &self,
        proof: &BatchStarkProof<SC>,
        w_binomial: Option<Val<SC>>,
        common: &CommonData<SC>,
        public_values: &[Val<SC>],
    ) -> Result<(), BatchStarkProverError> {
        let (airs, pvs, effective_common) =
            self.rebuild_verifier_statement::<D>(proof, w_binomial, common, public_values)?;

        p3_batch_stark::verify_batch(&self.config, &airs, &proof.proof, &pvs, &effective_common)
            .map_err(|e| BatchStarkProverError::Verify(format!("{e:?}")))
    }

    fn verify_compact<const D: usize>(
        &self,
        proof: PreprocessedOodCompactBatchStarkProof<SC>,
        w_binomial: Option<Val<SC>>,
        public_values: &[Val<SC>],
        canonical_setup: &CircuitProverData<SC>,
    ) -> Result<(), BatchStarkProverError>
    where
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: PartialEq,
    {
        let mut proof = proof.into_inner();
        let (airs, pvs, effective_common) = self.rebuild_verifier_statement::<D>(
            &proof,
            w_binomial,
            &proof.stark_common,
            public_values,
        )?;
        self.restore_preprocessed_ood_openings::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            canonical_setup,
        )?;

        p3_batch_stark::verify_batch(&self.config, &airs, &proof.proof, &pvs, &effective_common)
            .map_err(|e| BatchStarkProverError::Verify(format!("{e:?}")))
    }

    fn rebuild_verifier_statement<const D: usize>(
        &self,
        proof: &BatchStarkProof<SC>,
        w_binomial: Option<Val<SC>>,
        common: &CommonData<SC>,
        public_values: &[Val<SC>],
    ) -> Result<
        (
            Vec<CircuitTableAir<SC, D>>,
            Vec<Vec<Val<SC>>>,
            CommonData<SC>,
        ),
        BatchStarkProverError,
    > {
        let expected_public_values = proof.public_binding_lanes * D;
        if public_values.len() != expected_public_values {
            return Err(BatchStarkProverError::Verify(format!(
                "public binding values length mismatch: expected {expected_public_values}, got {}",
                public_values.len()
            )));
        }
        let prover_index_by_type: BTreeMap<NpoTypeId, usize> = self
            .non_primitive_provers
            .iter()
            .enumerate()
            .map(|(i, p)| (p.op_type(), i))
            .collect();

        // Rebuild AIRs in the same order as prove.
        let packing = &proof.table_packing;
        let public_lanes = packing.public_lanes();
        let alu_lanes = packing.alu_lanes();
        let min_height = packing.min_trace_height();

        let const_air = CircuitTableAir::Const(
            ConstAir::<Val<SC>, D>::new(proof.rows[PrimitiveTable::Const])
                .with_min_height(min_height),
        );
        let public_air = CircuitTableAir::Public(
            PublicAir::<Val<SC>, D>::new(proof.rows[PrimitiveTable::Public], public_lanes)
                .with_public_binding_lanes(proof.public_binding_lanes)
                .with_min_height(min_height),
        );
        let horner_k = packing.horner_packed_steps();
        let alu_air: CircuitTableAir<SC, D> = if D == 1 {
            CircuitTableAir::Alu(
                AluAir::<Val<SC>, D>::new(proof.rows[PrimitiveTable::Alu], alu_lanes)
                    .with_horner_pack_k(horner_k)
                    .with_min_height(min_height),
            )
        } else if D == 5 && proof.alu_quintic_trinomial {
            CircuitTableAir::Alu(
                AluAir::<Val<SC>, D>::new_quintic_trinomial(
                    proof.rows[PrimitiveTable::Alu],
                    alu_lanes,
                )
                .with_horner_pack_k(horner_k)
                .with_min_height(min_height),
            )
        } else {
            let w = w_binomial.ok_or(BatchStarkProverError::MissingWForExtension)?;
            CircuitTableAir::Alu(
                AluAir::<Val<SC>, D>::new_binomial(proof.rows[PrimitiveTable::Alu], alu_lanes, w)
                    .with_horner_pack_k(horner_k)
                    .with_min_height(min_height),
            )
        };
        let mut airs = vec![const_air, public_air, alu_air];
        let mut pvs: Vec<Vec<Val<SC>>> =
            Vec::with_capacity(NUM_PRIMITIVE_TABLES + proof.non_primitives.len());
        pvs.resize_with(NUM_PRIMITIVE_TABLES, Vec::new);
        pvs[PrimitiveTable::Public as usize] = public_values.to_vec();

        for entry in &proof.non_primitives {
            let pi = *prover_index_by_type.get(&entry.op_type).ok_or_else(|| {
                BatchStarkProverError::Verify(format!(
                    "unknown non-primitive op: {:?}",
                    entry.op_type
                ))
            })?;
            let plugin = &self.non_primitive_provers[pi];
            let air = plugin
                .batch_air_from_table_entry_with_min_height(
                    &self.config,
                    D,
                    proof.ext_degree as u32,
                    packing.min_trace_height(),
                    entry,
                )
                .map_err(BatchStarkProverError::Verify)?;
            airs.push(CircuitTableAir::Dynamic(air));
            pvs.push(entry.public_values.clone());
        }

        // Derive lookups from the rebuilt AIRs so the layout always reflects the effective
        // lane counts stored in `proof.table_packing`. The serialized `stark_common` only
        // carries the preprocessed binding, not the lookup contexts.
        let lookups: Vec<Lookups<Val<SC>>> = airs
            .iter()
            .map(|a| lookups_for_circuit_table_air::<SC, D>(a))
            .collect();
        let effective_common = CommonData::new(
            common.preprocessed.as_ref().map(|g| GlobalPreprocessed {
                commitment: g.commitment.clone(),
                instances: g.instances.clone(),
                matrix_to_instance: g.matrix_to_instance.clone(),
            }),
            lookups,
        );

        Ok((airs, pvs, effective_common))
    }

    fn restore_preprocessed_ood_openings<const D: usize>(
        &self,
        proof: &mut BatchProof<SC>,
        airs: &[CircuitTableAir<SC, D>],
        public_values: &[Vec<Val<SC>>],
        common: &CommonData<SC>,
        canonical_setup: &CircuitProverData<SC>,
    ) -> Result<(), BatchStarkProverError>
    where
        <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: PartialEq,
    {
        let Some(global) = &common.preprocessed else {
            return Ok(());
        };
        if !common_preprocessed_binding_eq(common, &canonical_setup.prover_data.common) {
            return Err(BatchStarkProverError::Verify(String::from(
                "canonical setup preprocessed binding changed during compact verification",
            )));
        }
        let preprocessed_prover_data = canonical_setup
            .prover_data
            .prover_only
            .preprocessed_prover_data
            .as_ref()
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "canonical setup lacks preprocessed prover data",
                ))
            })?;

        let (zeta, trace_domains) =
            self.sample_batch_zeta_and_trace_domains(airs, proof, public_values, common)?;
        let pcs = self.config.pcs();

        for (matrix_index, &inst_idx) in global.matrix_to_instance.iter().enumerate() {
            let meta = global
                .instances
                .get(inst_idx)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "missing preprocessed metadata for instance {inst_idx}"
                    ))
                })?;
            if meta.matrix_index != matrix_index {
                return Err(BatchStarkProverError::Verify(format!(
                    "preprocessed matrix metadata mismatch for instance {inst_idx}"
                )));
            }
            if inst_idx >= proof.opened_values.instances.len() || inst_idx >= airs.len() {
                return Err(BatchStarkProverError::Verify(format!(
                    "preprocessed instance index {inst_idx} out of bounds"
                )));
            }

            let pre_domain = pcs.natural_domain_for_degree(1 << meta.degree_bits);
            let pre_evals = pcs.get_evaluations_on_domain_no_random(
                preprocessed_prover_data,
                matrix_index,
                pre_domain,
            );
            let local = evaluate_matrix_columns_at(&pre_domain, &pre_evals, zeta);
            let opened = &mut proof.opened_values.instances[inst_idx].base_opened_values;
            match &opened.preprocessed_local {
                Some(existing) if existing != &local => {
                    return Err(BatchStarkProverError::Verify(format!(
                        "serialized preprocessed local opening mismatch for instance {inst_idx}"
                    )));
                }
                Some(_) => {}
                None => opened.preprocessed_local = Some(local),
            }

            if !airs[inst_idx].preprocessed_next_row_columns().is_empty() {
                let zeta_next = trace_domains[inst_idx].next_point(zeta).ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "trace domain lacks next point for instance {inst_idx}"
                    ))
                })?;
                let next = evaluate_matrix_columns_at(&pre_domain, &pre_evals, zeta_next);
                match &opened.preprocessed_next {
                    Some(existing) if existing != &next => {
                        return Err(BatchStarkProverError::Verify(format!(
                            "serialized preprocessed next opening mismatch for instance {inst_idx}"
                        )));
                    }
                    Some(_) => {}
                    None => opened.preprocessed_next = Some(next),
                }
            }
        }

        Ok(())
    }

    fn sample_batch_zeta_and_trace_domains<const D: usize>(
        &self,
        airs: &[CircuitTableAir<SC, D>],
        proof: &BatchProof<SC>,
        public_values: &[Vec<Val<SC>>],
        common: &CommonData<SC>,
    ) -> Result<(SC::Challenge, Vec<Domain<SC>>), BatchStarkProverError> {
        let (mut transcript, trace_domains) =
            self.initialise_batch_transcript_to_zeta(airs, proof, public_values, common)?;
        Ok((transcript.sample_zeta(), trace_domains))
    }

    fn initialise_batch_transcript_to_zeta<const D: usize>(
        &self,
        airs: &[CircuitTableAir<SC, D>],
        proof: &BatchProof<SC>,
        public_values: &[Vec<Val<SC>>],
        common: &CommonData<SC>,
    ) -> Result<(BatchTranscript<SC>, Vec<Domain<SC>>), BatchStarkProverError> {
        let all_lookups = &common.lookups;
        if airs.len() != proof.opened_values.instances.len()
            || airs.len() != public_values.len()
            || airs.len() != proof.degree_bits.len()
            || airs.len() != proof.global_lookup_data.len()
            || airs.len() != all_lookups.len()
        {
            return Err(BatchStarkProverError::Verify(String::from(
                "compact proof instance metadata length mismatch",
            )));
        }

        let pcs = self.config.pcs();
        let mut transcript = BatchTranscript::<SC>::new(self.config.initialise_challenger());
        let lookup_gadget = LogUpGadget::new();

        let mut preprocessed_widths = Vec::with_capacity(airs.len());
        let mut base_degree_bits = Vec::with_capacity(airs.len());
        let mut trace_domains = Vec::with_capacity(airs.len());
        let mut num_quotient_chunks = Vec::with_capacity(airs.len());

        for (i, air) in airs.iter().enumerate() {
            let (base_db, ext_domain_size) = validate_degree_bits(
                Some(i),
                proof.degree_bits[i],
                self.config.is_zk(),
                pcs.log_max_lde_height(),
            )
            .map_err(|e| BatchStarkProverError::Verify(format!("{e:?}")))?;
            base_degree_bits.push(base_db);
            trace_domains
                .push(pcs.natural_domain_for_degree(ext_domain_size >> self.config.is_zk()));

            let pre_w = common
                .preprocessed
                .as_ref()
                .and_then(|g| g.instances[i].as_ref().map(|m| m.width))
                .unwrap_or(0);
            preprocessed_widths.push(pre_w);

            let layout = AirLayout {
                preprocessed_width: pre_w,
                main_width: air.width(),
                num_public_values: air.num_public_values(),
                num_periodic_columns: air.num_periodic_columns(),
                ..Default::default()
            };
            let log_chunks =
                get_batch_log_num_quotient_chunks::<Val<SC>, SC::Challenge, _, LogUpGadget>(
                    air,
                    layout,
                    &all_lookups[i],
                    self.config.is_zk(),
                    &lookup_gadget,
                );
            let (_, chunks) =
                checked_log_size_sum(log_chunks, self.config.is_zk()).ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "quotient domain too large for instance {i}: log chunks {log_chunks}"
                    ))
                })?;
            num_quotient_chunks.push(chunks);
        }

        transcript.observe_instance_count(airs.len());
        for (i, air) in airs.iter().enumerate() {
            transcript.observe_instance_binding(
                proof.degree_bits[i],
                base_degree_bits[i],
                air.width(),
                num_quotient_chunks[i],
            );
        }
        transcript.observe_main(&proof.commitments.main, public_values);
        transcript.observe_preprocessed(&preprocessed_widths, common.preprocessed.as_ref());
        let _ = transcript.sample_perm_challenges(all_lookups, &lookup_gadget);
        let _ = transcript.observe_perm_and_sample_alpha(
            proof.commitments.permutation.as_ref(),
            &proof.global_lookup_data,
        );
        transcript.observe_quotient_commitment(&proof.commitments.quotient_chunks);
        if let Some(random_commitment) = &proof.commitments.random {
            transcript.observe_random_commitment(random_commitment);
        }

        Ok((transcript, trace_domains))
    }

    fn observe_pcs_opened_values<const D: usize>(
        &self,
        transcript: &mut BatchTranscript<SC>,
        proof: &BatchProof<SC>,
        airs: &[CircuitTableAir<SC, D>],
        common: &CommonData<SC>,
        trace_domains: &[Domain<SC>],
        zeta: SC::Challenge,
        label: &str,
    ) -> Result<(), BatchStarkProverError>
    where
        SC::Challenge: Clone,
    {
        if proof.commitments.random.is_some() {
            for (i, instance) in proof.opened_values.instances.iter().enumerate() {
                let random = instance.base_opened_values.random.as_ref().ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "{label} proof missing random opening for instance {i}"
                    ))
                })?;
                transcript.challenger.observe_algebra_slice(random);
            }
        }

        for (i, (air, instance)) in airs
            .iter()
            .zip(proof.opened_values.instances.iter())
            .enumerate()
        {
            transcript
                .challenger
                .observe_algebra_slice(&instance.base_opened_values.trace_local);
            if !air.main_next_row_columns().is_empty() {
                let _ = trace_domains[i].next_point(zeta.clone()).ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "{label} trace domain lacks next point for instance {i}"
                    ))
                })?;
                let trace_next =
                    instance
                        .base_opened_values
                        .trace_next
                        .as_ref()
                        .ok_or_else(|| {
                            BatchStarkProverError::Verify(format!(
                                "{label} proof missing trace next opening for instance {i}"
                            ))
                        })?;
                transcript.challenger.observe_algebra_slice(trace_next);
            }
        }

        for instance in &proof.opened_values.instances {
            for quotient_chunk in &instance.base_opened_values.quotient_chunks {
                transcript.challenger.observe_algebra_slice(quotient_chunk);
            }
        }

        if let Some(global) = &common.preprocessed {
            for &inst_idx in &global.matrix_to_instance {
                if inst_idx >= proof.opened_values.instances.len() || inst_idx >= airs.len() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "{label} preprocessed instance index {inst_idx} out of bounds",
                    )));
                }
                let instance = &proof.opened_values.instances[inst_idx];
                let local = instance
                    .base_opened_values
                    .preprocessed_local
                    .as_ref()
                    .ok_or_else(|| {
                        BatchStarkProverError::Verify(format!(
                            "{label} proof missing preprocessed local opening for instance {inst_idx}"
                        ))
                    })?;
                transcript.challenger.observe_algebra_slice(local);
                if !airs[inst_idx].preprocessed_next_row_columns().is_empty() {
                    let _ = trace_domains[inst_idx]
                        .next_point(zeta.clone())
                        .ok_or_else(|| {
                            BatchStarkProverError::Verify(format!(
                                "{label} trace domain lacks preprocessed next point for instance {inst_idx}"
                            ))
                        })?;
                    let next = instance
                        .base_opened_values
                        .preprocessed_next
                        .as_ref()
                        .ok_or_else(|| {
                            BatchStarkProverError::Verify(format!(
                                "{label} proof missing preprocessed next opening for instance {inst_idx}"
                            ))
                        })?;
                    transcript.challenger.observe_algebra_slice(next);
                }
            }
        }

        if proof.commitments.permutation.is_some() {
            for (i, instance) in proof.opened_values.instances.iter().enumerate() {
                if instance.permutation_local.len() != instance.permutation_next.len() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "{label} permutation opening length mismatch for instance {i}",
                    )));
                }
                if !instance.permutation_local.is_empty() {
                    let _ = trace_domains[i].next_point(zeta.clone()).ok_or_else(|| {
                        BatchStarkProverError::Verify(format!(
                            "{label} permutation domain lacks next point for instance {i}"
                        ))
                    })?;
                    transcript
                        .challenger
                        .observe_algebra_slice(&instance.permutation_local);
                    transcript
                        .challenger
                        .observe_algebra_slice(&instance.permutation_next);
                }
            }
        }

        Ok(())
    }
}

impl BatchStarkProver<GoldilocksTipsConfig> {
    /// Build a path-pruned Goldilocks/Tip5 compact proof from a full proof.
    ///
    /// The constructor replays the verifier transcript to derive FRI query
    /// indices, prunes only Merkle authentication paths, and then omits
    /// verifier-deterministic preprocessed openings. It never serializes
    /// prover-chosen query indices.
    pub fn compact_goldilocks_tip5_path_pruned_preprocessed(
        &self,
        proof: BatchStarkProof<GoldilocksTipsConfig>,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<GoldilocksTip5PathPrunedCompactBatchStarkProof, BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.compact_goldilocks_tip5_path_pruned_preprocessed_with_public_values(
            proof,
            &[],
            canonical_setup,
            fri_shape,
        )
    }

    /// Build a path-pruned Goldilocks/Tip5 compact proof while binding leading
    /// Public-table lanes to caller-supplied STARK public values.
    pub fn compact_goldilocks_tip5_path_pruned_preprocessed_with_public_values(
        &self,
        proof: BatchStarkProof<GoldilocksTipsConfig>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<GoldilocksTip5PathPrunedCompactBatchStarkProof, BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        proof.validate()?;
        fri_shape.validate()?;
        if !common_preprocessed_binding_eq(&proof.stark_common, &canonical_setup.prover_data.common)
        {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 path-pruned compact proof does not match canonical setup binding",
            )));
        }

        let ext_degree = proof.ext_degree;
        let w_binomial = proof.w_binomial;
        match ext_degree {
            1 => self.compact_goldilocks_tip5_path_pruned_preprocessed_inner::<1>(
                proof,
                None,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            2 => self.compact_goldilocks_tip5_path_pruned_preprocessed_inner::<2>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            4 => self.compact_goldilocks_tip5_path_pruned_preprocessed_inner::<4>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            5 => self.compact_goldilocks_tip5_path_pruned_preprocessed_inner::<5>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            6 => self.compact_goldilocks_tip5_path_pruned_preprocessed_inner::<6>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            8 => self.compact_goldilocks_tip5_path_pruned_preprocessed_inner::<8>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    /// Build the canonical-metadata compact body form of a path-pruned
    /// Goldilocks/Tip5 proof.
    ///
    /// The returned body omits all [`BatchStarkProof`] metadata. Verifiers must
    /// supply a trusted [`GoldilocksTip5BatchStarkProofMetadata`] template when
    /// checking it.
    pub fn compact_goldilocks_tip5_path_pruned_preprocessed_body(
        &self,
        proof: BatchStarkProof<GoldilocksTipsConfig>,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<GoldilocksTip5PathPrunedCompactBatchStarkProofBody, BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.compact_goldilocks_tip5_path_pruned_preprocessed_body_with_public_values(
            proof,
            &[],
            canonical_setup,
            fri_shape,
        )
    }

    /// Build the canonical-metadata compact body form while binding leading
    /// Public-table lanes to caller-supplied STARK public values.
    pub fn compact_goldilocks_tip5_path_pruned_preprocessed_body_with_public_values(
        &self,
        proof: BatchStarkProof<GoldilocksTipsConfig>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<GoldilocksTip5PathPrunedCompactBatchStarkProofBody, BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.compact_goldilocks_tip5_path_pruned_preprocessed_with_public_values(
            proof,
            public_values,
            canonical_setup,
            fri_shape,
        )
        .map(GoldilocksTip5PathPrunedCompactBatchStarkProof::into_body)
    }

    /// Verify a path-pruned Goldilocks/Tip5 compact proof.
    pub fn verify_goldilocks_tip5_path_pruned_preprocessed_compact(
        &self,
        proof: GoldilocksTip5PathPrunedCompactBatchStarkProof,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_with_public_values(
            proof,
            &[],
            canonical_setup,
        )
    }

    /// Verify a path-pruned Goldilocks/Tip5 compact proof while binding leading
    /// Public-table lanes to caller-supplied STARK public values.
    pub fn verify_goldilocks_tip5_path_pruned_preprocessed_compact_with_public_values(
        &self,
        proof: GoldilocksTip5PathPrunedCompactBatchStarkProof,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        proof.proof.validate()?;
        proof.fri_shape.validate()?;
        if !common_preprocessed_binding_eq(
            &proof.proof.stark_common,
            &canonical_setup.prover_data.common,
        ) {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 path-pruned compact proof does not match canonical setup binding",
            )));
        }

        let ext_degree = proof.proof.ext_degree;
        let w_binomial = proof.proof.w_binomial;
        match ext_degree {
            1 => self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_inner::<1>(
                proof,
                None,
                public_values,
                canonical_setup,
            ),
            2 => self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_inner::<2>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            4 => self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_inner::<4>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            5 => self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_inner::<5>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            6 => self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_inner::<6>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            8 => self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_inner::<8>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    /// Verify a canonical-metadata compact body.
    ///
    /// `metadata` must be rebuilt or pinned by the verifier for the exact
    /// statement being checked. It is not safe to deserialize this metadata from
    /// the prover and pass it through as trusted context.
    pub fn verify_goldilocks_tip5_path_pruned_preprocessed_compact_body(
        &self,
        proof: GoldilocksTip5PathPrunedCompactBatchStarkProofBody,
        metadata: &GoldilocksTip5BatchStarkProofMetadata,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_public_values(
            proof,
            &[],
            metadata,
            canonical_setup,
        )
    }

    /// Verify a canonical-metadata compact body while binding leading
    /// Public-table lanes to caller-supplied STARK public values.
    pub fn verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_public_values(
        &self,
        proof: GoldilocksTip5PathPrunedCompactBatchStarkProofBody,
        public_values: &[Goldilocks],
        metadata: &GoldilocksTip5BatchStarkProofMetadata,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_with_public_values(
            proof.into_wrapped(metadata),
            public_values,
            canonical_setup,
        )
    }

    /// Verify a canonical-metadata compact body against a single verifier-owned
    /// context.
    ///
    /// This is the production-oriented entrypoint for compact bodies: it checks
    /// the trusted metadata/setup binding, expected FRI shape, and explicit
    /// public-value binding before restoring omitted openings and delegating to
    /// the normal upstream verifier.
    pub fn verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_context(
        &self,
        proof: GoldilocksTip5PathPrunedCompactBatchStarkProofBody,
        context: GoldilocksTip5PathPrunedCompactVerifierContext<'_>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        context.validate()?;
        if proof.fri_shape != context.fri_shape {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 compact body FRI shape mismatch: expected {:?}, got {:?}",
                context.fri_shape, proof.fri_shape
            )));
        }
        self.verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_public_values(
            proof,
            context.public_values,
            context.metadata,
            context.canonical_setup,
        )
    }

    /// Verify a Goldilocks/Tip5 compact proof whose verifier-deterministic
    /// preprocessed OOD values and FRI input-batch openings were omitted.
    pub fn verify_goldilocks_tip5_preprocessed_compact(
        &self,
        proof: GoldilocksTip5PreprocessedCompactBatchStarkProof,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.verify_goldilocks_tip5_preprocessed_compact_with_public_values(
            proof,
            &[],
            canonical_setup,
        )
    }

    /// Verify a Goldilocks/Tip5 compact proof while binding leading
    /// Public-table lanes to caller-supplied STARK public values.
    pub fn verify_goldilocks_tip5_preprocessed_compact_with_public_values(
        &self,
        proof: GoldilocksTip5PreprocessedCompactBatchStarkProof,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTipsConfig as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksTip5Challenge,
                <GoldilocksTipsConfig as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        proof.proof.validate()?;
        proof.fri_shape.validate()?;
        if !common_preprocessed_binding_eq(
            &proof.proof.stark_common,
            &canonical_setup.prover_data.common,
        ) {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 compact proof does not match canonical setup binding",
            )));
        }

        let ext_degree = proof.proof.ext_degree;
        let w_binomial = proof.proof.w_binomial;
        match ext_degree {
            1 => self.verify_goldilocks_tip5_preprocessed_compact_inner::<1>(
                proof,
                None,
                public_values,
                canonical_setup,
            ),
            2 => self.verify_goldilocks_tip5_preprocessed_compact_inner::<2>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            4 => self.verify_goldilocks_tip5_preprocessed_compact_inner::<4>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            5 => self.verify_goldilocks_tip5_preprocessed_compact_inner::<5>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            6 => self.verify_goldilocks_tip5_preprocessed_compact_inner::<6>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            8 => self.verify_goldilocks_tip5_preprocessed_compact_inner::<8>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
            ),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    fn compact_goldilocks_tip5_path_pruned_preprocessed_inner<const D: usize>(
        &self,
        mut proof: BatchStarkProof<GoldilocksTipsConfig>,
        w_binomial: Option<Goldilocks>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<GoldilocksTip5PathPrunedCompactBatchStarkProof, BatchStarkProverError>
    where
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        let (airs, pvs, effective_common) = self.rebuild_verifier_statement::<D>(
            &proof,
            w_binomial,
            &proof.stark_common,
            public_values,
        )?;
        let (query_indices, log_global_max_height) = self
            .derive_goldilocks_tip5_fri_query_indices::<D>(
                &proof.proof,
                &airs,
                &pvs,
                &effective_common,
                fri_shape,
            )?;

        let (input_batch_paths, commit_phase_paths) = self.prune_goldilocks_tip5_fri_merkle_paths(
            &mut proof.proof,
            &effective_common,
            &query_indices,
            log_global_max_height,
            fri_shape,
        )?;
        omit_preprocessed_ood_openings(&mut proof.proof);
        omit_goldilocks_tip5_preprocessed_fri_input_batches(&mut proof.proof, &effective_common)?;

        if !common_preprocessed_binding_eq(&effective_common, &canonical_setup.prover_data.common) {
            return Err(BatchStarkProverError::Verify(String::from(
                "canonical setup preprocessed binding changed during Goldilocks/Tip5 path-pruned compaction",
            )));
        }

        Ok(GoldilocksTip5PathPrunedCompactBatchStarkProof {
            proof,
            fri_shape,
            input_batch_paths,
            commit_phase_paths,
        })
    }

    fn verify_goldilocks_tip5_path_pruned_preprocessed_compact_inner<const D: usize>(
        &self,
        proof: GoldilocksTip5PathPrunedCompactBatchStarkProof,
        w_binomial: Option<Goldilocks>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        let GoldilocksTip5PathPrunedCompactBatchStarkProof {
            mut proof,
            fri_shape,
            input_batch_paths,
            commit_phase_paths,
        } = proof;
        let (airs, pvs, effective_common) = self.rebuild_verifier_statement::<D>(
            &proof,
            w_binomial,
            &proof.stark_common,
            public_values,
        )?;
        self.restore_goldilocks_tip5_preprocessed_ood_openings::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            canonical_setup,
            fri_shape,
        )?;
        self.restore_goldilocks_tip5_path_pruned_fri_openings::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            fri_shape,
            &input_batch_paths,
            &commit_phase_paths,
        )?;
        self.restore_goldilocks_tip5_preprocessed_fri_input_batches::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            canonical_setup,
            fri_shape,
        )?;

        p3_batch_stark::verify_batch(&self.config, &airs, &proof.proof, &pvs, &effective_common)
            .map_err(|e| BatchStarkProverError::Verify(format!("{e:?}")))
    }

    fn verify_goldilocks_tip5_preprocessed_compact_inner<const D: usize>(
        &self,
        proof: GoldilocksTip5PreprocessedCompactBatchStarkProof,
        w_binomial: Option<Goldilocks>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        let fri_shape = proof.fri_shape;
        let mut proof = proof.into_inner();
        let (airs, pvs, effective_common) = self.rebuild_verifier_statement::<D>(
            &proof,
            w_binomial,
            &proof.stark_common,
            public_values,
        )?;
        self.restore_goldilocks_tip5_preprocessed_ood_openings::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            canonical_setup,
            fri_shape,
        )?;
        self.restore_goldilocks_tip5_preprocessed_fri_input_batches::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            canonical_setup,
            fri_shape,
        )?;

        p3_batch_stark::verify_batch(&self.config, &airs, &proof.proof, &pvs, &effective_common)
            .map_err(|e| BatchStarkProverError::Verify(format!("{e:?}")))
    }

    fn restore_goldilocks_tip5_preprocessed_ood_openings<const D: usize>(
        &self,
        proof: &mut BatchProof<GoldilocksTipsConfig>,
        airs: &[CircuitTableAir<GoldilocksTipsConfig, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksTipsConfig>,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        let Some(global) = &common.preprocessed else {
            return Ok(());
        };
        if !common_preprocessed_binding_eq(common, &canonical_setup.prover_data.common) {
            return Err(BatchStarkProverError::Verify(String::from(
                "canonical setup preprocessed binding changed during Goldilocks/Tip5 OOD restoration",
            )));
        }
        let preprocessed_prover_data = canonical_setup
            .prover_data
            .prover_only
            .preprocessed_prover_data
            .as_ref()
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "canonical setup lacks preprocessed prover data",
                ))
            })?;

        let (zeta, trace_domains) =
            self.sample_batch_zeta_and_trace_domains(airs, proof, public_values, common)?;
        let val_mmcs = goldilocks_tip5_val_mmcs(fri_shape.cap_height);
        let preprocessed_matrices = val_mmcs.get_matrices(preprocessed_prover_data);

        for (matrix_index, &inst_idx) in global.matrix_to_instance.iter().enumerate() {
            let meta = global
                .instances
                .get(inst_idx)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "missing preprocessed metadata for instance {inst_idx}"
                    ))
                })?;
            if meta.matrix_index != matrix_index {
                return Err(BatchStarkProverError::Verify(format!(
                    "preprocessed matrix metadata mismatch for instance {inst_idx}"
                )));
            }
            if inst_idx >= proof.opened_values.instances.len() || inst_idx >= airs.len() {
                return Err(BatchStarkProverError::Verify(format!(
                    "preprocessed instance index {inst_idx} out of bounds"
                )));
            }
            let preprocessed_matrix = preprocessed_matrices.get(matrix_index).ok_or_else(|| {
                BatchStarkProverError::Verify(format!(
                    "missing canonical preprocessed matrix {matrix_index}"
                ))
            })?;

            let local = evaluate_goldilocks_bit_reversed_lde_at(
                *preprocessed_matrix,
                fri_shape.log_blowup,
                zeta,
            )?;
            let opened = &mut proof.opened_values.instances[inst_idx].base_opened_values;
            match &opened.preprocessed_local {
                Some(existing) if existing != &local => {
                    return Err(BatchStarkProverError::Verify(format!(
                        "serialized Goldilocks/Tip5 preprocessed local opening mismatch for instance {inst_idx}"
                    )));
                }
                Some(_) => {}
                None => opened.preprocessed_local = Some(local),
            }

            if !airs[inst_idx].preprocessed_next_row_columns().is_empty() {
                let zeta_next = trace_domains[inst_idx].next_point(zeta).ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "trace domain lacks next point for instance {inst_idx}"
                    ))
                })?;
                let next = evaluate_goldilocks_bit_reversed_lde_at(
                    *preprocessed_matrix,
                    fri_shape.log_blowup,
                    zeta_next,
                )?;
                match &opened.preprocessed_next {
                    Some(existing) if existing != &next => {
                        return Err(BatchStarkProverError::Verify(format!(
                            "serialized Goldilocks/Tip5 preprocessed next opening mismatch for instance {inst_idx}"
                        )));
                    }
                    Some(_) => {}
                    None => opened.preprocessed_next = Some(next),
                }
            }
        }

        Ok(())
    }

    fn restore_goldilocks_tip5_preprocessed_fri_input_batches<const D: usize>(
        &self,
        proof: &mut BatchProof<GoldilocksTipsConfig>,
        airs: &[CircuitTableAir<GoldilocksTipsConfig, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksTipsConfig>,
        canonical_setup: &CircuitProverData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksTip5ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        if common.preprocessed.is_none() {
            return Ok(());
        }
        if !common_preprocessed_binding_eq(common, &canonical_setup.prover_data.common) {
            return Err(BatchStarkProverError::Verify(String::from(
                "canonical setup preprocessed binding changed during Goldilocks/Tip5 compact verification",
            )));
        }

        let preprocessed_prover_data = canonical_setup
            .prover_data
            .prover_only
            .preprocessed_prover_data
            .as_ref()
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "canonical setup lacks preprocessed prover data",
                ))
            })?;

        let expected_full_batches = goldilocks_tip5_full_fri_input_batch_count(proof, common);
        let expected_compact_batches = expected_full_batches.checked_sub(1).ok_or_else(|| {
            BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 compact proof has no input batch slot to omit",
            ))
        })?;
        let preprocessed_idx = goldilocks_tip5_preprocessed_trace_idx();
        if preprocessed_idx >= expected_full_batches {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 preprocessed batch index {preprocessed_idx} exceeds expected input batches {expected_full_batches}",
            )));
        }

        let (query_indices, log_global_max_height) = self
            .derive_goldilocks_tip5_fri_query_indices::<D>(
                proof,
                airs,
                public_values,
                common,
                fri_shape,
            )?;

        let val_mmcs = goldilocks_tip5_val_mmcs(fri_shape.cap_height);
        let preprocessed_max_height = val_mmcs.get_max_height(preprocessed_prover_data);
        if preprocessed_max_height == 0 || !preprocessed_max_height.is_power_of_two() {
            return Err(BatchStarkProverError::Verify(format!(
                "canonical preprocessed prover data has bad max height {preprocessed_max_height}",
            )));
        }
        let preprocessed_log_max_height = log2_strict_usize(preprocessed_max_height);
        if preprocessed_log_max_height > log_global_max_height {
            return Err(BatchStarkProverError::Verify(format!(
                "preprocessed batch height log {preprocessed_log_max_height} exceeds FRI global height log {log_global_max_height}",
            )));
        }

        for (query, (query_index, query_proof)) in query_indices
            .into_iter()
            .zip(proof.opening_proof.query_proofs.iter_mut())
            .enumerate()
        {
            if query_proof.input_proof.len() != expected_compact_batches {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 compact query {query} input batch count mismatch: expected {expected_compact_batches}, got {}",
                    query_proof.input_proof.len()
                )));
            }
            let reduced_index =
                query_index >> (log_global_max_height - preprocessed_log_max_height);
            let opening = val_mmcs.open_batch(reduced_index, preprocessed_prover_data);
            query_proof.input_proof.insert(preprocessed_idx, opening);
        }

        Ok(())
    }

    fn prune_goldilocks_tip5_fri_merkle_paths(
        &self,
        proof: &mut BatchProof<GoldilocksTipsConfig>,
        common: &CommonData<GoldilocksTipsConfig>,
        query_indices: &[usize],
        log_global_max_height: usize,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<
        (
            Vec<GoldilocksTip5PrunedMerklePaths>,
            Vec<GoldilocksTip5PrunedMerklePaths>,
        ),
        BatchStarkProverError,
    > {
        let query_count = proof.opening_proof.query_proofs.len();
        if query_indices.len() != query_count {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 path-pruned query index count mismatch: expected {query_count}, got {}",
                query_indices.len()
            )));
        }

        let expected_full_batches = goldilocks_tip5_full_fri_input_batch_count(proof, common);
        let preprocessed_idx = common
            .preprocessed
            .as_ref()
            .map(|_| goldilocks_tip5_preprocessed_trace_idx());
        if let Some(idx) = preprocessed_idx {
            if idx >= expected_full_batches {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 preprocessed batch index {idx} exceeds expected input batches {expected_full_batches}",
                )));
            }
        }

        for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
            if query_proof.input_proof.len() != expected_full_batches {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 full query {query} input batch count mismatch: expected {expected_full_batches}, got {}",
                    query_proof.input_proof.len()
                )));
            }
        }

        let kept_input_batches = (0..expected_full_batches)
            .filter(|&batch| Some(batch) != preprocessed_idx)
            .collect::<Vec<_>>();
        let mut input_batch_paths = Vec::with_capacity(kept_input_batches.len());
        for &batch in &kept_input_batches {
            let full_paths = proof
                .opening_proof
                .query_proofs
                .iter()
                .map(|query_proof| query_proof.input_proof[batch].opening_proof.clone())
                .collect::<Vec<_>>();
            let full_path_len = full_paths.first().map_or(0, Vec::len);
            let leaf_indices = goldilocks_tip5_input_batch_leaf_indices(
                query_indices,
                log_global_max_height,
                full_path_len,
                fri_shape.cap_height,
            )?;
            input_batch_paths.push(prune_goldilocks_tip5_merkle_paths(
                &leaf_indices,
                &full_paths,
            )?);
        }

        for query_proof in &mut proof.opening_proof.query_proofs {
            for &batch in &kept_input_batches {
                query_proof.input_proof[batch].opening_proof.clear();
            }
        }

        let expected_rounds = proof.opening_proof.commit_phase_commits.len();
        let mut domain_indices = query_indices.to_vec();
        let mut commit_phase_paths = Vec::with_capacity(expected_rounds);
        for round in 0..expected_rounds {
            let log_arity = proof
                .opening_proof
                .query_proofs
                .first()
                .and_then(|query| query.commit_phase_openings.get(round))
                .map(|opening| opening.log_arity as usize)
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 path-pruned proof missing commit phase round {round}",
                    ))
                })?;
            if !(1..=fri_shape.max_log_arity).contains(&log_arity) {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 FRI invalid log arity {log_arity} in round {round}; max {}",
                    fri_shape.max_log_arity
                )));
            }

            let mut full_paths = Vec::with_capacity(query_count);
            for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
                if query_proof.commit_phase_openings.len() != expected_rounds {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 FRI query {query} commit phase count mismatch: expected {expected_rounds}, got {}",
                        query_proof.commit_phase_openings.len()
                    )));
                }
                let opening = &query_proof.commit_phase_openings[round];
                if opening.log_arity as usize != log_arity {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 FRI query {query} log arity mismatch in round {round}",
                    )));
                }
                full_paths.push(opening.opening_proof.clone());
            }

            let leaf_indices = domain_indices
                .iter()
                .map(|index| index >> log_arity)
                .collect::<Vec<_>>();
            commit_phase_paths.push(prune_goldilocks_tip5_merkle_paths(
                &leaf_indices,
                &full_paths,
            )?);
            for index in &mut domain_indices {
                *index >>= log_arity;
            }
        }

        for query_proof in &mut proof.opening_proof.query_proofs {
            for opening in &mut query_proof.commit_phase_openings {
                opening.opening_proof.clear();
            }
        }

        Ok((input_batch_paths, commit_phase_paths))
    }

    fn restore_goldilocks_tip5_path_pruned_fri_openings<const D: usize>(
        &self,
        proof: &mut BatchProof<GoldilocksTipsConfig>,
        airs: &[CircuitTableAir<GoldilocksTipsConfig, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
        input_batch_paths: &[GoldilocksTip5PrunedMerklePaths],
        commit_phase_paths: &[GoldilocksTip5PrunedMerklePaths],
    ) -> Result<(), BatchStarkProverError> {
        let (query_indices, log_global_max_height) = self
            .derive_goldilocks_tip5_fri_query_indices::<D>(
                proof,
                airs,
                public_values,
                common,
                fri_shape,
            )?;
        let query_count = proof.opening_proof.query_proofs.len();
        if query_indices.len() != query_count {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 path-pruned query index count mismatch: expected {query_count}, got {}",
                query_indices.len()
            )));
        }

        let expected_full_batches = goldilocks_tip5_full_fri_input_batch_count(proof, common);
        let expected_compact_batches = expected_full_batches
            .checked_sub(usize::from(common.preprocessed.is_some()))
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "Goldilocks/Tip5 path-pruned input batch count underflow",
                ))
            })?;
        if input_batch_paths.len() != expected_compact_batches {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 path-pruned input batch dictionary count mismatch: expected {expected_compact_batches}, got {}",
                input_batch_paths.len()
            )));
        }
        for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
            if query_proof.input_proof.len() != expected_compact_batches {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 path-pruned query {query} input batch count mismatch: expected {expected_compact_batches}, got {}",
                    query_proof.input_proof.len()
                )));
            }
            for (batch, opening) in query_proof.input_proof.iter().enumerate() {
                if !opening.opening_proof.is_empty() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 path-pruned query {query} input batch {batch} carried an unpruned opening proof",
                    )));
                }
            }
        }

        for (batch, pruned) in input_batch_paths.iter().enumerate() {
            let label = format!("input batch {batch}");
            let expected_leaf_indices = goldilocks_tip5_input_batch_leaf_indices(
                &query_indices,
                log_global_max_height,
                pruned.full_path_len,
                fri_shape.cap_height,
            )?;
            let restored = restore_goldilocks_tip5_merkle_paths(
                pruned,
                query_count,
                &expected_leaf_indices,
                log_global_max_height,
                &label,
            )?;
            for (query, path) in restored.into_iter().enumerate() {
                proof.opening_proof.query_proofs[query].input_proof[batch].opening_proof = path;
            }
        }

        let expected_rounds = proof.opening_proof.commit_phase_commits.len();
        if commit_phase_paths.len() != expected_rounds {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 path-pruned commit phase dictionary count mismatch: expected {expected_rounds}, got {}",
                commit_phase_paths.len()
            )));
        }
        for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
            if query_proof.commit_phase_openings.len() != expected_rounds {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 path-pruned query {query} commit phase count mismatch: expected {expected_rounds}, got {}",
                    query_proof.commit_phase_openings.len()
                )));
            }
            for (round, opening) in query_proof.commit_phase_openings.iter().enumerate() {
                if !opening.opening_proof.is_empty() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 path-pruned query {query} commit phase {round} carried an unpruned opening proof",
                    )));
                }
            }
        }

        let mut domain_indices = query_indices;
        for (round, pruned) in commit_phase_paths.iter().enumerate() {
            let label = format!("commit phase {round}");
            let log_arity = proof
                .opening_proof
                .query_proofs
                .first()
                .and_then(|query| query.commit_phase_openings.get(round))
                .map(|opening| opening.log_arity as usize)
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 path-pruned proof missing commit phase round {round}",
                    ))
                })?;
            let expected_leaf_indices = domain_indices
                .iter()
                .map(|index| index >> log_arity)
                .collect::<Vec<_>>();
            let restored = restore_goldilocks_tip5_merkle_paths(
                pruned,
                query_count,
                &expected_leaf_indices,
                log_global_max_height,
                &label,
            )?;
            for (query, path) in restored.into_iter().enumerate() {
                proof.opening_proof.query_proofs[query].commit_phase_openings[round]
                    .opening_proof = path;
            }
            for index in &mut domain_indices {
                *index >>= log_arity;
            }
        }

        Ok(())
    }

    fn derive_goldilocks_tip5_fri_query_indices<const D: usize>(
        &self,
        proof: &BatchProof<GoldilocksTipsConfig>,
        airs: &[CircuitTableAir<GoldilocksTipsConfig, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksTipsConfig>,
        fri_shape: GoldilocksTip5FriShape,
    ) -> Result<(Vec<usize>, usize), BatchStarkProverError> {
        let (mut transcript, trace_domains) =
            self.initialise_batch_transcript_to_zeta(airs, proof, public_values, common)?;
        let zeta = transcript.sample_zeta();
        self.observe_goldilocks_tip5_pcs_opened_values(
            &mut transcript,
            proof,
            airs,
            common,
            &trace_domains,
            zeta,
        )?;

        let fri_proof = &proof.opening_proof;
        let expected_rounds = fri_proof.commit_phase_commits.len();
        let log_arities = if let Some(first_query) = fri_proof.query_proofs.first() {
            first_query
                .commit_phase_openings
                .iter()
                .enumerate()
                .map(|(round, opening)| {
                    let log_arity = opening.log_arity as usize;
                    if !(1..=fri_shape.max_log_arity).contains(&log_arity) {
                        return Err(BatchStarkProverError::Verify(format!(
                            "Goldilocks/Tip5 FRI invalid log arity {log_arity} in round {round}; max {}",
                            fri_shape.max_log_arity
                        )));
                    }
                    Ok(log_arity)
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        for (query, query_proof) in fri_proof.query_proofs.iter().enumerate() {
            if query_proof.commit_phase_openings.len() != expected_rounds {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 FRI query {query} commit phase count mismatch: expected {expected_rounds}, got {}",
                    query_proof.commit_phase_openings.len()
                )));
            }
            let got_log_arities = query_proof
                .commit_phase_openings
                .iter()
                .map(|opening| opening.log_arity as usize)
                .collect::<Vec<_>>();
            if got_log_arities != log_arities {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/Tip5 FRI query {query} log arities mismatch",
                )));
            }
        }

        let total_log_reduction = log_arities.iter().copied().sum::<usize>();
        let log_global_max_height = total_log_reduction
            .checked_add(fri_shape.log_blowup)
            .and_then(|v| v.checked_add(fri_shape.log_final_poly_len))
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "Goldilocks/Tip5 FRI global height overflow",
                ))
            })?;
        let _ = checked_power_of_two(log_global_max_height, "FRI global max height")?;

        if fri_proof.commit_pow_witnesses.len() != fri_proof.commit_phase_commits.len() {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 FRI commit PoW witness count mismatch: expected {}, got {}",
                fri_proof.commit_phase_commits.len(),
                fri_proof.commit_pow_witnesses.len()
            )));
        }

        let _: GoldilocksTip5Challenge = transcript.challenger.sample_algebra_element();
        for (commitment, witness) in fri_proof
            .commit_phase_commits
            .iter()
            .zip(&fri_proof.commit_pow_witnesses)
        {
            transcript.challenger.observe(commitment.clone());
            if !transcript
                .challenger
                .check_witness(fri_shape.commit_pow_bits, *witness)
            {
                return Err(BatchStarkProverError::Verify(String::from(
                    "Goldilocks/Tip5 FRI commit PoW witness rejected",
                )));
            }
            let _: GoldilocksTip5Challenge = transcript.challenger.sample_algebra_element();
        }

        let final_poly_len = fri_shape.final_poly_len()?;
        if fri_proof.final_poly.len() != final_poly_len {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 FRI final polynomial length mismatch: expected {final_poly_len}, got {}",
                fri_proof.final_poly.len()
            )));
        }
        transcript
            .challenger
            .observe_algebra_slice(&fri_proof.final_poly);

        if fri_proof.query_proofs.len() != fri_shape.num_queries {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/Tip5 FRI query count mismatch: expected {}, got {}",
                fri_shape.num_queries,
                fri_proof.query_proofs.len()
            )));
        }
        for &log_arity in &log_arities {
            transcript
                .challenger
                .observe(Goldilocks::from_usize(log_arity));
        }
        if !transcript
            .challenger
            .check_witness(fri_shape.query_pow_bits, fri_proof.query_pow_witness)
        {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/Tip5 FRI query PoW witness rejected",
            )));
        }

        Ok((
            (0..fri_proof.query_proofs.len())
                .map(|_| transcript.challenger.sample_bits(log_global_max_height))
                .collect(),
            log_global_max_height,
        ))
    }

    fn observe_goldilocks_tip5_pcs_opened_values<const D: usize>(
        &self,
        transcript: &mut BatchTranscript<GoldilocksTipsConfig>,
        proof: &BatchProof<GoldilocksTipsConfig>,
        airs: &[CircuitTableAir<GoldilocksTipsConfig, D>],
        common: &CommonData<GoldilocksTipsConfig>,
        trace_domains: &[Domain<GoldilocksTipsConfig>],
        zeta: GoldilocksTip5Challenge,
    ) -> Result<(), BatchStarkProverError> {
        if proof.commitments.random.is_some() {
            for (i, instance) in proof.opened_values.instances.iter().enumerate() {
                let random = instance.base_opened_values.random.as_ref().ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 proof missing random opening for instance {i}"
                    ))
                })?;
                transcript.challenger.observe_algebra_slice(random);
            }
        }

        for (i, (air, instance)) in airs
            .iter()
            .zip(proof.opened_values.instances.iter())
            .enumerate()
        {
            transcript
                .challenger
                .observe_algebra_slice(&instance.base_opened_values.trace_local);
            if !air.main_next_row_columns().is_empty() {
                let _ = trace_domains[i].next_point(zeta).ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 trace domain lacks next point for instance {i}"
                    ))
                })?;
                let trace_next =
                    instance
                        .base_opened_values
                        .trace_next
                        .as_ref()
                        .ok_or_else(|| {
                            BatchStarkProverError::Verify(format!(
                                "Goldilocks/Tip5 proof missing trace next opening for instance {i}"
                            ))
                        })?;
                transcript.challenger.observe_algebra_slice(trace_next);
            }
        }

        for instance in &proof.opened_values.instances {
            for quotient_chunk in &instance.base_opened_values.quotient_chunks {
                transcript.challenger.observe_algebra_slice(quotient_chunk);
            }
        }

        if let Some(global) = &common.preprocessed {
            for &inst_idx in &global.matrix_to_instance {
                if inst_idx >= proof.opened_values.instances.len() || inst_idx >= airs.len() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 preprocessed instance index {inst_idx} out of bounds",
                    )));
                }
                let instance = &proof.opened_values.instances[inst_idx];
                let local = instance
                    .base_opened_values
                    .preprocessed_local
                    .as_ref()
                    .ok_or_else(|| {
                        BatchStarkProverError::Verify(format!(
                            "Goldilocks/Tip5 proof missing preprocessed local opening for instance {inst_idx}"
                        ))
                    })?;
                transcript.challenger.observe_algebra_slice(local);
                if !airs[inst_idx].preprocessed_next_row_columns().is_empty() {
                    let _ = trace_domains[inst_idx].next_point(zeta).ok_or_else(|| {
                        BatchStarkProverError::Verify(format!(
                            "Goldilocks/Tip5 trace domain lacks preprocessed next point for instance {inst_idx}"
                        ))
                    })?;
                    let next = instance
                        .base_opened_values
                        .preprocessed_next
                        .as_ref()
                        .ok_or_else(|| {
                            BatchStarkProverError::Verify(format!(
                                "Goldilocks/Tip5 proof missing preprocessed next opening for instance {inst_idx}"
                            ))
                        })?;
                    transcript.challenger.observe_algebra_slice(next);
                }
            }
        }

        if proof.commitments.permutation.is_some() {
            for (i, instance) in proof.opened_values.instances.iter().enumerate() {
                if instance.permutation_local.len() != instance.permutation_next.len() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/Tip5 permutation opening length mismatch for instance {i}",
                    )));
                }
                if !instance.permutation_local.is_empty() {
                    let _ = trace_domains[i].next_point(zeta).ok_or_else(|| {
                        BatchStarkProverError::Verify(format!(
                            "Goldilocks/Tip5 permutation domain lacks next point for instance {i}"
                        ))
                    })?;
                    transcript
                        .challenger
                        .observe_algebra_slice(&instance.permutation_local);
                    transcript
                        .challenger
                        .observe_algebra_slice(&instance.permutation_next);
                }
            }
        }

        Ok(())
    }
}

impl BatchStarkProver<GoldilocksBlake3Config> {
    /// Build a path-pruned Goldilocks/BLAKE3 compact proof while binding
    /// leading Public-table lanes to caller-supplied STARK public values.
    pub fn compact_goldilocks_blake3_path_pruned_preprocessed_with_public_values(
        &self,
        proof: BatchStarkProof<GoldilocksBlake3Config>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
    ) -> Result<GoldilocksBlake3PathPrunedCompactBatchStarkProof, BatchStarkProverError>
    where
        <GoldilocksBlake3Config as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksBlake3Challenge,
                <GoldilocksBlake3Config as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        proof.validate()?;
        fri_shape.validate()?;
        if !common_preprocessed_binding_eq(&proof.stark_common, &canonical_setup.prover_data.common)
        {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 path-pruned compact proof does not match canonical setup binding",
            )));
        }

        let ext_degree = proof.ext_degree;
        let w_binomial = proof.w_binomial;
        match ext_degree {
            1 => self.compact_goldilocks_blake3_path_pruned_preprocessed_inner::<1>(
                proof,
                None,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            2 => self.compact_goldilocks_blake3_path_pruned_preprocessed_inner::<2>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            4 => self.compact_goldilocks_blake3_path_pruned_preprocessed_inner::<4>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            5 => self.compact_goldilocks_blake3_path_pruned_preprocessed_inner::<5>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            6 => self.compact_goldilocks_blake3_path_pruned_preprocessed_inner::<6>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            8 => self.compact_goldilocks_blake3_path_pruned_preprocessed_inner::<8>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
            ),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    /// Build a metadata-free compact Goldilocks/BLAKE3 body.
    pub fn compact_goldilocks_blake3_path_pruned_preprocessed_body_with_public_values(
        &self,
        proof: BatchStarkProof<GoldilocksBlake3Config>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
    ) -> Result<GoldilocksBlake3PathPrunedCompactBatchStarkProofBody, BatchStarkProverError>
    where
        <GoldilocksBlake3Config as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksBlake3Challenge,
                <GoldilocksBlake3Config as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        self.compact_goldilocks_blake3_path_pruned_preprocessed_with_public_values(
            proof,
            public_values,
            canonical_setup,
            fri_shape,
        )
        .map(GoldilocksBlake3PathPrunedCompactBatchStarkProof::into_body)
    }

    /// Verify a canonical-metadata Goldilocks/BLAKE3 compact body against a
    /// verifier-owned context.
    pub fn verify_goldilocks_blake3_path_pruned_preprocessed_compact_body_with_context(
        &self,
        proof: GoldilocksBlake3PathPrunedCompactBatchStarkProofBody,
        context: GoldilocksBlake3PathPrunedCompactVerifierContext<'_>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksBlake3Config as StarkGenericConfig>::Pcs: Pcs<
                GoldilocksBlake3Challenge,
                <GoldilocksBlake3Config as StarkGenericConfig>::Challenger,
                Commitment = <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment,
            >,
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        context.validate()?;
        if proof.fri_shape != context.fri_shape {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 compact body FRI shape mismatch: expected {:?}, got {:?}",
                context.fri_shape, proof.fri_shape
            )));
        }
        self.verify_goldilocks_blake3_path_pruned_preprocessed_compact_with_public_values(
            proof.into_wrapped(context.metadata),
            context.public_values,
            context.canonical_setup,
        )
    }

    fn compact_goldilocks_blake3_path_pruned_preprocessed_inner<const D: usize>(
        &self,
        mut proof: BatchStarkProof<GoldilocksBlake3Config>,
        w_binomial: Option<Goldilocks>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
    ) -> Result<GoldilocksBlake3PathPrunedCompactBatchStarkProof, BatchStarkProverError>
    where
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        let (airs, pvs, effective_common) = self.rebuild_verifier_statement::<D>(
            &proof,
            w_binomial,
            &proof.stark_common,
            public_values,
        )?;
        let (query_indices, log_global_max_height) = self
            .derive_goldilocks_blake3_fri_query_indices::<D>(
                &proof.proof,
                &airs,
                &pvs,
                &effective_common,
                fri_shape,
            )?;

        let (input_batch_paths, commit_phase_paths) = self
            .prune_goldilocks_blake3_fri_merkle_paths(
                &mut proof.proof,
                &effective_common,
                &query_indices,
                log_global_max_height,
                fri_shape,
            )?;
        omit_preprocessed_ood_openings(&mut proof.proof);
        omit_goldilocks_blake3_preprocessed_fri_input_batches(&mut proof.proof, &effective_common)?;

        if !common_preprocessed_binding_eq(&effective_common, &canonical_setup.prover_data.common) {
            return Err(BatchStarkProverError::Verify(String::from(
                "canonical setup preprocessed binding changed during Goldilocks/BLAKE3 path-pruned compaction",
            )));
        }

        Ok(GoldilocksBlake3PathPrunedCompactBatchStarkProof {
            proof,
            fri_shape,
            input_batch_paths,
            commit_phase_paths,
        })
    }

    fn verify_goldilocks_blake3_path_pruned_preprocessed_compact_with_public_values(
        &self,
        proof: GoldilocksBlake3PathPrunedCompactBatchStarkProof,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksBlake3Config>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        proof.proof.validate()?;
        proof.fri_shape.validate()?;
        if !common_preprocessed_binding_eq(
            &proof.proof.stark_common,
            &canonical_setup.prover_data.common,
        ) {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 path-pruned compact proof does not match canonical setup binding",
            )));
        }

        let GoldilocksBlake3PathPrunedCompactBatchStarkProof {
            proof,
            fri_shape,
            input_batch_paths,
            commit_phase_paths,
        } = proof;
        let ext_degree = proof.ext_degree;
        let w_binomial = proof.w_binomial;
        match ext_degree {
            1 => self.verify_goldilocks_blake3_path_pruned_preprocessed_compact_inner::<1>(
                proof,
                None,
                public_values,
                canonical_setup,
                fri_shape,
                input_batch_paths,
                commit_phase_paths,
            ),
            2 => self.verify_goldilocks_blake3_path_pruned_preprocessed_compact_inner::<2>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
                input_batch_paths,
                commit_phase_paths,
            ),
            4 => self.verify_goldilocks_blake3_path_pruned_preprocessed_compact_inner::<4>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
                input_batch_paths,
                commit_phase_paths,
            ),
            5 => self.verify_goldilocks_blake3_path_pruned_preprocessed_compact_inner::<5>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
                input_batch_paths,
                commit_phase_paths,
            ),
            6 => self.verify_goldilocks_blake3_path_pruned_preprocessed_compact_inner::<6>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
                input_batch_paths,
                commit_phase_paths,
            ),
            8 => self.verify_goldilocks_blake3_path_pruned_preprocessed_compact_inner::<8>(
                proof,
                w_binomial,
                public_values,
                canonical_setup,
                fri_shape,
                input_batch_paths,
                commit_phase_paths,
            ),
            d => Err(BatchStarkProverError::UnsupportedDegree(d)),
        }
    }

    fn verify_goldilocks_blake3_path_pruned_preprocessed_compact_inner<const D: usize>(
        &self,
        mut proof: BatchStarkProof<GoldilocksBlake3Config>,
        w_binomial: Option<Goldilocks>,
        public_values: &[Goldilocks],
        canonical_setup: &CircuitProverData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
        input_batch_paths: Vec<GoldilocksBlake3PrunedMerklePaths>,
        commit_phase_paths: Vec<GoldilocksBlake3PrunedMerklePaths>,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        let (airs, pvs, effective_common) = self.rebuild_verifier_statement::<D>(
            &proof,
            w_binomial,
            &proof.stark_common,
            public_values,
        )?;
        self.restore_goldilocks_blake3_preprocessed_ood_openings::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            canonical_setup,
            fri_shape,
        )?;
        self.restore_goldilocks_blake3_path_pruned_fri_openings::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            fri_shape,
            &input_batch_paths,
            &commit_phase_paths,
        )?;
        self.restore_goldilocks_blake3_preprocessed_fri_input_batches::<D>(
            &mut proof.proof,
            &airs,
            &pvs,
            &effective_common,
            canonical_setup,
            fri_shape,
        )?;

        p3_batch_stark::verify_batch(&self.config, &airs, &proof.proof, &pvs, &effective_common)
            .map_err(|e| BatchStarkProverError::Verify(format!("{e:?}")))
    }

    fn restore_goldilocks_blake3_preprocessed_ood_openings<const D: usize>(
        &self,
        proof: &mut BatchProof<GoldilocksBlake3Config>,
        airs: &[CircuitTableAir<GoldilocksBlake3Config, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksBlake3Config>,
        canonical_setup: &CircuitProverData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        let Some(global) = &common.preprocessed else {
            return Ok(());
        };
        if !common_preprocessed_binding_eq(common, &canonical_setup.prover_data.common) {
            return Err(BatchStarkProverError::Verify(String::from(
                "canonical setup preprocessed binding changed during Goldilocks/BLAKE3 OOD restoration",
            )));
        }
        let preprocessed_prover_data = canonical_setup
            .prover_data
            .prover_only
            .preprocessed_prover_data
            .as_ref()
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "canonical setup lacks preprocessed prover data",
                ))
            })?;

        let (zeta, trace_domains) =
            self.sample_batch_zeta_and_trace_domains(airs, proof, public_values, common)?;
        let val_mmcs = goldilocks_blake3_val_mmcs(fri_shape.cap_height);
        let preprocessed_matrices = val_mmcs.get_matrices(preprocessed_prover_data);

        for (matrix_index, &inst_idx) in global.matrix_to_instance.iter().enumerate() {
            let meta = global
                .instances
                .get(inst_idx)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "missing preprocessed metadata for instance {inst_idx}"
                    ))
                })?;
            if meta.matrix_index != matrix_index {
                return Err(BatchStarkProverError::Verify(format!(
                    "preprocessed matrix metadata mismatch for instance {inst_idx}"
                )));
            }
            if inst_idx >= proof.opened_values.instances.len() || inst_idx >= airs.len() {
                return Err(BatchStarkProverError::Verify(format!(
                    "preprocessed instance index {inst_idx} out of bounds"
                )));
            }
            let preprocessed_matrix = preprocessed_matrices.get(matrix_index).ok_or_else(|| {
                BatchStarkProverError::Verify(format!(
                    "missing canonical preprocessed matrix {matrix_index}"
                ))
            })?;

            let local = evaluate_goldilocks_bit_reversed_lde_at(
                *preprocessed_matrix,
                fri_shape.log_blowup,
                zeta,
            )?;
            let opened = &mut proof.opened_values.instances[inst_idx].base_opened_values;
            match &opened.preprocessed_local {
                Some(existing) if existing != &local => {
                    return Err(BatchStarkProverError::Verify(format!(
                        "serialized Goldilocks/BLAKE3 preprocessed local opening mismatch for instance {inst_idx}"
                    )));
                }
                Some(_) => {}
                None => opened.preprocessed_local = Some(local),
            }

            if !airs[inst_idx].preprocessed_next_row_columns().is_empty() {
                let zeta_next = trace_domains[inst_idx].next_point(zeta).ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "trace domain lacks next point for instance {inst_idx}"
                    ))
                })?;
                let next = evaluate_goldilocks_bit_reversed_lde_at(
                    *preprocessed_matrix,
                    fri_shape.log_blowup,
                    zeta_next,
                )?;
                match &opened.preprocessed_next {
                    Some(existing) if existing != &next => {
                        return Err(BatchStarkProverError::Verify(format!(
                            "serialized Goldilocks/BLAKE3 preprocessed next opening mismatch for instance {inst_idx}"
                        )));
                    }
                    Some(_) => {}
                    None => opened.preprocessed_next = Some(next),
                }
            }
        }

        Ok(())
    }

    fn restore_goldilocks_blake3_preprocessed_fri_input_batches<const D: usize>(
        &self,
        proof: &mut BatchProof<GoldilocksBlake3Config>,
        airs: &[CircuitTableAir<GoldilocksBlake3Config, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksBlake3Config>,
        canonical_setup: &CircuitProverData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
    ) -> Result<(), BatchStarkProverError>
    where
        <GoldilocksBlake3ValMmcs as Mmcs<Goldilocks>>::Commitment: PartialEq,
    {
        if common.preprocessed.is_none() {
            return Ok(());
        }
        if !common_preprocessed_binding_eq(common, &canonical_setup.prover_data.common) {
            return Err(BatchStarkProverError::Verify(String::from(
                "canonical setup preprocessed binding changed during Goldilocks/BLAKE3 compact verification",
            )));
        }

        let preprocessed_prover_data = canonical_setup
            .prover_data
            .prover_only
            .preprocessed_prover_data
            .as_ref()
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "canonical setup lacks preprocessed prover data",
                ))
            })?;

        let expected_full_batches = goldilocks_blake3_full_fri_input_batch_count(proof, common);
        let expected_compact_batches = expected_full_batches.checked_sub(1).ok_or_else(|| {
            BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 compact proof has no input batch slot to omit",
            ))
        })?;
        let preprocessed_idx = goldilocks_blake3_preprocessed_trace_idx();
        if preprocessed_idx >= expected_full_batches {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 preprocessed batch index {preprocessed_idx} exceeds expected input batches {expected_full_batches}",
            )));
        }

        let (query_indices, log_global_max_height) = self
            .derive_goldilocks_blake3_fri_query_indices::<D>(
                proof,
                airs,
                public_values,
                common,
                fri_shape,
            )?;

        let val_mmcs = goldilocks_blake3_val_mmcs(fri_shape.cap_height);
        let preprocessed_max_height = val_mmcs.get_max_height(preprocessed_prover_data);
        if preprocessed_max_height == 0 || !preprocessed_max_height.is_power_of_two() {
            return Err(BatchStarkProverError::Verify(format!(
                "canonical preprocessed prover data has bad max height {preprocessed_max_height}",
            )));
        }
        let preprocessed_log_max_height = log2_strict_usize(preprocessed_max_height);
        if preprocessed_log_max_height > log_global_max_height {
            return Err(BatchStarkProverError::Verify(format!(
                "preprocessed batch height log {preprocessed_log_max_height} exceeds FRI global height log {log_global_max_height}",
            )));
        }

        for (query, (query_index, query_proof)) in query_indices
            .into_iter()
            .zip(proof.opening_proof.query_proofs.iter_mut())
            .enumerate()
        {
            if query_proof.input_proof.len() != expected_compact_batches {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/BLAKE3 compact query {query} input batch count mismatch: expected {expected_compact_batches}, got {}",
                    query_proof.input_proof.len()
                )));
            }
            let reduced_index =
                query_index >> (log_global_max_height - preprocessed_log_max_height);
            let opening = val_mmcs.open_batch(reduced_index, preprocessed_prover_data);
            query_proof.input_proof.insert(preprocessed_idx, opening);
        }

        Ok(())
    }

    fn prune_goldilocks_blake3_fri_merkle_paths(
        &self,
        proof: &mut BatchProof<GoldilocksBlake3Config>,
        common: &CommonData<GoldilocksBlake3Config>,
        query_indices: &[usize],
        log_global_max_height: usize,
        fri_shape: GoldilocksBlake3FriShape,
    ) -> Result<
        (
            Vec<GoldilocksBlake3PrunedMerklePaths>,
            Vec<GoldilocksBlake3PrunedMerklePaths>,
        ),
        BatchStarkProverError,
    > {
        let query_count = proof.opening_proof.query_proofs.len();
        if query_indices.len() != query_count {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 path-pruned query index count mismatch: expected {query_count}, got {}",
                query_indices.len()
            )));
        }

        let expected_full_batches = goldilocks_blake3_full_fri_input_batch_count(proof, common);
        let preprocessed_idx = common
            .preprocessed
            .as_ref()
            .map(|_| goldilocks_blake3_preprocessed_trace_idx());
        if let Some(idx) = preprocessed_idx
            && idx >= expected_full_batches
        {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 preprocessed batch index {idx} exceeds expected input batches {expected_full_batches}",
            )));
        }

        for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
            if query_proof.input_proof.len() != expected_full_batches {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/BLAKE3 full query {query} input batch count mismatch: expected {expected_full_batches}, got {}",
                    query_proof.input_proof.len()
                )));
            }
        }

        let kept_input_batches = (0..expected_full_batches)
            .filter(|&batch| Some(batch) != preprocessed_idx)
            .collect::<Vec<_>>();
        let mut input_batch_paths = Vec::with_capacity(kept_input_batches.len());
        for &batch in &kept_input_batches {
            let full_paths = proof
                .opening_proof
                .query_proofs
                .iter()
                .map(|query_proof| query_proof.input_proof[batch].opening_proof.clone())
                .collect::<Vec<_>>();
            let full_path_len = full_paths.first().map_or(0, Vec::len);
            let leaf_indices = goldilocks_tip5_input_batch_leaf_indices(
                query_indices,
                log_global_max_height,
                full_path_len,
                fri_shape.cap_height,
            )?;
            let (full_path_len, paths) =
                prune_merkle_paths_generic(&leaf_indices, &full_paths, "Goldilocks/BLAKE3")?;
            input_batch_paths.push(GoldilocksBlake3PrunedMerklePaths {
                full_path_len,
                paths,
            });
        }

        for query_proof in &mut proof.opening_proof.query_proofs {
            for &batch in &kept_input_batches {
                query_proof.input_proof[batch].opening_proof.clear();
            }
        }

        let expected_rounds = proof.opening_proof.commit_phase_commits.len();
        let mut domain_indices = query_indices.to_vec();
        let mut commit_phase_paths = Vec::with_capacity(expected_rounds);
        for round in 0..expected_rounds {
            let log_arity = proof
                .opening_proof
                .query_proofs
                .first()
                .and_then(|query| query.commit_phase_openings.get(round))
                .map(|opening| opening.log_arity as usize)
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "Goldilocks/BLAKE3 path-pruned proof missing commit phase round {round}",
                    ))
                })?;
            if !(1..=fri_shape.max_log_arity).contains(&log_arity) {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/BLAKE3 FRI invalid log arity {log_arity} in round {round}; max {}",
                    fri_shape.max_log_arity
                )));
            }

            let mut full_paths = Vec::with_capacity(query_count);
            for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
                if query_proof.commit_phase_openings.len() != expected_rounds {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/BLAKE3 FRI query {query} commit phase count mismatch: expected {expected_rounds}, got {}",
                        query_proof.commit_phase_openings.len()
                    )));
                }
                let opening = &query_proof.commit_phase_openings[round];
                if opening.log_arity as usize != log_arity {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/BLAKE3 FRI query {query} log arity mismatch in round {round}",
                    )));
                }
                full_paths.push(opening.opening_proof.clone());
            }

            let leaf_indices = domain_indices
                .iter()
                .map(|index| index >> log_arity)
                .collect::<Vec<_>>();
            let (full_path_len, paths) =
                prune_merkle_paths_generic(&leaf_indices, &full_paths, "Goldilocks/BLAKE3")?;
            commit_phase_paths.push(GoldilocksBlake3PrunedMerklePaths {
                full_path_len,
                paths,
            });
            for index in &mut domain_indices {
                *index >>= log_arity;
            }
        }

        for query_proof in &mut proof.opening_proof.query_proofs {
            for opening in &mut query_proof.commit_phase_openings {
                opening.opening_proof.clear();
            }
        }

        Ok((input_batch_paths, commit_phase_paths))
    }

    fn restore_goldilocks_blake3_path_pruned_fri_openings<const D: usize>(
        &self,
        proof: &mut BatchProof<GoldilocksBlake3Config>,
        airs: &[CircuitTableAir<GoldilocksBlake3Config, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
        input_batch_paths: &[GoldilocksBlake3PrunedMerklePaths],
        commit_phase_paths: &[GoldilocksBlake3PrunedMerklePaths],
    ) -> Result<(), BatchStarkProverError> {
        let (query_indices, log_global_max_height) = self
            .derive_goldilocks_blake3_fri_query_indices::<D>(
                proof,
                airs,
                public_values,
                common,
                fri_shape,
            )?;
        let query_count = proof.opening_proof.query_proofs.len();
        if query_indices.len() != query_count {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 path-pruned query index count mismatch: expected {query_count}, got {}",
                query_indices.len()
            )));
        }

        let expected_full_batches = goldilocks_blake3_full_fri_input_batch_count(proof, common);
        let expected_compact_batches = expected_full_batches
            .checked_sub(usize::from(common.preprocessed.is_some()))
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "Goldilocks/BLAKE3 path-pruned input batch count underflow",
                ))
            })?;
        if input_batch_paths.len() != expected_compact_batches {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 path-pruned input batch dictionary count mismatch: expected {expected_compact_batches}, got {}",
                input_batch_paths.len()
            )));
        }
        for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
            if query_proof.input_proof.len() != expected_compact_batches {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/BLAKE3 path-pruned query {query} input batch count mismatch: expected {expected_compact_batches}, got {}",
                    query_proof.input_proof.len()
                )));
            }
            for (batch, opening) in query_proof.input_proof.iter().enumerate() {
                if !opening.opening_proof.is_empty() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/BLAKE3 path-pruned query {query} input batch {batch} carried an unpruned opening proof",
                    )));
                }
            }
        }

        for (batch, pruned) in input_batch_paths.iter().enumerate() {
            let label = format!("Goldilocks/BLAKE3 input batch {batch}");
            let expected_leaf_indices = goldilocks_tip5_input_batch_leaf_indices(
                &query_indices,
                log_global_max_height,
                pruned.full_path_len,
                fri_shape.cap_height,
            )?;
            let restored = restore_merkle_paths_generic(
                pruned.full_path_len,
                &pruned.paths,
                query_count,
                &expected_leaf_indices,
                log_global_max_height,
                &label,
            )?;
            for (query, path) in restored.into_iter().enumerate() {
                proof.opening_proof.query_proofs[query].input_proof[batch].opening_proof = path;
            }
        }

        let expected_rounds = proof.opening_proof.commit_phase_commits.len();
        if commit_phase_paths.len() != expected_rounds {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 path-pruned commit phase dictionary count mismatch: expected {expected_rounds}, got {}",
                commit_phase_paths.len()
            )));
        }
        for (query, query_proof) in proof.opening_proof.query_proofs.iter().enumerate() {
            if query_proof.commit_phase_openings.len() != expected_rounds {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/BLAKE3 path-pruned query {query} commit phase count mismatch: expected {expected_rounds}, got {}",
                    query_proof.commit_phase_openings.len()
                )));
            }
            for (round, opening) in query_proof.commit_phase_openings.iter().enumerate() {
                if !opening.opening_proof.is_empty() {
                    return Err(BatchStarkProverError::Verify(format!(
                        "Goldilocks/BLAKE3 path-pruned query {query} commit phase {round} carried an unpruned opening proof",
                    )));
                }
            }
        }

        let mut domain_indices = query_indices;
        for (round, pruned) in commit_phase_paths.iter().enumerate() {
            let label = format!("Goldilocks/BLAKE3 commit phase {round}");
            let log_arity = proof
                .opening_proof
                .query_proofs
                .first()
                .and_then(|query| query.commit_phase_openings.get(round))
                .map(|opening| opening.log_arity as usize)
                .ok_or_else(|| {
                    BatchStarkProverError::Verify(format!(
                        "Goldilocks/BLAKE3 path-pruned proof missing commit phase round {round}",
                    ))
                })?;
            let expected_leaf_indices = domain_indices
                .iter()
                .map(|index| index >> log_arity)
                .collect::<Vec<_>>();
            let restored = restore_merkle_paths_generic(
                pruned.full_path_len,
                &pruned.paths,
                query_count,
                &expected_leaf_indices,
                log_global_max_height,
                &label,
            )?;
            for (query, path) in restored.into_iter().enumerate() {
                proof.opening_proof.query_proofs[query].commit_phase_openings[round]
                    .opening_proof = path;
            }
            for index in &mut domain_indices {
                *index >>= log_arity;
            }
        }

        Ok(())
    }

    fn derive_goldilocks_blake3_fri_query_indices<const D: usize>(
        &self,
        proof: &BatchProof<GoldilocksBlake3Config>,
        airs: &[CircuitTableAir<GoldilocksBlake3Config, D>],
        public_values: &[Vec<Goldilocks>],
        common: &CommonData<GoldilocksBlake3Config>,
        fri_shape: GoldilocksBlake3FriShape,
    ) -> Result<(Vec<usize>, usize), BatchStarkProverError> {
        let (mut transcript, trace_domains) =
            self.initialise_batch_transcript_to_zeta(airs, proof, public_values, common)?;
        let zeta = transcript.sample_zeta();
        self.observe_pcs_opened_values(
            &mut transcript,
            proof,
            airs,
            common,
            &trace_domains,
            zeta,
            "Goldilocks/BLAKE3",
        )?;

        let fri_proof = &proof.opening_proof;
        let expected_rounds = fri_proof.commit_phase_commits.len();
        let log_arities = if let Some(first_query) = fri_proof.query_proofs.first() {
            first_query
                .commit_phase_openings
                .iter()
                .enumerate()
                .map(|(round, opening)| {
                    let log_arity = opening.log_arity as usize;
                    if !(1..=fri_shape.max_log_arity).contains(&log_arity) {
                        return Err(BatchStarkProverError::Verify(format!(
                            "Goldilocks/BLAKE3 FRI invalid log arity {log_arity} in round {round}; max {}",
                            fri_shape.max_log_arity
                        )));
                    }
                    Ok(log_arity)
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        for (query, query_proof) in fri_proof.query_proofs.iter().enumerate() {
            if query_proof.commit_phase_openings.len() != expected_rounds {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/BLAKE3 FRI query {query} commit phase count mismatch: expected {expected_rounds}, got {}",
                    query_proof.commit_phase_openings.len()
                )));
            }
            let got_log_arities = query_proof
                .commit_phase_openings
                .iter()
                .map(|opening| opening.log_arity as usize)
                .collect::<Vec<_>>();
            if got_log_arities != log_arities {
                return Err(BatchStarkProverError::Verify(format!(
                    "Goldilocks/BLAKE3 FRI query {query} log arities mismatch",
                )));
            }
        }

        let total_log_reduction = log_arities.iter().copied().sum::<usize>();
        let log_global_max_height = total_log_reduction
            .checked_add(fri_shape.log_blowup)
            .and_then(|v| v.checked_add(fri_shape.log_final_poly_len))
            .ok_or_else(|| {
                BatchStarkProverError::Verify(String::from(
                    "Goldilocks/BLAKE3 FRI global height overflow",
                ))
            })?;
        let _ = checked_power_of_two(log_global_max_height, "FRI global max height")?;

        if fri_proof.commit_pow_witnesses.len() != fri_proof.commit_phase_commits.len() {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 FRI commit PoW witness count mismatch: expected {}, got {}",
                fri_proof.commit_phase_commits.len(),
                fri_proof.commit_pow_witnesses.len()
            )));
        }

        let _: GoldilocksBlake3Challenge = transcript.challenger.sample_algebra_element();
        for (commitment, witness) in fri_proof
            .commit_phase_commits
            .iter()
            .zip(&fri_proof.commit_pow_witnesses)
        {
            transcript.challenger.observe(commitment.clone());
            if !transcript
                .challenger
                .check_witness(fri_shape.commit_pow_bits, *witness)
            {
                return Err(BatchStarkProverError::Verify(String::from(
                    "Goldilocks/BLAKE3 FRI commit PoW witness rejected",
                )));
            }
            let _: GoldilocksBlake3Challenge = transcript.challenger.sample_algebra_element();
        }

        let final_poly_len = fri_shape.final_poly_len()?;
        if fri_proof.final_poly.len() != final_poly_len {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 FRI final polynomial length mismatch: expected {final_poly_len}, got {}",
                fri_proof.final_poly.len()
            )));
        }
        transcript
            .challenger
            .observe_algebra_slice(&fri_proof.final_poly);

        if fri_proof.query_proofs.len() != fri_shape.num_queries {
            return Err(BatchStarkProverError::Verify(format!(
                "Goldilocks/BLAKE3 FRI query count mismatch: expected {}, got {}",
                fri_shape.num_queries,
                fri_proof.query_proofs.len()
            )));
        }
        for &log_arity in &log_arities {
            transcript
                .challenger
                .observe(Goldilocks::from_usize(log_arity));
        }
        if !transcript
            .challenger
            .check_witness(fri_shape.query_pow_bits, fri_proof.query_pow_witness)
        {
            return Err(BatchStarkProverError::Verify(String::from(
                "Goldilocks/BLAKE3 FRI query PoW witness rejected",
            )));
        }

        Ok((
            (0..fri_proof.query_proofs.len())
                .map(|_| transcript.challenger.sample_bits(log_global_max_height))
                .collect(),
            log_global_max_height,
        ))
    }
}

fn goldilocks_tip5_full_fri_input_batch_count(
    proof: &BatchProof<GoldilocksTipsConfig>,
    common: &CommonData<GoldilocksTipsConfig>,
) -> usize {
    usize::from(proof.commitments.random.is_some())
        + 1
        + 1
        + usize::from(common.preprocessed.is_some())
        + usize::from(proof.commitments.permutation.is_some())
}

fn goldilocks_blake3_full_fri_input_batch_count(
    proof: &BatchProof<GoldilocksBlake3Config>,
    common: &CommonData<GoldilocksBlake3Config>,
) -> usize {
    usize::from(proof.commitments.random.is_some())
        + 1
        + 1
        + usize::from(common.preprocessed.is_some())
        + usize::from(proof.commitments.permutation.is_some())
}

fn evaluate_matrix_columns_at<F, EF, D, M>(domain: &D, matrix: &M, point: EF) -> Vec<EF>
where
    F: Field,
    EF: ExtensionField<F>,
    D: PolynomialSpace<Val = F>,
    M: Matrix<F>,
{
    let height = matrix.height();
    let width = matrix.width();
    (0..width)
        .map(|col| {
            let evals = (0..height)
                .map(|row| {
                    matrix
                        .get(row, col)
                        .expect("row/column are in bounds by construction")
                })
                .collect::<Vec<_>>();
            domain.evaluate_polynomial_at(&evals, point)
        })
        .collect()
}

/// Poseidon2 AIR builders for the given extension degree `D` (typically `2` or `4`).
pub fn poseidon2_air_builders<SC, const D: usize>() -> Vec<Box<dyn NpoAirBuilder<SC, D>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<D> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
    Poseidon2AirBuilder<D>: NpoAirBuilder<SC, D>,
{
    vec![Box::new(Poseidon2AirBuilder)]
}

/// Create Poseidon2 table provers for D=4 (e.g. BabyBear, KoalaBear).
pub fn poseidon2_table_provers_d4<SC>(config: Poseidon2Config) -> Vec<Box<dyn TableProver<SC>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<4> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon2Prover::new(
        config,
        ConstraintProfile::Standard,
    ))]
}

/// Create Poseidon2 table provers for `D = 5` circuit traces (e.g. Koala quintic with base-first Poseidon).
pub fn poseidon2_table_provers_d5<SC>(config: Poseidon2Config) -> Vec<Box<dyn TableProver<SC>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<4>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon2Prover::new(
        config,
        ConstraintProfile::Standard,
    ))]
}

/// Poseidon2 AIR builders for D=2 (e.g. Goldilocks).
pub fn poseidon2_air_builders_d2<SC>() -> Vec<Box<dyn NpoAirBuilder<SC, 2>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<2> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon2AirBuilder::<2>)]
}

/// Poseidon2 AIR builders for D=4 (e.g. BabyBear, KoalaBear).
pub fn poseidon2_air_builders_d4<SC>() -> Vec<Box<dyn NpoAirBuilder<SC, 4>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<4> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon2AirBuilder::<4>)]
}

/// Poseidon2 AIR builders for `D = 5` circuit traces (e.g. KoalaBear quintic).
pub fn poseidon2_air_builders_d5<SC>() -> Vec<Box<dyn NpoAirBuilder<SC, 5>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon2AirBuilder::<5>)]
}

/// Poseidon1 AIR builders for the given extension degree `D` (typically `2` or `4`).
pub fn poseidon1_air_builders<SC, const D: usize>() -> Vec<Box<dyn NpoAirBuilder<SC, D>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<D> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
    Poseidon1AirBuilder<D>: NpoAirBuilder<SC, D>,
{
    vec![Box::new(Poseidon1AirBuilder)]
}

/// Create Poseidon1 table provers for D=4 (e.g. BabyBear, KoalaBear).
pub fn poseidon1_table_provers_d4<SC>(config: Poseidon1Config) -> Vec<Box<dyn TableProver<SC>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<4> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon1Prover::new(
        config,
        ConstraintProfile::Standard,
    ))]
}

/// Create Poseidon1 table provers for `D = 5` circuit traces (e.g. Koala quintic with base-first Poseidon).
pub fn poseidon1_table_provers_d5<SC>(config: Poseidon1Config) -> Vec<Box<dyn TableProver<SC>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField + BinomiallyExtendable<4>,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon1Prover::new(
        config,
        ConstraintProfile::Standard,
    ))]
}

/// Poseidon1 AIR builders for D=2 (e.g. Goldilocks).
pub fn poseidon1_air_builders_d2<SC>() -> Vec<Box<dyn NpoAirBuilder<SC, 2>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<2> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon1AirBuilder::<2>)]
}

/// Poseidon1 AIR builders for D=4 (e.g. BabyBear, KoalaBear).
pub fn poseidon1_air_builders_d4<SC>() -> Vec<Box<dyn NpoAirBuilder<SC, 4>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: BinomiallyExtendable<4> + StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon1AirBuilder::<4>)]
}

/// Poseidon1 AIR builders for `D = 5` circuit traces (e.g. KoalaBear quintic).
pub fn poseidon1_air_builders_d5<SC>() -> Vec<Box<dyn NpoAirBuilder<SC, 5>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    vec![Box::new(Poseidon1AirBuilder::<5>)]
}

/// Returns a type-erased Recompose preprocessor.
///
/// When `split_coeff_tables` is true, preprocesses both `recompose` and `recompose/coeff` rows.
pub fn recompose_preprocessor<F>(split_coeff_tables: bool) -> Box<dyn NpoPreprocessor<F>>
where
    F: StarkField + PrimeField,
    RecomposePreprocessor: NpoPreprocessor<F>,
{
    Box::new(RecomposePreprocessor::new(split_coeff_tables))
}

/// Recompose table provers for a given extension field degree.
///
/// When `split_coeff_tables` is true, returns both the standard table and the `recompose/coeff`
/// variant.
pub fn recompose_table_provers<SC, const D: usize>(
    lanes: usize,
    split_coeff_tables: bool,
) -> Vec<Box<dyn TableProver<SC>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    if split_coeff_tables {
        vec![
            Box::new(RecomposeProver::<D>::new(lanes, false)),
            Box::new(RecomposeProver::<D>::new(lanes, true)),
        ]
    } else {
        vec![Box::new(RecomposeProver::<D>::new(lanes, false))]
    }
}

/// Recompose AIR builders for a given extension field degree.
///
/// `split_coeff_tables` must match the value used in the paired [`recompose_table_provers`].
pub fn recompose_air_builders<SC, const D: usize>(
    lanes: usize,
    split_coeff_tables: bool,
) -> Vec<Box<dyn NpoAirBuilder<SC, D>>>
where
    SC: StarkGenericConfig + 'static + Send + Sync,
    Val<SC>: StarkField,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>:
        Algebra<SymbolicExpression<Val<SC>>> + Algebra<SC::Challenge>,
{
    if split_coeff_tables {
        vec![
            Box::new(RecomposeAirBuilder::<D>::new(lanes, false)),
            Box::new(RecomposeAirBuilder::<D>::new(lanes, true)),
        ]
    } else {
        vec![Box::new(RecomposeAirBuilder::<D>::new(lanes, false))]
    }
}

#[cfg(test)]
mod tests;
