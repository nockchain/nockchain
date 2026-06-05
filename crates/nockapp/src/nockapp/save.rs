use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use bincode::{config, encode_to_vec, Decode, Encode};
use blake3::{Hash, Hasher};
use bytes::Bytes;
use nockvm::noun::{NounAllocator, D, T};
use nockvm_macros::tas;
use thiserror::Error;
use tokio::fs::create_dir_all;
use tracing::{debug, error, warn};

use super::artifact::{ArtifactError, CheckedReader};
use crate::metrics::NockAppMetrics;
use crate::noun::slab::{Jammer, NockJammer, NounSlab};
use crate::JammedNoun;

pub const JAM_MAGIC_BYTES: u64 = tas!(b"CHKJAM");
const SNAPSHOT_VERSION_0: u32 = 0;
const SNAPSHOT_VERSION_1: u32 = 1;
const SNAPSHOT_VERSION_2: u32 = 2;
pub const LATEST_SNAPSHOT_VERSION: u32 = SNAPSHOT_VERSION_2;

#[derive(Clone, Debug)]
pub(crate) struct CheckpointSummary {
    pub path: PathBuf,
    pub event_num: u64,
}

pub struct CheckpointBootstrapReader<J = NockJammer> {
    path: PathBuf,
    _phantom: std::marker::PhantomData<J>,
}

impl<J> CheckpointBootstrapReader<J> {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<J: Jammer> CheckpointBootstrapReader<J> {
    pub async fn load_latest(
        &self,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Option<SaveableCheckpoint>, CheckpointError> {
        inspect_latest(&self.path)
            .await?
            .map(|(checkpoint, _)| checkpoint.into_saveable::<J>(metrics))
            .transpose()
    }

    pub(crate) async fn load_latest_state_only_with_summary(
        &self,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Option<(SaveableCheckpoint, CheckpointSummary)>, CheckpointError> {
        inspect_latest(&self.path)
            .await?
            .map(|(checkpoint, summary)| {
                checkpoint
                    .into_saveable_state_only::<J>(metrics)
                    .map(|checkpoint| (checkpoint, summary))
            })
            .transpose()
    }
}

/// This trait decouples the serf's capture of the current kernel state from the
/// snapshotting process.
pub trait Checkpoint: Sized {
    fn to_saveable(self) -> SaveableCheckpoint;
    fn event_num(&self) -> u64;
    fn from_saveable(saveable: SaveableCheckpoint) -> Result<Self, CheckpointError>;
}

#[derive(Debug, Clone)]
pub struct SaveableCheckpoint {
    pub ker_hash: Hash,
    pub event_num: u64,
    pub state: NounSlab,
    pub cold: NounSlab,
}

impl SaveableCheckpoint {
    fn empty_cold_slab() -> NounSlab {
        let mut slab = NounSlab::new();
        let root = T(&mut slab, &[D(0), D(0), D(0)]);
        slab.set_root(root);
        slab
    }

    #[allow(clippy::wrong_self_convention)]
    #[tracing::instrument(skip(self))]
    pub fn to_jammed_checkpoint<J: Jammer>(self) -> JammedCheckpointV2 {
        let SaveableCheckpoint {
            ker_hash,
            event_num,
            state,
            cold,
        } = self;

        let state_jam = JammedNoun::new(state.coerce_jammer::<J>().jam());
        let cold_jam = JammedNoun::new(cold.coerce_jammer::<J>().jam());
        JammedCheckpointV2::new(ker_hash, event_num, cold_jam, state_jam)
    }

    fn from_jammed_checkpoint_v1<J: Jammer>(
        jammed: JammedCheckpointV1,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Self, CheckpointError> {
        let mut slab: NounSlab<J> = NounSlab::new();
        let cue_start = Instant::now();
        let root = slab.cue_into(jammed.jam.0)?;
        metrics.map(|m| m.load_cue_time.add_timing(&cue_start.elapsed()));
        slab.set_root(root);
        let space = slab.noun_space();
        let cell = root.in_space(&space).as_cell()?;

        let mut state_slab: NounSlab = NounSlab::new();
        let state_copy = state_slab.copy_into(cell.head().noun(), &space);
        state_slab.set_root(state_copy);

        let mut cold_slab: NounSlab = NounSlab::new();
        let cold_copy = cold_slab.copy_into(cell.tail().noun(), &space);
        cold_slab.set_root(cold_copy);

        Ok(Self {
            ker_hash: jammed.ker_hash,
            event_num: jammed.event_num,
            state: state_slab,
            cold: cold_slab,
        })
    }

    fn from_legacy_jammed_checkpoint_state_only<J: Jammer>(
        ker_hash: Hash,
        event_num: u64,
        jam: JammedNoun,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Self, CheckpointError> {
        let mut slab: NounSlab<J> = NounSlab::new();
        let cue_start = Instant::now();
        let root = slab.cue_into(jam.0)?;
        metrics.map(|m| m.load_cue_time.add_timing(&cue_start.elapsed()));
        slab.set_root(root);
        let space = slab.noun_space();
        let cell = root.in_space(&space).as_cell()?;

        let mut state_slab: NounSlab = NounSlab::new();
        let state_copy = state_slab.copy_into(cell.head().noun(), &space);
        state_slab.set_root(state_copy);

        Ok(Self {
            ker_hash,
            event_num,
            state: state_slab,
            cold: Self::empty_cold_slab(),
        })
    }

    fn from_jammed_checkpoint_v1_state_only<J: Jammer>(
        jammed: JammedCheckpointV1,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Self, CheckpointError> {
        Self::from_legacy_jammed_checkpoint_state_only::<J>(
            jammed.ker_hash, jammed.event_num, jammed.jam, metrics,
        )
    }

    fn from_jammed_checkpoint_v2<J: Jammer>(
        jammed: JammedCheckpointV2,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Self, CheckpointError> {
        let mut durations = std::time::Duration::ZERO;

        let mut state_slab: NounSlab<J> = NounSlab::new();
        let state_start = Instant::now();
        let state_root = state_slab.cue_into(jammed.state_jam.0.clone())?;
        durations += state_start.elapsed();
        state_slab.set_root(state_root);
        let state_slab = state_slab.coerce_jammer::<NockJammer>();

        let mut cold_slab: NounSlab<J> = NounSlab::new();
        let cold_start = Instant::now();
        let cold_root = cold_slab.cue_into(jammed.cold_jam.0.clone())?;
        durations += cold_start.elapsed();
        cold_slab.set_root(cold_root);
        let cold_slab = cold_slab.coerce_jammer::<NockJammer>();

        if let Some(metrics) = metrics {
            metrics.load_cue_time.add_timing(&durations);
        }

        Ok(Self {
            ker_hash: jammed.ker_hash,
            event_num: jammed.event_num,
            state: state_slab,
            cold: cold_slab,
        })
    }

    fn from_jammed_checkpoint_v2_state_only<J: Jammer>(
        jammed: JammedCheckpointV2,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Self, CheckpointError> {
        let mut state_slab: NounSlab<J> = NounSlab::new();
        let state_start = Instant::now();
        let state_root = state_slab.cue_into(jammed.state_jam.0.clone())?;
        if let Some(metrics) = metrics {
            metrics.load_cue_time.add_timing(&state_start.elapsed());
        }
        state_slab.set_root(state_root);

        Ok(Self {
            ker_hash: jammed.ker_hash,
            event_num: jammed.event_num,
            state: state_slab.coerce_jammer::<NockJammer>(),
            cold: Self::empty_cold_slab(),
        })
    }
}

#[derive(Error, Debug)]
pub enum CheckpointError {
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("Bincode decoding error: {0}")]
    DecodeError(#[from] bincode::error::DecodeError),
    #[error("Artifact decoding error: {0}")]
    ArtifactError(#[from] ArtifactError),
    #[error("Bincode encoding error: {0}")]
    EncodeError(#[from] bincode::error::EncodeError),
    #[error(
        "Invalid checksum at {0}. The checkpoint is corrupt or incomplete; restore it from a known-good peer or remove it so Nockchain can try another checkpoint."
    )]
    InvalidChecksum(PathBuf),
    #[error(
        "Invalid checkpoint version at {0}. Use a compatible Nockchain binary for this checkpoint, or restore/remove the checkpoint so the peer can boot from a valid one."
    )]
    InvalidVersion(PathBuf),
    #[error("Sword noun error: {0}")]
    SwordNounError(#[from] nockvm::noun::Error),
    #[error("Sword cold error: {0}")]
    FromNounError(#[from] nockvm::jets::cold::FromNounError),
    #[error("Both checkpoints failed: {0}, {1}")]
    BothCheckpointsFailed(Box<CheckpointError>, Box<CheckpointError>),
    #[error("Sword interpreter error")]
    SwordInterpreterError,
    #[error("Cue error: {0}")]
    CueError(#[from] crate::noun::slab::CueError),
    #[error("Loading at version 1 failed: {v1}\\nLoading at version 0 failed: {v0}")]
    VersionsFailed {
        v1: Box<CheckpointError>,
        v0: Box<CheckpointError>,
    },
    #[error(
        "Loading at version 2 failed: {v2}\\nLoading at version 1 failed: {v1}\\nLoading at version 0 failed: {v0}"
    )]
    VersionsFailedV2 {
        v2: Box<CheckpointError>,
        v1: Box<CheckpointError>,
        v0: Box<CheckpointError>,
    },
}

pub type JammedCheckpoint = JammedCheckpointV2;

#[derive(Clone, Encode, Decode, PartialEq, Debug)]
pub struct JammedCheckpointV1 {
    /// Magic bytes to identify checkpoint format
    pub magic_bytes: u64,
    /// Version of checkpoint
    pub version: u32,
    /// Hash of the boot kernel
    #[bincode(with_serde)]
    pub ker_hash: Hash,
    /// Checksum derived from event_num and jam (the entries below)
    #[bincode(with_serde)]
    pub checksum: Hash,
    /// Checksum derived from event_num and jam (the entries below)
    #[bincode(with_serde)]
    /// Event number
    pub event_num: u64,
    /// Event number
    pub jam: JammedNoun,
}

impl JammedCheckpointV1 {
    pub fn new(ker_hash: Hash, event_num: u64, jam: JammedNoun) -> Self {
        let checksum = Self::checksum(event_num, &jam.0);
        Self {
            magic_bytes: JAM_MAGIC_BYTES,
            version: SNAPSHOT_VERSION_1,
            ker_hash,
            checksum,
            event_num,
            jam,
        }
    }

    pub fn validate(&self, path: &Path) -> Result<(), CheckpointError> {
        if self.version != SNAPSHOT_VERSION_1 {
            Err(CheckpointError::InvalidVersion(path.to_path_buf()))
        } else if self.checksum != Self::checksum(self.event_num, &self.jam.0) {
            Err(CheckpointError::InvalidChecksum(path.to_path_buf()))
        } else {
            Ok(())
        }
    }

    #[tracing::instrument(skip_all)]
    pub fn encode(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        // TODO: Make this zero copy in the future
        encode_to_vec(self, config::standard())
    }

    fn checksum(event_num: u64, jam: &Bytes) -> Hash {
        let jam_len = jam.len();
        let mut hasher = Hasher::new();
        hasher.update(&event_num.to_le_bytes());
        hasher.update(&jam_len.to_le_bytes());
        hasher.update(jam);
        hasher.finalize()
    }

    #[tracing::instrument(skip_all)]
    async fn load_from_file(path: &Path) -> Result<Self, CheckpointError> {
        debug!("Loading jammed checkpoint from file: {}", path.display());
        let bytes = tokio::fs::read(path).await?;
        let checkpoint = Self::decode_from_bytes(&bytes, path)?;
        checkpoint.validate(path)?;
        Ok(checkpoint)
    }

    fn decode_from_bytes(bytes: &[u8], path: &Path) -> Result<Self, CheckpointError> {
        let mut reader = CheckedReader::new(bytes, "checkpoint v1");
        let magic_bytes = reader.read_u64("magic bytes")?;
        let version = reader.read_u32("version")?;
        if magic_bytes != JAM_MAGIC_BYTES || version != SNAPSHOT_VERSION_1 {
            return Err(CheckpointError::InvalidVersion(path.to_path_buf()));
        }
        let ker_hash = reader.read_hash("kernel hash")?;
        let checksum = reader.read_hash("checksum")?;
        let event_num = reader.read_u64("event number")?;
        let jam = JammedNoun::new(Bytes::copy_from_slice(reader.read_bytes("jam")?));
        reader.finish()?;

        Ok(Self {
            magic_bytes,
            version,
            ker_hash,
            checksum,
            event_num,
            jam,
        })
    }
}

#[derive(Clone, Encode, Decode, PartialEq, Debug)]
pub struct JammedCheckpointV2 {
    /// Hash of the boot kernel
    #[bincode(with_serde)]
    pub ker_hash: Hash,
    /// Checksum derived from event_num and jam (the entries below)
    #[bincode(with_serde)]
    pub checksum: Hash,
    /// Event number
    pub event_num: u64,
    pub cold_jam: JammedNoun,
    pub state_jam: JammedNoun,
}

#[derive(Clone, Encode, Decode, PartialEq, Debug)]
struct JammedCheckpointV2Envelope {
    /// Magic bytes to identify checkpoint format
    pub magic_bytes: u64,
    pub version: u32,
    pub payload: Vec<u8>,
}

impl JammedCheckpointV2 {
    pub fn new(
        ker_hash: Hash,
        event_num: u64,
        cold_jam: JammedNoun,
        state_jam: JammedNoun,
    ) -> Self {
        let checksum = Self::checksum(event_num, &cold_jam.0, &state_jam.0);
        Self {
            ker_hash,
            checksum,
            event_num,
            cold_jam,
            state_jam,
        }
    }

    pub fn validate(&self, path: &Path) -> Result<(), CheckpointError> {
        if self.checksum != Self::checksum(self.event_num, &self.cold_jam.0, &self.state_jam.0) {
            Err(CheckpointError::InvalidChecksum(path.to_path_buf()))
        } else {
            Ok(())
        }
    }

    #[tracing::instrument(skip_all)]
    pub fn encode(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        // TODO: Make this zero copy in the future
        let payload = encode_to_vec(self, config::standard())?;
        let envelope = JammedCheckpointV2Envelope {
            magic_bytes: JAM_MAGIC_BYTES,
            version: SNAPSHOT_VERSION_2,
            payload,
        };
        encode_to_vec(envelope, config::standard())
    }

    fn checksum(event_num: u64, cold_jam: &Bytes, state_jam: &Bytes) -> Hash {
        let cold_jam_len = cold_jam.len();
        let state_jam_len = state_jam.len();
        let mut hasher = Hasher::new();
        hasher.update(&event_num.to_le_bytes());
        hasher.update(&cold_jam_len.to_le_bytes());
        hasher.update(cold_jam);
        hasher.update(&state_jam_len.to_le_bytes());
        hasher.update(state_jam);
        hasher.finalize()
    }

    #[tracing::instrument(skip_all)]
    async fn load_from_file(path: &Path) -> Result<Self, CheckpointError> {
        debug!("Loading jammed checkpoint from file: {}", path.display());
        let bytes = tokio::fs::read(path).await?;
        let checkpoint = Self::decode_from_bytes_with_path(&bytes, Some(path))?;
        checkpoint.validate(path)?;
        Ok(checkpoint)
    }

    fn from_payload(payload: &[u8]) -> Result<Self, CheckpointError> {
        let mut reader = CheckedReader::new(payload, "checkpoint v2 payload");
        let ker_hash = reader.read_hash("kernel hash")?;
        let checksum = reader.read_hash("checksum")?;
        let event_num = reader.read_u64("event number")?;
        let cold_jam = JammedNoun::new(Bytes::copy_from_slice(reader.read_bytes("cold jam")?));
        let state_jam = JammedNoun::new(Bytes::copy_from_slice(reader.read_bytes("state jam")?));
        reader.finish()?;

        Ok(Self {
            ker_hash,
            checksum,
            event_num,
            cold_jam,
            state_jam,
        })
    }

    fn decode_from_bytes_with_path(
        bytes: &[u8],
        path: Option<&Path>,
    ) -> Result<Self, CheckpointError> {
        let mut reader = CheckedReader::new(bytes, "checkpoint v2 envelope");
        let magic_bytes = reader.read_u64("magic bytes")?;
        let version = reader.read_u32("version")?;
        if magic_bytes != JAM_MAGIC_BYTES {
            return Err(CheckpointError::InvalidVersion(path_or_memory(path)));
        }
        if version != LATEST_SNAPSHOT_VERSION {
            return Err(CheckpointError::InvalidVersion(path_or_memory(path)));
        }
        let payload = reader.read_bytes("payload")?;
        reader.finish()?;
        let checkpoint = Self::from_payload(payload)?;
        Ok(checkpoint)
    }

    pub fn decode_from_bytes(bytes: &[u8]) -> Result<Self, CheckpointError> {
        let checkpoint = Self::decode_from_bytes_with_path(bytes, None)?;
        let fake_path = path_or_memory(None);
        checkpoint.validate(&fake_path)?;
        Ok(checkpoint)
    }
}

fn path_or_memory(path: Option<&Path>) -> PathBuf {
    path.map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("<memory>"))
}

#[derive(Clone, Debug)]
enum LoadedCheckpoint {
    V2(JammedCheckpointV2),
    V1(JammedCheckpointV1),
    V0(JammedCheckpointV0),
}

impl LoadedCheckpoint {
    fn event_num(&self) -> u64 {
        match self {
            LoadedCheckpoint::V2(cp) => cp.event_num,
            LoadedCheckpoint::V1(cp) => cp.event_num,
            LoadedCheckpoint::V0(cp) => cp.event_num,
        }
    }

    fn checksum(&self) -> Hash {
        match self {
            LoadedCheckpoint::V2(cp) => cp.checksum,
            LoadedCheckpoint::V1(cp) => cp.checksum,
            LoadedCheckpoint::V0(cp) => cp.checksum,
        }
    }

    fn into_saveable<J: Jammer>(
        self,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<SaveableCheckpoint, CheckpointError> {
        match self {
            LoadedCheckpoint::V2(cp) => {
                SaveableCheckpoint::from_jammed_checkpoint_v2::<J>(cp, metrics)
            }
            LoadedCheckpoint::V1(cp) => {
                SaveableCheckpoint::from_jammed_checkpoint_v1::<J>(cp, metrics)
            }
            LoadedCheckpoint::V0(cp) => SaveableCheckpoint::from_jammed_checkpoint_v2::<J>(
                JammedCheckpoint::from(cp),
                metrics,
            ),
        }
    }

    fn into_saveable_state_only<J: Jammer>(
        self,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<SaveableCheckpoint, CheckpointError> {
        match self {
            LoadedCheckpoint::V2(cp) => {
                SaveableCheckpoint::from_jammed_checkpoint_v2_state_only::<J>(cp, metrics)
            }
            LoadedCheckpoint::V1(cp) => {
                SaveableCheckpoint::from_jammed_checkpoint_v1_state_only::<J>(cp, metrics)
            }
            LoadedCheckpoint::V0(cp) => {
                SaveableCheckpoint::from_legacy_jammed_checkpoint_state_only::<J>(
                    cp.ker_hash, cp.event_num, cp.jam, metrics,
                )
            }
        }
    }
}

async fn inspect_latest(
    path: &Path,
) -> Result<Option<(LoadedCheckpoint, CheckpointSummary)>, CheckpointError> {
    let path_0 = path.join("0.chkjam");
    let path_1 = path.join("1.chkjam");

    if !path_0.exists() && !path_1.exists() {
        create_dir_all(path).await?;
        return Ok(None);
    }

    let checkpoint_0 = load_checkpoint_file(&path_0).await;
    let checkpoint_1 = load_checkpoint_file(&path_1).await;

    let (loaded_checkpoint, selected_path) = match (checkpoint_0, checkpoint_1) {
        (Ok(c0), Ok(c1)) => {
            if c0.event_num() > c1.event_num() {
                debug!(
                    "Loading checkpoint at: {}, checksum: {}",
                    path_0.display(),
                    c0.checksum()
                );
                (c0, path_0)
            } else {
                debug!(
                    "Loading checkpoint at: {}, checksum: {}",
                    path_1.display(),
                    c1.checksum()
                );
                (c1, path_1)
            }
        }
        (Ok(c0), Err(e1)) => {
            warn!("checkpoint at {} failed to load: {}", path_1.display(), e1);
            debug!(
                "Loading checkpoint at: {}, checksum: {}",
                path_0.display(),
                c0.checksum()
            );
            (c0, path_0)
        }
        (Err(e0), Ok(c1)) => {
            warn!("checkpoint at {} failed to load: {}", path_0.display(), e0);
            debug!(
                "Loading checkpoint at: {}, checksum: {}",
                path_1.display(),
                c1.checksum()
            );
            (c1, path_1)
        }
        (Err(e0), Err(e1)) => {
            error!("checkpoint at {} failed to load: {}", path_0.display(), e0);
            error!("checkpoint at {} failed to load: {}", path_1.display(), e1);
            return Err(CheckpointError::BothCheckpointsFailed(
                Box::new(e0),
                Box::new(e1),
            ));
        }
    };
    let summary = CheckpointSummary {
        path: selected_path,
        event_num: loaded_checkpoint.event_num(),
    };
    Ok(Some((loaded_checkpoint, summary)))
}

async fn load_checkpoint_file(path: &Path) -> Result<LoadedCheckpoint, CheckpointError> {
    match JammedCheckpointV2::load_from_file(path).await {
        Ok(cp) => Ok(LoadedCheckpoint::V2(cp)),
        Err(e_v2) => match JammedCheckpointV1::load_from_file(path).await {
            Ok(cp) => Ok(LoadedCheckpoint::V1(cp)),
            Err(e_v1) => match JammedCheckpointV0::load_from_file(path).await {
                Ok(cp0) => Ok(LoadedCheckpoint::V0(cp0)),
                Err(e_v0) => Err(CheckpointError::VersionsFailedV2 {
                    v2: Box::new(e_v2),
                    v1: Box::new(e_v1),
                    v0: Box::new(e_v0),
                }),
            },
        },
    }
}

impl From<JammedCheckpointV0> for JammedCheckpoint {
    fn from(v0: JammedCheckpointV0) -> Self {
        let v1 = JammedCheckpointV1 {
            magic_bytes: v0.magic_bytes,
            version: SNAPSHOT_VERSION_1,
            ker_hash: v0.ker_hash,
            checksum: v0.checksum,
            event_num: v0.event_num,
            jam: v0.jam,
        };

        let mut slab: NounSlab = NounSlab::new();
        let root = slab
            .cue_into(v1.jam.0.clone())
            .expect("legacy checkpoint jam should cue");
        slab.set_root(root);
        let space = slab.noun_space();
        let cell = root
            .in_space(&space)
            .as_cell()
            .expect("legacy checkpoint root should be a cell");

        let mut state_slab: NounSlab = NounSlab::new();
        let state_copy = state_slab.copy_into(cell.head().noun(), &space);
        state_slab.set_root(state_copy);
        let state_jam = JammedNoun::new(state_slab.jam());

        let mut cold_slab: NounSlab = NounSlab::new();
        let cold_copy = cold_slab.copy_into(cell.tail().noun(), &space);
        cold_slab.set_root(cold_copy);
        let cold_jam = JammedNoun::new(cold_slab.jam());

        JammedCheckpointV2::new(v1.ker_hash, v1.event_num, cold_jam, state_jam)
    }
}

#[derive(Clone, Encode, Decode, PartialEq, Debug)]
pub struct JammedCheckpointV0 {
    /// Magic bytes to identify checkpoint format
    pub magic_bytes: u64,
    /// Version of checkpoint
    pub version: u32,
    /// The buffer this checkpoint was saved to, either 0 or 1
    pub buff_index: bool,
    /// Hash of the boot kernel
    #[bincode(with_serde)]
    pub ker_hash: Hash,
    /// Checksum derived from event_num and jam (the entries below)
    #[bincode(with_serde)]
    pub checksum: Hash,
    /// Event number
    pub event_num: u64,
    /// Jammed noun of [kernel_state cold_state]
    pub jam: JammedNoun,
}

impl JammedCheckpointV0 {
    pub fn new(buff_index: bool, ker_hash: Hash, event_num: u64, jam: JammedNoun) -> Self {
        let checksum = Self::checksum(event_num, &jam.0);
        Self {
            magic_bytes: JAM_MAGIC_BYTES,
            version: SNAPSHOT_VERSION_0,
            buff_index,
            ker_hash,
            checksum,
            event_num,
            jam,
        }
    }

    pub fn validate(&self, path: &Path) -> Result<(), CheckpointError> {
        if self.version != SNAPSHOT_VERSION_0 {
            Err(CheckpointError::InvalidVersion(path.to_path_buf()))
        } else if self.checksum != Self::checksum(self.event_num, &self.jam.0) {
            Err(CheckpointError::InvalidChecksum(path.to_path_buf()))
        } else {
            Ok(())
        }
    }

    #[tracing::instrument(skip_all)]
    pub fn encode(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        // TODO: Make this zero copy in the future
        encode_to_vec(self, config::standard())
    }

    fn checksum(event_num: u64, jam: &Bytes) -> Hash {
        let jam_len = jam.len();
        let mut hasher = Hasher::new();
        hasher.update(&event_num.to_le_bytes());
        hasher.update(&jam_len.to_le_bytes());
        hasher.update(jam);
        hasher.finalize()
    }

    #[tracing::instrument(skip_all)]
    async fn load_from_file(path: &Path) -> Result<Self, CheckpointError> {
        debug!("Loading jammed checkpoint from file: {}", path.display());
        let bytes = tokio::fs::read(path).await?;
        let checkpoint = Self::decode_from_bytes(&bytes, path)?;
        checkpoint.validate(path)?;
        Ok(checkpoint)
    }

    fn decode_from_bytes(bytes: &[u8], path: &Path) -> Result<Self, CheckpointError> {
        let mut reader = CheckedReader::new(bytes, "checkpoint v0");
        let magic_bytes = reader.read_u64("magic bytes")?;
        let version = reader.read_u32("version")?;
        if magic_bytes != JAM_MAGIC_BYTES || version != SNAPSHOT_VERSION_0 {
            return Err(CheckpointError::InvalidVersion(path.to_path_buf()));
        }
        let buff_index = reader.read_bool("buffer index")?;
        let ker_hash = reader.read_hash("kernel hash")?;
        let checksum = reader.read_hash("checksum")?;
        let event_num = reader.read_u64("event number")?;
        let jam = JammedNoun::new(Bytes::copy_from_slice(reader.read_bytes("jam")?));
        reader.finish()?;

        Ok(Self {
            magic_bytes,
            version,
            buff_index,
            ker_hash,
            checksum,
            event_num,
            jam,
        })
    }
}

#[cfg(test)]
mod version_tests {
    use std::panic::AssertUnwindSafe;

    use blake3::hash;
    use futures::FutureExt;
    use nockvm::noun::{Noun, NounSpace, D, T};
    use tempfile::TempDir;

    use super::*;

    fn legacy_pair_jam(state_value: u64, cold_value: u64) -> JammedNoun {
        let mut slab = NounSlab::<NockJammer>::new();
        let space = NounSpace::empty();
        let state = slab.copy_into(D(state_value), &space);
        let cold = slab.copy_into(D(cold_value), &space);
        let root = T(&mut slab, &[state, cold]);
        slab.set_root(root);
        JammedNoun::new(slab.coerce_jammer::<NockJammer>().jam())
    }

    fn atom_value(noun: Noun, space: &NounSpace) -> u64 {
        noun.in_space(space)
            .as_atom()
            .expect("expected atom")
            .as_u64()
            .expect("expected atom to fit in u64")
    }

    fn v1_checkpoint_with_absurd_jam_length(event_num: u64) -> Vec<u8> {
        let checkpoint = JammedCheckpointV1::new(
            hash(b"corrupt-v1"),
            event_num,
            JammedNoun::new(Bytes::new()),
        );
        let mut bytes = checkpoint.encode().expect("encode v1 checkpoint");

        // The final byte is the bincode varint length for the empty Bytes field.
        assert_eq!(bytes.pop(), Some(0));

        // bincode standard varint marker 253 means the next eight bytes are a u64.
        // On 64-bit targets this decodes to usize::MAX and currently panics in Vec allocation
        // before checkpoint fallback can try the older checkpoint.
        bytes.push(253);
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        bytes
    }

    fn v2_checkpoint_with_absurd_payload_length() -> Vec<u8> {
        let checkpoint = JammedCheckpointV2::new(
            hash(b"corrupt-v2"),
            9,
            JammedNoun::new(Bytes::from_static(b"cold")),
            JammedNoun::new(Bytes::from_static(b"state")),
        );
        let mut bytes = checkpoint.encode().expect("encode v2 checkpoint");

        // The V2 envelope is magic (u64 varint: 9 bytes for CHKJAM), version (one byte for 2),
        // then a bincode Vec<u8> length for the payload.
        let payload_len_offset = 10;
        assert_eq!(
            bytes[payload_len_offset] as usize,
            bytes.len() - payload_len_offset - 1
        );
        bytes.truncate(payload_len_offset);
        bytes.push(253);
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        bytes
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    async fn loads_v1_checkpoint_via_reader() {
        let temp = TempDir::new().expect("create temp dir");
        let state_value = 5;
        let cold_value = 9;
        let legacy_jam = legacy_pair_jam(state_value, cold_value);
        let ker_hash = hash(b"legacy-v1");
        let checkpoint = JammedCheckpointV1::new(ker_hash, 7, legacy_jam.clone());
        let bytes = checkpoint.encode().expect("encode v1 checkpoint");
        std::fs::write(temp.path().join("0.chkjam"), bytes).expect("write checkpoint");

        let maybe_saveable =
            CheckpointBootstrapReader::<NockJammer>::new(temp.path().to_path_buf())
                .load_latest(None)
                .await
                .expect("load checkpoint");

        let saveable = maybe_saveable.expect("expected a checkpoint");
        assert_eq!(saveable.ker_hash, ker_hash);
        assert_eq!(saveable.event_num, 7);

        let state_space = saveable.state.noun_space();
        let cold_space = saveable.cold.noun_space();
        let loaded_state = atom_value(unsafe { *saveable.state.root() }, &state_space);
        let loaded_cold = atom_value(unsafe { *saveable.cold.root() }, &cold_space);
        assert_eq!(loaded_state, state_value);
        assert_eq!(loaded_cold, cold_value);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    async fn loads_v0_checkpoint_via_reader() {
        let temp = TempDir::new().expect("create temp dir");
        let state_value = 11;
        let cold_value = 22;
        let legacy_jam = legacy_pair_jam(state_value, cold_value);
        let ker_hash = hash(b"legacy-v0");
        let checkpoint = JammedCheckpointV0::new(false, ker_hash, 3, legacy_jam.clone());
        let bytes = checkpoint.encode().expect("encode v0 checkpoint");
        std::fs::write(temp.path().join("0.chkjam"), bytes).expect("write checkpoint");

        let maybe_saveable =
            CheckpointBootstrapReader::<NockJammer>::new(temp.path().to_path_buf())
                .load_latest(None)
                .await
                .expect("load checkpoint");

        let saveable = maybe_saveable.expect("expected a checkpoint");
        assert_eq!(saveable.ker_hash, ker_hash);
        assert_eq!(saveable.event_num, 3);

        let state_space = saveable.state.noun_space();
        let cold_space = saveable.cold.noun_space();
        let loaded_state = atom_value(unsafe { *saveable.state.root() }, &state_space);
        let loaded_cold = atom_value(unsafe { *saveable.cold.root() }, &cold_space);
        assert_eq!(loaded_state, state_value);
        assert_eq!(loaded_cold, cold_value);
    }

    #[test]
    fn decodes_v2_checkpoint_roundtrip() {
        let checkpoint = JammedCheckpointV2::new(
            hash(b"valid-v2"),
            11,
            JammedNoun::new(Bytes::from_static(b"cold")),
            JammedNoun::new(Bytes::from_static(b"state")),
        );
        let encoded = checkpoint.encode().expect("encode v2 checkpoint");
        let decoded =
            JammedCheckpointV2::decode_from_bytes(&encoded).expect("decode v2 checkpoint");

        assert_eq!(decoded, checkpoint);
    }

    #[test]
    fn corrupt_v2_payload_length_is_reported_without_panic() {
        let bytes = v2_checkpoint_with_absurd_payload_length();
        let load_result =
            std::panic::catch_unwind(|| JammedCheckpointV2::decode_from_bytes(&bytes));

        assert!(
            load_result.is_ok(),
            "corrupt v2 payload length must be reported as a decode error, not a panic"
        );
        assert!(load_result.expect("checked above").is_err());
    }

    #[tokio::test]
    async fn corrupt_checkpoint_length_does_not_panic_and_falls_back_to_previous_checkpoint() {
        let temp = TempDir::new().expect("create temp dir");
        let valid_state_value = 5;
        let valid_cold_value = 9;
        let valid_ker_hash = hash(b"valid-v1");
        let valid_checkpoint = JammedCheckpointV1::new(
            valid_ker_hash,
            7,
            legacy_pair_jam(valid_state_value, valid_cold_value),
        );

        std::fs::write(
            temp.path().join("0.chkjam"),
            valid_checkpoint.encode().expect("encode valid checkpoint"),
        )
        .expect("write valid checkpoint");
        std::fs::write(
            temp.path().join("1.chkjam"),
            v1_checkpoint_with_absurd_jam_length(8),
        )
        .expect("write corrupt checkpoint");

        let load_result = AssertUnwindSafe(
            CheckpointBootstrapReader::<NockJammer>::new(temp.path().to_path_buf())
                .load_latest(None),
        )
        .catch_unwind()
        .await;

        assert!(
            load_result.is_ok(),
            "corrupt checkpoint length must be reported as a load error, not a panic"
        );

        let maybe_saveable = load_result
            .expect("checked above")
            .expect("valid older checkpoint should be used");
        let saveable = maybe_saveable.expect("expected fallback checkpoint");
        assert_eq!(saveable.ker_hash, valid_ker_hash);
        assert_eq!(saveable.event_num, 7);
        let state_space = saveable.state.noun_space();
        let cold_space = saveable.cold.noun_space();
        assert_eq!(
            atom_value(unsafe { *saveable.state.root() }, &state_space),
            valid_state_value
        );
        assert_eq!(
            atom_value(unsafe { *saveable.cold.root() }, &cold_space),
            valid_cold_value
        );
    }
}

/*
// We need to figure out how to do this with quickcheck instead of a golden master jam
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_jammed_checkpoint_header() {
        let chk_header = std::path::PathBuf::from("../../../jammed_checkpoint_header.jam");
        let mut chk_header_bytes = std::fs::read(chk_header).unwrap();
        let result: (JammedCheckpoint, usize) =
            bincode::decode_from_slice(&mut chk_header_bytes, bincode::config::standard()).unwrap();
        let jammed_checkpoint = result.0;
        println!("jammed_checkpoint: {:?}", jammed_checkpoint);
        assert_eq!(jammed_checkpoint.magic_bytes, JAM_MAGIC_BYTES);
        assert_eq!(jammed_checkpoint.version, SNAPSHOT_VERSION);
        assert_eq!(jammed_checkpoint.buff_index, true);
    }
}
*/
