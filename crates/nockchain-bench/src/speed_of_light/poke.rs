//! Helpers for building pokes from archived block entries.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use bytes::Bytes;
use nockapp::nockapp::wire::WireRepr;
use nockapp::nockapp::{NockApp, NockAppError};
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, D, T};
use thiserror::Error;

use super::archive::{ArchiveError, BlockEntry, SolArchiveReader};
use super::profiling::sample_process_status;
use super::{noun_compat, pma_replay};

/// Extract the page noun from a block entry noun.
///
/// Block entry structure: [height [block_id [page txs]]]
pub(crate) fn extract_page_from_entry(
    entry_noun: Noun,
    space: &noun_compat::NounSpace,
) -> Result<Noun, String> {
    let tail =
        noun_compat::noun_tail(entry_noun, space).map_err(|_| "tail not a cell".to_string())?;
    let page_txs =
        noun_compat::noun_tail(tail, space).map_err(|_| "page_txs not a cell".to_string())?;
    noun_compat::noun_head(page_txs, space).map_err(|_| "page_txs not a cell".to_string())
}

/// Construct a poke cause: [%fact 0 [%heard-block page]]
pub fn make_heard_block_cause(page: Noun, slab: &mut NounSlab) -> Noun {
    let fact_tag = nockapp::utils::make_tas(slab, "fact").as_noun();
    let heard_block_tag = nockapp::utils::make_tas(slab, "heard-block").as_noun();

    let heard_block = T(slab, &[heard_block_tag, page]);
    // dumbnet fact payload now requires explicit version=%0.
    T(slab, &[fact_tag, D(0), heard_block])
}

/// Construct a poke cause: [%fact 0 [%heard-tx raw-tx]]
pub fn make_heard_tx_cause(raw_tx: Noun, slab: &mut NounSlab) -> Noun {
    let fact_tag = nockapp::utils::make_tas(slab, "fact").as_noun();
    let heard_tx_tag = nockapp::utils::make_tas(slab, "heard-tx").as_noun();

    let heard_tx = T(slab, &[heard_tx_tag, raw_tx]);
    T(slab, &[fact_tag, D(0), heard_tx])
}

fn cue_jam_into_slab(slab: &mut NounSlab, jam_bytes: &[u8]) -> Result<Noun, String> {
    match catch_unwind(AssertUnwindSafe(|| {
        slab.cue_into(Bytes::copy_from_slice(jam_bytes))
    })) {
        Ok(Ok(noun)) => Ok(noun),
        Ok(Err(error)) => Err(format!("cue failed: {error:?}")),
        Err(_) => Err("cue panicked".to_string()),
    }
}

/// Build a poke slab from jammed block-entry bytes.
///
/// This cues the entry noun, extracts the page, and builds the [%fact 0 [%heard-block page]] cause.
pub fn build_poke_slab_from_jam(jam_bytes: &[u8]) -> Result<NounSlab, String> {
    let mut entry_slab: NounSlab = NounSlab::new();
    let entry_noun = cue_jam_into_slab(&mut entry_slab, jam_bytes)?;

    let entry_space = noun_compat::space_for_slab(&entry_slab);
    let page = extract_page_from_entry(entry_noun, &entry_space)
        .map_err(|e| format!("extract page failed: {e}"))?;

    let mut poke_slab = NounSlab::new();
    let page_copy = pma_replay::copy_from_source_slab(&mut poke_slab, page, &entry_slab);
    let cause = make_heard_block_cause(page_copy, &mut poke_slab);
    poke_slab.set_root(cause);

    Ok(poke_slab)
}

/// Build a poke slab from jammed raw transaction bytes.
pub fn build_heard_tx_poke_slab_from_jam(jam_bytes: &[u8]) -> Result<NounSlab, String> {
    let mut raw_tx_slab: NounSlab = NounSlab::new();
    let raw_tx = cue_jam_into_slab(&mut raw_tx_slab, jam_bytes)?;

    let mut poke_slab = NounSlab::new();
    let raw_tx_copy = pma_replay::copy_from_source_slab(&mut poke_slab, raw_tx, &raw_tx_slab);
    let cause = make_heard_tx_cause(raw_tx_copy, &mut poke_slab);
    poke_slab.set_root(cause);

    Ok(poke_slab)
}

#[derive(Debug, Error)]
pub enum PokeStepError {
    #[error("failed to build poke slab: {0}")]
    Build(String),

    #[error("failed to build poke slab: {message}")]
    ArchivePrebuild {
        message: String,
        timings: ArchivePokeTimings,
    },

    #[error("archive error: {source}")]
    ArchiveRead {
        source: ArchiveError,
        timings: ArchivePokeTimings,
    },

    #[error("failed to poke block: {0}")]
    Poke(#[from] NockAppError),

    #[error("failed to poke block: {source}")]
    ArchivePoke {
        source: NockAppError,
        timings: ArchivePokeTimings,
    },

    #[error("archive error: {0}")]
    Archive(#[from] ArchiveError),
}

impl PokeStepError {
    pub fn archive_poke_timings(&self) -> Option<ArchivePokeTimings> {
        match self {
            Self::ArchivePrebuild { timings, .. } => Some(*timings),
            Self::ArchiveRead { timings, .. } => Some(*timings),
            Self::ArchivePoke { timings, .. } => Some(*timings),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivePokeTimings {
    pub block_duration: Duration,
    pub raw_tx_duration: Duration,
    pub total_duration: Duration,
    pub slab_prebuild_duration: Duration,
    pub block_slab_prebuild_duration: Duration,
    pub raw_tx_slab_prebuild_duration: Duration,
    pub slab_prebuild_start_rss_bytes: Option<u64>,
    pub slab_prebuild_peak_rss_bytes: Option<u64>,
    pub raw_tx_pokes_completed: u64,
    pub raw_tx_slabs_prebuilt: u64,
    pub raw_tx_payload_bytes_prebuilt: u64,
}

struct ArchiveBlockPokeSlabs {
    block: NounSlab,
    raw_txs: Vec<NounSlab>,
    metrics: ArchivePokePrebuildMetrics,
}

#[derive(Debug, Clone, Copy)]
struct ArchivePokePrebuildMetrics {
    slab_prebuild_duration: Duration,
    block_slab_prebuild_duration: Duration,
    raw_tx_slab_prebuild_duration: Duration,
    slab_prebuild_start_rss_bytes: Option<u64>,
    slab_prebuild_peak_rss_bytes: Option<u64>,
    raw_tx_slabs_prebuilt: u64,
    raw_tx_payload_bytes_prebuilt: u64,
}

impl ArchivePokePrebuildMetrics {
    fn timings(
        self,
        block_duration: Duration,
        raw_tx_duration: Duration,
        raw_tx_pokes_completed: u64,
    ) -> ArchivePokeTimings {
        ArchivePokeTimings {
            block_duration,
            raw_tx_duration,
            total_duration: block_duration + raw_tx_duration,
            slab_prebuild_duration: self.slab_prebuild_duration,
            block_slab_prebuild_duration: self.block_slab_prebuild_duration,
            raw_tx_slab_prebuild_duration: self.raw_tx_slab_prebuild_duration,
            slab_prebuild_start_rss_bytes: self.slab_prebuild_start_rss_bytes,
            slab_prebuild_peak_rss_bytes: self.slab_prebuild_peak_rss_bytes,
            raw_tx_pokes_completed,
            raw_tx_slabs_prebuilt: self.raw_tx_slabs_prebuilt,
            raw_tx_payload_bytes_prebuilt: self.raw_tx_payload_bytes_prebuilt,
        }
    }
}

fn current_rss_bytes() -> Option<u64> {
    let pid = i32::try_from(std::process::id()).ok()?;
    sample_process_status(pid, 0).map(|sample| sample.rss_bytes())
}

fn record_prebuild_rss(peak: &mut Option<u64>) {
    if let Some(rss) = current_rss_bytes() {
        *peak = Some(peak.map_or(rss, |current| current.max(rss)));
    }
}

struct PrebuildTimingState {
    slab_started_at: Instant,
    block_started_at: Instant,
    raw_tx_started_at: Option<Instant>,
    block_slab_prebuild_duration: Duration,
    rss_sampling_duration: Duration,
    raw_tx_rss_sampling_duration: Duration,
    raw_tx_payload_bytes_prebuilt: u64,
    raw_tx_slabs_prebuilt: u64,
    start_rss_bytes: Option<u64>,
    peak_rss_bytes: Option<u64>,
}

impl PrebuildTimingState {
    fn start() -> Self {
        let start_rss_bytes = current_rss_bytes();
        let now = Instant::now();
        Self {
            slab_started_at: now,
            block_started_at: now,
            raw_tx_started_at: None,
            block_slab_prebuild_duration: Duration::ZERO,
            rss_sampling_duration: Duration::ZERO,
            raw_tx_rss_sampling_duration: Duration::ZERO,
            raw_tx_payload_bytes_prebuilt: 0,
            raw_tx_slabs_prebuilt: 0,
            start_rss_bytes,
            peak_rss_bytes: start_rss_bytes,
        }
    }

    fn finish_block(&mut self) {
        self.block_slab_prebuild_duration = self.block_started_at.elapsed();
        self.rss_sampling_duration += sample_prebuild_rss(&mut self.peak_rss_bytes);
        self.raw_tx_started_at = Some(Instant::now());
    }

    fn finish_raw_tx_slab(&mut self, payload_len: usize) {
        self.raw_tx_payload_bytes_prebuilt = self
            .raw_tx_payload_bytes_prebuilt
            .saturating_add(payload_len as u64);
        self.raw_tx_slabs_prebuilt = self.raw_tx_slabs_prebuilt.saturating_add(1);
        let sampling_duration = sample_prebuild_rss(&mut self.peak_rss_bytes);
        self.rss_sampling_duration += sampling_duration;
        self.raw_tx_rss_sampling_duration += sampling_duration;
    }

    fn raw_tx_slab_prebuild_duration(&self) -> Duration {
        self.raw_tx_started_at
            .map(|started_at| {
                started_at
                    .elapsed()
                    .saturating_sub(self.raw_tx_rss_sampling_duration)
            })
            .unwrap_or_default()
    }

    fn slab_prebuild_duration(&self) -> Duration {
        self.slab_started_at
            .elapsed()
            .saturating_sub(self.rss_sampling_duration)
    }

    fn timings(&self) -> ArchivePokeTimings {
        let raw_tx_slab_prebuild_duration = self.raw_tx_slab_prebuild_duration();
        let slab_prebuild_duration = self.slab_prebuild_duration();
        ArchivePokeTimings {
            block_duration: self.block_slab_prebuild_duration,
            raw_tx_duration: raw_tx_slab_prebuild_duration,
            total_duration: slab_prebuild_duration,
            slab_prebuild_duration,
            block_slab_prebuild_duration: self.block_slab_prebuild_duration,
            raw_tx_slab_prebuild_duration,
            slab_prebuild_start_rss_bytes: self.start_rss_bytes,
            slab_prebuild_peak_rss_bytes: self.peak_rss_bytes,
            raw_tx_pokes_completed: 0,
            raw_tx_slabs_prebuilt: self.raw_tx_slabs_prebuilt,
            raw_tx_payload_bytes_prebuilt: self.raw_tx_payload_bytes_prebuilt,
        }
    }

    fn build_error(&self, message: String) -> PokeStepError {
        PokeStepError::ArchivePrebuild {
            message,
            timings: self.timings(),
        }
    }

    fn archive_error(&self, error: ArchiveError) -> PokeStepError {
        PokeStepError::ArchiveRead {
            source: error,
            timings: self.timings(),
        }
    }
}

fn sample_prebuild_rss(peak: &mut Option<u64>) -> Duration {
    let started_at = Instant::now();
    record_prebuild_rss(peak);
    started_at.elapsed()
}

fn build_archive_block_poke_slabs(
    reader: &SolArchiveReader,
    entry: &BlockEntry,
) -> Result<ArchiveBlockPokeSlabs, PokeStepError> {
    let body = reader.body();

    let mut timings = PrebuildTimingState::start();
    let block_jam = body
        .get_jam_for_entry(entry)
        .map_err(|error| timings.archive_error(error))?;
    let block =
        build_poke_slab_from_jam(block_jam).map_err(|message| timings.build_error(message))?;
    timings.finish_block();
    let mut raw_txs = Vec::new();

    for raw_tx_entry in body
        .raw_tx_entries_for_block(entry)
        .map_err(|error| timings.archive_error(error))?
    {
        let payload = body
            .get_raw_tx_payload(raw_tx_entry)
            .map_err(|error| timings.archive_error(error))?;
        let payload_len = payload.len();
        let poke_slab = build_heard_tx_poke_slab_from_jam(payload)
            .map_err(|message| timings.build_error(message))?;
        raw_txs.push(poke_slab);
        timings.finish_raw_tx_slab(payload_len);
    }

    Ok(ArchiveBlockPokeSlabs {
        block,
        raw_txs,
        metrics: ArchivePokePrebuildMetrics {
            slab_prebuild_duration: timings.slab_prebuild_duration(),
            block_slab_prebuild_duration: timings.block_slab_prebuild_duration,
            raw_tx_slab_prebuild_duration: timings.raw_tx_slab_prebuild_duration(),
            slab_prebuild_start_rss_bytes: timings.start_rss_bytes,
            slab_prebuild_peak_rss_bytes: timings.peak_rss_bytes,
            raw_tx_slabs_prebuilt: timings.raw_tx_slabs_prebuilt,
            raw_tx_payload_bytes_prebuilt: timings.raw_tx_payload_bytes_prebuilt,
        },
    })
}

fn archive_poke_error(
    source: NockAppError,
    metrics: ArchivePokePrebuildMetrics,
    block_duration: Duration,
    raw_tx_duration: Duration,
    raw_tx_pokes_completed: u64,
) -> PokeStepError {
    PokeStepError::ArchivePoke {
        source,
        timings: metrics.timings(block_duration, raw_tx_duration, raw_tx_pokes_completed),
    }
}

async fn poke_archive_block_slabs(
    nockapp: &mut NockApp,
    wire: WireRepr,
    slabs: ArchiveBlockPokeSlabs,
) -> Result<ArchivePokeTimings, PokeStepError> {
    let ArchiveBlockPokeSlabs {
        block,
        raw_txs,
        metrics,
    } = slabs;

    let block_started_at = Instant::now();
    nockapp
        .poke(wire.clone(), block)
        .await
        .map(|_| ())
        .map_err(|source| {
            archive_poke_error(
                source,
                metrics,
                metrics.block_slab_prebuild_duration + block_started_at.elapsed(),
                metrics.raw_tx_slab_prebuild_duration,
                0,
            )
        })?;
    let block_duration = metrics.block_slab_prebuild_duration + block_started_at.elapsed();
    let raw_tx_started_at = Instant::now();
    let mut raw_tx_pokes_completed = 0u64;

    for poke_slab in raw_txs {
        nockapp
            .poke(wire.clone(), poke_slab)
            .await
            .map(|_| ())
            .map_err(|source| {
                archive_poke_error(
                    source,
                    metrics,
                    block_duration,
                    metrics.raw_tx_slab_prebuild_duration + raw_tx_started_at.elapsed(),
                    raw_tx_pokes_completed,
                )
            })?;
        raw_tx_pokes_completed = raw_tx_pokes_completed.saturating_add(1);
    }

    let raw_tx_duration = metrics.raw_tx_slab_prebuild_duration + raw_tx_started_at.elapsed();
    Ok(metrics.timings(block_duration, raw_tx_duration, raw_tx_pokes_completed))
}

pub async fn poke_archive_block(
    nockapp: &mut NockApp,
    wire: WireRepr,
    reader: &SolArchiveReader,
    entry: &BlockEntry,
) -> Result<ArchivePokeTimings, PokeStepError> {
    let total_started_at = Instant::now();
    let slabs = build_archive_block_poke_slabs(reader, entry)?;
    let timings = poke_archive_block_slabs(nockapp, wire, slabs).await?;
    debug_assert!(timings.total_duration <= total_started_at.elapsed());
    Ok(timings)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;

    use nockapp::noun::slab::NockJammer;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;

    use super::super::noun_compat;
    use super::*;
    use crate::speed_of_light::archive::{RawTxPayload, SolArchiveReader, SolArchiveWriter};
    use crate::speed_of_light::types::{ProofVersion, SolHeight};

    fn dummy_hash(v: u64) -> Hash {
        Hash([Belt(v), Belt(v + 1), Belt(v + 2), Belt(v + 3), Belt(v + 4)])
    }

    fn block_entry_jam(height: u64) -> Vec<u8> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let page = T(&mut slab, &[D(1), D(2), D(3)]);
        let page_txs = T(&mut slab, &[page, D(0)]);
        let tail = T(&mut slab, &[D(height), page_txs]);
        let entry = T(&mut slab, &[D(height), tail]);
        slab.set_root(entry);
        slab.jam().as_ref().to_vec()
    }

    fn invalid_non_empty_raw_tx_jam() -> Vec<u8> {
        for len in 1..=16 {
            for byte in 0u8..=255 {
                let candidate = vec![byte; len];
                if build_heard_tx_poke_slab_from_jam(&candidate).is_err() {
                    return candidate;
                }
            }
        }
        panic!("expected at least one non-empty malformed jam candidate");
    }

    fn assert_versioned_fact_cause<J>(slab: &NounSlab<J>, expected_payload_tag: &str) {
        let space = noun_compat::space_for_slab(slab);
        let root = unsafe { slab.root() };

        let fact_tag_noun = noun_compat::noun_head(*root, &space).expect("cause tag");
        let fact_tag =
            noun_compat::decode_with_space::<String>(&fact_tag_noun, &space).expect("fact tag");
        assert_eq!(fact_tag, "fact");

        let fact_payload = noun_compat::noun_tail(*root, &space).expect("fact payload");
        let version_noun = noun_compat::noun_head(fact_payload, &space).expect("version");
        let version =
            noun_compat::decode_with_space::<u64>(&version_noun, &space).expect("fact version");
        assert_eq!(version, 0);

        let data_noun = noun_compat::noun_tail(fact_payload, &space).expect("fact data");
        let payload_tag_noun = noun_compat::noun_head(data_noun, &space).expect("payload tag");
        let payload_tag = noun_compat::decode_with_space::<String>(&payload_tag_noun, &space)
            .expect("payload tag");
        assert_eq!(payload_tag, expected_payload_tag);
    }

    #[test]
    fn test_extract_page_from_entry_reads_page_from_entry_shape() {
        let mut entry_slab: NounSlab<NockJammer> = NounSlab::new();
        let page = T(&mut entry_slab, &[D(1), D(2), D(3)]);
        let page_txs = T(&mut entry_slab, &[page, D(0)]);
        let tail = T(&mut entry_slab, &[D(42), page_txs]);
        let entry = T(&mut entry_slab, &[D(7), tail]);

        let entry_space = noun_compat::space_for_slab(&entry_slab);
        let extracted =
            extract_page_from_entry(entry, &entry_space).expect("page extraction should succeed");

        let mut extracted_slab: NounSlab<NockJammer> = NounSlab::new();
        let extracted_copy =
            pma_replay::copy_from_source_slab(&mut extracted_slab, extracted, &entry_slab);
        extracted_slab.set_root(extracted_copy);

        let mut expected_slab: NounSlab<NockJammer> = NounSlab::new();
        let expected_copy =
            pma_replay::copy_from_source_slab(&mut expected_slab, page, &entry_slab);
        expected_slab.set_root(expected_copy);

        assert_eq!(extracted_slab.jam().as_ref(), expected_slab.jam().as_ref());
    }

    #[test]
    fn test_make_heard_block_cause_includes_fact_version_zero() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let page = T(&mut slab, &[D(11), D(22)]);
        let cause = make_heard_block_cause(page, &mut slab);
        slab.set_root(cause);

        assert_versioned_fact_cause(&slab, "heard-block");
    }

    #[test]
    fn test_make_heard_tx_cause_includes_fact_version_zero() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let raw_tx = T(&mut slab, &[D(1), D(22)]);
        let cause = make_heard_tx_cause(raw_tx, &mut slab);
        slab.set_root(cause);

        assert_versioned_fact_cause(&slab, "heard-tx");
    }

    #[test]
    fn test_build_poke_slab_from_jam_emits_versioned_fact() {
        let mut entry_slab: NounSlab<NockJammer> = NounSlab::new();
        let page = T(&mut entry_slab, &[D(1), D(2), D(3)]);
        let page_txs = T(&mut entry_slab, &[page, D(0)]);
        let tail = T(&mut entry_slab, &[D(42), page_txs]);
        let entry = T(&mut entry_slab, &[D(7), tail]);
        entry_slab.set_root(entry);
        let jammed = entry_slab.jam();

        let poke_slab = build_poke_slab_from_jam(jammed.as_ref()).expect("should build poke slab");
        assert_versioned_fact_cause(&poke_slab, "heard-block");
    }

    #[test]
    fn test_build_heard_tx_poke_slab_from_jam_emits_versioned_fact() {
        let mut raw_tx_slab: NounSlab<NockJammer> = NounSlab::new();
        let raw_tx = T(&mut raw_tx_slab, &[D(1), D(22)]);
        raw_tx_slab.set_root(raw_tx);
        let jammed = raw_tx_slab.jam();

        let poke_slab =
            build_heard_tx_poke_slab_from_jam(jammed.as_ref()).expect("should build poke slab");
        assert_versioned_fact_cause(&poke_slab, "heard-tx");
    }

    #[test]
    fn test_build_poke_slab_from_jam_rejects_invalid_jam_bytes() {
        let error = build_poke_slab_from_jam(b"not-a-jam").expect_err("invalid jam should fail");
        assert!(
            error.contains("cue failed") || error.contains("extract page failed"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_build_archive_block_poke_slabs_rejects_bad_raw_tx_before_poke() {
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_raw_txs(
                SolHeight(7),
                dummy_hash(700),
                ProofVersion::V0,
                &block_entry_jam(7),
                [RawTxPayload {
                    tx_id: dummy_hash(701),
                    jam_bytes: &[],
                }],
            )
            .expect("archive block should be accepted");
        let reader = SolArchiveReader::from_bytes(writer.to_bytes().expect("serialize"))
            .expect("read archive");
        let body = reader.body();
        let entry = body.get_entry_by_height(SolHeight(7)).expect("block entry");

        let error = match build_archive_block_poke_slabs(&reader, entry) {
            Ok(_) => panic!("bad raw tx jam should fail before any poke"),
            Err(error) => error,
        };
        let PokeStepError::ArchivePrebuild { timings, .. } = error else {
            panic!("bad raw tx jam should return prebuild metrics");
        };
        assert_eq!(timings.raw_tx_slabs_prebuilt, 0);
        assert_eq!(timings.raw_tx_payload_bytes_prebuilt, 0);
        assert!(timings.slab_prebuild_duration >= timings.block_slab_prebuild_duration);
    }

    #[test]
    fn test_build_archive_block_poke_slabs_preserves_completed_raw_tx_metrics_on_late_failure() {
        let raw_tx_a = block_entry_jam(11);
        let bad_raw_tx = invalid_non_empty_raw_tx_jam();
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_raw_txs(
                SolHeight(7),
                dummy_hash(700),
                ProofVersion::V0,
                &block_entry_jam(7),
                [
                    RawTxPayload {
                        tx_id: dummy_hash(701),
                        jam_bytes: &raw_tx_a,
                    },
                    RawTxPayload {
                        tx_id: dummy_hash(702),
                        jam_bytes: &bad_raw_tx,
                    },
                ],
            )
            .expect("archive block should be accepted");
        let reader = SolArchiveReader::from_bytes(writer.to_bytes().expect("serialize"))
            .expect("read archive");
        let body = reader.body();
        let entry = body.get_entry_by_height(SolHeight(7)).expect("block entry");

        let error = match build_archive_block_poke_slabs(&reader, entry) {
            Ok(_) => panic!("second raw tx should fail before any poke"),
            Err(error) => error,
        };
        let PokeStepError::ArchivePrebuild { timings, .. } = error else {
            panic!("bad raw tx jam should return prebuild metrics");
        };

        assert_eq!(timings.raw_tx_slabs_prebuilt, 1);
        assert_eq!(timings.raw_tx_payload_bytes_prebuilt, raw_tx_a.len() as u64);
        assert!(timings.slab_prebuild_duration >= timings.block_slab_prebuild_duration);
    }

    #[test]
    fn test_build_archive_block_poke_slabs_preserves_metrics_on_archive_read_error() {
        let raw_tx = block_entry_jam(11);
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_raw_txs(
                SolHeight(7),
                dummy_hash(700),
                ProofVersion::V0,
                &block_entry_jam(7),
                [RawTxPayload {
                    tx_id: dummy_hash(701),
                    jam_bytes: &raw_tx,
                }],
            )
            .expect("archive block should be accepted");
        let reader = SolArchiveReader::from_bytes(writer.to_bytes().expect("serialize"))
            .expect("read archive");
        let body = reader.body();
        let mut entry = body
            .get_entry_by_height(SolHeight(7))
            .expect("block entry")
            .clone();
        entry.raw_tx_start = 99;

        let error = match build_archive_block_poke_slabs(&reader, &entry) {
            Ok(_) => panic!("invalid raw tx range should fail"),
            Err(error) => error,
        };
        let PokeStepError::ArchiveRead { source, timings } = &error else {
            panic!("archive read error should return typed prebuild metrics");
        };

        assert!(
            error.to_string().contains("archive error"),
            "unexpected message: {error}"
        );
        assert!(
            matches!(source, ArchiveError::RawTxRangeOutOfBounds { .. }),
            "unexpected source: {source:?}"
        );
        assert_eq!(timings.raw_tx_slabs_prebuilt, 0);
        assert_eq!(timings.raw_tx_payload_bytes_prebuilt, 0);
        assert!(timings.slab_prebuild_duration >= timings.block_slab_prebuild_duration);
        assert_eq!(error.archive_poke_timings(), Some(*timings));
    }

    #[test]
    fn test_build_archive_block_poke_slabs_reports_prebuild_work() {
        let raw_tx_a = block_entry_jam(11);
        let raw_tx_b = block_entry_jam(12);
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_raw_txs(
                SolHeight(7),
                dummy_hash(700),
                ProofVersion::V0,
                &block_entry_jam(7),
                [
                    RawTxPayload {
                        tx_id: dummy_hash(701),
                        jam_bytes: &raw_tx_a,
                    },
                    RawTxPayload {
                        tx_id: dummy_hash(702),
                        jam_bytes: &raw_tx_b,
                    },
                ],
            )
            .expect("archive block should be accepted");
        let reader = SolArchiveReader::from_bytes(writer.to_bytes().expect("serialize"))
            .expect("read archive");
        let body = reader.body();
        let entry = body.get_entry_by_height(SolHeight(7)).expect("block entry");

        let slabs = build_archive_block_poke_slabs(&reader, entry).expect("prebuild slabs");

        assert_eq!(slabs.raw_txs.len(), 2);
        assert_eq!(
            slabs.metrics.raw_tx_payload_bytes_prebuilt,
            (raw_tx_a.len() + raw_tx_b.len()) as u64
        );
        assert!(slabs.metrics.slab_prebuild_duration >= slabs.metrics.block_slab_prebuild_duration);
        assert!(
            slabs.metrics.slab_prebuild_duration >= slabs.metrics.raw_tx_slab_prebuild_duration
        );
        if let (Some(start_rss), Some(peak_rss)) = (
            slabs.metrics.slab_prebuild_start_rss_bytes, slabs.metrics.slab_prebuild_peak_rss_bytes,
        ) {
            assert!(peak_rss >= start_rss);
        }
    }

    trait TestArchivePokeDriver {
        fn poke_archive_slab<'a>(
            &'a mut self,
            wire: WireRepr,
            slab: NounSlab,
        ) -> Pin<Box<dyn Future<Output = Result<(), NockAppError>> + Send + 'a>>;
    }

    async fn poke_archive_block_slabs_with_driver(
        driver: &mut impl TestArchivePokeDriver,
        wire: WireRepr,
        slabs: ArchiveBlockPokeSlabs,
    ) -> Result<ArchivePokeTimings, PokeStepError> {
        let ArchiveBlockPokeSlabs {
            block,
            raw_txs,
            metrics,
        } = slabs;

        let block_started_at = Instant::now();
        driver
            .poke_archive_slab(wire.clone(), block)
            .await
            .map_err(|source| {
                archive_poke_error(
                    source,
                    metrics,
                    metrics.block_slab_prebuild_duration + block_started_at.elapsed(),
                    metrics.raw_tx_slab_prebuild_duration,
                    0,
                )
            })?;
        let block_duration = metrics.block_slab_prebuild_duration + block_started_at.elapsed();
        let raw_tx_started_at = Instant::now();
        let mut raw_tx_pokes_completed = 0u64;

        for poke_slab in raw_txs {
            driver
                .poke_archive_slab(wire.clone(), poke_slab)
                .await
                .map_err(|source| {
                    archive_poke_error(
                        source,
                        metrics,
                        block_duration,
                        metrics.raw_tx_slab_prebuild_duration + raw_tx_started_at.elapsed(),
                        raw_tx_pokes_completed,
                    )
                })?;
            raw_tx_pokes_completed = raw_tx_pokes_completed.saturating_add(1);
        }

        let raw_tx_duration = metrics.raw_tx_slab_prebuild_duration + raw_tx_started_at.elapsed();
        Ok(metrics.timings(block_duration, raw_tx_duration, raw_tx_pokes_completed))
    }

    struct FailingArchivePokeDriver {
        calls: usize,
        fail_on_call: usize,
    }

    impl TestArchivePokeDriver for FailingArchivePokeDriver {
        fn poke_archive_slab<'a>(
            &'a mut self,
            _wire: WireRepr,
            _slab: NounSlab,
        ) -> Pin<Box<dyn Future<Output = Result<(), NockAppError>> + Send + 'a>> {
            self.calls += 1;
            let should_fail = self.calls == self.fail_on_call;
            Box::pin(async move {
                if should_fail {
                    Err(NockAppError::PokeFailed)
                } else {
                    Ok(())
                }
            })
        }
    }

    fn archive_block_poke_slabs_with_two_raw_txs() -> (ArchiveBlockPokeSlabs, usize) {
        let raw_tx_a = block_entry_jam(11);
        let raw_tx_b = block_entry_jam(12);
        let expected_raw_tx_payload_bytes = raw_tx_a.len() + raw_tx_b.len();
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_raw_txs(
                SolHeight(7),
                dummy_hash(700),
                ProofVersion::V0,
                &block_entry_jam(7),
                [
                    RawTxPayload {
                        tx_id: dummy_hash(701),
                        jam_bytes: &raw_tx_a,
                    },
                    RawTxPayload {
                        tx_id: dummy_hash(702),
                        jam_bytes: &raw_tx_b,
                    },
                ],
            )
            .expect("archive block should be accepted");
        let reader = SolArchiveReader::from_bytes(writer.to_bytes().expect("serialize"))
            .expect("read archive");
        let body = reader.body();
        let entry = body.get_entry_by_height(SolHeight(7)).expect("block entry");

        (
            build_archive_block_poke_slabs(&reader, entry).expect("prebuild slabs"),
            expected_raw_tx_payload_bytes,
        )
    }

    #[tokio::test]
    async fn poke_archive_block_slabs_preserves_metrics_on_block_poke_failure() {
        let (slabs, expected_raw_tx_payload_bytes) = archive_block_poke_slabs_with_two_raw_txs();
        let expected_metrics = slabs.metrics;
        let mut driver = FailingArchivePokeDriver {
            calls: 0,
            fail_on_call: 1,
        };

        let error =
            poke_archive_block_slabs_with_driver(&mut driver, WireRepr::no_tags("test", 0), slabs)
                .await
                .expect_err("block poke should fail");

        let PokeStepError::ArchivePoke { source, timings } = error else {
            panic!("block poke failure should carry archive poke timings");
        };
        assert!(matches!(source, NockAppError::PokeFailed));
        assert_eq!(driver.calls, 1);
        assert_eq!(timings.raw_tx_pokes_completed, 0);
        assert_eq!(timings.raw_tx_slabs_prebuilt, 2);
        assert_eq!(
            timings.raw_tx_payload_bytes_prebuilt,
            expected_raw_tx_payload_bytes as u64
        );
        assert_eq!(
            timings.block_slab_prebuild_duration,
            expected_metrics.block_slab_prebuild_duration
        );
        assert_eq!(
            timings.raw_tx_slab_prebuild_duration,
            expected_metrics.raw_tx_slab_prebuild_duration
        );
        assert_eq!(
            timings.slab_prebuild_start_rss_bytes,
            expected_metrics.slab_prebuild_start_rss_bytes
        );
        assert_eq!(
            timings.slab_prebuild_peak_rss_bytes,
            expected_metrics.slab_prebuild_peak_rss_bytes
        );
        assert_eq!(
            timings.total_duration,
            timings.block_duration + timings.raw_tx_duration
        );
    }

    #[tokio::test]
    async fn poke_archive_block_slabs_preserves_progress_on_raw_tx_poke_failure() {
        let (slabs, expected_raw_tx_payload_bytes) = archive_block_poke_slabs_with_two_raw_txs();
        let expected_metrics = slabs.metrics;
        let mut driver = FailingArchivePokeDriver {
            calls: 0,
            fail_on_call: 3,
        };

        let error =
            poke_archive_block_slabs_with_driver(&mut driver, WireRepr::no_tags("test", 0), slabs)
                .await
                .expect_err("second raw tx poke should fail");

        let PokeStepError::ArchivePoke { source, timings } = error else {
            panic!("raw tx poke failure should carry archive poke timings");
        };
        assert!(matches!(source, NockAppError::PokeFailed));
        assert_eq!(driver.calls, 3);
        assert_eq!(timings.raw_tx_pokes_completed, 1);
        assert_eq!(timings.raw_tx_slabs_prebuilt, 2);
        assert_eq!(
            timings.raw_tx_payload_bytes_prebuilt,
            expected_raw_tx_payload_bytes as u64
        );
        assert_eq!(
            timings.block_slab_prebuild_duration,
            expected_metrics.block_slab_prebuild_duration
        );
        assert_eq!(
            timings.raw_tx_slab_prebuild_duration,
            expected_metrics.raw_tx_slab_prebuild_duration
        );
        assert_eq!(
            timings.slab_prebuild_start_rss_bytes,
            expected_metrics.slab_prebuild_start_rss_bytes
        );
        assert_eq!(
            timings.slab_prebuild_peak_rss_bytes,
            expected_metrics.slab_prebuild_peak_rss_bytes
        );
        assert_eq!(
            timings.total_duration,
            timings.block_duration + timings.raw_tx_duration
        );
    }

    #[test]
    fn archive_poke_timings_total_duration_uses_adjusted_components() {
        let metrics = ArchivePokePrebuildMetrics {
            slab_prebuild_duration: Duration::from_millis(25),
            block_slab_prebuild_duration: Duration::from_millis(5),
            raw_tx_slab_prebuild_duration: Duration::from_millis(20),
            slab_prebuild_start_rss_bytes: Some(100),
            slab_prebuild_peak_rss_bytes: Some(150),
            raw_tx_slabs_prebuilt: 2,
            raw_tx_payload_bytes_prebuilt: 64,
        };

        let timings = metrics.timings(Duration::from_millis(17), Duration::from_millis(31), 2);

        assert_eq!(timings.total_duration, Duration::from_millis(48));
        assert_eq!(timings.slab_prebuild_duration, Duration::from_millis(25));
        assert_eq!(timings.block_duration, Duration::from_millis(17));
        assert_eq!(timings.raw_tx_duration, Duration::from_millis(31));
    }

    #[test]
    fn archive_poke_failure_helpers_preserve_progress_metrics() {
        let metrics = ArchivePokePrebuildMetrics {
            slab_prebuild_duration: Duration::from_millis(12),
            block_slab_prebuild_duration: Duration::from_millis(4),
            raw_tx_slab_prebuild_duration: Duration::from_millis(8),
            slab_prebuild_start_rss_bytes: Some(1_000),
            slab_prebuild_peak_rss_bytes: Some(2_000),
            raw_tx_slabs_prebuilt: 2,
            raw_tx_payload_bytes_prebuilt: 128,
        };

        let block_error = archive_poke_error(
            NockAppError::PokeFailed,
            metrics,
            Duration::from_millis(14),
            metrics.raw_tx_slab_prebuild_duration,
            0,
        );
        let block_timings = block_error
            .archive_poke_timings()
            .expect("block poke failure timings");
        assert_eq!(block_timings.raw_tx_pokes_completed, 0);
        assert_eq!(block_timings.raw_tx_slabs_prebuilt, 2);
        assert_eq!(block_timings.raw_tx_payload_bytes_prebuilt, 128);
        assert_eq!(block_timings.total_duration, Duration::from_millis(22));

        let raw_tx_error = archive_poke_error(
            NockAppError::PokeFailed,
            metrics,
            Duration::from_millis(14),
            Duration::from_millis(20),
            1,
        );
        let raw_tx_timings = raw_tx_error
            .archive_poke_timings()
            .expect("raw tx poke failure timings");
        assert_eq!(raw_tx_timings.raw_tx_pokes_completed, 1);
        assert_eq!(raw_tx_timings.raw_tx_slabs_prebuilt, 2);
        assert_eq!(raw_tx_timings.raw_tx_payload_bytes_prebuilt, 128);
        assert_eq!(raw_tx_timings.total_duration, Duration::from_millis(34));
    }

    #[test]
    fn archive_poke_error_exposes_prebuild_timings() {
        let timings = ArchivePokeTimings {
            block_duration: Duration::from_millis(10),
            raw_tx_duration: Duration::from_millis(20),
            total_duration: Duration::from_millis(30),
            slab_prebuild_duration: Duration::from_millis(4),
            block_slab_prebuild_duration: Duration::from_millis(1),
            raw_tx_slab_prebuild_duration: Duration::from_millis(3),
            slab_prebuild_start_rss_bytes: Some(100),
            slab_prebuild_peak_rss_bytes: Some(200),
            raw_tx_pokes_completed: 1,
            raw_tx_slabs_prebuilt: 2,
            raw_tx_payload_bytes_prebuilt: 64,
        };
        let error = PokeStepError::ArchivePoke {
            source: NockAppError::PokeFailed,
            timings,
        };

        assert_eq!(error.archive_poke_timings(), Some(timings));
    }
}
