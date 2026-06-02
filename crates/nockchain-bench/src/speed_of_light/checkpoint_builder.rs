//! Build checkpoints by replaying archived blocks into a kernel.

use std::path::PathBuf;

use thiserror::Error;

use super::checkpoint::{CheckpointLoadError, CheckpointMetaError};
use super::kernel_utils::{KernelInitError, PeekChainError};
use super::start_height::StartHeightError;
use super::types::SolHeight;

#[derive(Debug, Error)]
pub enum CheckpointBuildError {
    #[error("Archive error: {0}")]
    Archive(#[from] super::archive::ArchiveError),

    #[error("Unsupported checkpoint path: {0}")]
    Unsupported(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Kernel load error: {0}")]
    KernelLoad(String),

    #[error("Checkpoint load error: {0}")]
    CheckpointLoad(#[from] CheckpointLoadError),

    #[error("Checkpoint metadata error: {0}")]
    CheckpointMeta(#[from] CheckpointMetaError),

    #[error("Cue error: {0}")]
    Cue(String),

    #[error("Poke error: {0}")]
    Poke(String),

    #[error("Noun decode error: {0}")]
    NounDecode(#[from] noun_serde::NounDecodeError),

    #[error("Start height error: {0}")]
    StartHeight(#[from] StartHeightError),

    #[error("Checkpoint chain height unavailable; pass --start-height explicitly")]
    CheckpointHeightUnavailable,

    #[error("Invalid height range: start {start} > target {target}")]
    InvalidHeightRange { start: u64, target: u64 },

    #[error("NockApp error: {0}")]
    NockApp(#[from] nockapp::nockapp::NockAppError),

    #[error("Kernel init error: {0}")]
    KernelInit(#[from] KernelInitError),

    #[error("Chain height peek error: {0}")]
    ChainPeek(#[from] PeekChainError),
}

#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    pub archive_path: String,
    pub kernel_path: String,
    pub checkpoint_path: Option<String>,
    pub build_mode: CheckpointBuildMode,
    pub start_height: Option<SolHeight>,
    pub target_height: SolHeight,
    pub output_path: PathBuf,
    pub work_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointBuildMode {
    Derived,
    Full,
}

#[derive(Debug, Clone)]
pub struct CheckpointResult {
    pub start_height: SolHeight,
    pub target_height: SolHeight,
    pub blocks_poked: u64,
    pub output_path: PathBuf,
}

pub struct CheckpointBuilder {
    config: CheckpointConfig,
}

impl CheckpointBuilder {
    pub fn new(config: CheckpointConfig) -> Self {
        Self { config }
    }

    pub async fn initialize(&mut self) -> Result<(), CheckpointBuildError> {
        let _ = &self.config;
        unsupported_checkpoint_materialization()
    }

    pub async fn run(&mut self) -> Result<CheckpointResult, CheckpointBuildError> {
        let _ = &self.config;
        unsupported_checkpoint_materialization()
    }
}

fn unsupported_checkpoint_materialization<T>() -> Result<T, CheckpointBuildError> {
    Err(CheckpointBuildError::Unsupported(
        "checkpoint materialization is not supported by current PMA replay".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_builder_reports_current_pma_replay_unsupported() {
        let err = unsupported_checkpoint_materialization::<()>()
            .expect_err("checkpoint materialization should be rejected");
        assert!(matches!(err, CheckpointBuildError::Unsupported(_)));
        assert!(err
            .to_string()
            .contains("checkpoint materialization is not supported by current PMA replay"));
    }
}
