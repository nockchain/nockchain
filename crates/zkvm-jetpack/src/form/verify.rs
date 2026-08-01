use std::cmp::Ordering;
use std::convert::TryFrom;

use nockchain_math::tip5::STATE_SIZE;
use nockvm::jets::JetErr;
use nockvm::mem::NOCK_STACK_SIZE;

use crate::form::belt::{based_check, bpow, Belt, FieldError, PRIME};
use crate::form::challenges::{augment_challenges, NUM_EXT_CHALS, NUM_MEGA_EXT_CHALS};
use crate::form::config::{
    compute_base_widths, compute_ext_widths, compute_full_widths, compute_mega_ext_widths,
    core_table_names, ConfigError,
};
use crate::form::felt::{fadd_, fmul_, fpow_, Felt};
use crate::form::math::fri::{fri_verify, FriError};
use crate::form::math::gen_trace::{build_tree_data, TreeData};
use crate::form::math::prover::ProcessedDegrees;
use crate::form::math::StarkCalc;
use crate::form::merk::{hash_bpoly_leaf, verify_merk_proof, MerkData};
use crate::form::poly::{BPolySlice, BPolyVec, FPolySlice, FPolyVec, PolySlice, PolyVec};
use crate::form::preprocess::{
    preprocess_degrees, preprocess_for, PreprocessData, PreprocessError,
};
use crate::form::proof::{
    reassemble_noun, CountMap, Proof, ProofData, ProofPathBf, ProofStream, ProofStreamError,
    ProofVersion,
};
use crate::form::term::Term;
use crate::form::tog::{absorb, belts, felt, felts, Tog};
use crate::form::verifier_math::{bpeval_lift_, evaluate_trace_degree_normalization};

#[derive(Copy, Clone)]
struct TermComponents {
    _base: &'static str,
    _a: &'static str,
    _b: &'static str,
    _c: &'static str,
}

#[derive(Copy, Clone)]
struct TermComponentIdx {
    a: usize,
    b: usize,
    c: usize,
}

const fn term_component_idx(term_index: usize) -> TermComponentIdx {
    let base = term_index * 3;
    TermComponentIdx {
        a: base,
        b: base + 1,
        c: base + 2,
    }
}

const fn term_components(
    base: &'static str,
    a: &'static str,
    b: &'static str,
    c: &'static str,
) -> TermComponents {
    TermComponents {
        _base: base,
        _a: a,
        _b: b,
        _c: c,
    }
}

const CHAL_A: TermComponents = term_components("a", "a-a", "a-b", "a-c");
const CHAL_B: TermComponents = term_components("b", "b-a", "b-b", "b-c");
const CHAL_C: TermComponents = term_components("c", "c-a", "c-b", "c-c");
const CHAL_D: TermComponents = term_components("d", "d-a", "d-b", "d-c");
const CHAL_E: TermComponents = term_components("e", "e-a", "e-b", "e-c");
const CHAL_F: TermComponents = term_components("f", "f-a", "f-b", "f-c");
const CHAL_G: TermComponents = term_components("g", "g-a", "g-b", "g-c");
const CHAL_P: TermComponents = term_components("p", "p-a", "p-b", "p-c");
const CHAL_Q: TermComponents = term_components("q", "q-a", "q-b", "q-c");
const CHAL_R: TermComponents = term_components("r", "r-a", "r-b", "r-c");
const CHAL_S: TermComponents = term_components("s", "s-a", "s-b", "s-c");
const CHAL_T: TermComponents = term_components("t", "t-a", "t-b", "t-c");
const CHAL_U: TermComponents = term_components("u", "u-a", "u-b", "u-c");
const CHAL_ALF: TermComponents = term_components("alf", "alf-a", "alf-b", "alf-c");
const CHAL_J: TermComponents = term_components("j", "j-a", "j-b", "j-c");
const CHAL_K: TermComponents = term_components("k", "k-a", "k-b", "k-c");
const CHAL_L: TermComponents = term_components("l", "l-a", "l-b", "l-c");
const CHAL_M: TermComponents = term_components("m", "m-a", "m-b", "m-c");
const CHAL_N: TermComponents = term_components("n", "n-a", "n-b", "n-c");
const CHAL_O: TermComponents = term_components("o", "o-a", "o-b", "o-c");
const CHAL_W: TermComponents = term_components("w", "w-a", "w-b", "w-c");
const CHAL_X: TermComponents = term_components("x", "x-a", "x-b", "x-c");
const CHAL_Y: TermComponents = term_components("y", "y-a", "y-b", "y-c");
const CHAL_Z: TermComponents = term_components("z", "z-a", "z-b", "z-c");
const CHAL_BET: TermComponents = term_components("bet", "bet-a", "bet-b", "bet-c");
const CHAL_GAM: TermComponents = term_components("gam", "gam-a", "gam-b", "gam-c");

const CHAL_RD1: [TermComponents; 14] = [
    CHAL_A, CHAL_B, CHAL_C, CHAL_D, CHAL_E, CHAL_F, CHAL_G, CHAL_P, CHAL_Q, CHAL_R, CHAL_S, CHAL_T,
    CHAL_U, CHAL_ALF,
];
const CHAL_RD2: [TermComponents; 12] = [
    CHAL_J, CHAL_K, CHAL_L, CHAL_M, CHAL_N, CHAL_O, CHAL_W, CHAL_X, CHAL_Y, CHAL_Z, CHAL_BET,
    CHAL_GAM,
];
const NUM_BASE_CHALS: usize = (CHAL_RD1.len() + CHAL_RD2.len()) * 3;
const CHAL_ALF_IDX: TermComponentIdx = term_component_idx(13);
const CHAL_J_IDX: TermComponentIdx = term_component_idx(14);
const CHAL_K_IDX: TermComponentIdx = term_component_idx(15);
const CHAL_L_IDX: TermComponentIdx = term_component_idx(16);
const CHAL_M_IDX: TermComponentIdx = term_component_idx(17);
const CHAL_Z_IDX: TermComponentIdx = term_component_idx(23);

const TERM_COMPUTE: &str = "compute";
const TERM_MEMORY: &str = "memory";

const TERM_COMPUTE_S_SIZE: TermComponents = term_components(
    "compute-s-size", "compute-s-size-a", "compute-s-size-b", "compute-s-size-c",
);
const TERM_COMPUTE_S_LEAF: TermComponents = term_components(
    "compute-s-leaf", "compute-s-leaf-a", "compute-s-leaf-b", "compute-s-leaf-c",
);
const TERM_COMPUTE_S_DYCK: TermComponents = term_components(
    "compute-s-dyck", "compute-s-dyck-a", "compute-s-dyck-b", "compute-s-dyck-c",
);
const TERM_COMPUTE_F_SIZE: TermComponents = term_components(
    "compute-f-size", "compute-f-size-a", "compute-f-size-b", "compute-f-size-c",
);
const TERM_COMPUTE_F_LEAF: TermComponents = term_components(
    "compute-f-leaf", "compute-f-leaf-a", "compute-f-leaf-b", "compute-f-leaf-c",
);
const TERM_COMPUTE_F_DYCK: TermComponents = term_components(
    "compute-f-dyck", "compute-f-dyck-a", "compute-f-dyck-b", "compute-f-dyck-c",
);
const TERM_COMPUTE_E_SIZE: TermComponents = term_components(
    "compute-e-size", "compute-e-size-a", "compute-e-size-b", "compute-e-size-c",
);
const TERM_COMPUTE_E_LEAF: TermComponents = term_components(
    "compute-e-leaf", "compute-e-leaf-a", "compute-e-leaf-b", "compute-e-leaf-c",
);
const TERM_COMPUTE_E_DYCK: TermComponents = term_components(
    "compute-e-dyck", "compute-e-dyck-a", "compute-e-dyck-b", "compute-e-dyck-c",
);
const TERM_COMPUTE_DECODE_MSET: TermComponents = term_components(
    "compute-decode-mset", "compute-decode-mset-a", "compute-decode-mset-b",
    "compute-decode-mset-c",
);
const TERM_COMPUTE_OP0_MSET: TermComponents = term_components(
    "compute-op0-mset", "compute-op0-mset-a", "compute-op0-mset-b", "compute-op0-mset-c",
);
const COMPUTE_TERMINALS: [TermComponents; 11] = [
    TERM_COMPUTE_S_SIZE, TERM_COMPUTE_S_LEAF, TERM_COMPUTE_S_DYCK, TERM_COMPUTE_F_SIZE,
    TERM_COMPUTE_F_LEAF, TERM_COMPUTE_F_DYCK, TERM_COMPUTE_E_SIZE, TERM_COMPUTE_E_LEAF,
    TERM_COMPUTE_E_DYCK, TERM_COMPUTE_DECODE_MSET, TERM_COMPUTE_OP0_MSET,
];

const TERM_MEMORY_NC: TermComponents =
    term_components("memory-nc", "memory-nc-a", "memory-nc-b", "memory-nc-c");
const TERM_MEMORY_KVS: TermComponents =
    term_components("memory-kvs", "memory-kvs-a", "memory-kvs-b", "memory-kvs-c");
const TERM_MEMORY_DECODE_MSET: TermComponents = term_components(
    "memory-decode-mset", "memory-decode-mset-a", "memory-decode-mset-b", "memory-decode-mset-c",
);
const TERM_MEMORY_OP0_MSET: TermComponents = term_components(
    "memory-op0-mset", "memory-op0-mset-a", "memory-op0-mset-b", "memory-op0-mset-c",
);
const MEMORY_TERMINALS: [TermComponents; 4] =
    [TERM_MEMORY_NC, TERM_MEMORY_KVS, TERM_MEMORY_DECODE_MSET, TERM_MEMORY_OP0_MSET];
// Historical context:
// - During early zkvm bring-up, table-level override tooling (including jute ordering) was used to
//   bisect constraint/debug issues.
// - Production proofs here only use compute+memory terminals. The jute table is a precompile table
//   and is not active in this native verifier path.
const TERMINAL_TABLES: [(&str, &[TermComponents]); 2] =
    [(TERM_COMPUTE, &COMPUTE_TERMINALS), (TERM_MEMORY, &MEMORY_TERMINALS)];

#[derive(Clone, Debug)]
struct TreeSnapshot {
    size: Felt,
    leaf: Felt,
    dyck: Felt,
    is_atom: bool,
}

impl From<TreeData> for TreeSnapshot {
    fn from(data: TreeData) -> Self {
        TreeSnapshot {
            size: data.size,
            leaf: data.leaf,
            dyck: data.dyck,
            is_atom: data.n.is_atom(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TerminalLookup {
    compute_s_size: Felt,
    compute_s_leaf: Felt,
    compute_s_dyck: Felt,
    compute_f_size: Felt,
    compute_f_leaf: Felt,
    compute_f_dyck: Felt,
    compute_e_size: Felt,
    compute_e_leaf: Felt,
    compute_e_dyck: Felt,
    compute_decode: Felt,
    compute_op0: Felt,
    memory_nc: Felt,
    memory_kvs: Felt,
    memory_decode: Felt,
    memory_op0: Felt,
}

#[derive(Default)]
struct TerminalLookupBuilder {
    compute_s_size: Option<Felt>,
    compute_s_leaf: Option<Felt>,
    compute_s_dyck: Option<Felt>,
    compute_f_size: Option<Felt>,
    compute_f_leaf: Option<Felt>,
    compute_f_dyck: Option<Felt>,
    compute_e_size: Option<Felt>,
    compute_e_leaf: Option<Felt>,
    compute_e_dyck: Option<Felt>,
    compute_decode: Option<Felt>,
    compute_op0: Option<Felt>,
    memory_nc: Option<Felt>,
    memory_kvs: Option<Felt>,
    memory_decode: Option<Felt>,
    memory_op0: Option<Felt>,
}

impl TerminalLookupBuilder {
    fn set_term(&mut self, term: &TermComponents, belts: [Belt; 3]) -> Result<(), VerifyError> {
        let felt = Felt::from(belts);
        match term._base {
            "compute-s-size" => self.compute_s_size = Some(felt),
            "compute-s-leaf" => self.compute_s_leaf = Some(felt),
            "compute-s-dyck" => self.compute_s_dyck = Some(felt),
            "compute-f-size" => self.compute_f_size = Some(felt),
            "compute-f-leaf" => self.compute_f_leaf = Some(felt),
            "compute-f-dyck" => self.compute_f_dyck = Some(felt),
            "compute-e-size" => self.compute_e_size = Some(felt),
            "compute-e-leaf" => self.compute_e_leaf = Some(felt),
            "compute-e-dyck" => self.compute_e_dyck = Some(felt),
            "compute-decode-mset" => self.compute_decode = Some(felt),
            "compute-op0-mset" => self.compute_op0 = Some(felt),
            "memory-nc" => self.memory_nc = Some(felt),
            "memory-kvs" => self.memory_kvs = Some(felt),
            "memory-decode-mset" => self.memory_decode = Some(felt),
            "memory-op0-mset" => self.memory_op0 = Some(felt),
            _ => return Err(VerifyError::Invalid("unknown terminal component")),
        }
        Ok(())
    }

    fn finalize(self) -> Result<TerminalLookup, VerifyError> {
        let required =
            |value: Option<Felt>, label: &'static str| value.ok_or(VerifyError::Invalid(label));
        Ok(TerminalLookup {
            compute_s_size: required(self.compute_s_size, "missing compute-s-size")?,
            compute_s_leaf: required(self.compute_s_leaf, "missing compute-s-leaf")?,
            compute_s_dyck: required(self.compute_s_dyck, "missing compute-s-dyck")?,
            compute_f_size: required(self.compute_f_size, "missing compute-f-size")?,
            compute_f_leaf: required(self.compute_f_leaf, "missing compute-f-leaf")?,
            compute_f_dyck: required(self.compute_f_dyck, "missing compute-f-dyck")?,
            compute_e_size: required(self.compute_e_size, "missing compute-e-size")?,
            compute_e_leaf: required(self.compute_e_leaf, "missing compute-e-leaf")?,
            compute_e_dyck: required(self.compute_e_dyck, "missing compute-e-dyck")?,
            compute_decode: required(self.compute_decode, "missing compute-decode-mset")?,
            compute_op0: required(self.compute_op0, "missing compute-op0-mset")?,
            memory_nc: required(self.memory_nc, "missing memory-nc")?,
            memory_kvs: required(self.memory_kvs, "missing memory-kvs")?,
            memory_decode: required(self.memory_decode, "missing memory-decode-mset")?,
            memory_op0: required(self.memory_op0, "missing memory-op0-mset")?,
        })
    }
}

use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, T};

#[derive(Debug)]
pub struct VerifyArgs {
    pub proof: Proof,
    // Legacy debugging hook from zkvm bring-up for table-scoped bisects.
    // Production native verification passes `None`; this is kept for compatibility with older
    // table-debug workflows.
    pub table_override: Option<Vec<Term>>,
    pub verifier_eny: u64,
}

#[derive(Debug)]
pub struct VerifyResult {
    pub commitment: [u64; 5],
    pub nonce: [u64; 5],
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum VerifyPreprocessError {
    Cue,
    Decode,
    VersionMismatch { expected: u64, found: u64 },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum VerifyFriError {
    Field,
    Jet,
    InvalidSample,
    Consistency,
    InvalidProof(&'static str),
    ProofStream,
}

#[derive(Debug)]
pub enum VerifyError {
    ProofStream(ProofStreamError),
    Config(ConfigError),
    Preprocess(VerifyPreprocessError),
    Stark(FieldError),
    Jet(JetErr),
    Fri(VerifyFriError),
    Invalid(&'static str),
}

const MAX_PUBLIC_PUZZLE_LEN: u64 = 1 << 16;
const MAX_TRACE_HEIGHT: u64 = 1 << 26;

impl From<ProofStreamError> for VerifyError {
    fn from(err: ProofStreamError) -> Self {
        VerifyError::ProofStream(err)
    }
}

impl From<ConfigError> for VerifyError {
    fn from(err: ConfigError) -> Self {
        VerifyError::Config(err)
    }
}

impl From<PreprocessError> for VerifyError {
    fn from(err: PreprocessError) -> Self {
        VerifyError::Preprocess(match err {
            PreprocessError::Cue => VerifyPreprocessError::Cue,
            PreprocessError::Decode => VerifyPreprocessError::Decode,
            PreprocessError::VersionMismatch { expected, found } => {
                VerifyPreprocessError::VersionMismatch { expected, found }
            }
        })
    }
}

impl From<FieldError> for VerifyError {
    fn from(err: FieldError) -> Self {
        VerifyError::Stark(err)
    }
}

impl From<JetErr> for VerifyError {
    fn from(err: JetErr) -> Self {
        VerifyError::Jet(err)
    }
}

impl From<FriError> for VerifyError {
    fn from(err: FriError) -> Self {
        VerifyError::Fri(match err {
            FriError::Field(_) => VerifyFriError::Field,
            FriError::Jet(_) => VerifyFriError::Jet,
            FriError::InvalidSample => VerifyFriError::InvalidSample,
            FriError::Consistency => VerifyFriError::Consistency,
            FriError::InvalidProof(label) => VerifyFriError::InvalidProof(label),
            FriError::ProofStream(_) => VerifyFriError::ProofStream,
        })
    }
}

pub fn verify(args: VerifyArgs) -> Result<VerifyResult, VerifyError> {
    let VerifyArgs {
        mut proof,
        table_override,
        verifier_eny,
    } = args;

    let version = proof.version;
    let preprocess = preprocess_for(&version);

    let proof_len = proof.objects.len();
    let mut stream = ProofStream::new(&mut proof)?;
    let puzzle = read_puzzle(&mut stream)?;
    ensure_digest_based(
        &puzzle.commitment, "puzzle commitment contains non-based elements",
    )?;
    ensure_digest_based(&puzzle.nonce, "puzzle nonce contains non-based elements")?;
    validate_puzzle_metadata(&puzzle)?;

    let mut table_names: Vec<Term> =
        table_override.unwrap_or_else(|| core_table_names(&version).to_vec());
    if table_names.is_empty() {
        return Err(VerifyError::Invalid("no tables configured for verifier"));
    }
    sort_table_names(&mut table_names);

    let base_widths = compute_base_widths(&version, Some(table_names.as_slice()))?;
    let ext_widths = compute_ext_widths(&version, Some(table_names.as_slice()))?;
    let mega_ext_widths = compute_mega_ext_widths(&version, Some(table_names.as_slice()))?;
    let full_widths = compute_full_widths(&version, Some(table_names.as_slice()))?;

    let heights = read_heights(&mut stream)?;
    if heights.len() != core_table_names(&version).len() {
        return Err(VerifyError::Invalid("table height count mismatch"));
    }
    validate_heights(&heights)?;

    if base_widths.len() != heights.len()
        || ext_widths.len() != heights.len()
        || mega_ext_widths.len() != heights.len()
        || full_widths.len() != heights.len()
    {
        return Err(VerifyError::Invalid("table metadata length mismatch"));
    }

    let calc = StarkCalc::new(&heights, &preprocess.constraints)?;
    let expected_items = expected_num_proof_items(&calc);
    if proof_len != expected_items {
        return Err(VerifyError::Invalid("proof length mismatch"));
    }

    // Materialize the puzzle nouns only after the cheap public metadata checks above.
    // This keeps malformed puzzle lengths/heights from driving large allocations first.
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let (subject, formula) = build_puzzle_subjects(&mut stack, &puzzle)?;
    let space = stack.noun_space();
    let product = reassemble_noun(&mut stack, &puzzle.leaf, &puzzle.dyck)
        .map_err(|_| VerifyError::Invalid("invalid puzzle leaf/dyck word"))?;
    if !based_noun(product, &space) {
        return Err(VerifyError::Invalid("puzzle product is not based"));
    }

    let base_root = expect_mroot(&mut stream, "base trace commitment")?;
    let round1_chals = sample_challenges(&mut stream, NUM_EXT_CHALS)?;
    let ext_root = expect_mroot(&mut stream, "extension trace commitment")?;
    let round2_chals = sample_challenges(&mut stream, NUM_MEGA_EXT_CHALS)?;

    let mut challenges = Vec::with_capacity(round1_chals.len() + round2_chals.len());
    challenges.extend_from_slice(&round1_chals);
    challenges.extend_from_slice(&round2_chals);
    augment_with_puzzle(&mut challenges, subject, &space)?;

    let augmented_chals: BPolyVec = PolyVec(challenges);
    let challenge_values = augmented_chals.as_slice();
    if challenge_values.len() < NUM_BASE_CHALS {
        return Err(VerifyError::Invalid("insufficient base challenges"));
    }
    let alf = felt_from_indices(challenge_values, CHAL_ALF_IDX, "alf challenge")?;
    let j = felt_from_indices(challenge_values, CHAL_J_IDX, "j challenge")?;
    let k = felt_from_indices(challenge_values, CHAL_K_IDX, "k challenge")?;
    let l = felt_from_indices(challenge_values, CHAL_L_IDX, "l challenge")?;
    let m = felt_from_indices(challenge_values, CHAL_M_IDX, "m challenge")?;
    let z = felt_from_indices(challenge_values, CHAL_Z_IDX, "z challenge")?;

    let tree_subject = build_tree_snapshot(subject, &alf, &space)?;
    let tree_formula = build_tree_snapshot(formula, &alf, &space)?;
    let tree_product = build_tree_snapshot(product, &alf, &space)?;

    let terminals = read_terminals(&mut stream)?;
    let expected_terminals = expected_terminal_count(&table_names)?;
    if terminals.0.len() != expected_terminals {
        return Err(VerifyError::Invalid("unexpected terminal buffer length"));
    }
    ensure_belts_based(&terminals.0, "terminal buffer contains non-based elements")?;
    let (terminal_lookup, dyn_list) = decode_terminals(&table_names, &terminals)?;

    verify_linking(
        &tree_subject, &tree_formula, &tree_product, &terminal_lookup, &j, &k, &l, &m, &z,
    )?;

    let mut rng = stream.transcript_rng()?;

    let total_extra_constraints = total_constraints(&preprocess.count_map, heights.len(), true)?;
    let extra_comp_weights = belts(&mut rng, (2 * total_extra_constraints) as u32);
    let extra_weight_map = build_weight_map(
        &extra_comp_weights,
        &preprocess.count_map,
        heights.len(),
        true,
    )?;
    let extra_comp_bpoly = read_poly(&mut stream)?;

    let mut rng = stream.transcript_rng()?;
    let extra_comp_eval_point = felt(&mut rng);
    let extra_trace_evaluations = read_evals(&mut stream)?;

    let total_cols: usize = full_widths.iter().copied().map(|w| w as usize).sum();
    if extra_trace_evaluations.0.len() != 2 * total_cols {
        return Err(VerifyError::Invalid(
            "extra trace evaluation length mismatch",
        ));
    }
    ensure_felts_based(
        &extra_trace_evaluations.0, "extra trace evaluations contain non-based elements",
    )?;

    let dyn_slices: Vec<BPolySlice<'_>> = dyn_list
        .iter()
        .map(|poly| PolySlice(poly.as_slice()))
        .collect();
    let augmented_slice: BPolySlice<'_> = PolySlice(augmented_chals.as_slice());
    let degrees = preprocess_degrees(&version, &heights);

    let extra_composition_eval = eval_composition(
        &PolySlice(extra_trace_evaluations.as_slice()),
        &heights,
        &degrees.extra,
        &preprocess.count_map,
        &dyn_slices,
        &extra_weight_map,
        &augmented_slice,
        &extra_comp_eval_point,
        &full_widths,
        true,
    )?;
    let extra_comp_bpoly_eval = bpeval_lift_(extra_comp_bpoly.as_slice(), &extra_comp_eval_point);
    if extra_composition_eval != extra_comp_bpoly_eval {
        return Err(VerifyError::Invalid(
            "extra composition evaluation mismatch",
        ));
    }

    let mega_ext_root = expect_mroot(&mut stream, "mega extension commitment")?;
    let mut rng = stream.transcript_rng()?;

    let total_constraints = total_constraints(&preprocess.count_map, heights.len(), false)?;
    let comp_weights = belts(&mut rng, (2 * total_constraints) as u32);
    let comp_weight_map =
        build_weight_map(&comp_weights, &preprocess.count_map, heights.len(), false)?;
    let (comp_root, num_comp_pieces) = read_comp_root(&mut stream)?;

    let mut rng = stream.transcript_rng()?;
    let deep_challenge = sample_deep_challenge(&mut rng, &calc)?;

    let trace_evaluations = read_evals(&mut stream)?;
    if trace_evaluations.0.len() != 2 * total_cols {
        return Err(VerifyError::Invalid("trace evaluation length mismatch"));
    }
    ensure_felts_based(
        &trace_evaluations.0, "trace evaluations contain non-based elements",
    )?;

    // V3 rechecks the extra composition polynomial at a challenge sampled
    // only after the trace and composition codewords have been committed.
    // V0-V2 deliberately keep their historical verification semantics.
    if version == ProofVersion::V3 {
        let extra_composition_deep_eval = eval_composition(
            &PolySlice(trace_evaluations.as_slice()),
            &heights,
            &degrees.extra,
            &preprocess.count_map,
            &dyn_slices,
            &extra_weight_map,
            &augmented_slice,
            &deep_challenge,
            &full_widths,
            true,
        )?;
        let extra_comp_bpoly_deep_eval = bpeval_lift_(extra_comp_bpoly.as_slice(), &deep_challenge);
        if extra_composition_deep_eval != extra_comp_bpoly_deep_eval {
            return Err(VerifyError::Invalid(
                "post-commitment extra composition evaluation mismatch",
            ));
        }
    }

    let composition_piece_evaluations = read_evals(&mut stream)?;
    if composition_piece_evaluations.0.len() != num_comp_pieces as usize {
        return Err(VerifyError::Invalid(
            "composition piece evaluation length mismatch",
        ));
    }
    ensure_felts_based(
        &composition_piece_evaluations.0,
        "composition piece evaluations contain non-based elements",
    )?;

    let mut rng = stream.transcript_rng()?;

    let composition_eval = eval_composition(
        &PolySlice(trace_evaluations.as_slice()),
        &heights,
        &degrees.base,
        &preprocess.count_map,
        &dyn_slices,
        &comp_weight_map,
        &augmented_slice,
        &deep_challenge,
        &full_widths,
        false,
    )?;
    let mut decomposition_eval = Felt::zero();
    let mut pow = Felt::one();
    for value in composition_piece_evaluations.0.iter() {
        decomposition_eval = fadd_(&decomposition_eval, &fmul_(&pow, value));
        pow = fmul_(&pow, &deep_challenge);
    }
    if composition_eval != decomposition_eval {
        return Err(VerifyError::Invalid("composition evaluation mismatch"));
    }

    let base_deep_weights_len = trace_evaluations.0.len()
        + extra_trace_evaluations.0.len()
        + composition_piece_evaluations.0.len();
    let deep_weights_len = if version == ProofVersion::V3 {
        base_deep_weights_len
            .checked_add(total_cols)
            .ok_or(VerifyError::Invalid("deep weight count overflow"))?
    } else {
        base_deep_weights_len
    };
    let deep_weights = PolyVec(felts(&mut rng, deep_weights_len as u32));

    let deep_root = expect_mroot(&mut stream, "deep composition commitment")?;
    let fri_output = fri_verify(&calc, &mut stream, deep_root)?;

    let mut merks = fri_output.merks;
    let mut elems = Vec::with_capacity(fri_output.indices.len());
    let axis_depth = depth_for_len(calc.fri.init_domain_len as u64);
    let deep_span = (calc.fri.init_domain_len / calc.fri.folding_deg) as u64;

    for idx in &fri_output.indices {
        let axis = index_to_axis(axis_depth, *idx);

        let ProofPathBf {
            leaf: base_leaf,
            path: base_path,
        } = read_mpathbf(&mut stream)?;
        let ProofPathBf {
            leaf: ext_leaf,
            path: ext_path,
        } = read_mpathbf(&mut stream)?;
        let ProofPathBf {
            leaf: mega_leaf,
            path: mega_path,
        } = read_mpathbf(&mut stream)?;
        let ProofPathBf {
            leaf: comp_leaf,
            path: comp_path,
        } = read_mpathbf(&mut stream)?;

        merks.push(MerkData {
            leaf: hash_bpoly_leaf(&base_leaf),
            axis,
            root: base_root,
            path: base_path,
        });
        merks.push(MerkData {
            leaf: hash_bpoly_leaf(&ext_leaf),
            axis,
            root: ext_root,
            path: ext_path,
        });
        merks.push(MerkData {
            leaf: hash_bpoly_leaf(&mega_leaf),
            axis,
            root: mega_ext_root,
            path: mega_path,
        });
        let comp_leaf_hash = hash_bpoly_leaf(&comp_leaf);
        let comp_elems = comp_leaf.0;
        merks.push(MerkData {
            leaf: comp_leaf_hash,
            axis,
            root: comp_root,
            path: comp_path,
        });

        let trace_elems = assemble_trace_elems(
            &base_leaf, &ext_leaf, &mega_leaf, &base_widths, &ext_widths, &mega_ext_widths,
        )?;
        ensure_belts_based(
            &comp_elems, "composition opening contains non-based elements",
        )?;

        let coset_idx = (*idx % deep_span) as u64;
        let entry = (*idx / deep_span) as usize;
        let coset = fri_output
            .deep_cosets
            .get(&coset_idx)
            .ok_or(VerifyError::Invalid("missing deep coset entry"))?;
        let deep_elem = *coset
            .0
            .get(entry)
            .ok_or(VerifyError::Invalid("deep coset entry out of range"))?;

        elems.push(DeepElem {
            idx: *idx,
            trace_elems,
            comp_elems,
            deep_elem,
        });
    }

    if !verify_merk_proofs(&merks, verifier_eny) {
        return Err(VerifyError::Invalid("failed to verify merkle proofs"));
    }

    let PolyVec(mut all_evals) = trace_evaluations;
    all_evals.extend_from_slice(&extra_trace_evaluations.0);
    let all_evals: FPolyVec = PolyVec(all_evals);
    let omega = Felt::lift(calc.fri.omega);

    for elem in elems {
        let mut deep_eval = crate::form::verifier_math::evaluate_deep(
            &PolySlice(all_evals.as_slice()),
            &PolySlice(composition_piece_evaluations.as_slice()),
            &elem.trace_elems,
            &elem.comp_elems,
            num_comp_pieces,
            &PolySlice(deep_weights.as_slice()),
            &heights,
            &full_widths,
            &omega,
            elem.idx,
            &deep_challenge,
            &extra_comp_eval_point,
        )?;
        if version == ProofVersion::V3 {
            let x = fmul_(&Felt::lift(calc.fri.generator), &fpow_(&omega, elem.idx));
            let degree_eval = evaluate_trace_degree_normalization(
                &elem.trace_elems,
                &deep_weights.0[base_deep_weights_len..],
                &heights,
                &full_widths,
                &x,
            )
            .ok_or(VerifyError::Invalid(
                "trace degree normalization shape mismatch",
            ))?;
            deep_eval = fadd_(&deep_eval, &degree_eval);
        }
        if deep_eval != elem.deep_elem {
            return Err(VerifyError::Invalid("deep evaluation mismatch"));
        }
    }

    Ok(VerifyResult {
        commitment: puzzle.commitment,
        nonce: puzzle.nonce,
    })
}

#[allow(clippy::too_many_arguments)]
fn eval_composition(
    trace_evaluations: &FPolySlice<'_>,
    heights: &[u64],
    processed_degrees: &ProcessedDegrees<'_>,
    counts_map: &CountMap,
    dyn_list: &[BPolySlice<'_>],
    weights_map: &[&[Belt]],
    challenges: &BPolySlice<'_>,
    deep_challenge: &Felt,
    table_full_widths: &[u64],
    is_extra: bool,
) -> Result<Felt, VerifyError> {
    crate::form::verifier_math::eval_composition_poly_with_degrees(
        trace_evaluations, heights, processed_degrees, counts_map, dyn_list, weights_map,
        challenges, deep_challenge, table_full_widths, is_extra,
    )
    .map_err(VerifyError::Jet)
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PuzzleSnapshot {
    commitment: [u64; 5],
    nonce: [u64; 5],
    len: u64,
    leaf: Vec<u64>,
    dyck: Vec<u64>,
}

#[allow(dead_code)]
struct VerifierContext<'a> {
    stream: ProofStream<'a>,
    preprocess: &'static PreprocessData,
    version: ProofVersion,
    puzzle: PuzzleSnapshot,
    table_names: Vec<Term>,
    heights: Vec<u64>,
    base_widths: Vec<u64>,
    full_widths: Vec<u64>,
    calc: StarkCalc,
    base_root: [u64; 5],
    ext_root: [u64; 5],
    challenges: Vec<Belt>,
    challenge_values: Vec<Belt>,
    terminal_lookup: TerminalLookup,
    dyn_list: Vec<Vec<Belt>>,
    tree_subject: TreeSnapshot,
    tree_formula: TreeSnapshot,
    tree_product: TreeSnapshot,
    alf: Felt,
    j: Felt,
    k: Felt,
    l: Felt,
    m: Felt,
    z: Felt,
}

#[derive(Debug, Clone)]
struct DeepElem {
    idx: u64,
    trace_elems: Vec<Belt>,
    comp_elems: Vec<Belt>,
    deep_elem: Felt,
}

fn based_noun(noun: Noun, space: &NounSpace) -> bool {
    let mut stack = vec![noun];
    while let Some(cur) = stack.pop() {
        if cur.is_atom() {
            let atom = match cur.in_space(space).as_atom() {
                Ok(atom) => atom,
                Err(_) => return false,
            };
            let value = match atom.as_u64() {
                Ok(value) => value,
                Err(_) => return false,
            };
            if value >= PRIME {
                return false;
            }
        } else {
            let cell = match cur.in_space(space).as_cell() {
                Ok(cell) => cell,
                Err(_) => return false,
            };
            stack.push(cell.head().noun());
            stack.push(cell.tail().noun());
        }
    }
    true
}

fn t_order_cmp(a: &Term, b: &Term) -> Ordering {
    let a_name = a.as_str();
    let b_name = b.as_str();
    // Keep this legacy branch to mirror historical Hoon t-ordering used by table-override tooling.
    // Jute itself is not active in this native verifier path.
    if b_name == "jute" {
        return Ordering::Less;
    }
    if a_name == "jute" {
        return Ordering::Greater;
    }
    term_atom_cmp(a_name.as_bytes(), b_name.as_bytes())
}

// Hoon t-order compares terms by their @tas atom value (little-endian bytes).
fn term_atom_cmp(a: &[u8], b: &[u8]) -> Ordering {
    let a_len = trim_high_zeros(a);
    let b_len = trim_high_zeros(b);
    if a_len != b_len {
        return a_len.cmp(&b_len);
    }
    for idx in (0..a_len).rev() {
        let cmp = a[idx].cmp(&b[idx]);
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
    Ordering::Equal
}

fn trim_high_zeros(bytes: &[u8]) -> usize {
    let mut len = bytes.len();
    while len > 0 && bytes[len - 1] == 0 {
        len -= 1;
    }
    len
}

fn sort_table_names(names: &mut [Term]) {
    names.sort_by(t_order_cmp);
}

fn read_puzzle(stream: &mut ProofStream<'_>) -> Result<PuzzleSnapshot, VerifyError> {
    match stream.pull()? {
        ProofData::Puzzle {
            com,
            nonce,
            len,
            leaf,
            dyck,
        } => Ok(PuzzleSnapshot {
            commitment: *com,
            nonce: *nonce,
            len: *len,
            leaf: leaf.clone(),
            dyck: dyck.clone(),
        }),
        _ => Err(VerifyError::Invalid(
            "expected puzzle as first proof object",
        )),
    }
}

fn read_heights(stream: &mut ProofStream<'_>) -> Result<Vec<u64>, VerifyError> {
    match stream.pull()? {
        ProofData::Heights(values) => Ok(values.clone()),
        _ => Err(VerifyError::Invalid("expected heights in proof stream")),
    }
}

fn expect_mroot(
    stream: &mut ProofStream<'_>,
    label: &'static str,
) -> Result<[u64; 5], VerifyError> {
    match stream.pull()? {
        ProofData::MRoot { p } => Ok(*p),
        _ => Err(VerifyError::Invalid(label)),
    }
}

fn sample_challenges(stream: &mut ProofStream<'_>, count: u32) -> Result<Vec<Belt>, VerifyError> {
    let mut rng: Tog = stream.transcript_rng()?;
    Ok(belts(&mut rng, count))
}

fn augment_with_puzzle(
    chals: &mut Vec<Belt>,
    subject: Noun,
    space: &NounSpace,
) -> Result<(), VerifyError> {
    augment_challenges(chals, subject, space)?;
    Ok(())
}

fn validate_puzzle_metadata(puzzle: &PuzzleSnapshot) -> Result<(), VerifyError> {
    if puzzle.len == 0 {
        return Err(VerifyError::Invalid("puzzle length must be non-zero"));
    }
    if !puzzle.len.is_power_of_two() {
        return Err(VerifyError::Invalid("puzzle length must be a power of two"));
    }
    if puzzle.len > MAX_PUBLIC_PUZZLE_LEN {
        return Err(VerifyError::Invalid("puzzle length exceeds verifier limit"));
    }
    Ok(())
}

fn validate_heights(heights: &[u64]) -> Result<(), VerifyError> {
    for &height in heights {
        if height == 0 {
            return Err(VerifyError::Invalid("table height must be non-zero"));
        }
        if !height.is_power_of_two() {
            return Err(VerifyError::Invalid("table height must be a power of two"));
        }
        if height > MAX_TRACE_HEIGHT {
            return Err(VerifyError::Invalid("table height exceeds verifier limit"));
        }
    }
    Ok(())
}

fn build_puzzle_subjects(
    stack: &mut NockStack,
    puzzle: &PuzzleSnapshot,
) -> Result<(Noun, Noun), VerifyError> {
    validate_puzzle_metadata(puzzle)?;
    let leaves = sample_puzzle_leaves(&puzzle.commitment, &puzzle.nonce, puzzle.len)?;
    let subject = build_balanced_tree(stack, &leaves)?;
    let formula = build_powork(stack, puzzle.len)?;
    Ok((subject, formula))
}

fn sample_puzzle_leaves(
    commitment: &[u64; 5],
    nonce: &[u64; 5],
    len: u64,
) -> Result<Vec<u64>, VerifyError> {
    let count =
        u32::try_from(len).map_err(|_| VerifyError::Invalid("puzzle length exceeds u32 range"))?;
    let mut sponge = [0u64; STATE_SIZE];
    let mut seed = Vec::with_capacity(10);
    seed.extend_from_slice(commitment);
    seed.extend_from_slice(nonce);
    absorb(&mut sponge, &seed);
    let mut rng = Tog { sponge };
    let belts = belts(&mut rng, count);
    Ok(belts.into_iter().map(|belt| belt.0).collect())
}

fn build_balanced_tree<T: NounAllocator>(
    alloc: &mut T,
    leaves: &[u64],
) -> Result<Noun, VerifyError> {
    if leaves.is_empty() {
        return Err(VerifyError::Invalid("puzzle leaf sequence is empty"));
    }
    if leaves.len() == 1 {
        return Ok(Atom::new(alloc, leaves[0]).as_noun());
    }
    let mid = leaves.len() / 2;
    let left = build_balanced_tree(alloc, &leaves[..mid])?;
    let right = build_balanced_tree(alloc, &leaves[mid..])?;
    Ok(T(alloc, &[left, right]))
}

fn build_powork<T: NounAllocator>(alloc: &mut T, len: u64) -> Result<Noun, VerifyError> {
    if len == 0 {
        return Err(VerifyError::Invalid("puzzle length must be non-zero"));
    }
    let mut form = T(alloc, &[D(1), D(0)]);
    for i in 0..len {
        let hed = len
            .checked_add(i)
            .ok_or(VerifyError::Invalid("puzzle length overflow"))?;
        // Build sub-expressions first to avoid borrow checker issues
        let axis_3_arg = T(alloc, &[D(0), D(hed)]);
        let axis_3 = T(alloc, &[D(3), axis_3_arg]);
        let zero_pair = T(alloc, &[D(0), D(0)]);
        let final_axis = T(alloc, &[D(0), D(hed)]);
        let head = T(alloc, &[D(6), axis_3, zero_pair, final_axis]);
        form = T(alloc, &[head, form]);
    }
    Ok(form)
}

fn build_tree_snapshot(
    noun: Noun,
    alf: &Felt,
    space: &NounSpace,
) -> Result<TreeSnapshot, VerifyError> {
    let data = build_tree_data(noun, alf, space)?;
    Ok(TreeSnapshot::from(data))
}

fn read_terminals(stream: &mut ProofStream<'_>) -> Result<BPolyVec, VerifyError> {
    match stream.pull()? {
        ProofData::Terms(values) => Ok(values.clone()),
        _ => Err(VerifyError::Invalid("expected terminals in proof stream")),
    }
}

fn components_for_table(name: &str) -> Result<&'static [TermComponents], VerifyError> {
    for (table_name, comps) in TERMINAL_TABLES.iter() {
        if *table_name == name {
            return Ok(comps);
        }
    }
    Err(VerifyError::Invalid("unknown table in terminal list"))
}

#[allow(clippy::type_complexity)]
fn decode_terminals(
    table_names: &[Term],
    terminals: &BPolyVec,
) -> Result<(TerminalLookup, Vec<Vec<Belt>>), VerifyError> {
    let mut builder = TerminalLookupBuilder::default();
    let mut dyn_list: Vec<Vec<Belt>> = Vec::with_capacity(table_names.len());
    let mut offset = 0usize;

    for table_name in table_names {
        let components = components_for_table(table_name.as_str())?;
        let mut table_dyn: Vec<Belt> = Vec::with_capacity(components.len() * 3);
        for term in components {
            let belt_a = *terminals
                .0
                .get(offset)
                .ok_or(VerifyError::Invalid("terminal buffer too short"))?;
            let belt_b = *terminals
                .0
                .get(offset + 1)
                .ok_or(VerifyError::Invalid("terminal buffer too short"))?;
            let belt_c = *terminals
                .0
                .get(offset + 2)
                .ok_or(VerifyError::Invalid("terminal buffer too short"))?;
            builder.set_term(term, [belt_a, belt_b, belt_c])?;
            table_dyn.push(belt_a);
            table_dyn.push(belt_b);
            table_dyn.push(belt_c);
            offset += 3;
        }
        dyn_list.push(table_dyn);
    }

    if offset != terminals.0.len() {
        return Err(VerifyError::Invalid("unexpected terminal buffer length"));
    }

    let lookup = builder.finalize()?;
    Ok((lookup, dyn_list))
}

fn felt_from_indices(
    values: &[Belt],
    indices: TermComponentIdx,
    label: &'static str,
) -> Result<Felt, VerifyError> {
    let a = *values.get(indices.a).ok_or(VerifyError::Invalid(label))?;
    let b = *values.get(indices.b).ok_or(VerifyError::Invalid(label))?;
    let c = *values.get(indices.c).ok_or(VerifyError::Invalid(label))?;
    Ok(Felt::from([a, b, c]))
}

#[allow(clippy::too_many_arguments)]
fn verify_linking(
    subject: &TreeSnapshot,
    formula: &TreeSnapshot,
    product: &TreeSnapshot,
    terminal_lookup: &TerminalLookup,
    j: &Felt,
    k: &Felt,
    l: &Felt,
    m: &Felt,
    z: &Felt,
) -> Result<(), VerifyError> {
    let ifp_f = compress_tree(j, k, l, formula);
    let ifp_s = compress_tree(j, k, l, subject);

    let memory_nc_expected = if subject.is_atom { *z } else { fmul_(z, z) };
    let memory_nc = terminal_lookup.memory_nc;
    if memory_nc_expected != memory_nc {
        if std::env::var("VERIFY_DEBUG").is_ok() {
            eprintln!(
                "memory-nc mismatch: expected {:?}, got {:?}, z {:?}, subject_is_atom {}",
                memory_nc_expected, memory_nc, z, subject.is_atom
            );
        }
        return Err(VerifyError::Invalid("memory table node count mismatch"));
    }

    let memory_kvs_expected = if subject.is_atom {
        fmul_(z, &ifp_f)
    } else {
        let left = fmul_(z, &fadd_(&ifp_s, m));
        let z_sq = fmul_(z, z);
        let right = fmul_(&z_sq, &ifp_f);
        fadd_(&left, &right)
    };
    let memory_kvs = terminal_lookup.memory_kvs;
    if memory_kvs_expected != memory_kvs {
        return Err(VerifyError::Invalid("memory table kvs mismatch"));
    }

    let compute_s_size = terminal_lookup.compute_s_size;
    if subject.size != compute_s_size {
        return Err(VerifyError::Invalid("compute subject size mismatch"));
    }
    let compute_s_dyck = terminal_lookup.compute_s_dyck;
    if subject.dyck != compute_s_dyck {
        return Err(VerifyError::Invalid("compute subject dyck mismatch"));
    }
    let compute_s_leaf = terminal_lookup.compute_s_leaf;
    if subject.leaf != compute_s_leaf {
        return Err(VerifyError::Invalid("compute subject leaf mismatch"));
    }

    let compute_f_size = terminal_lookup.compute_f_size;
    if formula.size != compute_f_size {
        return Err(VerifyError::Invalid("compute formula size mismatch"));
    }
    let compute_f_dyck = terminal_lookup.compute_f_dyck;
    if formula.dyck != compute_f_dyck {
        return Err(VerifyError::Invalid("compute formula dyck mismatch"));
    }
    let compute_f_leaf = terminal_lookup.compute_f_leaf;
    if formula.leaf != compute_f_leaf {
        return Err(VerifyError::Invalid("compute formula leaf mismatch"));
    }

    let compute_e_size = terminal_lookup.compute_e_size;
    if product.size != compute_e_size {
        return Err(VerifyError::Invalid("compute product size mismatch"));
    }
    let compute_e_dyck = terminal_lookup.compute_e_dyck;
    if product.dyck != compute_e_dyck {
        return Err(VerifyError::Invalid("compute product dyck mismatch"));
    }
    let compute_e_leaf = terminal_lookup.compute_e_leaf;
    if product.leaf != compute_e_leaf {
        return Err(VerifyError::Invalid("compute product leaf mismatch"));
    }

    let compute_decode = terminal_lookup.compute_decode;
    let memory_decode = terminal_lookup.memory_decode;
    if compute_decode != memory_decode {
        return Err(VerifyError::Invalid("decode multiset mismatch"));
    }

    let compute_op0 = terminal_lookup.compute_op0;
    let memory_op0 = terminal_lookup.memory_op0;
    if compute_op0 != memory_op0 {
        return Err(VerifyError::Invalid("op0 multiset mismatch"));
    }

    Ok(())
}

fn compress_tree(j: &Felt, k: &Felt, l: &Felt, tree: &TreeSnapshot) -> Felt {
    let mut acc = Felt::zero();
    acc = fadd_(&acc, &fmul_(j, &tree.size));
    acc = fadd_(&acc, &fmul_(k, &tree.dyck));
    fadd_(&acc, &fmul_(l, &tree.leaf))
}

fn expected_num_proof_items(calc: &StarkCalc) -> usize {
    let num_rounds = calc.fri.num_rounds();
    let num_spot_checks = calc.fri.num_spot_checks;
    12 + num_rounds + (num_rounds * num_spot_checks) + (4 * num_spot_checks)
}

fn expected_terminal_count(table_names: &[Term]) -> Result<usize, VerifyError> {
    let mut total = 0usize;
    for name in table_names {
        let comps = components_for_table(name.as_str())?;
        let count = comps
            .len()
            .checked_mul(3)
            .ok_or(VerifyError::Invalid("terminal count overflow"))?;
        total = total
            .checked_add(count)
            .ok_or(VerifyError::Invalid("terminal count overflow"))?;
    }
    Ok(total)
}

fn read_poly(stream: &mut ProofStream<'_>) -> Result<BPolyVec, VerifyError> {
    match stream.pull()? {
        ProofData::Poly(values) => Ok(values.clone()),
        _ => Err(VerifyError::Invalid("expected poly in proof stream")),
    }
}

fn read_evals(stream: &mut ProofStream<'_>) -> Result<FPolyVec, VerifyError> {
    match stream.pull()? {
        ProofData::Evals(values) => Ok(values.clone()),
        _ => Err(VerifyError::Invalid("expected evals in proof stream")),
    }
}

fn read_comp_root(stream: &mut ProofStream<'_>) -> Result<([u64; 5], u64), VerifyError> {
    match stream.pull()? {
        ProofData::CompM { p, num } => Ok((*p, *num)),
        _ => Err(VerifyError::Invalid("expected comp-m in proof stream")),
    }
}

fn read_mpathbf(stream: &mut ProofStream<'_>) -> Result<ProofPathBf, VerifyError> {
    match stream.pull()? {
        ProofData::MPathBf(path) => Ok(path.clone()),
        _ => Err(VerifyError::Invalid("expected m-pathbf in proof stream")),
    }
}

fn total_constraints(
    count_map: &CountMap,
    num_tables: usize,
    include_extra: bool,
) -> Result<usize, VerifyError> {
    let mut total = 0usize;
    for i in 0..num_tables {
        let counts = count_map
            .0
            .get(&i)
            .ok_or(VerifyError::Invalid("missing constraint counts"))?;
        let mut table_total = counts.boundary + counts.row + counts.transition + counts.terminal;
        if include_extra {
            table_total += counts.extra;
        }
        total = total
            .checked_add(table_total)
            .ok_or(VerifyError::Invalid("constraint count overflow"))?;
    }
    Ok(total)
}

fn build_weight_map<'a>(
    weights: &'a [Belt],
    count_map: &CountMap,
    num_tables: usize,
    include_extra: bool,
) -> Result<Vec<&'a [Belt]>, VerifyError> {
    let mut map = Vec::with_capacity(num_tables);
    let mut offset = 0usize;
    for i in 0..num_tables {
        let counts = count_map
            .0
            .get(&i)
            .ok_or(VerifyError::Invalid("missing constraint counts"))?;
        let mut num_constraints =
            counts.boundary + counts.row + counts.transition + counts.terminal;
        if include_extra {
            num_constraints += counts.extra;
        }
        let needed = num_constraints
            .checked_mul(2)
            .ok_or(VerifyError::Invalid("weight count overflow"))?;
        let end = offset
            .checked_add(needed)
            .ok_or(VerifyError::Invalid("weight count overflow"))?;
        let slice = weights
            .get(offset..end)
            .ok_or(VerifyError::Invalid("weight buffer too short"))?;
        map.push(slice);
        offset = end;
    }
    if offset != weights.len() {
        return Err(VerifyError::Invalid("weight buffer length mismatch"));
    }
    Ok(map)
}

fn sample_deep_challenge(rng: &mut Tog, calc: &StarkCalc) -> Result<Felt, VerifyError> {
    let n = calc.fri.init_domain_len as u64;
    let exp_offset = Felt::lift(Belt(bpow(calc.fri.generator.0, n)));
    loop {
        let candidate = felt(rng);
        let exp_candidate = fpow_(&candidate, n);
        if exp_candidate != Felt::one() && exp_candidate != exp_offset {
            return Ok(candidate);
        }
    }
}

fn depth_for_len(len: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    len.ilog2() as u64 + 1
}

fn index_to_axis(depth: u64, index: u64) -> u64 {
    (1u64 << depth.saturating_sub(1)) + index
}

fn ensure_belts_based(values: &[Belt], label: &'static str) -> Result<(), VerifyError> {
    if values.iter().all(|belt| belt.0 < PRIME) {
        Ok(())
    } else {
        Err(VerifyError::Invalid(label))
    }
}

fn ensure_digest_based(values: &[u64; 5], label: &'static str) -> Result<(), VerifyError> {
    if values.iter().copied().all(based_check) {
        Ok(())
    } else {
        Err(VerifyError::Invalid(label))
    }
}

fn ensure_felts_based(values: &[Felt], label: &'static str) -> Result<(), VerifyError> {
    if values
        .iter()
        .all(|felt| felt.0.iter().all(|belt| belt.0 < PRIME))
    {
        Ok(())
    } else {
        Err(VerifyError::Invalid(label))
    }
}

fn assemble_trace_elems(
    base: &BPolyVec,
    ext: &BPolyVec,
    mega: &BPolyVec,
    base_widths: &[u64],
    ext_widths: &[u64],
    mega_widths: &[u64],
) -> Result<Vec<Belt>, VerifyError> {
    ensure_belts_based(&base.0, "base opening contains non-based elements")?;
    ensure_belts_based(&ext.0, "extension opening contains non-based elements")?;
    ensure_belts_based(
        &mega.0, "mega extension opening contains non-based elements",
    )?;

    if base_widths.len() != ext_widths.len() || base_widths.len() != mega_widths.len() {
        return Err(VerifyError::Invalid("trace width length mismatch"));
    }

    let base_total: usize = base_widths.iter().map(|w| *w as usize).sum();
    let ext_total: usize = ext_widths.iter().map(|w| *w as usize).sum();
    let mega_total: usize = mega_widths.iter().map(|w| *w as usize).sum();
    if base.0.len() != base_total || ext.0.len() != ext_total || mega.0.len() != mega_total {
        return Err(VerifyError::Invalid("trace opening length mismatch"));
    }

    let mut trace_elems = Vec::with_capacity(base.0.len() + ext.0.len() + mega.0.len());
    let mut base_offset = 0usize;
    let mut ext_offset = 0usize;
    let mut mega_offset = 0usize;

    for i in 0..base_widths.len() {
        let bw = base_widths[i] as usize;
        let ew = ext_widths[i] as usize;
        let mw = mega_widths[i] as usize;

        trace_elems.extend_from_slice(&base.0[base_offset..base_offset + bw]);
        trace_elems.extend_from_slice(&ext.0[ext_offset..ext_offset + ew]);
        trace_elems.extend_from_slice(&mega.0[mega_offset..mega_offset + mw]);

        base_offset += bw;
        ext_offset += ew;
        mega_offset += mw;
    }

    Ok(trace_elems)
}

fn verify_merk_proofs(merks: &[MerkData], eny: u64) -> bool {
    let mut sponge = [0u64; STATE_SIZE];
    let seed = [eny % PRIME];
    absorb(&mut sponge, &seed);
    let mut rng = Tog { sponge };

    let mut tagged: Vec<(Belt, &MerkData)> = Vec::with_capacity(merks.len());
    for merk in merks {
        let rnd = belts(&mut rng, 1);
        tagged.push((rnd[0], merk));
    }

    tagged.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, merk) in tagged {
        if !verify_merk_proof(merk.leaf, merk.axis, merk.root, &merk.path) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use nockvm::ext::NounExt;
    use nockvm::mem::{NockStack, NOCK_STACK_SIZE};
    use noun_serde::NounDecode;

    use super::{verify, VerifyArgs};
    use crate::form::proof::{Proof, ProofData};

    fn decode_proof(bytes: &[u8]) -> Proof {
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, bytes)
            .expect("proof fixture should cue");
        let space = stack.noun_space();
        Proof::from_noun(&noun, &space).expect("proof fixture should decode")
    }

    fn verify_fixture(bytes: &[u8]) {
        let proof = decode_proof(bytes);
        let result = verify(VerifyArgs {
            proof,
            table_override: None,
            verifier_eny: 0,
        })
        .expect("public proof fixture should verify");
        assert!(result
            .commitment
            .iter()
            .all(|value| *value < nockchain_math::belt::PRIME));
        assert!(result
            .nonce
            .iter()
            .all(|value| *value < nockchain_math::belt::PRIME));
    }

    #[test]
    fn verifies_public_proof_fixtures() {
        for bytes in [
            include_bytes!("../../../roswell/tests/fixtures/proof-v0-len1.jam").as_slice(),
            include_bytes!("../../../roswell/tests/fixtures/proof-v1-len1.jam").as_slice(),
            include_bytes!("../../../roswell/tests/fixtures/proof-v2-len1.jam").as_slice(),
            include_bytes!("../../../roswell/tests/fixtures/proof-v3-len1.jam").as_slice(),
        ] {
            verify_fixture(bytes);
        }
    }

    #[test]
    fn rejects_v2_transcript_relabelled_as_v3() {
        let mut proof = decode_proof(include_bytes!(
            "../../../roswell/tests/fixtures/proof-v2-len1.jam"
        ));
        proof.version = crate::form::proof::ProofVersion::V3;
        let result = verify(VerifyArgs {
            proof,
            table_override: None,
            verifier_eny: 0,
        });
        assert!(result.is_err());
    }

    #[test]
    fn rejects_mutated_public_proof_fixture() {
        let mut proof = decode_proof(include_bytes!(
            "../../../roswell/tests/fixtures/proof-v2-len1.jam"
        ));
        match &mut proof.objects[0] {
            ProofData::Puzzle { len, .. } => *len += 1,
            _ => panic!("public proof fixture should start with puzzle metadata"),
        }
        let result = verify(VerifyArgs {
            proof,
            table_override: None,
            verifier_eny: 0,
        });
        assert!(result.is_err());
    }

    #[test]
    fn rejects_oversized_public_puzzle_before_materialization() {
        let mut proof = decode_proof(include_bytes!(
            "../../../roswell/tests/fixtures/proof-v2-len1.jam"
        ));
        match &mut proof.objects[0] {
            ProofData::Puzzle { len, .. } => *len = super::MAX_PUBLIC_PUZZLE_LEN * 2,
            _ => panic!("public proof fixture should start with puzzle metadata"),
        }

        let result = verify(VerifyArgs {
            proof,
            table_override: None,
            verifier_eny: 0,
        });
        assert!(matches!(
            result,
            Err(super::VerifyError::Invalid(
                "puzzle length exceeds verifier limit"
            ))
        ));
    }

    #[test]
    fn rejects_invalid_public_trace_heights_before_fri_sizing() {
        let mut proof = decode_proof(include_bytes!(
            "../../../roswell/tests/fixtures/proof-v2-len1.jam"
        ));
        match &mut proof.objects[1] {
            ProofData::Heights(heights) => heights[0] = super::MAX_TRACE_HEIGHT * 2,
            _ => panic!("public proof fixture should carry heights as second object"),
        }

        let result = verify(VerifyArgs {
            proof,
            table_override: None,
            verifier_eny: 0,
        });
        assert!(matches!(
            result,
            Err(super::VerifyError::Invalid(
                "table height exceeds verifier limit"
            ))
        ));
    }
}
