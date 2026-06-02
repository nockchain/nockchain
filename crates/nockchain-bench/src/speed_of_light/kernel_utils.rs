//! Shared kernel initialization and peek helpers for speed-of-light tooling.

use std::path::{Path, PathBuf};

use nockapp::nockapp::save::SaveableCheckpoint;
use nockapp::nockapp::wire::WireRepr;
use nockapp::nockapp::NockApp;
use nockapp::noun::slab::NounSlab;
use nockchain_types::tx_engine::common::{BlockHeight, Hash, Page};
use nockvm::noun::SIG;
use thiserror::Error;

use super::boot_source::ResolvedBootSource;
use super::checkpoint::{load_saveable_checkpoint, CheckpointLoadError};
use super::{noun_compat, pma_replay};

#[derive(Debug, Error)]
pub enum KernelInitError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("NockApp error: {0}")]
    NockApp(#[from] nockapp::nockapp::NockAppError),

    #[error("Kernel boot error: {0}")]
    Boot(String),
}

#[derive(Debug, Error)]
pub enum CheckpointBackedInitError {
    #[error("failed to load checkpoint: {0}")]
    CheckpointLoad(#[from] CheckpointLoadError),

    #[error("failed to initialize checkpoint-backed kernel: {0}")]
    KernelInit(#[from] KernelInitError),
}

#[derive(Debug, Error)]
pub enum BootSourceBackedInitError {
    #[error("failed to load checkpoint: {0}")]
    CheckpointLoad(#[from] CheckpointLoadError),

    #[error("failed to initialize boot-source-backed kernel: {0}")]
    KernelInit(#[from] KernelInitError),
}

#[derive(Debug, Error)]
pub enum PeekChainError {
    #[error("NockApp error: {0}")]
    NockApp(#[from] nockapp::nockapp::NockAppError),

    #[error("Noun decode error: {0}")]
    NounDecode(#[from] noun_serde::NounDecodeError),
}

/// Minimal valid libp2p peer id used for synthetic SOL replay wires.
const SOL_REPLAY_PEER_ID: &str = "11";
/// Canonical wire for replaying archived `%heard-block` facts.
///
/// This mirrors the normal network ingress path:
/// `[%poke %libp2p 1 %gossip %peer-id <peer-id> ~]`.
pub fn sol_replay_wire() -> WireRepr {
    WireRepr::new(
        "libp2p",
        1,
        vec!["gossip".into(), "peer-id".into(), SOL_REPLAY_PEER_ID.into()],
    )
}

/// Initialize a NockApp with a kernel and optional checkpoint.
pub async fn init_nockapp(
    kernel_path: &Path,
    checkpoint: Option<SaveableCheckpoint>,
    work_dir: &PathBuf,
    prefer_existing_checkpoint: bool,
    fsync: bool,
) -> Result<NockApp, KernelInitError> {
    if prefer_existing_checkpoint {
        return Err(KernelInitError::Boot(
            "prefer_existing_checkpoint replay is not supported by PMA replay".to_string(),
        ));
    }

    pma_replay::init_replay_nockapp(kernel_path, checkpoint, work_dir, fsync).await
}

/// Load a checkpoint from disk and boot a checkpoint-backed NockApp.
pub async fn init_checkpoint_backed_nockapp(
    checkpoint_path: &Path,
    kernel_path: &Path,
    work_dir: &PathBuf,
    fsync: bool,
) -> Result<NockApp, CheckpointBackedInitError> {
    let checkpoint = load_saveable_checkpoint(checkpoint_path)?;
    init_nockapp(kernel_path, Some(checkpoint), work_dir, false, fsync)
        .await
        .map_err(Into::into)
}

pub async fn init_boot_source_backed_nockapp(
    boot_source: &ResolvedBootSource,
    kernel_path: &Path,
    work_dir: &PathBuf,
    fsync: bool,
) -> Result<NockApp, BootSourceBackedInitError> {
    match boot_source {
        ResolvedBootSource::Checkpoint { checkpoint, .. } => {
            init_checkpoint_backed_nockapp(checkpoint, kernel_path, work_dir, fsync)
                .await
                .map_err(|error| match error {
                    CheckpointBackedInitError::CheckpointLoad(source) => {
                        BootSourceBackedInitError::CheckpointLoad(source)
                    }
                    CheckpointBackedInitError::KernelInit(source) => {
                        BootSourceBackedInitError::KernelInit(source)
                    }
                })
        }
        ResolvedBootSource::Snapshot { pma, manifest, .. } => {
            pma_replay::init_snapshot_replay_nockapp(kernel_path, pma, manifest, work_dir, fsync)
                .await
                .map_err(Into::into)
        }
    }
}

/// Peek the heaviest chain (height, hash) from a running NockApp.
pub async fn peek_heaviest_chain(
    nockapp: &mut NockApp,
) -> Result<Option<(BlockHeight, Hash)>, PeekChainError> {
    let mut path_slab = NounSlab::new();
    let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-chain").as_noun();
    let path_noun = nockvm::noun::T(&mut path_slab, &[tag, SIG]);
    path_slab.set_root(path_noun);

    let result = nockapp.peek(path_slab).await?;
    let result_noun = unsafe { result.root() };
    let result_space = noun_compat::space_for_slab(&result);
    decode_heaviest_chain_result(*result_noun, &result_space)
}

pub async fn peek_heaviest_chain_or_block(
    nockapp: &mut NockApp,
) -> Result<Option<(BlockHeight, Hash)>, PeekChainError> {
    if let Some(tip) = peek_heaviest_chain(nockapp).await? {
        return Ok(Some(tip));
    }

    peek_heaviest_block(nockapp).await
}

async fn peek_heaviest_block(
    nockapp: &mut NockApp,
) -> Result<Option<(BlockHeight, Hash)>, PeekChainError> {
    let mut path_slab = NounSlab::new();
    let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-block").as_noun();
    let path_noun = nockvm::noun::T(&mut path_slab, &[tag, SIG]);
    path_slab.set_root(path_noun);

    let result = nockapp.peek(path_slab).await?;
    let result_noun = unsafe { result.root() };
    let result_space = noun_compat::space_for_slab(&result);
    decode_heaviest_block_result(*result_noun, &result_space)
}

fn decode_heaviest_chain_result(
    result_noun: nockvm::noun::Noun,
    space: &noun_compat::NounSpace,
) -> Result<Option<(BlockHeight, Hash)>, PeekChainError> {
    let opt: Option<Option<(BlockHeight, Hash)>> =
        noun_compat::decode_with_space(&result_noun, space)?;
    Ok(opt.flatten())
}

fn decode_heaviest_block_result(
    result_noun: nockvm::noun::Noun,
    space: &noun_compat::NounSpace,
) -> Result<Option<(BlockHeight, Hash)>, PeekChainError> {
    let opt: Option<Option<Page>> = noun_compat::decode_with_space(&result_noun, space)?;
    Ok(opt.flatten().map(|page| {
        (
            BlockHeight(nockchain_math::belt::Belt(page.height)),
            page.digest,
        )
    }))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use nockchain_math::belt::Belt;
    use noun_serde::NounEncode;

    use super::*;

    #[test]
    fn test_sol_replay_wire_matches_libp2p_gossip_shape() {
        let wire = sol_replay_wire();
        assert_eq!(wire.source, "libp2p");
        assert_eq!(wire.version, 1);
        assert_eq!(wire.tags_as_csv(), "libp2p,1,gossip,peer-id,11");
    }

    #[test]
    fn test_decode_heaviest_chain_result_reads_optional_height_and_hash() {
        let mut slab: NounSlab = NounSlab::new();
        let expected_hash = Hash([Belt(10), Belt(11), Belt(12), Belt(13), Belt(14)]);
        let response = Some(Some((BlockHeight(Belt(42)), expected_hash.clone())));
        let noun = response.to_noun(&mut slab);
        let space = noun_compat::space_for_slab(&slab);

        let decoded =
            decode_heaviest_chain_result(noun, &space).expect("heaviest-chain decode should work");

        let (height, hash) = decoded.expect("heaviest-chain peek should produce data");
        assert_eq!(height.0 .0, 42);
        assert_eq!(hash.to_base58(), expected_hash.to_base58());
    }

    #[test]
    fn test_decode_heaviest_chain_result_returns_none_when_chain_is_missing() {
        let mut slab: NounSlab = NounSlab::new();
        let response: Option<Option<(BlockHeight, Hash)>> = Some(None);
        let noun = response.to_noun(&mut slab);
        let space = noun_compat::space_for_slab(&slab);

        let decoded =
            decode_heaviest_chain_result(noun, &space).expect("heaviest-chain decode should work");

        assert!(decoded.is_none());
    }

    #[test]
    fn test_decode_heaviest_block_result_reads_page_height_and_hash() {
        let mut slab: NounSlab = NounSlab::new();
        let expected_hash = Hash([Belt(10), Belt(11), Belt(12), Belt(13), Belt(14)]);
        let page = Page {
            digest: expected_hash.clone(),
            pow: None,
            parent: Hash([Belt(20), Belt(21), Belt(22), Belt(23), Belt(24)]),
            tx_ids: Vec::new(),
            coinbase: nockchain_types::tx_engine::common::CoinbaseSplit::V1,
            timestamp: 0,
            epoch_counter: 0,
            target: nockchain_types::tx_engine::common::BigNum::from_u64(1),
            accumulated_work: nockchain_types::tx_engine::common::BigNum::from_u64(1),
            height: 42,
            msg: Vec::new(),
        };
        let response = Some(Some(page));
        let noun = response.to_noun(&mut slab);
        let space = noun_compat::space_for_slab(&slab);

        let decoded =
            decode_heaviest_block_result(noun, &space).expect("heaviest-block decode should work");

        let (height, hash) = decoded.expect("heaviest-block peek should produce data");
        assert_eq!(height.0 .0, 42);
        assert_eq!(hash.to_base58(), expected_hash.to_base58());
    }

    #[tokio::test]
    async fn test_pma_init_nockapp_rejects_prefer_existing_checkpoint() {
        let err = match init_nockapp(
            Path::new("unused-kernel"),
            None,
            &PathBuf::from("."),
            true,
            true,
        )
        .await
        {
            Ok(_) => panic!("PMA replay wrapper should reject prefer_existing_checkpoint"),
            Err(err) => err,
        };

        match err {
            KernelInitError::Boot(message) => assert_eq!(
                message,
                "prefer_existing_checkpoint replay is not supported by PMA replay"
            ),
            other => panic!("expected boot error, got {other:?}"),
        }
    }
}
