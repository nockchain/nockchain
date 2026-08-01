use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};

use nockchain_math::noun_ext::NounMathExt;
use nockvm::ext::NounExt;
use nockvm::mem::{NockStack, NOCK_STACK_SIZE};
use nockvm::noun::{Noun, NounSpace};
use noun_serde::{NounDecode, NounDecodeError};

use crate::form::math::prover::{degree_processing, ProcessedDegrees};
use crate::form::proof::{Constraints, ConstraintsSlice, CountMap, ProofVersion};

const CONSTRAINTS_0_1: &[u8] = include_bytes!("../../../../hoon/jams/constraints-0-1.jam");
const CONSTRAINTS_2: &[u8] = include_bytes!("../../../../hoon/jams/constraints-2.jam");

#[derive(Debug)]
pub enum PreprocessError {
    Cue,
    Decode,
    VersionMismatch { expected: u64, found: u64 },
}

impl fmt::Display for PreprocessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreprocessError::Cue => write!(f, "failed to cue constraints jam"),
            PreprocessError::Decode => write!(f, "failed to decode constraints noun"),
            PreprocessError::VersionMismatch { expected, found } => write!(
                f,
                "constraint version mismatch: expected {expected}, found {found}"
            ),
        }
    }
}

impl std::error::Error for PreprocessError {}

impl From<nockvm::interpreter::Error> for PreprocessError {
    fn from(_err: nockvm::interpreter::Error) -> Self {
        PreprocessError::Cue
    }
}

impl From<NounDecodeError> for PreprocessError {
    fn from(_err: NounDecodeError) -> Self {
        PreprocessError::Decode
    }
}

pub struct PreprocessData {
    pub constraints: Constraints,
    pub count_map: CountMap,
}

impl NounDecode for PreprocessData {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let [_degrees, constraints, count_map] = noun.uncell(space)?;
        Ok(Self {
            constraints: Constraints::from_noun(&constraints, space)?,
            count_map: CountMap::from_noun(&count_map, space)?,
        })
    }
}

fn load_preprocess(bytes: &[u8], expected_version: u64) -> Result<PreprocessData, PreprocessError> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let noun = Noun::cue_bytes_slice(&mut stack, bytes)?;
    let space = stack.noun_space();
    let [version, data] = noun.uncell(&space).map_err(|_| PreprocessError::Decode)?;
    let found = version
        .in_space(&space)
        .as_atom()
        .map_err(|_| PreprocessError::Decode)?
        .as_u64()
        .map_err(|_| PreprocessError::Decode)?;
    if found != expected_version {
        return Err(PreprocessError::VersionMismatch {
            expected: expected_version,
            found,
        });
    }
    Ok(PreprocessData::from_noun(&data, &space)?)
}

static PRE_V0_V1: LazyLock<PreprocessData> = LazyLock::new(|| {
    load_preprocess(CONSTRAINTS_0_1, 0).expect("failed to load constraints-0-1.jam")
});
static PRE_V2: LazyLock<PreprocessData> =
    LazyLock::new(|| load_preprocess(CONSTRAINTS_2, 2).expect("failed to load constraints-2.jam"));
static PRE_V0_V1_SLICE: LazyLock<ConstraintsSlice<'static>> =
    LazyLock::new(|| PRE_V0_V1.constraints.to_slice());
static PRE_V2_SLICE: LazyLock<ConstraintsSlice<'static>> =
    LazyLock::new(|| PRE_V2.constraints.to_slice());

struct ProcessedDegreesCache {
    inner: Mutex<HashMap<Vec<u64>, Arc<ProcessedDegreesPair<'static>>>>,
}

impl ProcessedDegreesCache {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn get_or_init(
        &self,
        heights: &[u64],
        constraints: &'static ConstraintsSlice<'static>,
    ) -> Arc<ProcessedDegreesPair<'static>> {
        let guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        if let Some(existing) = guard.get(heights) {
            return Arc::clone(existing);
        }
        drop(guard);

        let heights_vec = heights.to_vec();
        let base = degree_processing(&heights_vec, false, constraints);
        let extra = degree_processing(&heights_vec, true, constraints);
        let computed = Arc::new(ProcessedDegreesPair { base, extra });

        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        if let Some(existing) = guard.get(heights) {
            return Arc::clone(existing);
        }
        guard.insert(heights_vec, Arc::clone(&computed));
        computed
    }
}

static PRE_V0_V1_DEGREES: LazyLock<ProcessedDegreesCache> =
    LazyLock::new(ProcessedDegreesCache::new);
static PRE_V2_DEGREES: LazyLock<ProcessedDegreesCache> = LazyLock::new(ProcessedDegreesCache::new);

pub fn preprocess_for(version: &ProofVersion) -> &'static PreprocessData {
    match version {
        ProofVersion::V0 | ProofVersion::V1 => &PRE_V0_V1,
        ProofVersion::V2 | ProofVersion::V3 => &PRE_V2,
    }
}

pub struct ProcessedDegreesPair<'a> {
    pub base: ProcessedDegrees<'a>,
    pub extra: ProcessedDegrees<'a>,
}

fn constraints_slice_for(version: &ProofVersion) -> &'static ConstraintsSlice<'static> {
    match version {
        ProofVersion::V0 | ProofVersion::V1 => &PRE_V0_V1_SLICE,
        ProofVersion::V2 | ProofVersion::V3 => &PRE_V2_SLICE,
    }
}

fn processed_degrees_cache_for(version: &ProofVersion) -> &'static ProcessedDegreesCache {
    match version {
        ProofVersion::V0 | ProofVersion::V1 => &PRE_V0_V1_DEGREES,
        ProofVersion::V2 | ProofVersion::V3 => &PRE_V2_DEGREES,
    }
}

pub fn preprocess_degrees(
    version: &ProofVersion,
    heights: &[u64],
) -> Arc<ProcessedDegreesPair<'static>> {
    let constraints = constraints_slice_for(version);
    processed_degrees_cache_for(version).get_or_init(heights, constraints)
}
