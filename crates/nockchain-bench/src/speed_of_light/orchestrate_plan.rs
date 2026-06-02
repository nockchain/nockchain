use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::archive::{ArchiveError, ArchiveFilter, SolArchiveReader};
use super::boot_source::{BootSourceError, BootSourceFileRole, BootSourceInput, TrustedBootSource};
use super::final_tip::ExpectedFinalTip;
use super::fixture::{extract_fixture_to_paths, FixtureError, SolFixtureManifest};
use super::peek_bench::{resolve_range, PeekBenchError, PeekRangeRequest, ResolvedPeekRange};
use super::replay_window::{select_replay_window, ReplayWindowOptions};

pub const ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION: &str = "orchestrate-plan/v2";
pub const TRUSTED_PLAN_SCHEMA_VERSION: &str = "trusted-plan/v2";
const MAX_TRUSTED_STEPS: usize = 1_000_000;

#[derive(Debug, Error)]
pub enum OrchestratePlanError {
    #[error("unsupported orchestrate plan schema_version {0:?}")]
    UnsupportedSchemaVersion(Option<String>),
    #[error("plan must contain at least one step")]
    EmptyPlan,
    #[error("{kind} range is empty: start_height {start_height} > end_height {end_height}")]
    EmptyRange {
        kind: &'static str,
        start_height: u64,
        end_height: u64,
    },
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse plan JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to read archive: {0}")]
    Archive(#[from] ArchiveError),
    #[error("failed to extract fixture: {0}")]
    Fixture(#[from] FixtureError),
    #[error("failed to resolve read range: {0}")]
    PeekRange(#[from] PeekBenchError),
    #[error("failed to resolve boot source: {0}")]
    BootSource(#[from] BootSourceError),
    #[error("trusted plan expands to {count} steps, exceeding maximum {max}")]
    TooManySteps { count: usize, max: usize },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrchestratePlanInput {
    #[serde(default)]
    pub schema_version: Option<String>,
    pub boot: BootSourceInput,
    pub kernel: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_final_tip: Option<ExpectedFinalTip>,
    #[serde(default)]
    pub steps: Vec<PlanStepInput>,
}

#[derive(Debug, Clone)]
pub struct GeneratedReplayOptions {
    pub fixture_path: PathBuf,
    pub output_root: PathBuf,
    pub blocks: Option<u64>,
    pub skip_genesis: bool,
}

#[derive(Debug, Clone)]
pub struct GeneratedReplayPlan {
    pub plan_input: OrchestratePlanInput,
    pub manifest: SolFixtureManifest,
    pub checkpoint_path: PathBuf,
    pub archive_path: PathBuf,
    pub kernel_path: PathBuf,
    pub selected_heights: Vec<u64>,
    pub expected_final_tip: Option<ExpectedFinalTip>,
}

#[derive(Debug, Clone)]
pub struct GeneratedReadOptions {
    pub boot: BootSourceInput,
    pub kernel_path: PathBuf,
    pub start_height: u64,
    pub range: PeekRangeRequest,
    pub peek_mode: PeekMode,
    pub tip_height: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadRangeResolution {
    pub requested_start_height: u64,
    pub requested_end_height: Option<u64>,
    pub requested_count: Option<u64>,
    pub resolved_start_height: u64,
    pub resolved_end_height: u64,
    pub resolution_tip_height: u64,
    pub resolution_tip_hash: Option<String>,
    pub start_height: u64,
    pub end_height: u64,
    pub tip_height: u64,
    pub peek_count: u64,
}

#[derive(Debug, Clone)]
pub struct GeneratedReadPlan {
    pub plan_input: OrchestratePlanInput,
    pub read_range_resolution: ReadRangeResolution,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanStepInput {
    PokeArchiveBlock {
        archive: PathBuf,
        height: u64,
        #[serde(default)]
        label: Option<String>,
    },
    PeekHeight {
        height: u64,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        cache_expectation: CacheExpectation,
    },
    ForceCold {
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        cold_target: Option<ColdTarget>,
        #[serde(default)]
        tolerance_pages: Option<u64>,
        #[serde(default)]
        max_attempts: Option<u32>,
    },
    PeekHeightCold {
        height: u64,
        #[serde(default)]
        label: Option<String>,
        #[serde(default = "default_cold_cache_expectation")]
        cache_expectation: CacheExpectation,
        #[serde(default)]
        cold_target: Option<ColdTarget>,
        #[serde(default)]
        tolerance_pages: Option<u64>,
        #[serde(default)]
        max_attempts: Option<u32>,
    },
    PokeArchiveRange {
        archive: PathBuf,
        start_height: u64,
        end_height: u64,
        #[serde(default)]
        label_prefix: Option<String>,
    },
    PeekHeightRange {
        start_height: u64,
        end_height: u64,
        #[serde(default)]
        peek_mode: PeekMode,
        #[serde(default)]
        label_prefix: Option<String>,
        #[serde(default)]
        cache_expectation: Option<CacheExpectation>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CacheExpectation {
    Cold,
    Warm,
    Ambient,
    #[default]
    Unknown,
}

fn default_cold_cache_expectation() -> CacheExpectation {
    CacheExpectation::Cold
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColdTarget {
    PmaReplayNockstack,
}

impl Default for ColdTarget {
    fn default() -> Self {
        Self::PmaReplayNockstack
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PeekMode {
    Warm,
    ColdEach,
}

impl Default for PeekMode {
    fn default() -> Self {
        Self::Warm
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedPlan {
    pub schema_version: String,
    pub boot: TrustedPlanBoot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_final_tip: Option<ExpectedFinalTip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid_reasons: Vec<String>,
    pub inputs: Vec<ResolvedInput>,
    pub steps: Vec<TrustedStep>,
    pub normalized_plan_sha256_hex: String,
    pub step_signature_sha256_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedPlanBoot {
    pub source: TrustedBootSource,
    pub kernel_input_id: String,
    #[serde(default = "default_fsync_enabled")]
    #[serde(
        serialize_with = "serialize_fsync_bool",
        deserialize_with = "deserialize_fsync_bool"
    )]
    pub fsync: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedInput {
    pub input_id: String,
    pub role: InputRole,
    pub absolute_path: PathBuf,
    pub sha256_hex: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    Checkpoint,
    SnapshotPma,
    SnapshotManifest,
    Kernel,
    Archive,
    SourcePlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrustedStep {
    PokeArchiveBlock {
        step_index: usize,
        step_id: String,
        label: String,
        archive_input_id: String,
        height: u64,
    },
    PeekHeight {
        step_index: usize,
        step_id: String,
        label: String,
        height: u64,
        #[serde(default)]
        cache_expectation: CacheExpectation,
    },
    ForceCold {
        step_index: usize,
        step_id: String,
        label: String,
        cold_target: ColdTarget,
        tolerance_pages: Option<u64>,
        max_attempts: Option<u32>,
    },
    PeekHeightCold {
        step_index: usize,
        step_id: String,
        label: String,
        height: u64,
        #[serde(default = "default_cold_cache_expectation")]
        cache_expectation: CacheExpectation,
        cold_target: ColdTarget,
        tolerance_pages: Option<u64>,
        max_attempts: Option<u32>,
    },
}

impl TrustedStep {
    pub fn step_id(&self) -> &str {
        match self {
            Self::PokeArchiveBlock { step_id, .. }
            | Self::PeekHeight { step_id, .. }
            | Self::ForceCold { step_id, .. }
            | Self::PeekHeightCold { step_id, .. } => step_id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::PokeArchiveBlock { label, .. }
            | Self::PeekHeight { label, .. }
            | Self::ForceCold { label, .. }
            | Self::PeekHeightCold { label, .. } => label,
        }
    }
}

pub fn load_plan_input(path: &Path) -> Result<OrchestratePlanInput, OrchestratePlanError> {
    let bytes = std::fs::read(path).map_err(|source| OrchestratePlanError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn normalize_plan(input: OrchestratePlanInput) -> Result<TrustedPlan, OrchestratePlanError> {
    if !matches!(
        input.schema_version.as_deref(),
        None | Some(ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION)
    ) {
        return Err(OrchestratePlanError::UnsupportedSchemaVersion(
            input.schema_version,
        ));
    }

    let resolved_boot = input.boot.resolve()?;
    let trusted_source = resolved_boot.trusted_source();

    let mut inventory = InputInventory::default();
    for (input_id, role, path) in resolved_boot.input_paths() {
        let inserted = inventory.insert(boot_file_role_to_input_role(role), path)?;
        debug_assert_eq!(inserted, input_id);
    }
    let kernel_input_id = inventory.insert(InputRole::Kernel, input.kernel)?;
    let expected_final_tip = input.expected_final_tip;
    let mut steps = Vec::new();

    for step in input.steps {
        expand_step(step, &mut inventory, &mut steps)?;
    }

    if steps.is_empty() {
        return Err(OrchestratePlanError::EmptyPlan);
    }
    if steps.len() > MAX_TRUSTED_STEPS {
        return Err(OrchestratePlanError::TooManySteps {
            count: steps.len(),
            max: MAX_TRUSTED_STEPS,
        });
    }

    let mut plan = TrustedPlan {
        schema_version: TRUSTED_PLAN_SCHEMA_VERSION.to_string(),
        boot: TrustedPlanBoot {
            source: trusted_source,
            kernel_input_id,
            fsync: default_fsync_enabled(),
        },
        expected_final_tip,
        invalid_reasons: Vec::new(),
        inputs: inventory.into_inputs()?,
        steps,
        normalized_plan_sha256_hex: String::new(),
        step_signature_sha256_hex: String::new(),
    };
    refresh_plan_hashes(&mut plan)?;
    Ok(plan)
}

pub fn refresh_plan_hashes(plan: &mut TrustedPlan) -> Result<(), OrchestratePlanError> {
    plan.normalized_plan_sha256_hex = String::new();
    plan.step_signature_sha256_hex = String::new();
    plan.normalized_plan_sha256_hex = sha256_hex(&canonical_json_bytes(plan)?);
    plan.step_signature_sha256_hex = sha256_hex(&step_signature_bytes(plan)?);
    Ok(())
}

pub fn build_generated_replay_plan(
    options: &GeneratedReplayOptions,
) -> Result<GeneratedReplayPlan, OrchestratePlanError> {
    let extracted_dir = options.output_root.join("input/extracted/fixture-0");
    std::fs::create_dir_all(&extracted_dir).map_err(|source| OrchestratePlanError::Io {
        path: extracted_dir.clone(),
        source,
    })?;
    let checkpoint_path = extracted_dir.join("checkpoint.chkjam");
    let archive_path = extracted_dir.join("archive.solarch");
    let kernel_path = extracted_dir.join("kernel.jam");
    let manifest = extract_fixture_to_paths(
        &options.fixture_path, &checkpoint_path, &archive_path, &kernel_path,
    )?;

    let archive = SolArchiveReader::from_file(&archive_path)?;
    let replay_window = select_replay_window(
        &archive,
        ReplayWindowOptions {
            filter: ArchiveFilter::default(),
            skip_genesis: options.skip_genesis,
            block_limit: options.blocks.filter(|blocks| *blocks > 0),
        },
    )?;
    if replay_window.blocks.is_empty() {
        return Err(OrchestratePlanError::EmptyPlan);
    }
    let heights: Vec<u64> = replay_window
        .blocks
        .iter()
        .map(|entry| entry.height.as_u64())
        .collect();

    let steps = heights
        .iter()
        .copied()
        .map(|height| PlanStepInput::PokeArchiveBlock {
            archive: archive_path.clone(),
            height,
            label: None,
        })
        .collect();

    Ok(GeneratedReplayPlan {
        plan_input: OrchestratePlanInput {
            schema_version: Some(ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION.to_string()),
            boot: BootSourceInput::Checkpoint {
                checkpoint: checkpoint_path.clone(),
            },
            kernel: kernel_path.clone(),
            expected_final_tip: replay_window.expected_final_tip.clone(),
            steps,
        },
        manifest,
        checkpoint_path,
        archive_path,
        kernel_path,
        selected_heights: heights,
        expected_final_tip: replay_window.expected_final_tip,
    })
}

pub fn build_generated_read_plan(
    options: &GeneratedReadOptions,
) -> Result<GeneratedReadPlan, OrchestratePlanError> {
    let range = resolve_range(options.start_height, options.range, options.tip_height)?;
    let steps = match options.peek_mode {
        PeekMode::Warm => vec![PlanStepInput::PeekHeightRange {
            start_height: range.start_height,
            end_height: range.end_height,
            peek_mode: PeekMode::Warm,
            label_prefix: None,
            cache_expectation: Some(CacheExpectation::Warm),
        }],
        PeekMode::ColdEach => vec![PlanStepInput::PeekHeightRange {
            start_height: range.start_height,
            end_height: range.end_height,
            peek_mode: PeekMode::ColdEach,
            label_prefix: None,
            cache_expectation: Some(CacheExpectation::Cold),
        }],
    };

    Ok(GeneratedReadPlan {
        plan_input: OrchestratePlanInput {
            schema_version: Some(ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION.to_string()),
            boot: options.boot.clone(),
            kernel: options.kernel_path.clone(),
            expected_final_tip: None,
            steps,
        },
        read_range_resolution: ReadRangeResolution::from_request(options, range),
    })
}

pub fn step_signature_bytes(plan: &TrustedPlan) -> Result<Vec<u8>, OrchestratePlanError> {
    let mut output = Vec::new();
    for step in &plan.steps {
        serde_json::to_writer(&mut output, &step_signature_value(step)?)?;
        output.write_all(b"\n").expect("write vec cannot fail");
    }
    Ok(output)
}

fn expand_step(
    step: PlanStepInput,
    inventory: &mut InputInventory,
    steps: &mut Vec<TrustedStep>,
) -> Result<(), OrchestratePlanError> {
    match step {
        PlanStepInput::PokeArchiveBlock {
            archive,
            height,
            label,
        } => {
            let archive_input_id = inventory.insert(InputRole::Archive, archive)?;
            push_poke_step(steps, archive_input_id, height, label);
        }
        PlanStepInput::PeekHeight {
            height,
            label,
            cache_expectation,
        } => push_peek_step(steps, height, label, cache_expectation),
        PlanStepInput::ForceCold {
            label,
            cold_target,
            tolerance_pages,
            max_attempts,
        } => push_force_cold_step(
            steps,
            label,
            cold_target.unwrap_or_default(),
            tolerance_pages,
            max_attempts,
        ),
        PlanStepInput::PeekHeightCold {
            height,
            label,
            cache_expectation,
            cold_target,
            tolerance_pages,
            max_attempts,
        } => push_cold_peek_step(
            steps,
            height,
            label,
            cache_expectation,
            cold_target.unwrap_or_default(),
            tolerance_pages,
            max_attempts,
        ),
        PlanStepInput::PokeArchiveRange {
            archive,
            start_height,
            end_height,
            label_prefix,
        } => {
            validate_range("poke_archive", start_height, end_height)?;
            let archive_input_id = inventory.insert(InputRole::Archive, archive)?;
            for height in start_height..=end_height {
                let label = label_prefix
                    .as_ref()
                    .map(|prefix| format!("{prefix}-{height}"));
                push_poke_step(steps, archive_input_id.clone(), height, label);
            }
        }
        PlanStepInput::PeekHeightRange {
            start_height,
            end_height,
            peek_mode,
            label_prefix,
            cache_expectation,
        } => {
            validate_range("peek_height", start_height, end_height)?;
            for height in start_height..=end_height {
                let label = label_prefix
                    .as_ref()
                    .map(|prefix| format!("{prefix}-{height}"));
                match peek_mode {
                    PeekMode::Warm => push_peek_step(
                        steps,
                        height,
                        label,
                        cache_expectation.unwrap_or(CacheExpectation::Warm),
                    ),
                    PeekMode::ColdEach => push_cold_peek_step(
                        steps,
                        height,
                        label,
                        cache_expectation.unwrap_or(CacheExpectation::Cold),
                        ColdTarget::default(),
                        None,
                        None,
                    ),
                }
            }
        }
    }
    Ok(())
}

fn validate_range(
    kind: &'static str,
    start_height: u64,
    end_height: u64,
) -> Result<(), OrchestratePlanError> {
    if start_height > end_height {
        return Err(OrchestratePlanError::EmptyRange {
            kind,
            start_height,
            end_height,
        });
    }
    Ok(())
}

fn default_fsync_enabled() -> bool {
    super::harness::default_fsync_enabled()
}

fn serialize_fsync_bool<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(super::harness::fsync_mode_label(*value))
}

fn deserialize_fsync_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(serde::de::Error::custom("fsync must be \"on\" or \"off\"")),
    }
}

fn push_poke_step(
    steps: &mut Vec<TrustedStep>,
    archive_input_id: String,
    height: u64,
    label: Option<String>,
) {
    let step_index = steps.len();
    let label = label.unwrap_or_else(|| format!("step-{step_index:04}"));
    steps.push(TrustedStep::PokeArchiveBlock {
        step_index,
        step_id: step_id_from_label(step_index, &label),
        label,
        archive_input_id,
        height,
    });
}

fn push_peek_step(
    steps: &mut Vec<TrustedStep>,
    height: u64,
    label: Option<String>,
    cache_expectation: CacheExpectation,
) {
    let step_index = steps.len();
    let label = label.unwrap_or_else(|| format!("step-{step_index:04}"));
    steps.push(TrustedStep::PeekHeight {
        step_index,
        step_id: step_id_from_label(step_index, &label),
        label,
        height,
        cache_expectation,
    });
}

fn push_force_cold_step(
    steps: &mut Vec<TrustedStep>,
    label: Option<String>,
    cold_target: ColdTarget,
    tolerance_pages: Option<u64>,
    max_attempts: Option<u32>,
) {
    let step_index = steps.len();
    let label = label.unwrap_or_else(|| format!("step-{step_index:04}"));
    steps.push(TrustedStep::ForceCold {
        step_index,
        step_id: step_id_from_label(step_index, &label),
        label,
        cold_target,
        tolerance_pages,
        max_attempts,
    });
}

fn push_cold_peek_step(
    steps: &mut Vec<TrustedStep>,
    height: u64,
    label: Option<String>,
    cache_expectation: CacheExpectation,
    cold_target: ColdTarget,
    tolerance_pages: Option<u64>,
    max_attempts: Option<u32>,
) {
    let step_index = steps.len();
    let label = label.unwrap_or_else(|| format!("step-{step_index:04}"));
    steps.push(TrustedStep::PeekHeightCold {
        step_index,
        step_id: step_id_from_label(step_index, &label),
        label,
        height,
        cache_expectation,
        cold_target,
        tolerance_pages,
        max_attempts,
    });
}

#[derive(Default)]
struct InputInventory {
    by_key: BTreeMap<(InputRole, PathBuf), String>,
    next_by_role: BTreeMap<InputRole, usize>,
}

impl InputInventory {
    fn insert(&mut self, role: InputRole, path: PathBuf) -> Result<String, OrchestratePlanError> {
        let absolute_path = canonicalize_input_path(&path)?;
        if let Some(input_id) = self.by_key.get(&(role, absolute_path.clone())) {
            return Ok(input_id.clone());
        }

        let next = self.next_by_role.entry(role).or_default();
        let input_id = format!("{}-{next}", input_role_prefix(role));
        *next += 1;
        self.by_key.insert((role, absolute_path), input_id.clone());
        Ok(input_id)
    }

    fn into_inputs(self) -> Result<Vec<ResolvedInput>, OrchestratePlanError> {
        let mut inputs: Vec<_> = self
            .by_key
            .into_iter()
            .map(|((role, absolute_path), input_id)| {
                let (sha256_hex, size_bytes) = hash_file_or_path_bytes(&absolute_path)?;
                Ok(ResolvedInput {
                    input_id,
                    role,
                    absolute_path,
                    sha256_hex,
                    size_bytes,
                    container_path: None,
                })
            })
            .collect::<Result<_, OrchestratePlanError>>()?;
        inputs.sort_by(|left: &ResolvedInput, right| left.input_id.cmp(&right.input_id));
        Ok(inputs)
    }
}

fn input_role_prefix(role: InputRole) -> &'static str {
    match role {
        InputRole::Checkpoint => "checkpoint",
        InputRole::SnapshotPma => "snapshot-pma",
        InputRole::SnapshotManifest => "snapshot-manifest",
        InputRole::Kernel => "kernel",
        InputRole::Archive => "archive",
        InputRole::SourcePlan => "source-plan",
    }
}

fn boot_file_role_to_input_role(role: BootSourceFileRole) -> InputRole {
    match role {
        BootSourceFileRole::Checkpoint => InputRole::Checkpoint,
        BootSourceFileRole::SnapshotPma => InputRole::SnapshotPma,
        BootSourceFileRole::SnapshotManifest => InputRole::SnapshotManifest,
    }
}

fn hash_file_or_path_bytes(path: &Path) -> Result<(String, u64), OrchestratePlanError> {
    let mut file = File::open(path).map_err(|source| OrchestratePlanError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let size_bytes = file
        .metadata()
        .map_err(|source| OrchestratePlanError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| OrchestratePlanError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((hex_string(&hasher.finalize()), size_bytes))
}

fn canonicalize_input_path(path: &Path) -> Result<PathBuf, OrchestratePlanError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    absolute
        .canonicalize()
        .map_err(|source| OrchestratePlanError::Io {
            path: absolute,
            source,
        })
}

fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, OrchestratePlanError> {
    let value = serde_json::to_value(value)?;
    Ok(serde_json::to_vec(&value)?)
}

fn step_signature_value(step: &TrustedStep) -> Result<serde_json::Value, OrchestratePlanError> {
    let value = match step {
        TrustedStep::PokeArchiveBlock {
            step_index,
            archive_input_id,
            height,
            ..
        } => serde_json::json!({
            "archive_input_id": archive_input_id,
            "height": height,
            "step_index": step_index,
            "type": "poke_archive_block"
        }),
        TrustedStep::PeekHeight {
            step_index,
            height,
            cache_expectation,
            ..
        } => serde_json::json!({
            "cache_expectation": cache_expectation,
            "height": height,
            "step_index": step_index,
            "type": "peek_height"
        }),
        TrustedStep::ForceCold {
            step_index,
            cold_target,
            tolerance_pages,
            max_attempts,
            ..
        } => serde_json::json!({
            "cold_target": cold_target,
            "max_attempts": max_attempts,
            "step_index": step_index,
            "tolerance_pages": tolerance_pages,
            "type": "force_cold"
        }),
        TrustedStep::PeekHeightCold {
            step_index,
            height,
            cache_expectation,
            cold_target,
            tolerance_pages,
            max_attempts,
            ..
        } => serde_json::json!({
            "cache_expectation": cache_expectation,
            "cold_target": cold_target,
            "height": height,
            "max_attempts": max_attempts,
            "step_index": step_index,
            "tolerance_pages": tolerance_pages,
            "type": "peek_height_cold"
        }),
    };
    Ok(value)
}

fn step_id_from_label(step_index: usize, label: &str) -> String {
    let slug = slugify_label(label);
    if slug == format!("step-{step_index:04}") {
        slug
    } else {
        format!("step-{step_index:04}-{slug}")
    }
}

fn slugify_label(label: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in label.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "step".to_string()
    } else {
        slug
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_string(&hasher.finalize())
}

fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl From<ResolvedPeekRange> for ReadRangeResolution {
    fn from(range: ResolvedPeekRange) -> Self {
        Self {
            requested_start_height: range.start_height,
            requested_end_height: Some(range.end_height),
            requested_count: None,
            resolved_start_height: range.start_height,
            resolved_end_height: range.end_height,
            resolution_tip_height: range.tip_height,
            resolution_tip_hash: None,
            start_height: range.start_height,
            end_height: range.end_height,
            tip_height: range.tip_height,
            peek_count: range.end_height.saturating_sub(range.start_height) + 1,
        }
    }
}

impl ReadRangeResolution {
    fn from_request(options: &GeneratedReadOptions, range: ResolvedPeekRange) -> Self {
        let (requested_end_height, requested_count) = match options.range {
            PeekRangeRequest::EndHeight(end_height) => (Some(end_height), None),
            PeekRangeRequest::Count(count) => (None, Some(count)),
            PeekRangeRequest::ToTip => (None, None),
        };
        Self {
            requested_start_height: options.start_height,
            requested_end_height,
            requested_count,
            resolved_start_height: range.start_height,
            resolved_end_height: range.end_height,
            resolution_tip_height: range.tip_height,
            resolution_tip_hash: None,
            start_height: range.start_height,
            end_height: range.end_height,
            tip_height: range.tip_height,
            peek_count: range.end_height.saturating_sub(range.start_height) + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use nockapp::nockapp::save::JammedCheckpointV2;
    use nockapp::JammedNoun;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::speed_of_light::fixture::{
        write_fixture_file_from_paths, SolFixtureCheckpointKind, SolFixtureManifest,
    };
    use crate::speed_of_light::{ProofVersion, SolArchiveWriter, SolHeight};

    const REFERENCE_SNAPSHOT_DIR: &str =
        "/shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool";

    fn checkpoint_boot(path: &str) -> serde_json::Value {
        json!({ "type": "checkpoint", "checkpoint": path })
    }

    fn write_checkpoint(path: &Path, event_num: u64) {
        let checkpoint = JammedCheckpointV2::new(
            blake3::hash(b"kernel"),
            event_num,
            JammedNoun::new(Bytes::from_static(b"cold")),
            JammedNoun::new(Bytes::from_static(b"state")),
        );
        std::fs::write(path, checkpoint.encode().expect("encode checkpoint"))
            .expect("write checkpoint");
    }

    fn normalize(value: serde_json::Value) -> Result<TrustedPlan, OrchestratePlanError> {
        let tempdir = tempdir().expect("tempdir");
        let value = materialize_paths(value, tempdir.path());
        normalize_plan(serde_json::from_value(value).expect("valid input shape"))
    }

    fn materialize_paths(mut value: serde_json::Value, root: &Path) -> serde_json::Value {
        match &mut value {
            serde_json::Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    if matches!(
                        key.as_str(),
                        "checkpoint" | "kernel" | "archive" | "pma" | "manifest"
                    ) {
                        if let Some(path) = child.as_str() {
                            let materialized = root.join(path);
                            if let Some(parent) = materialized.parent() {
                                std::fs::create_dir_all(parent).expect("create parent");
                            }
                            if key == "checkpoint" {
                                write_checkpoint(&materialized, 0);
                            } else {
                                std::fs::write(&materialized, path.as_bytes())
                                    .expect("write input");
                            }
                            *child = serde_json::Value::String(
                                materialized.to_string_lossy().to_string(),
                            );
                        }
                    } else {
                        *child = materialize_paths(child.take(), root);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    *child = materialize_paths(child.take(), root);
                }
            }
            _ => {}
        }
        value
    }

    #[test]
    fn orchestrate_plan_accepts_missing_and_orchestrate_schema_version() {
        let missing = normalize(json!({
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [{ "type": "peek_height", "height": 7 }]
        }))
        .expect("missing schema accepted");
        assert_eq!(missing.schema_version, TRUSTED_PLAN_SCHEMA_VERSION);

        let current = normalize(json!({
            "schema_version": "orchestrate-plan/v2",
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [{ "type": "peek_height", "height": 7 }]
        }))
        .expect("orchestrate input schema accepted");
        assert_eq!(current.schema_version, TRUSTED_PLAN_SCHEMA_VERSION);
    }

    #[test]
    fn trusted_plan_boot_serializes_fsync_as_string_enum() {
        let plan = normalize(json!({
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [{ "type": "peek_height", "height": 7 }]
        }))
        .expect("normalize plan");

        let value = serde_json::to_value(plan).expect("trusted plan json");
        assert_eq!(value["boot"]["fsync"], json!("on"));
    }

    #[test]
    fn orchestrate_plan_normalizes_snapshot_boot_source_inputs() {
        let snapshot_dir = PathBuf::from(REFERENCE_SNAPSHOT_DIR);
        let plan = normalize_plan(OrchestratePlanInput {
            schema_version: Some(ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION.to_string()),
            boot: BootSourceInput::Snapshot {
                pma: snapshot_dir.join("snapshot.pma"),
                manifest: snapshot_dir.join("snapshot.manifest"),
            },
            kernel: snapshot_dir.join("kernel.jam"),
            expected_final_tip: None,
            steps: vec![PlanStepInput::PeekHeight {
                height: 0,
                label: None,
                cache_expectation: CacheExpectation::Warm,
            }],
        })
        .expect("normalize snapshot plan");

        assert!(matches!(
            plan.boot.source,
            TrustedBootSource::Snapshot {
                ref pma_input_id,
                ref manifest_input_id,
                event_num: 5
            } if pma_input_id == "snapshot-pma-0"
                && manifest_input_id == "snapshot-manifest-0"
        ));
        let ids: Vec<_> = plan
            .inputs
            .iter()
            .map(|input| (input.input_id.as_str(), input.role))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("kernel-0", InputRole::Kernel),
                ("snapshot-manifest-0", InputRole::SnapshotManifest),
                ("snapshot-pma-0", InputRole::SnapshotPma),
            ]
        );
    }

    #[test]
    fn orchestrate_plan_rejects_unknown_schema_version() {
        let err = normalize(json!({
            "schema_version": "trusted-plan/v2",
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [{ "type": "peek_height", "height": 7 }]
        }))
        .expect_err("unknown schema rejected");

        assert!(matches!(
            err,
            OrchestratePlanError::UnsupportedSchemaVersion(Some(_))
        ));
    }

    #[test]
    fn orchestrate_plan_expands_ranges_and_assigns_defaults() {
        let plan = normalize(json!({
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [
                {
                    "type": "poke_archive_range",
                    "archive": "archive.solarch",
                    "start_height": 10,
                    "end_height": 11,
                    "label_prefix": "poke"
                },
                {
                    "type": "peek_height_range",
                    "start_height": 20,
                    "end_height": 21,
                    "peek_mode": "cold-each"
                },
                { "type": "force_cold" }
            ]
        }))
        .expect("normalize plan");

        let ids: Vec<_> = plan.steps.iter().map(TrustedStep::step_id).collect();
        assert_eq!(
            ids,
            vec!["step-0000-poke-10", "step-0001-poke-11", "step-0002", "step-0003", "step-0004",]
        );
        assert_eq!(plan.steps[0].label(), "poke-10");
        assert_eq!(plan.steps[2].label(), "step-0002");
        assert_eq!(plan.steps[4].label(), "step-0004");
        assert!(matches!(
            plan.steps[2],
            TrustedStep::PeekHeightCold {
                cold_target: ColdTarget::PmaReplayNockstack,
                ..
            }
        ));
    }

    #[test]
    fn orchestrate_plan_assigns_deterministic_input_ids() {
        let plan = normalize(json!({
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [
                { "type": "poke_archive_block", "archive": "archive-a.solarch", "height": 1 },
                { "type": "poke_archive_block", "archive": "archive-b.solarch", "height": 2 },
                { "type": "poke_archive_block", "archive": "archive-a.solarch", "height": 3 }
            ]
        }))
        .expect("normalize plan");

        let ids: Vec<_> = plan
            .inputs
            .iter()
            .map(|input| (input.input_id.as_str(), input.role))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("archive-0", InputRole::Archive),
                ("archive-1", InputRole::Archive),
                ("checkpoint-0", InputRole::Checkpoint),
                ("kernel-0", InputRole::Kernel),
            ]
        );

        match (&plan.steps[0], &plan.steps[2]) {
            (
                TrustedStep::PokeArchiveBlock {
                    archive_input_id: first,
                    ..
                },
                TrustedStep::PokeArchiveBlock {
                    archive_input_id: third,
                    ..
                },
            ) => assert_eq!(first, third),
            other => panic!("expected poke steps, got {other:?}"),
        }
    }

    #[test]
    fn orchestrate_plan_signature_ignores_labels_and_host_paths() {
        let first = normalize(json!({
            "boot": checkpoint_boot("a/checkpoint.chkjam"),
            "kernel": "a/kernel.jam",
            "steps": [{ "type": "peek_height", "height": 7, "label": "first" }]
        }))
        .expect("first plan");
        let second = normalize(json!({
            "boot": checkpoint_boot("b/checkpoint.chkjam"),
            "kernel": "b/kernel.jam",
            "steps": [{ "type": "peek_height", "height": 7, "label": "second" }]
        }))
        .expect("second plan");
        let changed = normalize(json!({
            "boot": checkpoint_boot("b/checkpoint.chkjam"),
            "kernel": "b/kernel.jam",
            "steps": [{ "type": "peek_height", "height": 8, "label": "second" }]
        }))
        .expect("changed plan");

        assert_eq!(
            first.step_signature_sha256_hex,
            second.step_signature_sha256_hex
        );
        assert_ne!(
            first.step_signature_sha256_hex,
            changed.step_signature_sha256_hex
        );
    }

    #[test]
    fn orchestrate_plan_step_signature_bytes_are_canonical_ndjson() {
        let plan = normalize(json!({
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [
                { "type": "poke_archive_block", "archive": "archive.solarch", "height": 11, "label": "ignored" },
                { "type": "peek_height", "height": 12 }
            ]
        }))
        .expect("normalize plan");

        let bytes = step_signature_bytes(&plan).expect("signature bytes");
        let expected = concat!(
            "{\"archive_input_id\":\"archive-0\",\"height\":11,\"step_index\":0,\"type\":\"poke_archive_block\"}\n",
            "{\"cache_expectation\":\"unknown\",\"height\":12,\"step_index\":1,\"type\":\"peek_height\"}\n",
        )
        .as_bytes()
        .to_vec();

        assert_eq!(bytes, expected);
        assert!(!bytes.ends_with(b"\n\n"));
    }

    fn write_archive(path: &Path, heights: &[u64]) {
        let mut writer = SolArchiveWriter::new();
        for height in heights {
            writer
                .add_block_with_tx_count_for_test(
                    SolHeight(*height),
                    Hash([Belt(0), Belt(0), Belt(0), Belt(0), Belt(*height)]),
                    0,
                    ProofVersion::V0,
                    &[1, 2, 3],
                )
                .expect("add block");
        }
        writer.write_to_file(path).expect("write archive");
    }

    fn write_fixture(dir: &Path, heights: &[u64]) -> PathBuf {
        let checkpoint_path = dir.join("source.chkjam");
        let archive_path = dir.join("source.solarch");
        let kernel_path = dir.join("source.jam");
        let fixture_path = dir.join("source.soltest");
        write_checkpoint(&checkpoint_path, 0);
        std::fs::write(&kernel_path, b"kernel").expect("kernel");
        write_archive(&archive_path, heights);
        let manifest = SolFixtureManifest {
            source_archive_path: archive_path.display().to_string(),
            source_archive_event_num: Some(100),
            checkpoint_kind: SolFixtureCheckpointKind::Derived,
            checkpoint_height: SolHeight(0),
            checkpoint_event_num: 0,
            archive_start_height: SolHeight(*heights.first().unwrap_or(&0)),
            archive_end_height: SolHeight(*heights.last().unwrap_or(&0)),
            include_mempool: false,
            chunk_size: 8,
            kernel_hash_hex: "k".repeat(64),
            checkpoint_hash_hex: "c".repeat(64),
            archive_hash_hex: "a".repeat(64),
        };
        write_fixture_file_from_paths(
            &fixture_path, &manifest, &checkpoint_path, &archive_path, &kernel_path,
        )
        .expect("write fixture");
        fixture_path
    }

    fn replay_options(fixture_path: PathBuf, output_root: PathBuf) -> GeneratedReplayOptions {
        GeneratedReplayOptions {
            fixture_path,
            output_root,
            blocks: None,
            skip_genesis: false,
        }
    }

    #[test]
    fn generated_replay_omitted_and_zero_blocks_select_all_after_skip_genesis() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let fixture_path = write_fixture(temp_dir.path(), &[0, 1, 2]);

        let mut omitted = replay_options(fixture_path.clone(), temp_dir.path().join("omitted"));
        omitted.skip_genesis = true;
        let omitted = build_generated_replay_plan(&omitted).expect("omitted blocks");
        assert_eq!(omitted.selected_heights, vec![1, 2]);

        let mut zero = replay_options(fixture_path, temp_dir.path().join("zero"));
        zero.blocks = Some(0);
        zero.skip_genesis = true;
        let zero = build_generated_replay_plan(&zero).expect("zero blocks");
        assert_eq!(zero.selected_heights, vec![1, 2]);
    }

    #[test]
    fn generated_replay_positive_blocks_selects_prefix_and_can_be_empty() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let fixture_path = write_fixture(temp_dir.path(), &[0, 1, 2]);

        let mut prefix = replay_options(fixture_path.clone(), temp_dir.path().join("prefix"));
        prefix.blocks = Some(2);
        let prefix = build_generated_replay_plan(&prefix).expect("prefix");
        assert_eq!(prefix.selected_heights, vec![0, 1]);

        let genesis_only_fixture = write_fixture(temp_dir.path(), &[0]);
        let mut empty = replay_options(genesis_only_fixture, temp_dir.path().join("empty"));
        empty.blocks = Some(0);
        empty.skip_genesis = true;
        let error = build_generated_replay_plan(&empty).expect_err("empty selection rejected");
        assert!(matches!(error, OrchestratePlanError::EmptyPlan));
    }

    #[test]
    fn generated_read_rejects_invalid_ranges_and_records_resolution() {
        let options = GeneratedReadOptions {
            boot: BootSourceInput::Checkpoint {
                checkpoint: PathBuf::from("checkpoint.chkjam"),
            },
            kernel_path: PathBuf::from("kernel.jam"),
            start_height: 8,
            range: PeekRangeRequest::Count(4),
            peek_mode: PeekMode::Warm,
            tip_height: 10,
        };
        assert!(matches!(
            build_generated_read_plan(&options),
            Err(OrchestratePlanError::PeekRange(
                PeekBenchError::CountPastTip { .. }
            ))
        ));

        let options = GeneratedReadOptions {
            start_height: 3,
            range: PeekRangeRequest::Count(4),
            ..options
        };
        let generated = build_generated_read_plan(&options).expect("read plan");
        assert_eq!(
            generated.read_range_resolution,
            ReadRangeResolution {
                requested_start_height: 3,
                requested_end_height: None,
                requested_count: Some(4),
                resolved_start_height: 3,
                resolved_end_height: 6,
                resolution_tip_height: 10,
                resolution_tip_hash: None,
                start_height: 3,
                end_height: 6,
                tip_height: 10,
                peek_count: 4,
            }
        );
        let trusted = normalize_generated_test_plan(generated.plan_input).expect("trusted plan");
        assert_eq!(trusted.steps.len(), 4);
        assert!(matches!(
            trusted.steps[0],
            TrustedStep::PeekHeight {
                cache_expectation: CacheExpectation::Warm,
                ..
            }
        ));
    }

    #[test]
    fn generated_read_expands_warm_and_cold_modes() {
        let warm = build_generated_read_plan(&GeneratedReadOptions {
            boot: BootSourceInput::Checkpoint {
                checkpoint: PathBuf::from("checkpoint.chkjam"),
            },
            kernel_path: PathBuf::from("kernel.jam"),
            start_height: 5,
            range: PeekRangeRequest::EndHeight(6),
            peek_mode: PeekMode::Warm,
            tip_height: 6,
        })
        .expect("warm");
        let cold = build_generated_read_plan(&GeneratedReadOptions {
            peek_mode: PeekMode::ColdEach,
            ..GeneratedReadOptions {
                boot: BootSourceInput::Checkpoint {
                    checkpoint: PathBuf::from("checkpoint.chkjam"),
                },
                kernel_path: PathBuf::from("kernel.jam"),
                start_height: 5,
                range: PeekRangeRequest::EndHeight(6),
                peek_mode: PeekMode::Warm,
                tip_height: 6,
            }
        })
        .expect("cold");

        assert!(matches!(
            normalize_generated_test_plan(warm.plan_input)
                .expect("warm trusted")
                .steps[0],
            TrustedStep::PeekHeight {
                cache_expectation: CacheExpectation::Warm,
                ..
            }
        ));
        assert!(matches!(
            normalize_generated_test_plan(cold.plan_input)
                .expect("cold trusted")
                .steps[0],
            TrustedStep::PeekHeightCold {
                cache_expectation: CacheExpectation::Cold,
                ..
            }
        ));
    }

    #[test]
    fn normalize_plan_preserves_explicit_cache_expectations() {
        let trusted = normalize(json!({
            "boot": checkpoint_boot("checkpoint.chkjam"),
            "kernel": "kernel.jam",
            "steps": [
                { "type": "peek_height", "height": 1, "cache_expectation": "warm" },
                { "type": "peek_height", "height": 2, "cache_expectation": "ambient" },
                { "type": "peek_height", "height": 3 },
                { "type": "peek_height_cold", "height": 4 }
            ]
        }))
        .expect("normalize plan");

        assert!(matches!(
            trusted.steps[0],
            TrustedStep::PeekHeight {
                cache_expectation: CacheExpectation::Warm,
                ..
            }
        ));
        assert!(matches!(
            trusted.steps[1],
            TrustedStep::PeekHeight {
                cache_expectation: CacheExpectation::Ambient,
                ..
            }
        ));
        assert!(matches!(
            trusted.steps[2],
            TrustedStep::PeekHeight {
                cache_expectation: CacheExpectation::Unknown,
                ..
            }
        ));
        assert!(matches!(
            trusted.steps[3],
            TrustedStep::PeekHeightCold {
                cache_expectation: CacheExpectation::Cold,
                ..
            }
        ));
    }

    fn normalize_generated_test_plan(
        input: OrchestratePlanInput,
    ) -> Result<TrustedPlan, OrchestratePlanError> {
        let tempdir = tempdir().expect("tempdir");
        let value = serde_json::to_value(&input).expect("serialize generated input");
        let value = materialize_paths(value, tempdir.path());
        normalize_plan(serde_json::from_value(value).expect("generated input"))
    }
}
