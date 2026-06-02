//! Bench-local PMA replay helpers.

use std::fs;
use std::path::{Path, PathBuf};

use nockapp::kernel::boot::TraceOpts;
pub use nockapp::kernel::form::SnapshotReplayInfo;
use nockapp::kernel::form::{Kernel, LoadState, PmaConfig, SnapshotReplayConfigError};
use nockapp::nockapp::save::SaveableCheckpoint;
use nockapp::nockapp::NockApp;
use nockapp::noun::slab::NounSlab;
use nockvm::noun::Noun;
use tracing::info;
use zkvm_jetpack::hot::produce_prover_hot_state;

use super::kernel_utils::KernelInitError;

fn replay_pma_dir(work_dir: &Path) -> PathBuf {
    work_dir.join("replay-pma")
}

fn prepare_replay_pma_dir(work_dir: &Path) -> Result<PathBuf, std::io::Error> {
    let replay_pma_dir = replay_pma_dir(work_dir);
    if replay_pma_dir.exists() {
        fs::remove_dir_all(&replay_pma_dir)?;
    }
    fs::create_dir_all(&replay_pma_dir)?;
    Ok(replay_pma_dir)
}

fn replay_pma_words() -> usize {
    nockapp::utils::NOCK_STACK_SIZE_MEDIUM
}

fn replay_pma_config(work_dir: &Path, fsync_enabled: bool) -> Result<PmaConfig, std::io::Error> {
    let replay_pma_dir = prepare_replay_pma_dir(work_dir)?;
    // Replay uses fresh PMA slabs and disables production snapshot restore.
    Ok(PmaConfig::for_replay(
        replay_pma_dir.join("0.pma"),
        replay_pma_dir.join("1.pma"),
        replay_pma_words(),
        None,
        fsync_enabled,
    ))
}

pub fn snapshot_replay_pma_config(
    work_dir: &Path,
    snapshot_pma: &Path,
    snapshot_manifest: &Path,
    fsync_enabled: bool,
) -> Result<(PmaConfig, SnapshotReplayInfo), KernelInitError> {
    let replay_pma_dir = prepare_replay_pma_dir(work_dir)?;
    PmaConfig::for_snapshot_replay(
        snapshot_pma,
        snapshot_manifest,
        replay_pma_dir.join("0.pma"),
        replay_pma_dir.join("1.pma"),
        replay_pma_words(),
        None,
        fsync_enabled,
    )
    .map_err(snapshot_replay_config_error)
}

fn snapshot_replay_config_error(error: SnapshotReplayConfigError) -> KernelInitError {
    KernelInitError::Boot(error.to_string())
}

fn checkpoint_to_load_state(checkpoint: SaveableCheckpoint) -> LoadState {
    let SaveableCheckpoint {
        ker_hash,
        event_num,
        state,
        cold: _,
    } = checkpoint;

    LoadState {
        ker_hash,
        event_num,
        kernel_state: state,
    }
}

pub async fn init_replay_nockapp(
    kernel_path: &Path,
    checkpoint: Option<SaveableCheckpoint>,
    work_dir: &PathBuf,
    fsync_enabled: bool,
) -> Result<NockApp, KernelInitError> {
    let kernel_bytes = std::fs::read(kernel_path)?;
    info!(kernel_size = kernel_bytes.len(), "Loaded kernel jam");

    let hot_state = produce_prover_hot_state();
    info!(jets = hot_state.len(), "Got hot state entries");
    let pma_config = replay_pma_config(work_dir, fsync_enabled)?;
    let load_state = checkpoint.map(checkpoint_to_load_state);

    let kernel = Kernel::load_with_hot_state_medium(
        &kernel_bytes,
        None::<SaveableCheckpoint>,
        &hot_state,
        vec![],
        TraceOpts::default(),
        Some(pma_config),
    )
    .await
    .map_err(nockapp::nockapp::NockAppError::from)
    .map_err(KernelInitError::from)?;

    if let Some(load_state) = load_state {
        info!(
            event_num = load_state.event_num,
            "Importing state-only PMA replay bootstrap and dropping stored cold state"
        );
        kernel
            .import(load_state)
            .await
            .map_err(nockapp::nockapp::NockAppError::from)
            .map_err(KernelInitError::from)?;
    }

    NockApp::new(move |_metrics| async move {
        Ok::<Kernel<SaveableCheckpoint>, nockapp::CrownError>(kernel)
    })
    .await
    .map_err(KernelInitError::from)
}

pub async fn init_snapshot_replay_nockapp(
    kernel_path: &Path,
    snapshot_pma: &Path,
    snapshot_manifest: &Path,
    work_dir: &PathBuf,
    fsync_enabled: bool,
) -> Result<NockApp, KernelInitError> {
    let kernel_bytes = std::fs::read(kernel_path)?;
    info!(kernel_size = kernel_bytes.len(), "Loaded kernel jam");

    let hot_state = produce_prover_hot_state();
    info!(jets = hot_state.len(), "Got hot state entries");
    let (pma_config, info) =
        snapshot_replay_pma_config(work_dir, snapshot_pma, snapshot_manifest, fsync_enabled)?;
    info!(
        event_num = info.event_num,
        pma_words = info.pma_words,
        alloc_words = info.alloc_words,
        "Prepared snapshot replay PMA"
    );

    let kernel = Kernel::load_with_hot_state_medium(
        &kernel_bytes,
        None::<SaveableCheckpoint>,
        &hot_state,
        vec![],
        TraceOpts::default(),
        Some(pma_config),
    )
    .await
    .map_err(nockapp::nockapp::NockAppError::from)
    .map_err(KernelInitError::from)?;

    NockApp::new(move |_metrics| async move {
        Ok::<Kernel<SaveableCheckpoint>, nockapp::CrownError>(kernel)
    })
    .await
    .map_err(KernelInitError::from)
}

pub fn copy_from_source_slab<J, K>(dst: &mut NounSlab<J>, noun: Noun, src: &NounSlab<K>) -> Noun {
    use nockvm::noun::NounAllocator;

    let space = src.noun_space();
    dst.copy_into(noun, &space)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use nockapp::nockapp::save::SaveableCheckpoint;
    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::D;
    use tempfile::tempdir;

    use super::{prepare_replay_pma_dir, replay_pma_config, replay_pma_dir, replay_pma_words};

    #[test]
    fn test_prepare_replay_pma_dir_recreates_directory_and_removes_stale_files() {
        let tempdir = tempdir().expect("tempdir should be created");
        let replay_pma_dir = replay_pma_dir(tempdir.path());
        fs::create_dir_all(&replay_pma_dir).expect("replay-pma dir should be created");
        fs::write(replay_pma_dir.join("0.pma"), b"stale slab 0").expect("stale slab 0");
        fs::write(replay_pma_dir.join("1.pma"), b"stale slab 1").expect("stale slab 1");

        let prepared_dir =
            prepare_replay_pma_dir(tempdir.path()).expect("replay-pma dir should be prepared");

        assert_eq!(prepared_dir, replay_pma_dir);
        assert_eq!(prepared_dir, tempdir.path().join("replay-pma"));
        assert!(prepared_dir.is_dir());
        assert!(!prepared_dir.join("0.pma").exists());
        assert!(!prepared_dir.join("1.pma").exists());
    }

    #[test]
    fn test_replay_pma_words_matches_expected_medium_stack_size() {
        assert_eq!(replay_pma_words(), nockapp::utils::NOCK_STACK_SIZE_MEDIUM);
    }

    #[test]
    fn test_replay_pma_config_returns_fresh_replay_shape() {
        let tempdir = tempdir().expect("tempdir should be created");

        let config =
            replay_pma_config(tempdir.path(), true).expect("replay config should be prepared");
        let replay_pma_dir = replay_pma_dir(tempdir.path());

        assert_eq!(config.path_0, replay_pma_dir.join("0.pma"));
        assert_eq!(config.path_1, replay_pma_dir.join("1.pma"));
        assert_eq!(config.words, replay_pma_words());
        assert!(!config.open_existing);
        assert!(!config.create_snapshots);
        assert_eq!(config.rotating_snapshot_interval_event_time, None);
        assert_eq!(config.gc_interval, None);
    }

    #[test]
    fn replay_pma_config_accepts_requested_fsync_mode_for_replay() {
        let tempdir = tempdir().expect("tempdir should be created");

        let config_on =
            replay_pma_config(tempdir.path(), true).expect("replay config should be prepared");
        let replay_pma_dir = replay_pma_dir(tempdir.path());
        assert_eq!(config_on.path_0, replay_pma_dir.join("0.pma"));
        assert_eq!(config_on.path_1, replay_pma_dir.join("1.pma"));
        assert_eq!(config_on.words, replay_pma_words());
        assert!(!config_on.open_existing);
        assert!(!config_on.create_snapshots);
        assert_eq!(config_on.rotating_snapshot_interval_event_time, None);
        assert_eq!(config_on.gc_interval, None);

        let config_off =
            replay_pma_config(tempdir.path(), false).expect("replay config should be prepared");
        assert_eq!(config_off.path_0, replay_pma_dir.join("0.pma"));
        assert_eq!(config_off.path_1, replay_pma_dir.join("1.pma"));
        assert_eq!(config_off.words, replay_pma_words());
        assert!(!config_off.open_existing);
        assert!(!config_off.create_snapshots);
        assert_eq!(config_off.rotating_snapshot_interval_event_time, None);
        assert_eq!(config_off.gc_interval, None);
    }

    #[test]
    fn snapshot_replay_pma_config_copies_snapshot_into_replay_slab() {
        let tempdir = tempdir().expect("tempdir should be created");
        let snapshot_dir =
            PathBuf::from("/shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool");
        let snapshot_pma = snapshot_dir.join("snapshot.pma");
        let snapshot_manifest = snapshot_dir.join("snapshot.manifest");

        let (config, info) = super::snapshot_replay_pma_config(
            tempdir.path(),
            &snapshot_pma,
            &snapshot_manifest,
            true,
        )
        .expect("snapshot replay config should be prepared");

        let replay_pma_dir = replay_pma_dir(tempdir.path());
        assert_eq!(config.path_0, replay_pma_dir.join("0.pma"));
        assert_eq!(config.path_1, replay_pma_dir.join("1.pma"));
        assert!(config.path_0.exists());
        assert!(!config.path_1.exists());
        assert!(config.open_existing);
        assert!(!config.create_snapshots);
        assert_eq!(config.rotating_snapshot_interval_event_time, None);
        assert_eq!(config.gc_interval, None);
        assert_eq!(info.event_num, 5);
    }

    #[test]
    fn checkpoint_to_load_state_preserves_state_and_discards_cold() {
        let mut state = NounSlab::new();
        state.set_root(D(42));
        let state_jam = state.jam();

        let mut cold = NounSlab::new();
        cold.set_root(D(99));
        let cold_jam = cold.jam();

        let checkpoint = SaveableCheckpoint {
            ker_hash: blake3::Hash::from_bytes([7; 32]),
            event_num: 123,
            state,
            cold,
        };

        let load_state = super::checkpoint_to_load_state(checkpoint);

        assert_eq!(load_state.ker_hash, blake3::Hash::from_bytes([7; 32]));
        assert_eq!(load_state.event_num, 123);
        assert_eq!(load_state.kernel_state.jam(), state_jam);
        assert_ne!(load_state.kernel_state.jam(), cold_jam);
    }
}
