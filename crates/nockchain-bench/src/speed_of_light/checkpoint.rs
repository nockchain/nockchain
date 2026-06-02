//! Checkpoint loading utilities for speed-of-light benchmarks

use std::path::{Path, PathBuf};

use nockapp::nockapp::save::{CheckpointError, JammedCheckpointV2, SaveableCheckpoint};
use nockapp::noun::slab::NounSlab;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CheckpointLoadError {
    #[error("IO error reading checkpoint: {0}")]
    Io(#[from] std::io::Error),

    #[error("Checkpoint decode error: {0}")]
    Checkpoint(#[from] CheckpointError),

    #[error("Cue error: {0}")]
    Cue(#[from] nockapp::noun::slab::CueError),
}

#[derive(Debug, Error)]
pub enum CheckpointMetaError {
    #[error("IO error reading checkpoint: {0}")]
    Io(#[from] std::io::Error),

    #[error("Checkpoint decode error: {0}")]
    Checkpoint(#[from] CheckpointError),

    #[error("No checkpoint files found in {0}")]
    NotFound(PathBuf),
}

/// Loaded checkpoint data ready for use
pub struct LoadedCheckpoint {
    /// Kernel state as a NounSlab
    pub state: NounSlab,
    /// Cold jet state as a NounSlab
    pub cold: NounSlab,
    /// Event number at checkpoint
    pub event_num: u64,
    /// Kernel hash
    pub ker_hash: blake3::Hash,
}

impl From<LoadedCheckpoint> for SaveableCheckpoint {
    fn from(value: LoadedCheckpoint) -> Self {
        Self {
            ker_hash: value.ker_hash,
            event_num: value.event_num,
            state: value.state,
            cold: value.cold,
        }
    }
}

/// Load a checkpoint from a .chkjam file
///
/// # Arguments
/// * `path` - Path to the checkpoint file (e.g., "0.chkjam")
///
/// # Returns
/// A `LoadedCheckpoint` containing the kernel state and cold state as NounSlabs
pub fn load_checkpoint<P: AsRef<Path>>(path: P) -> Result<LoadedCheckpoint, CheckpointLoadError> {
    let bytes = std::fs::read(path.as_ref())?;
    load_checkpoint_from_bytes(&bytes)
}

/// Load a checkpoint and convert it directly into the runtime boot format.
pub fn load_saveable_checkpoint<P: AsRef<Path>>(
    path: P,
) -> Result<SaveableCheckpoint, CheckpointLoadError> {
    load_checkpoint(path).map(Into::into)
}

/// Load a checkpoint from raw bytes
pub fn load_checkpoint_from_bytes(bytes: &[u8]) -> Result<LoadedCheckpoint, CheckpointLoadError> {
    let jammed = JammedCheckpointV2::decode_from_bytes(bytes)?;

    let ker_hash = jammed.ker_hash;
    let event_num = jammed.event_num;

    // Cue the state jam into a NounSlab
    let mut state_slab = NounSlab::new();
    let state_root = state_slab.cue_into(jammed.state_jam.0.clone())?;
    state_slab.set_root(state_root);

    // Cue the cold jam into a NounSlab
    let mut cold_slab = NounSlab::new();
    let cold_root = cold_slab.cue_into(jammed.cold_jam.0.clone())?;
    cold_slab.set_root(cold_root);

    Ok(LoadedCheckpoint {
        state: state_slab,
        cold: cold_slab,
        event_num,
        ker_hash,
    })
}

/// Read event_num from a checkpoint file without cueing its state.
pub fn checkpoint_event_num<P: AsRef<Path>>(path: P) -> Result<u64, CheckpointMetaError> {
    let bytes = std::fs::read(path.as_ref())?;
    let jammed = JammedCheckpointV2::decode_from_bytes(&bytes)?;
    Ok(jammed.event_num)
}

/// Select the latest checkpoint file from a snapshot directory.
pub fn select_latest_checkpoint_path<P: AsRef<Path>>(
    snapshot_dir: P,
) -> Result<PathBuf, CheckpointMetaError> {
    let dir = snapshot_dir.as_ref();
    let path_0 = dir.join("0.chkjam");
    let path_1 = dir.join("1.chkjam");

    let has_0 = path_0.exists();
    let has_1 = path_1.exists();

    match (has_0, has_1) {
        (false, false) => Err(CheckpointMetaError::NotFound(dir.to_path_buf())),
        (true, false) => Ok(path_0),
        (false, true) => Ok(path_1),
        (true, true) => {
            let event_0 = checkpoint_event_num(&path_0)?;
            let event_1 = checkpoint_event_num(&path_1)?;
            if event_1 >= event_0 {
                Ok(path_1)
            } else {
                Ok(path_0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bytes::Bytes;
    use nockapp::JammedNoun;

    use super::*;

    #[test]
    #[ignore = "requires checkpoint file"]
    fn test_load_checkpoint() {
        let checkpoint_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("0.chkjam");

        if checkpoint_path.exists() {
            let loaded = load_checkpoint(&checkpoint_path).expect("should load checkpoint");
            println!("Loaded checkpoint at event_num: {}", loaded.event_num);
        }
    }

    #[test]
    fn test_select_latest_checkpoint_prefers_higher_event_num() {
        let temp_dir = tempfile::tempdir().expect("should create temp dir");
        let path_0 = temp_dir.path().join("0.chkjam");
        let path_1 = temp_dir.path().join("1.chkjam");

        write_dummy_checkpoint(&path_0, 10);
        write_dummy_checkpoint(&path_1, 20);

        let selected = select_latest_checkpoint_path(temp_dir.path())
            .expect("should select latest checkpoint");
        assert_eq!(selected, path_1);
    }

    #[test]
    fn test_select_latest_checkpoint_single_file() {
        let temp_dir = tempfile::tempdir().expect("should create temp dir");
        let path_0 = temp_dir.path().join("0.chkjam");

        write_dummy_checkpoint(&path_0, 42);

        let selected = select_latest_checkpoint_path(temp_dir.path())
            .expect("should select existing checkpoint");
        assert_eq!(selected, path_0);
    }

    fn write_dummy_checkpoint(path: &Path, event_num: u64) {
        let ker_hash = blake3::Hash::from_bytes([event_num as u8; 32]);
        let cold = JammedNoun::new(Bytes::from_static(b"cold"));
        let state = JammedNoun::new(Bytes::from_static(b"state"));
        let checkpoint = JammedCheckpointV2::new(ker_hash, event_num, cold, state);
        let bytes = checkpoint.encode().expect("should encode checkpoint");
        std::fs::write(path, bytes).expect("should write checkpoint");
    }
}
