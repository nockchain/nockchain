use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use nockapp::kernel::form::{inspect_snapshot_replay_source, SnapshotReplayConfigError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::checkpoint::{checkpoint_event_num, CheckpointMetaError};

const CHECKPOINT_INPUT_ID: &str = "checkpoint-0";
const SNAPSHOT_PMA_INPUT_ID: &str = "snapshot-pma-0";
const SNAPSHOT_MANIFEST_INPUT_ID: &str = "snapshot-manifest-0";

#[derive(Debug, Error)]
pub enum BootSourceError {
    #[error("exactly one boot source is required")]
    MissingSource,
    #[error("checkpoint boot source conflicts with snapshot boot source")]
    ConflictingSources,
    #[error("snapshot boot source requires both --snapshot-pma and --snapshot-manifest")]
    IncompleteSnapshotPair {
        snapshot_pma: Option<PathBuf>,
        snapshot_manifest: Option<PathBuf>,
    },
    #[error("failed to canonicalize boot source path {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to inspect checkpoint boot source {path}: {source}")]
    CheckpointMeta {
        path: PathBuf,
        source: CheckpointMetaError,
    },
    #[error("failed to inspect snapshot boot source PMA {pma} with manifest {manifest}: {source}")]
    SnapshotMeta {
        pma: PathBuf,
        manifest: PathBuf,
        source: SnapshotReplayConfigError,
    },
    #[error("failed to hash boot source input {path}: {source}")]
    Hash {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BootSourceInput {
    Checkpoint { checkpoint: PathBuf },
    Snapshot { pma: PathBuf, manifest: PathBuf },
}

impl BootSourceInput {
    pub fn from_cli_parts(
        checkpoint: Option<PathBuf>,
        snapshot_pma: Option<PathBuf>,
        snapshot_manifest: Option<PathBuf>,
    ) -> Result<Self, BootSourceError> {
        match (checkpoint, snapshot_pma, snapshot_manifest) {
            (Some(checkpoint), None, None) => Ok(Self::Checkpoint { checkpoint }),
            (None, Some(pma), Some(manifest)) => Ok(Self::Snapshot { pma, manifest }),
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                Err(BootSourceError::ConflictingSources)
            }
            (None, Some(snapshot_pma), None) => Err(BootSourceError::IncompleteSnapshotPair {
                snapshot_pma: Some(snapshot_pma),
                snapshot_manifest: None,
            }),
            (None, None, Some(snapshot_manifest)) => Err(BootSourceError::IncompleteSnapshotPair {
                snapshot_pma: None,
                snapshot_manifest: Some(snapshot_manifest),
            }),
            (None, None, None) => Err(BootSourceError::MissingSource),
        }
    }

    pub fn resolve(self) -> Result<ResolvedBootSource, BootSourceError> {
        match self {
            Self::Checkpoint { checkpoint } => {
                let checkpoint = canonicalize_boot_path(checkpoint)?;
                let event_num = checkpoint_event_num(&checkpoint).map_err(|source| {
                    BootSourceError::CheckpointMeta {
                        path: checkpoint.clone(),
                        source,
                    }
                })?;
                Ok(ResolvedBootSource::Checkpoint {
                    checkpoint,
                    event_num: Some(event_num),
                })
            }
            Self::Snapshot { pma, manifest } => {
                let pma = canonicalize_boot_path(pma)?;
                let manifest = canonicalize_boot_path(manifest)?;
                let info = inspect_snapshot_replay_source(&pma, &manifest).map_err(|source| {
                    BootSourceError::SnapshotMeta {
                        pma: pma.clone(),
                        manifest: manifest.clone(),
                        source,
                    }
                })?;
                Ok(ResolvedBootSource::Snapshot {
                    pma,
                    manifest,
                    event_num: info.event_num,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootSourceKind {
    Checkpoint,
    Snapshot,
}

impl BootSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoint",
            Self::Snapshot => "snapshot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedBootSource {
    Checkpoint {
        checkpoint: PathBuf,
        event_num: Option<u64>,
    },
    Snapshot {
        pma: PathBuf,
        manifest: PathBuf,
        event_num: u64,
    },
}

impl ResolvedBootSource {
    pub fn kind(&self) -> BootSourceKind {
        match self {
            Self::Checkpoint { .. } => BootSourceKind::Checkpoint,
            Self::Snapshot { .. } => BootSourceKind::Snapshot,
        }
    }

    pub fn event_num(&self) -> Option<u64> {
        match self {
            Self::Checkpoint { event_num, .. } => *event_num,
            Self::Snapshot { event_num, .. } => Some(*event_num),
        }
    }

    pub fn trusted_source(&self) -> TrustedBootSource {
        match self {
            Self::Checkpoint { event_num, .. } => TrustedBootSource::Checkpoint {
                checkpoint_input_id: CHECKPOINT_INPUT_ID.to_string(),
                event_num: *event_num,
            },
            Self::Snapshot { event_num, .. } => TrustedBootSource::Snapshot {
                pma_input_id: SNAPSHOT_PMA_INPUT_ID.to_string(),
                manifest_input_id: SNAPSHOT_MANIFEST_INPUT_ID.to_string(),
                event_num: *event_num,
            },
        }
    }

    pub fn trusted_input_files(&self) -> Result<Vec<TrustedBootSourceFile>, BootSourceError> {
        self.input_paths()
            .into_iter()
            .map(|(input_id, role, absolute_path)| {
                let (sha256_hex, size_bytes) = hash_file(&absolute_path)?;
                Ok(TrustedBootSourceFile {
                    input_id: input_id.to_string(),
                    role,
                    absolute_path,
                    sha256_hex,
                    size_bytes,
                })
            })
            .collect()
    }

    pub fn input_paths(&self) -> Vec<(&'static str, BootSourceFileRole, PathBuf)> {
        match self {
            Self::Checkpoint { checkpoint, .. } => {
                vec![(
                    CHECKPOINT_INPUT_ID,
                    BootSourceFileRole::Checkpoint,
                    checkpoint.clone(),
                )]
            }
            Self::Snapshot { pma, manifest, .. } => vec![
                (
                    SNAPSHOT_PMA_INPUT_ID,
                    BootSourceFileRole::SnapshotPma,
                    pma.clone(),
                ),
                (
                    SNAPSHOT_MANIFEST_INPUT_ID,
                    BootSourceFileRole::SnapshotManifest,
                    manifest.clone(),
                ),
            ],
        }
    }

    pub fn to_input(&self) -> BootSourceInput {
        match self {
            Self::Checkpoint { checkpoint, .. } => BootSourceInput::Checkpoint {
                checkpoint: checkpoint.clone(),
            },
            Self::Snapshot { pma, manifest, .. } => BootSourceInput::Snapshot {
                pma: pma.clone(),
                manifest: manifest.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrustedBootSource {
    Checkpoint {
        checkpoint_input_id: String,
        event_num: Option<u64>,
    },
    Snapshot {
        pma_input_id: String,
        manifest_input_id: String,
        event_num: u64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum BootSourceFileRole {
    Checkpoint,
    SnapshotPma,
    SnapshotManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedBootSourceFile {
    pub input_id: String,
    pub role: BootSourceFileRole,
    pub absolute_path: PathBuf,
    pub sha256_hex: String,
    pub size_bytes: u64,
}

fn canonicalize_boot_path(path: PathBuf) -> Result<PathBuf, BootSourceError> {
    path.canonicalize()
        .map_err(|source| BootSourceError::Canonicalize { path, source })
}

fn hash_file(path: &Path) -> Result<(String, u64), BootSourceError> {
    let mut file = File::open(path).map_err(|source| BootSourceError::Hash {
        path: path.to_path_buf(),
        source,
    })?;
    let size_bytes = file
        .metadata()
        .map_err(|source| BootSourceError::Hash {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| BootSourceError::Hash {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), size_bytes))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bytes::Bytes;
    use nockapp::nockapp::save::JammedCheckpointV2;
    use nockapp::JammedNoun;
    use tempfile::TempDir;

    use super::*;

    const REFERENCE_SNAPSHOT_DIR: &str =
        "/shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool";

    fn write_checkpoint(path: &std::path::Path, event_num: u64) {
        let checkpoint = JammedCheckpointV2::new(
            blake3::hash(b"kernel"),
            event_num,
            JammedNoun::new(Bytes::from_static(b"cold")),
            JammedNoun::new(Bytes::from_static(b"state")),
        );
        let bytes = checkpoint.encode().expect("encode checkpoint");
        std::fs::write(path, bytes).expect("write checkpoint");
    }

    #[test]
    fn boot_source_parses_authored_checkpoint_json() {
        let boot: BootSourceInput =
            serde_json::from_str(r#"{"type":"checkpoint","checkpoint":"checkpoint.chkjam"}"#)
                .expect("parse checkpoint boot source");

        assert_eq!(
            boot,
            BootSourceInput::Checkpoint {
                checkpoint: PathBuf::from("checkpoint.chkjam")
            }
        );
    }

    #[test]
    fn boot_source_parses_authored_snapshot_json() {
        let boot: BootSourceInput = serde_json::from_str(
            r#"{"type":"snapshot","pma":"snapshot.pma","manifest":"snapshot.manifest"}"#,
        )
        .expect("parse snapshot boot source");

        assert_eq!(
            boot,
            BootSourceInput::Snapshot {
                pma: PathBuf::from("snapshot.pma"),
                manifest: PathBuf::from("snapshot.manifest")
            }
        );
    }

    #[test]
    fn boot_source_rejects_incomplete_snapshot_pairs() {
        let err = BootSourceInput::from_cli_parts(None, Some(PathBuf::from("snapshot.pma")), None)
            .expect_err("missing manifest should fail");
        assert!(matches!(
            err,
            BootSourceError::IncompleteSnapshotPair { .. }
        ));

        let err =
            BootSourceInput::from_cli_parts(None, None, Some(PathBuf::from("snapshot.manifest")))
                .expect_err("missing PMA should fail");
        assert!(matches!(
            err,
            BootSourceError::IncompleteSnapshotPair { .. }
        ));
    }

    #[test]
    fn checkpoint_boot_source_normalizes_to_trusted_checkpoint_id() {
        let temp = TempDir::new().expect("temp dir");
        let checkpoint = temp.path().join("checkpoint.chkjam");
        write_checkpoint(&checkpoint, 123);

        let resolved = BootSourceInput::Checkpoint {
            checkpoint: checkpoint.clone(),
        }
        .resolve()
        .expect("resolve checkpoint boot source");

        assert_eq!(
            resolved.trusted_source(),
            TrustedBootSource::Checkpoint {
                checkpoint_input_id: "checkpoint-0".to_string(),
                event_num: Some(123)
            }
        );
        let files = resolved.trusted_input_files().expect("trusted files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].input_id, "checkpoint-0");
        assert_eq!(files[0].role, BootSourceFileRole::Checkpoint);
        assert_eq!(
            files[0].absolute_path,
            checkpoint.canonicalize().expect("canonicalize")
        );
        assert!(!files[0].sha256_hex.is_empty());
        assert!(files[0].size_bytes > 0);
    }

    #[test]
    fn snapshot_boot_source_normalizes_to_stable_trusted_snapshot_ids() {
        let dir = PathBuf::from(REFERENCE_SNAPSHOT_DIR);
        let pma = dir.join("snapshot.pma");
        let manifest = dir.join("snapshot.manifest");

        let resolved = BootSourceInput::Snapshot {
            pma: pma.clone(),
            manifest: manifest.clone(),
        }
        .resolve()
        .expect("resolve snapshot boot source");

        let event_num = resolved.event_num().expect("snapshot event_num");
        assert_eq!(
            resolved.trusted_source(),
            TrustedBootSource::Snapshot {
                pma_input_id: "snapshot-pma-0".to_string(),
                manifest_input_id: "snapshot-manifest-0".to_string(),
                event_num,
            }
        );
        assert_eq!(resolved.kind(), BootSourceKind::Snapshot);
        let files = resolved.trusted_input_files().expect("trusted files");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].input_id, "snapshot-pma-0");
        assert_eq!(files[0].role, BootSourceFileRole::SnapshotPma);
        assert_eq!(files[1].input_id, "snapshot-manifest-0");
        assert_eq!(files[1].role, BootSourceFileRole::SnapshotManifest);
        assert!(files.iter().all(|file| !file.sha256_hex.is_empty()));
    }

    #[test]
    fn snapshot_input_ids_are_stable_for_unusual_extensions() {
        let dir = PathBuf::from(REFERENCE_SNAPSHOT_DIR);
        let pma = dir.join("snapshot.pma");
        let manifest = dir.join("snapshot.manifest");
        let temp = TempDir::new().expect("temp dir");
        let odd_pma = temp.path().join("snapshot-without-extension");
        let odd_manifest = temp.path().join("manifest.weird");
        std::fs::copy(&pma, &odd_pma).expect("copy pma");
        std::fs::copy(&manifest, &odd_manifest).expect("copy manifest");

        let resolved = BootSourceInput::Snapshot {
            pma: odd_pma,
            manifest: odd_manifest,
        }
        .resolve()
        .expect("resolve snapshot boot source");
        let files = resolved.trusted_input_files().expect("trusted files");

        assert_eq!(files[0].input_id, "snapshot-pma-0");
        assert_eq!(files[1].input_id, "snapshot-manifest-0");
    }
}
