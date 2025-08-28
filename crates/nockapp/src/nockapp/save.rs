use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use bincode::config::Configuration;
use bincode::{config, encode_to_vec, Decode, Encode};
use blake3::{Hash, Hasher};
use bytes::Bytes;
use nockvm_macros::tas;
use thiserror::Error;
use tokio::fs::create_dir_all;
use tokio::sync::oneshot;
use tracing::{debug, error, trace, warn};

use crate::metrics::NockAppMetrics;
use crate::noun::slab::{Jammer, NockJammer, NounSlab};
use crate::JammedNoun;

const JAM_MAGIC_BYTES: u64 = tas!(b"CHKJAM");
const SNAPSHOT_VERSION_0: u32 = 0;
const SNAPSHOT_VERSION_1: u32 = 1;

pub enum WhichSnapshot {
    Snapshot0,
    Snapshot1,
}

impl WhichSnapshot {
    pub fn next(&self) -> Self {
        match self {
            WhichSnapshot::Snapshot0 => WhichSnapshot::Snapshot1,
            WhichSnapshot::Snapshot1 => WhichSnapshot::Snapshot0,
        }
    }
}

/// State object which handles all NockApp saves and loads
pub struct Saver<J = NockJammer> {
    path_0: PathBuf,
    path_1: PathBuf,
    save_to_next: WhichSnapshot,
    waiters: Vec<(u64, oneshot::Sender<()>)>,
    last_event_num: u64,
    _phantom: std::marker::PhantomData<J>,
}

impl<J> Saver<J> {
    pub fn last_path(&self) -> PathBuf {
        match self.save_to_next {
            WhichSnapshot::Snapshot1 => self.path_0.clone(),
            WhichSnapshot::Snapshot0 => self.path_1.clone(),
        }
    }

    pub fn next_path(&self) -> PathBuf {
        match self.save_to_next {
            WhichSnapshot::Snapshot1 => self.path_1.clone(),
            WhichSnapshot::Snapshot0 => self.path_0.clone(),
        }
    }

    /// The future from this function should not be awaited before any mutex
    /// around the 'Saver' is released, or a deadlock will result.
    #[tracing::instrument(skip(self))]
    #[allow(clippy::async_yields_async)]
    pub async fn wait_for_snapshot<'a>(
        &'a mut self,
        wait_for_event_num: u64,
    ) -> impl Future<Output = Result<(), oneshot::error::RecvError>> {
        if self.last_event_num >= wait_for_event_num {
            return futures::future::Either::Left(std::future::ready(Ok(())));
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.waiters.push((wait_for_event_num, tx));
        futures::future::Either::Right(rx)
    }

    /// Check if we need to save
    pub fn save_needed(&self, event_num: u64) -> bool {
        self.last_event_num < event_num
    }
}

impl<J: Jammer> Saver<J> {
    pub async fn try_load<C: Checkpoint>(
        path: &PathBuf,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<(Self, Option<C>), CheckpointError> {
        let path_0 = path.join("0.chkjam");
        let path_1 = path.join("1.chkjam");
        let waiters = Vec::new();

        // No snapshot to load
        if !path_0.exists() && !path_1.exists() {
            create_dir_all(path).await?;
            return Ok((
                Self {
                    path_0,
                    path_1,
                    save_to_next: WhichSnapshot::Snapshot0,
                    waiters,
                    last_event_num: 0,
                    _phantom: std::marker::PhantomData,
                },
                None,
            ));
        }

        let checkpoint_0 = match JammedCheckpointV1::load_from_file(&path_0).await {
            Ok(c) => Ok(c),
            Err(e_v1) => JammedCheckpointV0::load_from_file(&path_0)
                .await
                .map(|c0| JammedCheckpointV1::from(c0))
                .and_then(|c| c.validate(&path_0).map(|_| c))
                .map_err(|e_v0| CheckpointError::VersionsFailed {
                    v1: Box::new(e_v1),
                    v0: Box::new(e_v0),
                }),
        };

        let checkpoint_1 = match JammedCheckpointV1::load_from_file(&path_1).await {
            Ok(c) => Ok(c),
            Err(e_v1) => JammedCheckpointV0::load_from_file(&path_1)
                .await
                .map(|c0| JammedCheckpointV1::from(c0))
                .and_then(|c| c.validate(&path_1).map(|_| c))
                .map_err(|e_v0| CheckpointError::VersionsFailed {
                    v1: Box::new(e_v1),
                    v0: Box::new(e_v0),
                }),
        };

        let (jammed_checkpoint, save_to_next) = match (checkpoint_0, checkpoint_1) {
            (Ok(c0), Ok(c1)) => {
                if c0.event_num > c1.event_num {
                    debug!(
                        "Loading checkpoint at: {}, checksum: {}",
                        path_0.display(),
                        c0.checksum
                    );
                    (c0, WhichSnapshot::Snapshot1)
                } else {
                    debug!(
                        "Loading checkpoint at: {}, checksum: {}",
                        path_1.display(),
                        c1.checksum
                    );
                    (c1, WhichSnapshot::Snapshot0)
                }
            }
            (Ok(c0), Err(e)) => {
                warn!("checkpoint at {} failed to load: {}", path_1.display(), e);
                debug!(
                    "Loading checkpoint at: {}, checksum: {}",
                    path_0.display(),
                    c0.checksum
                );
                (c0, WhichSnapshot::Snapshot1)
            }
            (Err(e), Ok(c1)) => {
                warn!("checkpoint at {} failed to load: {}", path_0.display(), e);
                debug!(
                    "Loading checkpoint at: {}, checksum: {}",
                    path_1.display(),
                    c1.checksum
                );
                (c1, WhichSnapshot::Snapshot0)
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
        let last_event_num = jammed_checkpoint.event_num;
        let saveable = SaveableCheckpoint::from_jammed_checkpoint::<J>(jammed_checkpoint, metrics)?;
        trace!("After from_jammed_checkpoint");
        let c = C::from_saveable(saveable)?;
        Ok((
            Self {
                path_0,
                path_1,
                save_to_next,
                waiters,
                last_event_num,
                _phantom: std::marker::PhantomData,
            },
            Some(c),
        ))
    }

    #[tracing::instrument(skip_all)]
    pub async fn save<C: Checkpoint>(
        &mut self,
        checkpoint: C,
        metrics: Arc<NockAppMetrics>,
    ) -> Result<(), CheckpointError> {
        let event_num = checkpoint.event_num();
        trace!("Saving checkpoint at event_num {}", event_num);
        let saveable = checkpoint.to_saveable();
        trace!("Converted checkpoint to saveable");
        let jammed = saveable.to_jammed_checkpoint::<J>(metrics);
        trace!("Converted saveable to jammed");
        let path = self.next_path();
        jammed.save_to_file(&path).await?;
        self.save_to_next = self.save_to_next.next();
        std::mem::drop(jammed);
        debug!(
            "Saved checkpoint to file: {}",
            &path.as_os_str().to_str().unwrap()
        );
        let mut still_waiting = Vec::new();
        for (waiting_event_num, waiter) in self.waiters.drain(..) {
            if waiting_event_num <= event_num {
                let _ = waiter.send(()); // An error means the receiver was dropped
            } else {
                still_waiting.push((waiting_event_num, waiter));
            }
        }

        self.last_event_num = event_num;
        self.waiters = still_waiting;

        Ok(())
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
    pub noun: NounSlab,
}

impl SaveableCheckpoint {
    #[tracing::instrument(skip(self, metrics))]
    fn to_jammed_checkpoint<J: Jammer>(self, metrics: Arc<NockAppMetrics>) -> JammedCheckpointV1 {
        let jam_start = Instant::now();
        let jam = JammedNoun(self.noun.coerce_jammer::<J>().jam());
        metrics.save_jam_time.add_timing(&jam_start.elapsed());

        JammedCheckpointV1::new(self.ker_hash, self.event_num, jam)
    }

    fn from_jammed_checkpoint<'a, J: Jammer>(
        jammed: JammedCheckpointV1,
        metrics: Option<Arc<NockAppMetrics>>,
    ) -> Result<Self, CheckpointError> {
        let mut slab = NounSlab::new();
        let cue_start = Instant::now();
        let root = slab.cue_into(jammed.jam.0)?;
        metrics.map(|m| m.load_cue_time.add_timing(&cue_start.elapsed()));
        slab.set_root(root);
        Ok(Self {
            ker_hash: jammed.ker_hash,
            event_num: jammed.event_num,
            noun: slab,
        })
    }
}

impl Checkpoint for SaveableCheckpoint {
    fn to_saveable(self) -> SaveableCheckpoint {
        self
    }

    fn from_saveable(saveable: SaveableCheckpoint) -> Result<Self, CheckpointError> {
        Ok(saveable)
    }

    fn event_num(&self) -> u64 {
        self.event_num
    }
}

#[derive(Error, Debug)]
pub enum CheckpointError {
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("Bincode decoding error: {0}")]
    DecodeError(#[from] bincode::error::DecodeError),
    #[error("Bincode encoding error: {0}")]
    EncodeError(#[from] bincode::error::EncodeError),
    #[error("Invalid checksum at {0}")]
    InvalidChecksum(PathBuf),
    #[error("Invalid version at {0}")]
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
    #[error("Loading at version 1 failed: {v1}\nLoading at version 0 failed: {v0}")]
    VersionsFailed {
        v1: Box<CheckpointError>,
        v0: Box<CheckpointError>,
    },
}

pub type JammedCheckpoint = JammedCheckpointV1;

#[derive(Encode, Decode, PartialEq, Debug)]
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

    pub fn validate(&self, path: &PathBuf) -> Result<(), CheckpointError> {
        if self.version != SNAPSHOT_VERSION_1 {
            Err(CheckpointError::InvalidVersion(path.clone()))
        } else if self.checksum != Self::checksum(self.event_num, &self.jam.0) {
            Err(CheckpointError::InvalidChecksum(path.clone()))
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
    async fn load_from_file(path: &PathBuf) -> Result<Self, CheckpointError> {
        debug!(
            "Loading jammed checkpoint from file: {}",
            path.as_os_str().to_str().unwrap()
        );
        let bytes = tokio::fs::read(path).await?;
        let config = bincode::config::standard();
        let (checkpoint, _) = bincode::decode_from_slice::<Self, Configuration>(&bytes, config)?;
        checkpoint.validate(path)?;
        Ok(checkpoint)
    }

    #[tracing::instrument(skip(self))]
    async fn save_to_file(&self, path: &PathBuf) -> Result<(), CheckpointError> {
        let bytes = self.encode()?;
        trace!("Saving jammed checkpoint to file: {}", path.display());
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }
}

impl From<JammedCheckpointV0> for JammedCheckpoint {
    fn from(v0: JammedCheckpointV0) -> Self {
        JammedCheckpointV1 {
            magic_bytes: v0.magic_bytes,
            version: SNAPSHOT_VERSION_1,
            ker_hash: v0.ker_hash,
            checksum: v0.checksum,
            event_num: v0.event_num,
            jam: v0.jam,
        }
    }
}

#[derive(Encode, Decode, PartialEq, Debug)]
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

    pub fn validate(&self, path: &PathBuf) -> Result<(), CheckpointError> {
        if self.version != SNAPSHOT_VERSION_0 {
            Err(CheckpointError::InvalidVersion(path.clone()))
        } else if self.checksum != Self::checksum(self.event_num, &self.jam.0) {
            Err(CheckpointError::InvalidChecksum(path.clone()))
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
    async fn load_from_file(path: &PathBuf) -> Result<Self, CheckpointError> {
        debug!(
            "Loading jammed checkpoint from file: {}",
            path.as_os_str().to_str().unwrap()
        );
        let bytes = tokio::fs::read(path).await?;
        let config = bincode::config::standard();
        let (checkpoint, _) = bincode::decode_from_slice::<Self, Configuration>(&bytes, config)?;
        checkpoint.validate(path)?;
        Ok(checkpoint)
    }

    #[tracing::instrument(skip(self))]
    #[allow(dead_code)] // Preserving this for posterity
    async fn save_to_file(&self, path: &PathBuf) -> Result<(), CheckpointError> {
        let bytes = self.encode()?;
        trace!("Saving jammed checkpoint to file: {}", path.display());
        tokio::fs::write(path, bytes).await?;
        Ok(())
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
