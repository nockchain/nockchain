#![allow(dead_code)]
#![allow(clippy::items_after_test_module)]
#![allow(clippy::missing_safety_doc)]
use std::any::Any;
use std::fs;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bincode::{config, Decode, Encode};
use blake3::{Hash, Hasher};
use byteorder::{LittleEndian, WriteBytesExt};
use nockvm::hamt::Hamt;
use nockvm::interpreter::{self, interpret, Error, Mote, NockCancelToken};
use nockvm::jets::cold::{Cold, Nounable};
use nockvm::jets::hot::{HotEntry, URBIT_HOT_STATE};
use nockvm::jets::nock::util::mook;
use nockvm::mem::{AllocationError, NewStackError, NockStack};
use nockvm::mug::met3_usize;
use nockvm::noun::{Atom, Cell, DirectAtom, IndirectAtom, Noun, D, T};
use nockvm::offset::PmaOffsetWords;
use nockvm::pma::{Pma, PmaCopy, PmaCopyFrom};
use nockvm::trace::{path_to_cord, write_serf_trace_safe};
use nockvm_macros::tas;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use crate::event_log::{EventLog, EventLogConfig, EventLogEntry, ReplayLogEntry};
use crate::kernel::boot::TraceOpts;
use crate::metrics::NockAppMetrics;
use crate::nockapp::wire::{wire_to_noun, WireRepr};
use crate::noun::slab::{Jammer, NockJammer, NounSlab};
use crate::noun::slam;
use crate::save::SaveableCheckpoint;
use crate::snapshot::{
    maybe_create_epoch_snapshot, maybe_create_rotating_snapshot, SnapshotManifest,
};
use crate::utils::{
    create_context, current_da, durability, NOCK_STACK_SIZE, NOCK_STACK_SIZE_HUGE,
    NOCK_STACK_SIZE_LARGE, NOCK_STACK_SIZE_MEDIUM, NOCK_STACK_SIZE_SMALL, NOCK_STACK_SIZE_TINY,
};
use crate::{AtomExt, CrownError, IndirectAtomExt, NounExt, Result, ToBytesExt};

pub(crate) const STATE_AXIS: u64 = 6;
const LOAD_AXIS: u64 = 4;
const PEEK_AXIS: u64 = 22;
const POKE_AXIS: u64 = 23;

const SERF_FINISHED_INTERVAL: Duration = Duration::from_millis(100);
const SERF_THREAD_STACK_SIZE: usize = 256 * 1024 * 1024; // 8MB
const REPLAY_PRESERVE_BATCH: usize = 64;
const PMA_GC_DROP_ALLOCATED_PREFIX_NUMERATOR: usize = 1;
const PMA_GC_DROP_ALLOCATED_PREFIX_DENOMINATOR: usize = 1;
const PMA_EVENT_PREFLIGHT_MIN_FREE_WORDS: usize = 4096;
const PMA_EVENT_PREFLIGHT_FREE_WORDS_ENV: &str = "NOCK_PMA_EVENT_PREFLIGHT_FREE_WORDS";
const NOCK_STACK_FREE_GAP_TRIM_ENV: &str = "NOCK_STACK_FREE_GAP_TRIM";
const NOCK_STACK_FREE_GAP_TRIM_THRESHOLD_BYTES: usize = 512 * 1024 * 1024;
const NOCK_STACK_FREE_GAP_TRIM_THRESHOLD_WORDS: usize =
    NOCK_STACK_FREE_GAP_TRIM_THRESHOLD_BYTES / std::mem::size_of::<u64>();

fn duration_ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

fn words_to_mib(words: usize) -> f64 {
    (words as f64 * 8.0) / (1024.0 * 1024.0)
}

fn bytes_to_mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn pma_event_preflight_free_words(pma: &Pma) -> usize {
    if let Ok(value) = std::env::var(PMA_EVENT_PREFLIGHT_FREE_WORDS_ENV) {
        if let Ok(words) = value.parse::<usize>() {
            return words;
        }
    }
    PMA_EVENT_PREFLIGHT_MIN_FREE_WORDS.max(pma.size_words() / 100)
}

fn stack_free_gap_trim_enabled() -> bool {
    match std::env::var(NOCK_STACK_FREE_GAP_TRIM_ENV) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no" | "disable" | "disabled"
        ),
        Err(std::env::VarError::NotPresent) => true,
        Err(std::env::VarError::NotUnicode(_)) => true,
    }
}

fn log_preserve_component(event_num: u64, component: &'static str, elapsed: Duration) {
    debug!(
        event_num,
        component,
        elapsed_ms = duration_ms(elapsed),
        "preserve component done"
    );
}

fn log_pma_preserve_component(event_num: u64, component: &'static str, segment: PmaCopySegment) {
    debug!(
        event_num,
        component,
        elapsed_ms = duration_ms(segment.elapsed),
        alloc_words = segment.alloc_words,
        alloc_mib = words_to_mib(segment.alloc_words),
        "preserve component done"
    );
}

#[derive(Clone, Debug)]
struct SerfInitDiagnostics {
    phase: &'static str,
    stack_size_words: usize,
    checkpoint_event_num: Option<u64>,
    checkpoint_ker_hash: Option<Hash>,
    current_ker_hash: Option<Hash>,
}

impl SerfInitDiagnostics {
    fn new(stack_size_words: usize) -> Self {
        Self {
            phase: "starting serf initialization",
            stack_size_words,
            checkpoint_event_num: None,
            checkpoint_ker_hash: None,
            current_ker_hash: None,
        }
    }

    fn with_phase(&self, phase: &'static str) -> Self {
        let mut diagnostics = self.clone();
        diagnostics.phase = phase;
        diagnostics
    }

    fn with_current_ker_hash(&self, current_ker_hash: Hash) -> Self {
        let mut diagnostics = self.clone();
        diagnostics.current_ker_hash = Some(current_ker_hash);
        diagnostics
    }

    fn with_checkpoint(&self, saveable: &SaveableCheckpoint) -> Self {
        let mut diagnostics = self.clone();
        diagnostics.checkpoint_event_num = Some(saveable.event_num);
        diagnostics.checkpoint_ker_hash = Some(saveable.ker_hash);
        diagnostics
    }
}

fn run_serf_init_phase<T, F>(
    diagnostics: &SerfInitDiagnostics,
    phase: &'static str,
    f: F,
) -> Result<T>
where
    F: FnOnce() -> T,
{
    catch_unwind(AssertUnwindSafe(f))
        .map_err(|payload| serf_init_panic_to_error(diagnostics.with_phase(phase), payload))
}

fn serf_init_panic_to_error(
    diagnostics: SerfInitDiagnostics,
    payload: Box<dyn Any + Send>,
) -> CrownError {
    if let Some(err) = payload.downcast_ref::<AllocationError>() {
        return CrownError::SerfInitAllocationError(serf_init_allocation_message(
            &diagnostics,
            &err.to_string(),
        ));
    }
    if let Some(err) = payload.downcast_ref::<NewStackError>() {
        return CrownError::SerfInitAllocationError(serf_init_allocation_message(
            &diagnostics,
            &err.to_string(),
        ));
    }

    CrownError::SerfInitPanic(format!(
        "Serf initialization panicked during {}: {}. Preserve the checkpoint or state jam, rerun with RUST_BACKTRACE=1, and report this as a bug.",
        diagnostics.phase,
        panic_payload_message(payload.as_ref())
    ))
}

fn serf_init_allocation_message(diagnostics: &SerfInitDiagnostics, err: &str) -> String {
    let stack_bytes = diagnostics
        .stack_size_words
        .saturating_mul(std::mem::size_of::<u64>());
    let stack_mib = stack_bytes / (1024 * 1024);
    let checkpoint = match (
        diagnostics.checkpoint_event_num, diagnostics.checkpoint_ker_hash,
    ) {
        (Some(event_num), Some(ker_hash)) => {
            format!(
                "checkpoint event_num={event_num} checkpoint_ker_hash={}",
                ker_hash.to_hex()
            )
        }
        _ => "no checkpoint metadata was available".to_string(),
    };
    let current = diagnostics
        .current_ker_hash
        .map(|ker_hash| ker_hash.to_hex().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    format!(
        "Nock stack exhausted during Serf initialization phase `{}`. Configured stack size: {} words (~{} MiB). {} current_ker_hash={}. Try restarting this peer with a larger `--stack-size` (`large` or `huge`). If it still fails with `huge`, restore a checkpoint or state jam from a synced peer and preserve the failing artifact for debugging. Allocation error: {}",
        diagnostics.phase,
        diagnostics.stack_size_words,
        stack_mib,
        checkpoint,
        current,
        err
    )
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

pub struct LoadState {
    pub ker_hash: Hash,
    pub event_num: u64,
    pub kernel_state: NounSlab,
}

#[derive(Clone, Debug)]
pub struct PmaConfig {
    pub path_0: PathBuf,
    pub path_1: PathBuf,
    pub words: usize,
    pub reserved_words: Option<usize>,
    pub open_existing: bool,
    pub create_snapshots: bool,
    pub rotating_snapshot_interval_event_time: Option<Duration>,
    pub(crate) restore_manifest: Option<SnapshotManifest>,
    pub gc_interval: Option<Duration>,
}

#[derive(Clone, Copy, Debug)]
enum PmaSlab {
    Slab0,
    Slab1,
}

impl PmaSlab {
    fn next(self) -> Self {
        match self {
            PmaSlab::Slab0 => PmaSlab::Slab1,
            PmaSlab::Slab1 => PmaSlab::Slab0,
        }
    }
}

#[derive(Clone, Debug)]
struct PmaSlabPaths {
    path_0: PathBuf,
    path_1: PathBuf,
}

impl PmaSlabPaths {
    fn path(&self, slab: PmaSlab) -> &PathBuf {
        match slab {
            PmaSlab::Slab0 => &self.path_0,
            PmaSlab::Slab1 => &self.path_1,
        }
    }
}

const PMA_PERSIST_MAGIC: u64 = u64::from_le_bytes(*b"PMAPERS1");
const PMA_PERSIST_VERSION: u32 = 5;
const PMA_PERSIST_VERSION_V4: u32 = 4;
const SNAPSHOT_UNUSED_COLD_OFFSET: PmaOffsetWords = PmaOffsetWords::from_words(0);

#[derive(Clone, Encode, Decode, Debug)]
struct PmaPersistMetadata {
    magic: u64,
    version: u32,
    #[bincode(with_serde)]
    ker_hash: Hash,
    event_num: u64,
    kernel_state_raw: u64,
    pma_reserved_words: u64,
    #[bincode(with_serde)]
    checksum: Hash,
}

#[derive(Clone, Encode, Decode, Debug)]
struct PmaPersistMetadataV4 {
    magic: u64,
    version: u32,
    #[bincode(with_serde)]
    ker_hash: Hash,
    event_num: u64,
    kernel_state_raw: u64,
    #[bincode(with_serde)]
    checksum: Hash,
}

impl PmaPersistMetadata {
    fn new(ker_hash: Hash, event_num: u64, kernel_state_raw: u64, pma_reserved_words: u64) -> Self {
        let checksum = Self::checksum(ker_hash, event_num, kernel_state_raw, pma_reserved_words);
        Self {
            magic: PMA_PERSIST_MAGIC,
            version: PMA_PERSIST_VERSION,
            ker_hash,
            event_num,
            kernel_state_raw,
            pma_reserved_words,
            checksum,
        }
    }

    fn checksum(
        ker_hash: Hash,
        event_num: u64,
        kernel_state_raw: u64,
        pma_reserved_words: u64,
    ) -> Hash {
        let mut hasher = Hasher::new();
        hasher.update(ker_hash.as_bytes());
        hasher.update(&event_num.to_le_bytes());
        hasher.update(&kernel_state_raw.to_le_bytes());
        hasher.update(&pma_reserved_words.to_le_bytes());
        hasher.finalize()
    }

    fn validate(&self) -> bool {
        if self.magic != PMA_PERSIST_MAGIC || self.version != PMA_PERSIST_VERSION {
            return false;
        }
        self.checksum
            == Self::checksum(
                self.ker_hash, self.event_num, self.kernel_state_raw, self.pma_reserved_words,
            )
    }

    fn load_from_path(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        if let Ok((meta, _)) =
            bincode::decode_from_slice::<Self, config::Configuration>(&bytes, config::standard())
        {
            if meta.validate() {
                return Some(meta);
            }
        }
        let (legacy, _) =
            bincode::decode_from_slice::<PmaPersistMetadataV4, config::Configuration>(
                &bytes,
                config::standard(),
            )
            .ok()?;
        legacy.validate().then(|| legacy.into_current())
    }

    fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        let bytes =
            bincode::encode_to_vec(self, config::standard()).map_err(std::io::Error::other)?;
        durability::write_atomic(path, &bytes, "pma_meta_write")?;
        Ok(())
    }
}

impl PmaPersistMetadataV4 {
    fn checksum(ker_hash: Hash, event_num: u64, kernel_state_raw: u64) -> Hash {
        let mut hasher = Hasher::new();
        hasher.update(ker_hash.as_bytes());
        hasher.update(&event_num.to_le_bytes());
        hasher.update(&kernel_state_raw.to_le_bytes());
        hasher.finalize()
    }

    fn validate(&self) -> bool {
        if self.magic != PMA_PERSIST_MAGIC || self.version != PMA_PERSIST_VERSION_V4 {
            return false;
        }
        self.checksum == Self::checksum(self.ker_hash, self.event_num, self.kernel_state_raw)
    }

    fn into_current(self) -> PmaPersistMetadata {
        PmaPersistMetadata::new(self.ker_hash, self.event_num, self.kernel_state_raw, 0)
    }
}

fn pma_meta_path(path: &Path) -> PathBuf {
    path.with_extension("meta")
}

// Slab selection is intentionally kernel-hash-agnostic: a kernel upgrade
// (new bytecode -> new BLAKE3 hash) must not hide the genuinely-current slab.
// Gating this on `ker_hash` caused both slabs to report `None` after an
// upgrade, so selection fell through to its `Slab0` default and then reported
// the (possibly stale) slab-0 metadata as "missing or invalid" even when the
// live state lived in slab 1. Structural integrity is still guarded by the
// metadata checksum (`PmaPersistMetadata::validate`) and the PMA trailer
// (`Pma::open`); cross-kernel state compatibility is the kernel's own
// Hoon-level `+load` responsibility, consistent with the checkpoint and
// snapshot restore paths.
fn pma_meta_status(path: &Path) -> Option<(u64, SystemTime)> {
    let meta_path = pma_meta_path(path);
    let meta = PmaPersistMetadata::load_from_path(&meta_path)?;
    let modified = std::fs::metadata(&meta_path)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some((meta.event_num, modified))
}

fn select_active_pma_slab(paths: &PmaSlabPaths) -> PmaSlab {
    let status_0 = pma_meta_status(&paths.path_0);
    let status_1 = pma_meta_status(&paths.path_1);
    match (status_0, status_1) {
        (Some((event_0, mod_0)), Some((event_1, mod_1))) => {
            if event_0 > event_1 {
                PmaSlab::Slab0
            } else if event_1 > event_0 || mod_1 > mod_0 {
                PmaSlab::Slab1
            } else {
                PmaSlab::Slab0
            }
        }
        (Some(_), None) => PmaSlab::Slab0,
        (None, Some(_)) => PmaSlab::Slab1,
        (None, None) => PmaSlab::Slab0,
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ExistingPmaStatus {
    Missing,
    Valid { path: PathBuf, event_num: u64 },
    Invalid { path: PathBuf, reason: String },
}

pub(crate) fn inspect_existing_pma(
    path_0: &Path,
    path_1: &Path,
    kernel_bytes: &[u8],
) -> ExistingPmaStatus {
    let mut hasher = Hasher::new();
    hasher.update(kernel_bytes);
    let ker_hash = hasher.finalize();
    let paths = PmaSlabPaths {
        path_0: path_0.to_path_buf(),
        path_1: path_1.to_path_buf(),
    };
    let active = select_active_pma_slab(&paths);
    let active_path = paths.path(active).clone();
    let inactive_path = paths.path(active.next()).clone();
    let active_meta_path = pma_meta_path(&active_path);
    let inactive_meta_path = pma_meta_path(&inactive_path);
    let any_pma_artifacts = active_path.exists()
        || inactive_path.exists()
        || active_meta_path.exists()
        || inactive_meta_path.exists();

    if !active_path.exists() {
        return if any_pma_artifacts {
            ExistingPmaStatus::Invalid {
                path: active_path,
                reason: "selected PMA slab is missing".to_string(),
            }
        } else {
            ExistingPmaStatus::Missing
        };
    }

    let Some(meta) = PmaPersistMetadata::load_from_path(&active_meta_path) else {
        return ExistingPmaStatus::Invalid {
            path: active_path,
            reason: format!(
                "missing or invalid PMA metadata at {}",
                active_meta_path.display()
            ),
        };
    };

    // A kernel-hash mismatch does NOT invalidate the PMA. The persisted kernel
    // state is a kernel-agnostic noun and is loaded into the freshly-cued
    // current kernel, which performs its own Hoon-level state migration. This
    // mirrors the checkpoint (`form.rs` "loading checkpoint state into current
    // kernel") and snapshot (`boot.rs` "loading snapshot state into current
    // kernel") restore paths. Forcing a full event-log replay on every kernel
    // upgrade is the operational cost we are eliminating here.
    if meta.ker_hash != ker_hash {
        warn!(
            metadata = %meta.ker_hash,
            kernel = %ker_hash,
            "PMA kernel hash mismatch; loading existing PMA state into current kernel"
        );
    }

    match Pma::read_file_metadata(&active_path) {
        Ok(_) => ExistingPmaStatus::Valid {
            path: active_path,
            event_num: meta.event_num,
        },
        Err(err) => ExistingPmaStatus::Invalid {
            path: active_path,
            reason: format!("failed to open PMA: {err}"),
        },
    }
}

struct PmaGcState {
    paths: PmaSlabPaths,
    active: PmaSlab,
    interval: Duration,
    last_gc: Instant,
    words: usize,
    reserved_words: Option<usize>,
}

impl PmaGcState {
    fn new(
        paths: PmaSlabPaths,
        active: PmaSlab,
        interval: Duration,
        words: usize,
        reserved_words: Option<usize>,
    ) -> Self {
        Self {
            paths,
            active,
            interval,
            last_gc: Instant::now(),
            words,
            reserved_words,
        }
    }

    fn active_path(&self) -> &PathBuf {
        self.paths.path(self.active)
    }

    fn inactive_path(&self) -> &PathBuf {
        self.paths.path(self.active.next())
    }

    fn should_gc(&self) -> bool {
        self.last_gc.elapsed() >= self.interval
    }

    fn mark_gc_completed(&mut self) {
        self.active = self.active.next();
        self.last_gc = Instant::now();
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PmaCopySegment {
    pub elapsed: Duration,
    pub alloc_words: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PmaCopyDetail {
    pub warm: PmaCopySegment,
    pub test_jets: PmaCopySegment,
    pub hot: PmaCopySegment,
    pub cache: PmaCopySegment,
    pub cold: PmaCopySegment,
    pub arvo: PmaCopySegment,
}

#[derive(Clone, Copy, Debug)]
pub struct PmaTimingSample {
    pub event: Duration,
    pub pma_copy: Duration,
    pub detail: Option<PmaCopyDetail>,
}

#[derive(Default)]
pub(crate) struct PmaTiming {
    samples: Mutex<Vec<PmaTimingSample>>,
}

impl PmaTiming {
    pub(crate) fn record(
        &self,
        event: Duration,
        pma_copy: Duration,
        detail: Option<PmaCopyDetail>,
    ) {
        let mut samples = self.samples.lock().unwrap_or_else(|err| err.into_inner());
        samples.push(PmaTimingSample {
            event,
            pma_copy,
            detail,
        });
    }

    pub(crate) fn take_samples(&self) -> Vec<PmaTimingSample> {
        let mut samples = self.samples.lock().unwrap_or_else(|err| err.into_inner());
        std::mem::take(&mut *samples)
    }
}

#[derive(Debug, Clone)]
struct AcceptedEventMetadata {
    wire_source: String,
    wire_version: i64,
    wire_tags_json: String,
    cause_hash: Vec<u8>,
    created_at_ms: i64,
}

struct AcceptedPoke {
    effects: Noun,
    durable_event: Option<EventLogEntry>,
}

// Actions to request of the serf thread
pub(crate) enum SerfAction<C> {
    // Make a CheckPoint
    Checkpoint {
        result: oneshot::Sender<C>,
    },
    Import {
        state: LoadState,
        result: oneshot::Sender<Result<()>>,
    },
    Export {
        result: oneshot::Sender<Result<LoadState>>,
    },
    // Get the state noun of the kernel as a slab
    GetKernelStateSlab {
        result: oneshot::Sender<Result<NounSlab>>,
    },
    // Get the cold state as a NounSlab
    GetColdStateSlab {
        result: oneshot::Sender<NounSlab>,
    },
    // Run a peek
    Peek {
        ovo: NounSlab,
        result: oneshot::Sender<Result<NounSlab>>,
    },
    Replay {
        jobs: Vec<ReplayLogEntry>,
        result: oneshot::Sender<Result<()>>,
    },
    // Run a poke
    //
    // TODO: send back the event number after each poke
    Poke {
        wire: WireRepr,
        cause: NounSlab,
        result: oneshot::Sender<Result<NounSlab>>,
        result_ack: oneshot::Receiver<()>,
    },
    // Provide metrics
    ProvideMetrics {
        metrics: Arc<NockAppMetrics>,
        result: oneshot::Sender<()>,
    },
    // Flush durable PMA state and stop the loop
    Stop {
        result: oneshot::Sender<Result<()>>,
    },
}

pub struct SerfThread<C> {
    handle: Option<std::thread::JoinHandle<()>>,
    action_sender: mpsc::Sender<SerfAction<C>>,
    pub cancel_token: NockCancelToken,
    inhibit: Arc<AtomicBool>,
    pub event_number: Arc<AtomicU64>,
    pub(crate) pma_timing: Option<Arc<PmaTiming>>,
}

impl<C: SerfCheckpoint + Send + 'static> SerfThread<C> {
    pub async fn new(
        kernel_bytes: Vec<u8>,
        checkpoint: Option<C>,
        constant_hot_state: Vec<HotEntry>,
        nock_stack_size: usize,
        pma: Option<PmaConfig>,
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        Self::new_with_event_log(
            kernel_bytes, checkpoint, constant_hot_state, nock_stack_size, pma, None, test_jets,
            trace,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new_with_event_log(
        kernel_bytes: Vec<u8>,
        checkpoint: Option<C>,
        constant_hot_state: Vec<HotEntry>,
        nock_stack_size: usize,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let (action_sender, action_receiver) = mpsc::channel(1);
        let pma_timing = std::env::var_os("NOCK_PMA_TIMING")
            .is_some()
            .then(|| Arc::new(PmaTiming::default()));
        let (init_sender, init_receiver) = oneshot::channel();
        let inhibit = Arc::new(AtomicBool::new(false));
        let inhibit_clone = inhibit.clone();
        let pma_timing_thread = pma_timing.clone();
        let handle = std::thread::Builder::new()
            .name("serf".to_string())
            .stack_size(SERF_THREAD_STACK_SIZE)
            .spawn(move || {
                let diagnostics = SerfInitDiagnostics::new(nock_stack_size);
                let ker_hash = {
                    let mut hasher = Hasher::new();
                    hasher.update(&kernel_bytes);
                    hasher.finalize()
                };
                let diagnostics = diagnostics.with_current_ker_hash(ker_hash);
                let mut pma_meta_load = false;
                let (
                    pma,
                    pma_meta_path,
                    pma_gc_state,
                    create_snapshots,
                    rotating_snapshot_interval_event_time,
                    restore_manifest,
                ) = match pma {
                    Some(config) => {
                        let PmaConfig {
                            path_0,
                            path_1,
                            words,
                            reserved_words,
                            open_existing,
                            create_snapshots,
                            rotating_snapshot_interval_event_time,
                            restore_manifest,
                            gc_interval,
                        } = config;
                        let paths = PmaSlabPaths { path_0, path_1 };
                        let active = if open_existing {
                            select_active_pma_slab(&paths)
                        } else {
                            PmaSlab::Slab0
                        };
                        let active_path = paths.path(active).clone();
                        let pma_meta_path = pma_meta_path(&active_path);
                        let pma_result = if open_existing && active_path.exists() {
                            pma_meta_load = true;
                            match reserved_words {
                                Some(reserved_words) => Pma::open_with_min_and_reserved(
                                    active_path.clone(),
                                    words,
                                    reserved_words,
                                ),
                                None => Pma::open_with_min(active_path.clone(), words),
                            }
                        } else {
                            pma_meta_load = false;
                            match reserved_words {
                                Some(reserved_words) => Pma::new_with_reserved(
                                    words,
                                    reserved_words,
                                    active_path.clone(),
                                ),
                                None => Pma::new(words, active_path.clone()),
                            }
                        };
                        match pma_result {
                            Ok(pma) => (
                                Some(pma),
                                Some(pma_meta_path),
                                gc_interval.map(|interval| {
                                    PmaGcState::new(paths, active, interval, words, reserved_words)
                                }),
                                create_snapshots,
                                rotating_snapshot_interval_event_time,
                                restore_manifest,
                            ),
                            Err(err) => {
                                let _ = init_sender.send(Err(CrownError::Unknown(err.to_string())));
                                return;
                            }
                        }
                    }
                    None => (None, None, None, false, None, None),
                };
                let event_log = if let Some(config) = event_log {
                    match EventLog::open(config.clone()) {
                        Ok(log) => Some(log),
                        Err(err) => {
                            let _ = init_sender.send(Err(CrownError::Unknown(format!(
                                "failed to open event log at {}: {err}",
                                config.path.display()
                            ))));
                            return;
                        }
                    }
                } else {
                    None
                };
                if let (Some(pma), Some(meta_path), Some(manifest)) = (
                    pma.as_ref(),
                    pma_meta_path.as_ref(),
                    restore_manifest.as_ref(),
                ) {
                    let pma_reserved_words = match u64::try_from(pma.reserved_words()) {
                        Ok(words) => words,
                        Err(_) => {
                            let _ = init_sender.send(Err(CrownError::Unknown(
                                "PMA reservation exceeds u64 while synthesizing metadata"
                                    .to_string(),
                            )));
                            return;
                        }
                    };
                    let synthesized = PmaPersistMetadata::new(
                        ker_hash, manifest.event_num, manifest.kernel_root_raw, pma_reserved_words,
                    );
                    if let Err(err) = synthesized.save_to_path(meta_path) {
                        let _ = init_sender.send(Err(CrownError::Unknown(format!(
                            "failed to synthesize PMA metadata from snapshot manifest at {}: {err}",
                            meta_path.display()
                        ))));
                        return;
                    }
                }
                let stack = match run_serf_init_phase(&diagnostics, "allocate Nock stack", || {
                    NockStack::new(nock_stack_size, 0)
                }) {
                    Ok(stack) => stack,
                    Err(err) => {
                        let _ = init_sender.send(Err(err));
                        return;
                    }
                };
                let serf = Serf::new(
                    stack, pma, pma_meta_path, pma_meta_load, pma_gc_state, create_snapshots,
                    rotating_snapshot_interval_event_time, event_log, checkpoint, &kernel_bytes,
                    &constant_hot_state, test_jets, trace, nock_stack_size,
                );
                let serf = match serf {
                    Ok(serf) => serf,
                    Err(err) => {
                        let _ = init_sender.send(Err(err));
                        return;
                    }
                };
                let _ = init_sender.send(Ok((serf.event_num.clone(), serf.context.cancel_token())));
                serf_loop(serf, action_receiver, inhibit_clone, pma_timing_thread);
            })?;
        let (event_number, cancel_token) = init_receiver
            .await
            .map_err(|err| CrownError::Unknown(err.to_string()))??;
        Ok(SerfThread {
            inhibit,
            handle: Some(handle),
            action_sender,
            event_number,
            cancel_token,
            pma_timing,
        })
    }
}

impl<C> SerfThread<C> {
    pub(crate) fn provide_metrics(
        &mut self,
        metrics: Arc<NockAppMetrics>,
    ) -> impl Future<Output = Result<()>> {
        let action_sender = self.action_sender.clone();
        let (result, result_recv) = oneshot::channel();
        async move {
            action_sender
                .send(SerfAction::ProvideMetrics { metrics, result })
                .await?;
            Ok(result_recv.await?)
        }
    }

    pub(crate) fn stop(&mut self) -> impl Future<Output = Result<()>> {
        let action_sender = self.action_sender.clone();
        let join_handle = self.handle.take().expect("Serf join handle already taken.");
        let tokio_join_handle = tokio::task::spawn_blocking(move || join_handle.join());
        let (result, result_recv) = oneshot::channel();
        async move {
            if let Err(err) = action_sender.send(SerfAction::Stop { result }).await {
                let join_res = tokio_join_handle.await;
                return match join_res {
                    Ok(Ok(())) => Err(err.into()),
                    Ok(Err(e)) => Err(CrownError::Unknown(format!("Serf thread panicked: {e:?}"))),
                    Err(e) => Err(CrownError::JoinError(e)),
                };
            }

            let stop_res = result_recv.await?;
            let join_res = tokio_join_handle.await;
            match join_res {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(CrownError::Unknown(format!("Serf thread panicked: {e:?}"))),
                Err(e) => Err(CrownError::JoinError(e)),
            }?;
            stop_res
        }
    }

    pub(crate) fn join(&mut self) -> Result<(), Box<dyn Any + Send + 'static>> {
        self.handle
            .take()
            .expect("Serf thread already joined")
            .join()
    }

    pub(crate) async fn get_kernel_state_slab(&self) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        self.action_sender
            .send(SerfAction::GetKernelStateSlab { result })
            .await?;
        result_fut.await?
    }

    pub(crate) async fn get_cold_state_slab(&self) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        self.action_sender
            .send(SerfAction::GetColdStateSlab { result })
            .await?;
        Ok(result_fut.await?)
    }

    pub(crate) fn peek(&self, ovo: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender.send(SerfAction::Peek { ovo, result }).await?;
            result_fut.await?
        }
    }

    // We are very carefully ensuring that the future does not contain the &self reference, to allow spawning a task without lifetime issues
    pub fn poke(&self, wire: WireRepr, cause: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        let (result, result_fut) = oneshot::channel();
        let (result_ack_sender, result_ack) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender
                .send(SerfAction::Poke {
                    wire,
                    cause,
                    result,
                    result_ack,
                })
                .await?;
            let res = result_fut.await?;
            let _ = result_ack_sender.send(());
            res
        }
    }

    pub fn poke_timeout(
        &self,
        wire: WireRepr,
        cause: NounSlab,
        timeout: Duration,
    ) -> impl Future<Output = Result<NounSlab>> {
        let (result, result_fut) = oneshot::channel();
        let (result_ack_sender, result_ack) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        let cancel = self.cancel_token.clone();
        async move {
            let deadline = tokio::time::Instant::now() + timeout;
            tokio::time::timeout_at(
                deadline,
                action_sender.send(SerfAction::Poke {
                    wire,
                    cause,
                    result,
                    result_ack,
                }),
            )
            .await
            .map_err(|_| CrownError::Timeout)??;

            let cancel_for_task = cancel.clone();
            let cancel_task = tokio::spawn(async move {
                tokio::time::sleep_until(deadline).await;
                cancel_for_task.cancel();
            });

            let result = tokio::time::timeout_at(deadline, result_fut).await;
            cancel_task.abort();
            let _ = cancel_task.await;

            match result {
                Ok(Ok(res)) => {
                    let _ = result_ack_sender.send(());
                    res
                }
                Ok(Err(err)) => {
                    let _ = result_ack_sender.send(());
                    Err(err.into())
                }
                Err(_) => {
                    cancel.cancel();
                    let _ = result_ack_sender.send(());
                    Err(CrownError::Timeout)
                }
            }
        }
    }

    pub(crate) fn poke_sync(&self, wire: WireRepr, cause: NounSlab) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        let (result_ack_sender, result_ack) = oneshot::channel();
        self.action_sender.blocking_send(SerfAction::Poke {
            wire,
            cause,
            result,
            result_ack,
        })?;
        let res = result_fut.blocking_recv()?;
        let _ = result_ack_sender.send(());
        res
    }

    pub(crate) fn peek_sync(&self, ovo: NounSlab) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        self.action_sender
            .blocking_send(SerfAction::Peek { ovo, result })?;
        result_fut.blocking_recv()?
    }

    pub(crate) fn checkpoint(&self) -> impl Future<Output = Result<C>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender
                .send(SerfAction::Checkpoint { result })
                .await?;
            Ok(result_fut.await?)
        }
    }

    pub(crate) fn replay_event_jobs(
        &self,
        jobs: Vec<ReplayLogEntry>,
    ) -> impl Future<Output = Result<()>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender
                .send(SerfAction::Replay { jobs, result })
                .await?;
            result_fut.await?
        }
    }

    pub fn import(&self, state: LoadState) -> impl Future<Output = Result<()>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender
                .send(SerfAction::Import { state, result })
                .await?;
            result_fut.await?
        }
    }

    pub fn export(&self) -> impl Future<Output = Result<LoadState>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender.send(SerfAction::Export { result }).await?;
            result_fut.await?
        }
    }
}

fn serf_loop<C: SerfCheckpoint>(
    mut serf: Serf,
    mut action_receiver: mpsc::Receiver<SerfAction<C>>,
    inhibit: Arc<AtomicBool>,
    pma_timing: Option<Arc<PmaTiming>>,
) {
    loop {
        let start = std::time::Instant::now();
        let Some(action) = action_receiver.blocking_recv() else {
            break;
        };
        let recv_elapsed = start.elapsed();
        if let Some(nockapp_metrics) = &serf.metrics {
            nockapp_metrics
                .serf_loop_blocking_recv
                .add_timing(&recv_elapsed);
        };
        let action_start = std::time::Instant::now();
        match action {
            SerfAction::Stop { result } => {
                let stop_res = serf.flush_pma_state_for_shutdown();
                let _ = result.send(stop_res).inspect_err(|_err| {
                    debug!("Failed to send shutdown flush result to dropped channel");
                });
                break;
            }
            SerfAction::Export { result } => {
                let space = serf.context.stack.noun_space();
                let kernel_state_noun = serf
                    .arvo
                    .in_space(&space)
                    .slot(STATE_AXIS)
                    .map(|handle| handle.noun());
                let kernel_state = kernel_state_noun.map_or_else(
                    |err| Err(CrownError::from(err)),
                    |noun| {
                        let mut slab = NounSlab::new();
                        slab.copy_into(noun, &space);
                        Ok(slab)
                    },
                );
                let load_state = kernel_state.map(|kernel_state| LoadState {
                    kernel_state,
                    ker_hash: serf.ker_hash,
                    event_num: serf.event_num.load(Ordering::SeqCst),
                });
                let _ = result.send(load_state).inspect_err(|_err| {
                    debug!("Failed to send to dropped channel");
                });
            }
            SerfAction::Import { state, result } => {
                let state_noun = state.kernel_state.copy_to_stack(serf.stack());
                let arvo = serf.load(state_noun);
                match arvo {
                    Err(e) => {
                        let _ = result.send(Err(e)).map_err(|err| {
                            debug!("Tried to send to dropped channel: {:?}", err);
                        });
                    }
                    Ok(arvo) => {
                        if serf.ker_hash != state.ker_hash {
                            debug!(
                                "Importing state from kernel hash {} into kernel hash {}",
                                state.ker_hash, serf.ker_hash
                            );
                        }
                        unsafe {
                            serf.event_update(state.event_num, arvo);
                            let _ = serf.preserve_event_update_leftovers();
                        }
                        let _ = result.send(Ok(())).map_err(|err| {
                            debug!("Tried to send to dropped channel: {:?}", err);
                        });
                    }
                }
            }
            SerfAction::GetKernelStateSlab { result } => {
                let space = serf.context.stack.noun_space();
                let kernel_state_noun = serf
                    .arvo
                    .in_space(&space)
                    .slot(STATE_AXIS)
                    .map(|handle| handle.noun());
                let kernel_state_slab = kernel_state_noun.map_or_else(
                    |err| Err(CrownError::from(err)),
                    |noun| {
                        let mut slab = NounSlab::new();
                        slab.copy_into(noun, &space);
                        Ok(slab)
                    },
                );
                let _ = result.send(kernel_state_slab).inspect_err(|_e| {
                    debug!("Tried to send to dropped result channel");
                });
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_get_kernel_state_slab
                        .add_timing(&action_elapsed);
                };
            }
            SerfAction::GetColdStateSlab { result } => {
                let cold_state_noun = serf.context.cold.into_noun(serf.stack());
                let cold_state_slab = {
                    let mut slab = NounSlab::new();
                    let space = serf.context.stack.noun_space();
                    slab.copy_into(cold_state_noun, &space);
                    slab
                };
                let _ = result.send(cold_state_slab).inspect_err(|_e| {
                    debug!("Could not send cold state to dropped channel.");
                });
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_get_cold_state_slab
                        .add_timing(&action_elapsed);
                };
            }
            SerfAction::Checkpoint { result } => {
                let metrics_checkpoint = serf.metrics.clone();
                let checkpoint = create_checkpoint(&mut serf, &metrics_checkpoint);
                //result.send(checkpoint).expect("Could not send checkpoint");
                if result.send(checkpoint).is_err() {
                    debug!(
                        "Checkpoint receiver dropped before receiving result - likely timed out"
                    );
                };
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_checkpoint
                        .add_timing(&action_elapsed);
                };
            }
            SerfAction::Peek { ovo, result } => {
                if inhibit.load(Ordering::SeqCst) {
                    let _ = result
                        .send(Err(CrownError::Unknown("Serf stopping".to_string())))
                        .inspect_err(|_e| {
                            debug!("Tried to send inhibited peek state to dropped channel");
                        });
                } else {
                    let ovo_noun = ovo.copy_to_stack(serf.stack());
                    let noun_res = serf.peek(ovo_noun);
                    let space = serf.context.stack.noun_space();
                    let noun_slab_res = noun_res.map(|noun| {
                        let mut slab = NounSlab::new();
                        slab.copy_into(noun, &space);
                        slab
                    });
                    let _ = result.send(noun_slab_res).inspect_err(|_e| {
                        debug!("Tried to send peek state to dropped channel");
                    });
                };
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics.serf_loop_peek.add_timing(&action_elapsed);
                };
            }
            SerfAction::Replay { jobs, result } => {
                if inhibit.load(Ordering::SeqCst) {
                    let _ = result
                        .send(Err(CrownError::Unknown("Serf stopping".to_string())))
                        .inspect_err(|_e| {
                            debug!("Failed to send inhibited replay result from serf thread");
                        });
                } else {
                    let replay_res = serf.replay_event_jobs(jobs);
                    let _ = result.send(replay_res).inspect_err(|_e| {
                        debug!("Failed to send replay result from serf thread");
                    });
                }
            }
            SerfAction::Poke {
                wire,
                cause,
                result,
                result_ack,
            } => {
                if inhibit.load(Ordering::SeqCst) {
                    let _ = result
                        .send(Err(CrownError::Unknown("Serf stopping".to_string())))
                        .inspect_err(|_e| {
                            debug!("Failed to send inihibited poke result from serf thread");
                        });
                } else {
                    if let Err(err) = serf.ensure_pma_capacity_before_event() {
                        let _ = result.send(Err(err)).inspect_err(|_e| {
                            debug!("Failed to send PMA preflight failure from serf thread");
                        });
                        let _ = result_ack.blocking_recv().inspect_err(|_e| {
                            debug!("Failed to receive result ack after PMA preflight failure");
                        });
                        continue;
                    }
                    let cause_noun = cause.copy_to_stack(serf.stack());
                    let event_num_before = serf.event_num.load(Ordering::SeqCst) + 1;
                    debug!(
                        event_num = event_num_before,
                        source = %wire.source,
                        "poke action start"
                    );
                    let event_start = Instant::now();
                    let noun_res = serf.poke(wire, cause_noun);
                    let event_elapsed = event_start.elapsed();
                    let space = serf.context.stack.noun_space();
                    let (noun_slab_res, did_update, mut durable_event) = match noun_res {
                        Ok(accepted_poke) => {
                            let mut slab = NounSlab::new();
                            slab.copy_into(accepted_poke.effects, &space);
                            (Ok(slab), true, accepted_poke.durable_event)
                        }
                        Err(err) => (Err(err), false, None),
                    };
                    if let Some(durable_event) = durable_event.as_mut() {
                        durable_event.event_processing_duration = event_elapsed;
                    }
                    let mut pma_elapsed = None;
                    let mut pma_detail = None;
                    let mut durable_append_elapsed = None;
                    let cleanup_start = did_update.then(Instant::now);
                    if did_update {
                        debug!(event_num = event_num_before, "poke cleanup start");
                        let pma_start = Instant::now();
                        unsafe {
                            pma_detail =
                                serf.preserve_event_update_leftovers_with_pre_persist(|serf| {
                                    if let Some(durable_event) = durable_event.as_ref() {
                                        durable_append_elapsed =
                                            Some(serf.append_durable_event(durable_event));
                                        serf.cumulative_event_processing_time_since_snapshot = serf
                                            .cumulative_event_processing_time_since_snapshot
                                            .saturating_add(
                                                durable_event.event_processing_duration,
                                            );
                                    }
                                });
                        }
                        pma_elapsed = Some(pma_start.elapsed());
                        debug!(
                            event_num = event_num_before,
                            elapsed_ms = duration_ms(pma_start.elapsed()),
                            "poke cleanup stage done: preserve_event_update_leftovers"
                        );
                    }
                    let _ = result.send(noun_slab_res).inspect_err(|_e| {
                        debug!("Failed to send poke result from serf thread");
                    });
                    if let Some(timing) = &pma_timing {
                        let pma_elapsed = pma_elapsed.unwrap_or_else(|| Duration::from_millis(0));
                        timing.record(event_elapsed, pma_elapsed, pma_detail);
                    }
                    if std::env::var_os("NOCK_PMA_TIMING").is_some() {
                        let event_ms = event_elapsed.as_secs_f64() * 1000.0;
                        let pma_ms = pma_elapsed
                            .map(|elapsed| elapsed.as_secs_f64() * 1000.0)
                            .unwrap_or(0.0);
                        let total_ms = event_ms + pma_ms;
                        let total_alloc_words = pma_detail.map_or(0usize, |detail| {
                            detail.warm.alloc_words
                                + detail.test_jets.alloc_words
                                + detail.hot.alloc_words
                                + detail.cache.alloc_words
                                + detail.cold.alloc_words
                                + detail.arvo.alloc_words
                        });
                        let total_alloc_mib = words_to_mib(total_alloc_words);
                        let event_num = serf.event_num.load(Ordering::SeqCst);
                        info!(
                            "pma-timing: event_ms={:.3} pma_copy_ms={:.3} total_ms={:.3} alloc_words={} alloc_mib={:.3} event_num={}",
                            event_ms,
                            pma_ms,
                            total_ms,
                            total_alloc_words,
                            total_alloc_mib,
                            event_num
                        );
                    }
                    let _ = result_ack.blocking_recv().inspect_err(|_e| {
                        debug!("Failed to receive result ack in serf thread");
                    });
                    let mut snapshot_stage_elapsed = None;
                    if did_update {
                        let snapshot_start = Instant::now();
                        serf.maybe_create_rotating_snapshot();
                        snapshot_stage_elapsed = Some(snapshot_start.elapsed());
                        debug!(
                            event_num = event_num_before,
                            elapsed_ms = duration_ms(snapshot_start.elapsed()),
                            "poke cleanup stage done: rotating_snapshot"
                        );
                    }
                    let cleanup_elapsed = cleanup_start.map(|start| start.elapsed());
                    let poke_total_ms = duration_ms(event_elapsed)
                        + cleanup_elapsed.map(duration_ms).unwrap_or(0.0);
                    debug!(
                        event_num = event_num_before,
                        did_update,
                        poke_total_ms,
                        event_eval_ms = duration_ms(event_elapsed),
                        preserve_ms = pma_elapsed.map(duration_ms),
                        durable_append_ms = durable_append_elapsed.map(duration_ms),
                        snapshot_stage_ms = snapshot_stage_elapsed.map(duration_ms),
                        cleanup_total_ms = cleanup_elapsed.map(duration_ms),
                        "poke action done"
                    );
                };
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics.serf_loop_poke.add_timing(&action_elapsed);
                };
            }
            SerfAction::ProvideMetrics { metrics, result } => {
                serf.metrics = Some(metrics);
                let _ = result.send(()).inspect_err(|_e| {
                    debug!("Failed to send metric-provision result from serf thread");
                });
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_provide_metrics
                        .add_timing(&action_elapsed);
                };
            }
        };
        let elapsed = start.elapsed();
        if let Some(nockapp_metrics) = &serf.metrics {
            nockapp_metrics.serf_loop_all.add_timing(&elapsed);
        };
    }
}

fn create_checkpoint<C: SerfCheckpoint>(
    serf: &mut Serf,
    metrics: &Option<Arc<NockAppMetrics>>,
) -> C {
    let ker_hash = serf.ker_hash;
    let event_num = serf.event_num.load(Ordering::SeqCst);
    let space = serf.context.stack.noun_space();
    let ker_state = serf
        .arvo
        .in_space(&space)
        .slot(STATE_AXIS)
        .map(|handle| handle.noun())
        .unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
    let cold_state = serf.context.cold;

    let checkpoint = C::new(
        serf.stack(),
        ker_hash,
        event_num,
        ker_state,
        cold_state,
        metrics,
    );

    if let Some(pma) = serf.pma.as_ref() {
        if let Err(err) = pma.sync_all() {
            warn!("Failed to msync PMA after checkpoint save: {err}");
        }
        if let Err(err) = durability::sync_path_data(pma.path(), "checkpoint_pma_fdatasync") {
            warn!("Failed to fdatasync PMA after checkpoint save: {err}");
        }
        if let Err(err) = pma.advise_drop_allocated_prefix(4, 5) {
            warn!("Failed to madvise PMA prefix after checkpoint save: {err}");
        }
    }

    checkpoint
}

/// Represents a Sword kernel, containing a Serf and snapshot location.
pub struct Kernel<C> {
    /// The Serf managing the interface to the Sword.
    pub(crate) serf: SerfThread<C>,
}

impl<C: SerfCheckpoint + 'static> Kernel<C> {
    /// Loads a kernel with a custom hot state.
    ///
    /// # Arguments
    ///
    /// * `snap_dir` - Directory for storing snapshots.
    /// * `kernel` - Byte slice containing the kernel as a jammed noun.
    /// * `hot_state` - Custom hot state entries.
    /// * `trace` - Whether to enable tracing.
    ///
    /// # Returns
    ///
    /// A new `Kernel` instance.
    pub async fn load_with_hot_state(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
    ) -> Result<Self> {
        Self::load_with_hot_state_with_event_log(
            kernel, checkpoint, hot_state, test_jets, trace, pma, None,
        )
        .await
    }

    pub(crate) async fn load_with_hot_state_with_event_log(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new_with_event_log(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE, pma, event_log, test_jets,
            trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_tiny(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
    ) -> Result<Self> {
        Self::load_with_hot_state_tiny_with_event_log(
            kernel, checkpoint, hot_state, test_jets, trace, pma, None,
        )
        .await
    }

    pub(crate) async fn load_with_hot_state_tiny_with_event_log(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new_with_event_log(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_TINY, pma, event_log, test_jets,
            trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_small(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
    ) -> Result<Self> {
        Self::load_with_hot_state_small_with_event_log(
            kernel, checkpoint, hot_state, test_jets, trace, pma, None,
        )
        .await
    }

    pub(crate) async fn load_with_hot_state_small_with_event_log(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new_with_event_log(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_SMALL, pma, event_log,
            test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_medium(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
    ) -> Result<Self> {
        Self::load_with_hot_state_medium_with_event_log(
            kernel, checkpoint, hot_state, test_jets, trace, pma, None,
        )
        .await
    }

    pub(crate) async fn load_with_hot_state_medium_with_event_log(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new_with_event_log(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_MEDIUM, pma, event_log,
            test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_large(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
    ) -> Result<Self> {
        Self::load_with_hot_state_large_with_event_log(
            kernel, checkpoint, hot_state, test_jets, trace, pma, None,
        )
        .await
    }

    pub(crate) async fn load_with_hot_state_large_with_event_log(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new_with_event_log(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_LARGE, pma, event_log,
            test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub fn take_pma_timing_samples(&self) -> Option<Vec<(Duration, Duration)>> {
        self.serf.pma_timing.as_ref().map(|timing| {
            timing
                .take_samples()
                .into_iter()
                .map(|sample| (sample.event, sample.pma_copy))
                .collect()
        })
    }

    pub fn take_pma_timing_samples_detailed(&self) -> Option<Vec<PmaTimingSample>> {
        self.serf
            .pma_timing
            .as_ref()
            .map(|timing| timing.take_samples())
    }

    pub async fn load_with_hot_state_huge(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
    ) -> Result<Self> {
        Self::load_with_hot_state_huge_with_event_log(
            kernel, checkpoint, hot_state, test_jets, trace, pma, None,
        )
        .await
    }

    pub(crate) async fn load_with_hot_state_huge_with_event_log(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new_with_event_log(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_HUGE, pma, event_log, test_jets,
            trace,
        )
        .await?;
        Ok(Self { serf })
    }

    /// Loads a kernel with default hot state.
    ///
    /// # Arguments
    ///
    /// * `snap_dir` - Directory for storing snapshots.
    /// * `kernel` - Byte slice containing the kernel code.
    /// * `trace` - Whether to enable tracing.
    ///
    /// # Returns
    ///
    /// A new `Kernel` instance.
    pub async fn load(
        kernel: &[u8],
        checkpoint: Option<C>,
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
    ) -> Result<Self> {
        Self::load_with_event_log(kernel, checkpoint, test_jets, trace, pma, None).await
    }

    pub(crate) async fn load_with_event_log(
        kernel: &[u8],
        checkpoint: Option<C>,
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        pma: Option<PmaConfig>,
        event_log: Option<EventLogConfig>,
    ) -> Result<Self> {
        Self::load_with_hot_state_with_event_log(
            kernel,
            checkpoint,
            &Vec::new(),
            test_jets,
            trace,
            pma,
            event_log,
        )
        .await
    }

    /// Produces a checkpoint of the kernel state.
    pub fn checkpoint(&self) -> impl Future<Output = Result<C>> {
        self.serf.checkpoint()
    }

    pub(crate) fn replay_event_jobs(
        &self,
        jobs: Vec<ReplayLogEntry>,
    ) -> impl Future<Output = Result<()>> {
        self.serf.replay_event_jobs(jobs)
    }
}

impl<C> Kernel<C> {
    // We are very carefully ensuring the future does not contain the "self" reference to ensure no lifetime issues when spawning tasks
    pub fn poke(&self, wire: WireRepr, cause: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        self.serf.poke(wire, cause)
    }

    pub fn poke_sync(&self, wire: WireRepr, cause: NounSlab) -> Result<NounSlab> {
        self.serf.poke_sync(wire, cause)
    }

    pub fn peek_sync(&self, ovo: NounSlab) -> Result<NounSlab> {
        self.serf.peek_sync(ovo)
    }

    pub fn poke_timeout(
        &self,
        wire: WireRepr,
        cause: NounSlab,
        timeout: Duration,
    ) -> impl Future<Output = Result<NounSlab>> {
        self.serf.poke_timeout(wire, cause, timeout)
    }

    // We are very carefully ensuring the future does not contain the "self" reference to ensure no lifetime issues when spawning tasks
    #[tracing::instrument(name = "crown::Kernel::peek", skip_all)]
    pub(crate) fn peek(&self, ovo: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        self.serf.peek(ovo)
    }

    pub fn import(&self, state: LoadState) -> impl Future<Output = Result<()>> {
        self.serf.import(state)
    }

    pub fn export(&self) -> impl Future<Output = Result<LoadState>> {
        self.serf.export()
    }

    pub(crate) fn provide_metrics(
        &mut self,
        metrics: Arc<NockAppMetrics>,
    ) -> impl Future<Output = Result<()>> {
        self.serf.provide_metrics(metrics)
    }
}

/// Represents the Serf, which maintains context and provides an interface to
/// the Sword.
pub struct Serf {
    /// Hash of boot kernel
    pub ker_hash: Hash,
    /// The current Arvo state.
    pub arvo: Noun,
    /// The interpreter context.
    pub context: interpreter::Context,
    /// Persistent memory arena for long-lived state.
    pub pma: Option<Pma>,
    /// Optional metadata path for PMA persistence.
    pub pma_meta_path: Option<PathBuf>,
    /// Optional GC configuration for PMA slab compaction.
    pma_gc_state: Option<PmaGcState>,
    snapshot_creation_enabled: bool,
    rotating_snapshot_interval_event_time: Option<Duration>,
    cumulative_event_processing_time_since_snapshot: Duration,
    /// Optional append-only event log used as the durability boundary.
    event_log: Option<EventLog>,
    /// Cancellation
    pub cancel_token: NockCancelToken,
    /// The current event number.
    pub event_num: Arc<AtomicU64>,
    /// A metrics
    pub metrics: Option<Arc<NockAppMetrics>>,
}

impl Serf {
    /// Creates a new Serf instance.
    ///
    /// # Arguments
    ///
    /// * `stack` - The Nock stack.
    /// * `checkpoint` - Optional checkpoint to restore from.
    /// * `kernel_bytes` - Byte slice containing the kernel code.
    /// * `constant_hot_state` - Custom hot state entries.
    /// * `trace_info` - Optional nockvm tracing implementation.
    ///
    /// # Returns
    ///
    /// A new `Serf` instance.
    #[allow(clippy::too_many_arguments)]
    fn new<C: SerfCheckpoint>(
        mut stack: NockStack,
        mut pma: Option<Pma>,
        pma_meta_path: Option<PathBuf>,
        pma_meta_load: bool,
        pma_gc_state: Option<PmaGcState>,
        snapshot_creation_enabled: bool,
        rotating_snapshot_interval_event_time: Option<Duration>,
        mut event_log: Option<EventLog>,
        checkpoint: Option<C>,
        kernel_bytes: &[u8],
        constant_hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
        nock_stack_size: usize,
    ) -> Result<Self> {
        let hot_state = [URBIT_HOT_STATE, constant_hot_state].concat();

        if let Some(ref pma) = pma {
            stack.install_pma_arena(Arc::clone(pma.arena()));
        }

        let mut hasher = Hasher::new();
        hasher.update(kernel_bytes);
        let ker_hash = hasher.finalize();
        let mut diagnostics =
            SerfInitDiagnostics::new(nock_stack_size).with_current_ker_hash(ker_hash);

        let mut reset_pma = false;
        let pma_state = if pma_meta_load && checkpoint.is_none() {
            if let (Some(_pma), Some(meta_path)) = (pma.as_ref(), pma_meta_path.as_ref()) {
                if let Some(meta) = PmaPersistMetadata::load_from_path(meta_path) {
                    // A kernel-hash mismatch does not discard the PMA: the
                    // persisted state noun is loaded into the freshly-cued
                    // current kernel, which runs its own Hoon-level `+load`
                    // state migration. This matches the checkpoint restore
                    // path below ("loading checkpoint state into current
                    // kernel"). The metadata checksum (already validated by
                    // `load_from_path`) and the PMA trailer guard structural
                    // integrity; kernel-hash equality is not required for
                    // memory safety.
                    if meta.ker_hash != ker_hash {
                        warn!(
                            "PMA metadata kernel hash mismatch (metadata: {}, kernel: {}); loading existing PMA state into current kernel",
                            meta.ker_hash, ker_hash
                        );
                    }
                    let kernel_state = unsafe { Noun::from_raw(meta.kernel_state_raw) };
                    Some((kernel_state, meta.event_num))
                } else {
                    if meta_path.exists() {
                        warn!(
                            "Failed to load PMA metadata at {}; starting fresh",
                            meta_path.display()
                        );
                    }
                    reset_pma = true;
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if !pma_meta_load {
            if let Some(meta_path) = pma_meta_path.as_ref() {
                let _ = fs::remove_file(meta_path);
            }
        }

        if reset_pma {
            if let Some(pma) = pma.as_mut() {
                pma.reset();
            }
            if let Some(meta_path) = pma_meta_path.as_ref() {
                let _ = fs::remove_file(meta_path);
            }
        }

        let (maybe_state, cold, event_num_raw) = if let Some(c) = checkpoint {
            let saveable = c.load();
            diagnostics = diagnostics.with_checkpoint(&saveable);

            let ker_state = run_serf_init_phase(
                &diagnostics,
                "copy checkpoint state into Nock stack",
                || saveable.state.copy_to_stack(&mut stack),
            )?;
            let cold_noun = run_serf_init_phase(
                &diagnostics,
                "copy checkpoint cold state into Nock stack",
                || saveable.cold.copy_to_stack(&mut stack),
            )?;
            let cold_vecs =
                run_serf_init_phase(&diagnostics, "decode checkpoint cold state", || {
                    let space = stack.noun_space();
                    Cold::from_noun(&mut stack, &cold_noun, &space).map_err(|err| {
                        CrownError::Unknown(format!(
                            "could not load cold state from checkpoint: {err}"
                        ))
                    })
                })??;
            let cold = run_serf_init_phase(&diagnostics, "rebuild checkpoint cold state", || {
                Cold::from_vecs(&mut stack, cold_vecs.0, cold_vecs.1, cold_vecs.2)
            })?;
            if saveable.ker_hash != ker_hash {
                warn!(
                    checkpoint = %saveable.ker_hash.to_hex(),
                    current = %ker_hash.to_hex(),
                    "checkpoint kernel hash mismatch; loading checkpoint state into current kernel"
                );
            }
            (Some(ker_state), cold, saveable.event_num)
        } else if let Some((ker_state, event_num)) = pma_state {
            info!("Loaded PMA state at event_num {}", event_num);
            (Some(ker_state), Cold::new(&mut stack), event_num)
        } else {
            let cold = run_serf_init_phase(&diagnostics, "initialize empty cold state", || {
                Cold::new(&mut stack)
            })?;
            (None, cold, 0)
        };

        let event_num = Arc::new(AtomicU64::new(event_num_raw));

        let mut context =
            run_serf_init_phase(&diagnostics, "create Nock interpreter context", || {
                create_context(stack, &hot_state, cold, trace.into(), test_jets)
            })?;
        let cancel_token = context.cancel_token();

        let mut arvo = run_serf_init_phase(&diagnostics, "boot kernel", || {
            let kernel_trap = Noun::cue_bytes_slice(&mut context.stack, kernel_bytes)
                .expect("invalid kernel jam");
            let fol = T(&mut context.stack, &[D(9), D(2), D(0), D(1)]);

            if context.trace_info.is_some() {
                let start = Instant::now();
                let arvo = interpret(&mut context, kernel_trap, fol).unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                write_serf_trace_safe(&mut context, "boot", start);
                arvo
            } else {
                interpret(&mut context, kernel_trap, fol).unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                })
            }
        })?;

        let cumulative_event_processing_time_since_snapshot = if snapshot_creation_enabled
            && rotating_snapshot_interval_event_time.is_some()
        {
            match event_log.as_mut() {
                Some(event_log) => {
                    let latest_ready_snapshot_event_num = event_log
                        .latest_ready_snapshot_event_num()
                        .unwrap_or_else(|err| {
                            panic!("serf: failed to load latest ready snapshot event_num: {err}")
                        })
                        .unwrap_or(0);
                    event_log
                            .event_processing_time_after(latest_ready_snapshot_event_num)
                            .unwrap_or_else(|err| {
                                panic!(
                                    "serf: failed to load cumulative event processing time since latest snapshot: {err}"
                                )
                            })
                }
                None => Duration::ZERO,
            }
        } else {
            Duration::ZERO
        };

        let mut serf = Self {
            ker_hash,
            arvo,
            context,
            pma,
            pma_meta_path,
            pma_gc_state,
            snapshot_creation_enabled,
            rotating_snapshot_interval_event_time,
            cumulative_event_processing_time_since_snapshot,
            event_log,
            event_num,
            cancel_token,
            metrics: None,
        };

        if let Some(kernel_state) = maybe_state {
            arvo =
                run_serf_init_phase(&diagnostics, "load checkpoint state through kernel", || {
                    serf.load(kernel_state)
                })??;
        }

        run_serf_init_phase(&diagnostics, "preserve loaded kernel state", || unsafe {
            serf.event_update(event_num_raw, arvo);
            let _ = serf.preserve_event_update_leftovers();
        })?;
        serf.ensure_epoch_snapshot();
        Ok(serf)
    }

    /// Performs a peek operation on the Arvo state.
    ///
    /// # Arguments
    ///
    /// * `ovo` - The peek request noun.
    ///
    /// # Returns
    ///
    /// Result containing the peeked data or an error.
    #[tracing::instrument(skip_all)]
    pub fn peek(&mut self, ovo: Noun) -> Result<Noun> {
        if self.context.trace_info.is_some() {
            let trace_name = "peek";
            let start = Instant::now();
            let slam_res = self.slam(PEEK_AXIS, ovo);
            write_serf_trace_safe(&mut self.context, trace_name, start);

            slam_res
        } else {
            self.slam(PEEK_AXIS, ovo)
        }
    }

    /// Generates a goof (error) noun.
    ///
    /// # Arguments
    ///
    /// * `mote` - The error mote.
    /// * `traces` - Trace information.
    ///
    /// # Returns
    ///
    /// A noun representing the error.
    pub fn goof(&mut self, mote: Mote, traces: Noun) -> Noun {
        let tone = Cell::new(&mut self.context.stack, D(2), traces);
        let space = self.context.stack.noun_space();
        let tang = mook(&mut self.context, tone, false)
            .expect("serf: goof: +mook crashed on bail")
            .in_space(&space)
            .tail()
            .noun();
        T(&mut self.context.stack, &[D(mote as u64), tang])
    }

    /// Performs a load operation on the Arvo state.
    ///
    /// # Arguments
    ///
    /// * `old` - The state to load.
    ///
    /// # Returns
    ///
    /// Result containing the loaded kernel or an error.
    pub fn load(&mut self, old: Noun) -> Result<Noun> {
        match self.soft(old, LOAD_AXIS, Some("load".to_string())) {
            Ok(res) => Ok(res),
            Err(goof) => {
                self.print_goof(goof);
                Err(CrownError::SerfLoadError)
            }
        }
    }

    pub fn print_goof(&mut self, goof: Noun) {
        let space = self.context.stack.noun_space();
        let tang = goof
            .in_space(&space)
            .as_cell()
            .expect("print goof: expected goof to be a cell")
            .tail()
            .noun();
        tang.in_space(&space).list_iter().for_each(|tank| {
            //  TODO: Slogger should be emitting Results in case of failure
            self.context
                .slogger
                .slog(&mut self.context.stack, 1, tank.noun());
        });
    }

    /// Performs a poke operation on the Arvo state.
    ///
    /// # Arguments
    ///
    /// * `job` - The poke job noun.
    ///
    /// # Returns
    ///
    /// Result containing the poke response or an error.
    #[tracing::instrument(level = "info", skip_all)]
    fn do_poke(
        &mut self,
        job: Noun,
        metadata: Option<&AcceptedEventMetadata>,
    ) -> Result<AcceptedPoke> {
        let space = self.context.stack.noun_space();
        match self.soft(job, POKE_AXIS, Some("poke".to_string())) {
            Ok(res) => {
                let cell = res
                    .in_space(&space)
                    .as_cell()
                    .expect("serf: poke: +slam returned atom");
                let fec = cell.head().noun();
                let eve = self.event_num.load(Ordering::SeqCst);

                unsafe {
                    self.event_update(eve + 1, cell.tail().noun());
                }
                let durable_event = metadata
                    .map(|metadata| self.capture_accepted_event(job, eve + 1, metadata))
                    .transpose()?;
                Ok(AcceptedPoke {
                    effects: fec,
                    durable_event,
                })
            }
            Err(goof) => self.poke_swap(job, goof, metadata),
        }
    }

    /// Slams (applies) a gate at a specific axis of Arvo.
    ///
    /// # Arguments
    ///
    /// * `axis` - The axis to slam.
    /// * `ovo` - The sample noun.
    ///
    /// # Returns
    ///
    /// Result containing the slammed result or an error.
    pub fn slam(&mut self, axis: u64, ovo: Noun) -> Result<Noun> {
        let arvo = self.arvo;
        slam(&mut self.context, arvo, axis, ovo, self.metrics.clone())
    }

    /// Performs a "soft" computation, handling errors gracefully.
    ///
    /// # Arguments
    ///
    /// * `ovo` - The input noun.
    /// * `axis` - The axis to slam.
    /// * `trace_name` - Optional name for tracing.
    ///
    /// # Returns
    ///
    /// Result containing the computed noun or an error noun.
    fn soft(&mut self, ovo: Noun, axis: u64, trace_name: Option<String>) -> Result<Noun, Noun> {
        let slam_res = if self.context.trace_info.is_some() {
            let start = Instant::now();
            let slam_res = self.slam(axis, ovo);
            write_serf_trace_safe(
                &mut self.context,
                trace_name.as_ref().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                }),
                start,
            );

            slam_res
        } else {
            self.slam(axis, ovo)
        };

        match slam_res {
            Ok(res) => Ok(res),
            Err(error) => match error {
                CrownError::InterpreterError(e) => {
                    let (mote, traces) = match e.0 {
                        Error::Deterministic(mote, traces)
                        | Error::NonDeterministic(mote, traces) => (mote, traces),
                        Error::ScryBlocked(_) | Error::ScryCrashed(_) => {
                            panic!("serf: soft: .^ invalid outside of virtual Nock")
                        }
                    };

                    Err(self.goof(mote, traces))
                }
                _ => Err(D(0)),
            },
        }
    }

    /// Plays a list of events.
    ///
    /// # Arguments
    ///
    /// * `lit` - The list of events to play.
    ///
    /// # Returns
    ///
    /// Result containing the final Arvo state or an error.
    fn play_list(&mut self, mut lit: Noun) -> Result<Noun> {
        let space = self.context.stack.noun_space();
        let mut eve = self.event_num.load(Ordering::SeqCst);
        while let Ok(cell) = lit.in_space(&space).as_cell() {
            let ovo = cell.head().noun();
            lit = cell.tail().noun();
            let trace_name = if self.context.trace_info.is_some() {
                Some(format!("play [{}]", eve))
            } else {
                None
            };

            match self.soft(ovo, POKE_AXIS, trace_name) {
                Ok(res) => {
                    let arvo = res.in_space(&space).as_cell()?.tail().noun();
                    eve += 1;

                    unsafe {
                        self.event_update(eve, arvo);
                    }
                }
                Err(goof) => {
                    let mut goof_slab = NounSlab::new();
                    goof_slab.copy_into(goof, &space);
                    return Err(CrownError::KernelError(Some(goof_slab)));
                }
            }
        }
        Ok(self.arvo)
    }

    /// Handles a poke error by swapping in a new event.
    ///
    /// # Arguments
    ///
    /// * `job` - The original poke job.
    /// * `goof` - The error noun.
    ///
    /// # Returns
    ///
    /// Result containing the new event or an error.
    fn poke_swap(
        &mut self,
        job: Noun,
        goof: Noun,
        metadata: Option<&AcceptedEventMetadata>,
    ) -> Result<AcceptedPoke> {
        let stack = &mut self.context.stack;
        let space = stack.noun_space();
        self.context.cache = Hamt::<Noun>::new(stack);
        let job_cell = job
            .in_space(&space)
            .as_cell()
            .expect("serf: poke: job not a cell");
        // job data is job without event_num
        let job_data = job_cell
            .tail()
            .as_cell()
            .expect("serf: poke: data not a cell");
        //  job input is job without event_num or wire
        let job_input = job_data.tail().noun();
        let wire = T(stack, &[D(0), D(tas!(b"arvo")), D(0)]);
        let crud = DirectAtom::new_panic(tas!(b"crud"));
        let event_num = D(self.event_num.load(Ordering::SeqCst) + 1);

        let ovo = T(stack, &[event_num, wire, goof, job_input]);
        let trace_name = if self.context.trace_info.is_some() {
            Some(Self::poke_trace_name(
                &mut self.context.stack,
                wire,
                crud.as_atom(),
            ))
        } else {
            None
        };

        match self.soft(ovo, POKE_AXIS, trace_name) {
            Ok(res) => {
                let cell = res
                    .in_space(&space)
                    .as_cell()
                    .expect("serf: poke: crud +slam returned atom");
                let fec = cell.head().noun();
                let eve = self.event_num.load(Ordering::SeqCst);

                unsafe {
                    self.event_update(eve + 1, cell.tail().noun());
                }
                let durable_event = metadata
                    .map(|metadata| self.capture_accepted_event(ovo, eve + 1, metadata))
                    .transpose()?;
                Ok(AcceptedPoke {
                    effects: fec,
                    durable_event,
                })
            }
            Err(goof_crud) => {
                let mut goof_slab = NounSlab::new();
                goof_slab.copy_into(goof_crud, &space);
                Err(CrownError::KernelError(Some(goof_slab)))
            }
        }
    }

    /// Generates a trace name for a poke operation.
    ///
    /// # Arguments
    ///
    /// * `stack` - The Nock stack.
    /// * `wire` - The wire noun.
    /// * `vent` - The vent atom.
    ///
    /// # Returns
    ///
    /// A string representing the trace name.
    fn poke_trace_name(stack: &mut NockStack, wire: Noun, vent: Atom) -> String {
        let wpc = path_to_cord(stack, wire);
        let space = stack.noun_space();
        let wpc_len = met3_usize(wpc, &space);
        let wpc_handle = wpc.in_space(&space);
        let wpc_bytes = &wpc_handle.as_ne_bytes()[0..wpc_len];
        let wpc_str = match std::str::from_utf8(wpc_bytes) {
            Ok(valid) => valid,
            Err(error) => {
                let (valid, _) = wpc_bytes.split_at(error.valid_up_to());
                unsafe { std::str::from_utf8_unchecked(valid) }
            }
        };

        let vc_len = met3_usize(vent, &space);
        let vent_handle = vent.in_space(&space);
        let vc_bytes = &vent_handle.as_ne_bytes()[0..vc_len];
        let vc_str = match std::str::from_utf8(vc_bytes) {
            Ok(valid) => valid,
            Err(error) => {
                let (valid, _) = vc_bytes.split_at(error.valid_up_to());
                unsafe { std::str::from_utf8_unchecked(valid) }
            }
        };

        format!("poke [{} {}]", wpc_str, vc_str)
    }

    /// Performs a poke operation with a given cause.
    ///
    /// # Arguments
    ///
    /// * `wire` - The wire noun.
    /// * `cause` - The cause noun.
    ///
    /// # Returns
    ///
    /// Result containing the poke response or an error.
    #[tracing::instrument(level = "info", skip_all, fields(
        src = wire.source
    ))]
    fn poke(&mut self, wire: WireRepr, cause: Noun) -> Result<AcceptedPoke> {
        let metadata = self.prepare_accepted_event_metadata(&wire, cause)?;
        let random_bytes = rand::random::<u64>();
        let bytes = random_bytes.as_bytes()?;
        let eny: Atom = Atom::from_bytes(&mut self.context.stack, &bytes);
        let our = <nockvm::noun::Atom as AtomExt>::from_value(&mut self.context.stack, 0)?; // Using 0 as default value
        let mut t_vec: Vec<u8> = vec![];
        t_vec.write_u128::<LittleEndian>(current_da().0)?;
        let now: Atom = <IndirectAtom as IndirectAtomExt>::from_bytes(
            &mut self.context.stack,
            t_vec.as_slice(),
        );

        let event_num = D(self.event_num.load(Ordering::SeqCst) + 1);
        let base_wire_noun = wire_to_noun(&mut self.context.stack, &wire);
        let wire = T(&mut self.context.stack, &[D(tas!(b"poke")), base_wire_noun]);
        let poke = T(
            &mut self.context.stack,
            &[event_num, wire, eny.as_noun(), our.as_noun(), now.as_noun(), cause],
        );

        self.do_poke(poke, metadata.as_ref())
    }

    fn prepare_accepted_event_metadata(
        &self,
        wire: &WireRepr,
        cause: Noun,
    ) -> Result<Option<AcceptedEventMetadata>> {
        if self.event_log.is_none() {
            return Ok(None);
        }
        let space = self.context.stack.noun_space();
        let cause_jam = NockJammer::jam(cause, &space).to_vec();
        let cause_hash = blake3::hash(&cause_jam);
        let wire_tags = wire
            .tags
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();
        let wire_tags_json = serde_json::to_string(&wire_tags)
            .map_err(|err| CrownError::SaveError(format!("wire tags json encode failed: {err}")))?;
        Ok(Some(AcceptedEventMetadata {
            wire_source: wire.source.to_string(),
            wire_version: i64::try_from(wire.version)?,
            wire_tags_json,
            cause_hash: cause_hash.as_bytes().to_vec(),
            created_at_ms: Self::current_time_ms()?,
        }))
    }

    fn capture_accepted_event(
        &self,
        accepted_job: Noun,
        event_num: u64,
        metadata: &AcceptedEventMetadata,
    ) -> Result<EventLogEntry> {
        let space = self.context.stack.noun_space();
        let job_jam = NockJammer::jam(accepted_job, &space).to_vec();
        let job_hash = blake3::hash(&job_jam);
        Ok(EventLogEntry {
            event_num,
            job_jam,
            wire_source: metadata.wire_source.clone(),
            wire_version: metadata.wire_version,
            wire_tags_json: metadata.wire_tags_json.clone(),
            cause_hash: metadata.cause_hash.clone(),
            job_hash: job_hash.as_bytes().to_vec(),
            event_processing_duration: Duration::ZERO,
            created_at_ms: metadata.created_at_ms,
        })
    }

    fn current_time_ms() -> Result<i64> {
        Ok(i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|err| CrownError::SaveError(format!("system clock error: {err}")))?
                .as_millis(),
        )?)
    }

    fn append_durable_event(&mut self, event: &EventLogEntry) -> Duration {
        let Some(event_log) = self.event_log.as_mut() else {
            debug!(
                event_num = event.event_num,
                "event-log append skipped: event log disabled"
            );
            return Duration::from_millis(0);
        };
        debug!(
            event_num = event.event_num,
            path = %event_log.path().display(),
            sqlite_sync_mode = if durability::fsync_disabled() { "OFF" } else { "FULL" },
            "event-log append start"
        );
        let start = Instant::now();
        if let Err(err) = event_log.append_event(event) {
            if let Some(metrics) = &self.metrics {
                metrics.event_log_commit_failures.increment();
            }
            let elapsed = start.elapsed();
            error!(
                event_num = event.event_num,
                path = %event_log.path().display(),
                elapsed_ms = duration_ms(elapsed),
                error = %err,
                "event-log append failed"
            );
            std::process::abort();
        }
        let elapsed = start.elapsed();
        debug!(
            event_num = event.event_num,
            path = %event_log.path().display(),
            elapsed_ms = duration_ms(elapsed),
            "event-log append done"
        );
        if let Some(metrics) = &self.metrics {
            metrics.event_log_append.add_timing(&elapsed);
        }
        elapsed
    }

    fn ensure_pma_capacity_before_event(&mut self) -> Result<()> {
        let Some(pma) = self.pma.as_mut() else {
            return Ok(());
        };
        let reserve_words = pma_event_preflight_free_words(pma);
        if pma.free_words() >= reserve_words {
            return Ok(());
        }
        let event_num = self.event_num.load(Ordering::SeqCst).saturating_add(1);
        info!(
            event_num,
            path = %pma.path().display(),
            capacity_words = pma.size_words(),
            alloc_words = pma.alloc_offset(),
            free_words = pma.free_words(),
            reserve_words,
            "event PMA preflight growth start"
        );
        pma.ensure_free_words(reserve_words).map_err(|err| {
            CrownError::SaveError(format!(
                "PMA preflight growth failed before event {event_num}: {err}"
            ))
        })?;
        info!(
            event_num,
            path = %pma.path().display(),
            capacity_words = pma.size_words(),
            alloc_words = pma.alloc_offset(),
            free_words = pma.free_words(),
            reserve_words,
            "event PMA preflight growth done"
        );
        Ok(())
    }

    fn ensure_epoch_snapshot(&mut self) {
        if !self.snapshot_creation_enabled {
            debug!("epoch snapshot skipped: creation disabled");
            return;
        }
        let kernel_root_raw = {
            let space = self.context.stack.noun_space();
            let kernel_state = self
                .arvo
                .in_space(&space)
                .slot(STATE_AXIS)
                .map(|handle| handle.noun());
            match kernel_state {
                Ok(noun) => unsafe { noun.as_raw() },
                Err(err) => {
                    warn!("epoch snapshot skipped: failed to resolve kernel root: {err:?}");
                    return;
                }
            }
        };

        let event_num = self.event_num.load(Ordering::SeqCst);
        let (Some(event_log), Some(pma)) = (&mut self.event_log, &self.pma) else {
            return;
        };
        let build_start = Instant::now();
        info!(event_num, "epoch snapshot build start");
        match maybe_create_epoch_snapshot(
            event_log, pma, self.ker_hash, event_num, kernel_root_raw, SNAPSHOT_UNUSED_COLD_OFFSET,
        ) {
            Ok(created) => {
                let elapsed = build_start.elapsed();
                if created {
                    self.cumulative_event_processing_time_since_snapshot = Duration::ZERO;
                }
                info!(
                    event_num,
                    created,
                    elapsed_ms = duration_ms(elapsed),
                    "epoch snapshot build done"
                );
                if let Some(metrics) = &self.metrics {
                    metrics.snapshot_build.add_timing(&elapsed);
                }
            }
            Err(err) => {
                let elapsed = build_start.elapsed();
                if let Some(metrics) = &self.metrics {
                    metrics.snapshot_build_failures.increment();
                }
                warn!(
                    event_num,
                    elapsed_ms = duration_ms(elapsed),
                    error = %err,
                    "epoch snapshot build failed"
                );
            }
        }
    }

    fn replay_event_jobs(&mut self, jobs: Vec<ReplayLogEntry>) -> Result<()> {
        let replay_start = Instant::now();
        info!(jobs = jobs.len(), "event replay start");
        let mut dirty_since_preserve = false;
        for (idx, entry) in jobs.into_iter().enumerate() {
            let expected_current = entry.event_num.checked_sub(1).ok_or_else(|| {
                CrownError::Unknown("event replay encountered event_num 0".to_string())
            })?;
            let current = self.event_num.load(Ordering::SeqCst);
            if current != expected_current {
                return Err(CrownError::Unknown(format!(
                    "event replay expected current event_num {expected_current} before applying {}, found {current}",
                    entry.event_num
                )));
            }
            let job = Noun::cue_bytes_slice(&mut self.context.stack, &entry.job_jam)?;
            self.replay_one_event(entry.event_num, job)?;
            dirty_since_preserve = true;
            if (idx + 1) % REPLAY_PRESERVE_BATCH == 0 {
                let preserve_start = Instant::now();
                unsafe {
                    let _ = self.preserve_event_update_leftovers();
                }
                debug!(
                    replay_batch_end = idx + 1,
                    elapsed_ms = duration_ms(preserve_start.elapsed()),
                    "event replay preserve batch done"
                );
                dirty_since_preserve = false;
            }
        }
        if dirty_since_preserve {
            let preserve_start = Instant::now();
            unsafe {
                let _ = self.preserve_event_update_leftovers();
            }
            debug!(
                elapsed_ms = duration_ms(preserve_start.elapsed()),
                "event replay final preserve done"
            );
        }
        info!(
            elapsed_ms = duration_ms(replay_start.elapsed()),
            "event replay done"
        );
        Ok(())
    }

    fn replay_one_event(&mut self, event_num: u64, job: Noun) -> Result<()> {
        let space = self.context.stack.noun_space();
        let job_cell = job.in_space(&space).as_cell().map_err(CrownError::from)?;
        let job_event_num = job_cell.head().as_atom()?.as_u64()?;
        if job_event_num != event_num {
            return Err(CrownError::Unknown(format!(
                "event replay job event_num {job_event_num} does not match SQLite event_num {event_num}"
            )));
        }
        let trace_name = if self.context.trace_info.is_some() {
            Some(format!("replay [{event_num}]"))
        } else {
            None
        };
        match self.soft(job, POKE_AXIS, trace_name) {
            Ok(res) => {
                let cell = res
                    .in_space(&space)
                    .as_cell()
                    .expect("serf: replay: +slam returned atom");
                unsafe {
                    self.event_update(event_num, cell.tail().noun());
                }
                let current = self.event_num.load(Ordering::SeqCst);
                if current != event_num {
                    return Err(CrownError::Unknown(format!(
                        "event replay applied event {event_num}, but current event_num is {current}"
                    )));
                }
                Ok(())
            }
            Err(goof) => {
                let mut goof_slab = NounSlab::new();
                goof_slab.copy_into(goof, &space);
                Err(CrownError::KernelError(Some(goof_slab)))
            }
        }
    }

    fn maybe_create_rotating_snapshot(&mut self) {
        if !self.snapshot_creation_enabled {
            debug!("rotating snapshot skipped: creation disabled");
            return;
        }
        let Some(interval) = self.rotating_snapshot_interval_event_time else {
            return;
        };
        if self.cumulative_event_processing_time_since_snapshot < interval {
            return;
        }
        let kernel_root_raw = {
            let space = self.context.stack.noun_space();
            let kernel_state = self
                .arvo
                .in_space(&space)
                .slot(STATE_AXIS)
                .map(|handle| handle.noun());
            match kernel_state {
                Ok(noun) => unsafe { noun.as_raw() },
                Err(err) => {
                    warn!("rotating snapshot skipped: failed to resolve kernel root: {err:?}");
                    return;
                }
            }
        };

        let event_num = self.event_num.load(Ordering::SeqCst);
        let (Some(event_log), Some(pma)) = (&mut self.event_log, &self.pma) else {
            return;
        };
        let build_start = Instant::now();
        info!(
            event_num,
            rotating_interval_event_time = ?self.rotating_snapshot_interval_event_time,
            cumulative_event_processing_time_since_snapshot = ?self.cumulative_event_processing_time_since_snapshot,
            "rotating snapshot build start"
        );
        match maybe_create_rotating_snapshot(
            event_log, pma, self.ker_hash, event_num, kernel_root_raw, SNAPSHOT_UNUSED_COLD_OFFSET,
            self.cumulative_event_processing_time_since_snapshot,
            self.rotating_snapshot_interval_event_time,
        ) {
            Ok(status) => {
                let elapsed = build_start.elapsed();
                let created = status.created();
                if created {
                    self.cumulative_event_processing_time_since_snapshot = Duration::ZERO;
                }
                if let Some(metrics) = &self.metrics {
                    metrics.snapshot_build.add_timing(&elapsed);
                }
                if let Some(err) = status.cleanup_error() {
                    if let Some(metrics) = &self.metrics {
                        metrics.snapshot_build_failures.increment();
                    }
                    warn!(
                        event_num,
                        created,
                        elapsed_ms = duration_ms(elapsed),
                        error = %err,
                        "rotating snapshot build completed with cleanup error"
                    );
                    return;
                }
                info!(
                    event_num,
                    created,
                    elapsed_ms = duration_ms(elapsed),
                    "rotating snapshot build done"
                );
            }
            Err(err) => {
                let elapsed = build_start.elapsed();
                if let Some(metrics) = &self.metrics {
                    metrics.snapshot_build_failures.increment();
                }
                warn!(
                    event_num,
                    elapsed_ms = duration_ms(elapsed),
                    error = %err,
                    "rotating snapshot build failed"
                );
            }
        }
    }

    /// Updates the Serf's state after an event.
    ///
    /// # Arguments
    ///
    /// * `new_event_num` - The new event number.
    /// * `new_arvo` - The new Arvo state.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it modifies the Serf's state directly.
    #[tracing::instrument(level = "info", skip_all)]
    pub unsafe fn event_update(&mut self, new_event_num: u64, new_arvo: Noun) {
        self.arvo = new_arvo;
        self.event_num.store(new_event_num, Ordering::SeqCst);

        self.context.cache = Hamt::new(&mut self.context.stack);
        self.context.scry_stack = D(0);
    }

    unsafe fn copy_segment<T: PmaCopy>(
        label: &str,
        value: &mut T,
        stack: &NockStack,
        pma: &mut Pma,
        trace_pma: bool,
        segment: Option<&mut PmaCopySegment>,
    ) -> PmaCopySegment {
        if trace_pma {
            debug!("pma-copy: {label}");
        }
        let before = pma.alloc_offset();
        let start = Instant::now();
        value.copy_to_pma(stack, pma);
        let copy_segment = PmaCopySegment {
            elapsed: start.elapsed(),
            alloc_words: pma.alloc_offset().saturating_sub(before),
        };
        if let Some(segment) = segment {
            *segment = copy_segment;
        }
        copy_segment
    }

    unsafe fn copy_durable_state_to_pma(
        &mut self,
        pma: &mut Pma,
        trace_pma: bool,
        detail: Option<&mut PmaCopyDetail>,
    ) -> PmaCopySegment {
        let stack = &self.context.stack;
        Self::copy_segment(
            "arvo",
            &mut self.arvo,
            stack,
            pma,
            trace_pma,
            detail.map(|detail| &mut detail.arvo),
        )
    }

    unsafe fn copy_runtime_state_to_pma(
        &mut self,
        pma: &mut Pma,
        trace_pma: bool,
        mut detail: Option<&mut PmaCopyDetail>,
    ) {
        let event_num = self.event_num.load(Ordering::SeqCst);
        let context = &mut self.context;
        let stack = &context.stack;

        let segment = Self::copy_segment("warm", &mut context.warm, stack, pma, trace_pma, None);
        if let Some(detail) = detail.as_mut() {
            detail.warm = segment;
        }
        log_pma_preserve_component(event_num, "pma_warm_copy", segment);

        let segment = Self::copy_segment(
            "test_jets", &mut context.test_jets, stack, pma, trace_pma, None,
        );
        if let Some(detail) = detail.as_mut() {
            detail.test_jets = segment;
        }
        log_pma_preserve_component(event_num, "pma_test_jets_copy", segment);

        let segment = Self::copy_segment("hot", &mut context.hot, stack, pma, trace_pma, None);
        if let Some(detail) = detail.as_mut() {
            detail.hot = segment;
        }
        log_pma_preserve_component(event_num, "pma_hot_copy", segment);

        let segment = Self::copy_segment("cache", &mut context.cache, stack, pma, trace_pma, None);
        if let Some(detail) = detail.as_mut() {
            detail.cache = segment;
        }
        log_pma_preserve_component(event_num, "pma_cache_copy", segment);

        let segment = Self::copy_segment("cold", &mut context.cold, stack, pma, trace_pma, None);
        if let Some(detail) = detail.as_mut() {
            detail.cold = segment;
        }
        log_pma_preserve_component(event_num, "pma_cold_copy", segment);
    }

    unsafe fn copy_from_pma_segment<T: PmaCopyFrom>(
        value: &mut T,
        from_pma: &Pma,
        to_pma: &mut Pma,
    ) -> PmaCopySegment {
        let before = to_pma.alloc_offset();
        let start = Instant::now();
        value.copy_from_pma(from_pma, to_pma);
        PmaCopySegment {
            elapsed: start.elapsed(),
            alloc_words: to_pma.alloc_offset().saturating_sub(before),
        }
    }

    unsafe fn copy_runtime_state_from_pma(
        &mut self,
        from_pma: &Pma,
        to_pma: &mut Pma,
    ) -> PmaCopyDetail {
        let context = &mut self.context;
        PmaCopyDetail {
            warm: Self::copy_from_pma_segment(&mut context.warm, from_pma, to_pma),
            test_jets: Self::copy_from_pma_segment(&mut context.test_jets, from_pma, to_pma),
            hot: Self::copy_from_pma_segment(&mut context.hot, from_pma, to_pma),
            cache: Self::copy_from_pma_segment(&mut context.cache, from_pma, to_pma),
            cold: Self::copy_from_pma_segment(&mut context.cold, from_pma, to_pma),
            arvo: PmaCopySegment::default(),
        }
    }

    #[cfg(feature = "pma-assert")]
    fn assert_durable_state_in_pma(&self, pma: &Pma) {
        self.arvo.assert_in_pma(pma);
    }

    #[cfg(feature = "pma-assert")]
    fn assert_runtime_state_in_pma(&self, pma: &Pma) {
        self.context.warm.assert_in_pma(pma);
        self.context.test_jets.assert_in_pma(pma);
        self.context.hot.assert_in_pma(pma);
        self.context.cache.assert_in_pma(pma);
        self.context.cold.assert_in_pma(pma);
    }

    unsafe fn preserve_runtime_state_in_stack(&mut self, event_num: u64) {
        let stack = &mut self.context.stack;
        let start = Instant::now();
        stack.preserve(&mut self.context.warm);
        log_preserve_component(event_num, "warm", start.elapsed());

        let start = Instant::now();
        stack.preserve(&mut self.context.test_jets);
        log_preserve_component(event_num, "test_jets", start.elapsed());

        let start = Instant::now();
        stack.preserve(&mut self.context.hot);
        log_preserve_component(event_num, "hot", start.elapsed());

        let start = Instant::now();
        stack.preserve(&mut self.context.cache);
        log_preserve_component(event_num, "cache", start.elapsed());

        let start = Instant::now();
        stack.preserve(&mut self.context.cold);
        log_preserve_component(event_num, "cold", start.elapsed());
    }

    unsafe fn preserve_persistent_state_in_stack(&mut self, event_num: u64) {
        self.preserve_runtime_state_in_stack(event_num);
        let stack = &mut self.context.stack;
        let start = Instant::now();
        stack.preserve(&mut self.arvo);
        log_preserve_component(event_num, "arvo", start.elapsed());
    }

    fn persist_pma_metadata(&self, pma: &Pma) {
        if let Err(err) = self.persist_pma_metadata_strict(pma) {
            let Some(meta_path) = self.pma_meta_path.as_ref() else {
                return;
            };
            error!(
                path = %meta_path.display(),
                pma_path = %pma.path().display(),
                error = %err,
                "fatal PMA metadata persistence failure"
            );
            std::process::abort();
        }
    }

    fn persist_pma_state(&self, pma: &Pma) {
        pma.persist_metadata();
        if let Err(err) = self.sync_pma_data_strict(pma, "poke_cleanup_pma_fdatasync") {
            error!(
                path = %pma.path().display(),
                error = %err,
                "fatal PMA slab fdatasync failure after SQLite commit"
            );
            std::process::abort();
        }
        self.persist_pma_metadata(pma);
    }

    fn persist_pma_metadata_strict(&self, pma: &Pma) -> Result<()> {
        let Some(meta_path) = self.pma_meta_path.as_ref() else {
            return Ok(());
        };
        let space = self.context.stack.noun_space();
        let kernel_state = self
            .arvo
            .in_space(&space)
            .slot(STATE_AXIS)
            .map(|handle| handle.noun())
            .map_err(|err| {
                CrownError::SaveError(format!(
                    "failed to resolve kernel root for PMA metadata at {}: {err:?}",
                    meta_path.display()
                ))
            })?;
        let kernel_state_raw = unsafe { kernel_state.as_raw() };
        let event_num = self.event_num.load(Ordering::SeqCst);
        let pma_reserved_words = u64::try_from(pma.reserved_words()).map_err(|_| {
            CrownError::SaveError(
                "PMA reservation exceeds u64 while persisting metadata".to_string(),
            )
        })?;
        let meta = PmaPersistMetadata::new(
            self.ker_hash, event_num, kernel_state_raw, pma_reserved_words,
        );
        meta.save_to_path(meta_path)?;
        Ok(())
    }

    fn sync_pma_data_strict(&self, pma: &Pma, context: &str) -> Result<()> {
        durability::sync_path_data(pma.path(), context)?;
        Ok(())
    }

    fn flush_pma_state_for_shutdown(&self) -> Result<()> {
        let Some(pma) = self.pma.as_ref() else {
            return Ok(());
        };
        if pma.is_growth_poisoned() {
            return Err(CrownError::SaveError(
                "PMA growth failed after partial file growth; restart the node so PMA growth journal recovery can complete before publishing metadata".to_string(),
            ));
        }
        let event_num = self.event_num.load(Ordering::SeqCst);
        info!(
            event_num,
            path = %pma.path().display(),
            "shutdown PMA flush start"
        );
        pma.persist_metadata();
        pma.sync_all()?;
        self.sync_pma_data_strict(pma, "shutdown_pma_fdatasync")?;
        self.persist_pma_metadata_strict(pma)?;
        info!(
            event_num,
            path = %pma.path().display(),
            "shutdown PMA flush done"
        );
        Ok(())
    }

    fn maybe_pma_gc(&mut self, pma: Pma) -> (Pma, bool) {
        let Some(gc_state) = self.pma_gc_state.as_ref() else {
            return (pma, false);
        };
        if !gc_state.should_gc() {
            return (pma, false);
        }

        let from_path = gc_state.active_path().clone();
        let to_path = gc_state.inactive_path().clone();
        let from_meta_path = pma_meta_path(&from_path);
        let to_meta_path = pma_meta_path(&to_path);
        let gc_words = gc_state.words;
        let gc_reserved_words = gc_state.reserved_words;
        let event_num = self.event_num.load(Ordering::SeqCst);
        if !self.has_pma_gc_recovery_anchor() {
            warn!(
                event_num,
                from = %from_path.display(),
                to = %to_path.display(),
                "pma-gc: skipped because no ready snapshot recovery anchor is available"
            );
            return (pma, false);
        }

        let gc_start = Instant::now();
        let from_alloc = pma.alloc_offset();
        info!(
            "pma-gc: start: event_num={} from={} to={} from_alloc_words={}",
            event_num,
            from_path.display(),
            to_path.display(),
            from_alloc
        );

        if let Err(err) = Self::remove_pma_meta_for_gc(&to_meta_path, "pma_gc_inactive_meta_remove")
        {
            warn!(
                event_num,
                path = %to_meta_path.display(),
                error = %err,
                "pma-gc: skipped because inactive PMA metadata could not be removed"
            );
            return (pma, false);
        }

        let create_start = Instant::now();
        let to_pma_result = match gc_reserved_words {
            Some(reserved_words) => {
                Pma::new_with_reserved(gc_words, reserved_words, to_path.clone())
            }
            None => Pma::new(gc_words, to_path.clone()),
        };
        let mut to_pma = match to_pma_result {
            Ok(pma) => pma,
            Err(err) => {
                warn!(
                    "pma-gc: failed to create new PMA slab at {}: {err}",
                    to_path.display()
                );
                return (pma, false);
            }
        };
        let create_elapsed = create_start.elapsed();

        if let Err(err) =
            Self::remove_pma_meta_for_gc(&from_meta_path, "pma_gc_active_meta_invalidate")
        {
            warn!(
                event_num,
                path = %from_meta_path.display(),
                error = %err,
                "pma-gc: skipped because active PMA metadata could not be invalidated"
            );
            return (pma, false);
        }

        let arvo_start = Instant::now();
        let arvo_alloc_before = to_pma.alloc_offset();
        unsafe {
            self.arvo.copy_from_pma(&pma, &mut to_pma);
        }
        let arvo_segment = PmaCopySegment {
            elapsed: arvo_start.elapsed(),
            alloc_words: to_pma.alloc_offset().saturating_sub(arvo_alloc_before),
        };
        let mut runtime_segments = unsafe { self.copy_runtime_state_from_pma(&pma, &mut to_pma) };
        runtime_segments.arvo = arvo_segment;

        debug!(
            "pma-gc: copy timings: arvo_ms={} warm_ms={} test_jets_ms={} hot_ms={} cache_ms={} cold_ms={} arvo_alloc_words={} warm_alloc_words={} test_jets_alloc_words={} hot_alloc_words={} cache_alloc_words={} cold_alloc_words={}",
            runtime_segments.arvo.elapsed.as_millis(),
            runtime_segments.warm.elapsed.as_millis(),
            runtime_segments.test_jets.elapsed.as_millis(),
            runtime_segments.hot.elapsed.as_millis(),
            runtime_segments.cache.elapsed.as_millis(),
            runtime_segments.cold.elapsed.as_millis(),
            runtime_segments.arvo.alloc_words,
            runtime_segments.warm.alloc_words,
            runtime_segments.test_jets.alloc_words,
            runtime_segments.hot.alloc_words,
            runtime_segments.cache.alloc_words,
            runtime_segments.cold.alloc_words
        );

        self.context
            .stack
            .install_pma_arena(Arc::clone(to_pma.arena()));
        self.pma_meta_path = Some(to_meta_path);
        self.persist_pma_state(&to_pma);

        let to_alloc = to_pma.alloc_offset();
        let pageout_start = Instant::now();
        match to_pma.advise_drop_allocated_prefix(
            PMA_GC_DROP_ALLOCATED_PREFIX_NUMERATOR, PMA_GC_DROP_ALLOCATED_PREFIX_DENOMINATOR,
        ) {
            Ok(advised_bytes) => {
                debug!(
                    event_num,
                    path = %to_pma.path().display(),
                    advised_bytes,
                    elapsed_ms = duration_ms(pageout_start.elapsed()),
                    "pma-gc: advised compacted PMA pages out"
                );
            }
            Err(err) => {
                warn!(
                    event_num,
                    path = %to_pma.path().display(),
                    error = %err,
                    elapsed_ms = duration_ms(pageout_start.elapsed()),
                    "pma-gc: failed to advise compacted PMA pages out"
                );
            }
        }
        if let Some(gc_state) = self.pma_gc_state.as_mut() {
            gc_state.mark_gc_completed();
        }
        info!(
            "pma-gc: done: total_ms={} create_ms={} to_alloc_words={}",
            gc_start.elapsed().as_millis(),
            create_elapsed.as_millis(),
            to_alloc
        );

        (to_pma, true)
    }

    fn has_pma_gc_recovery_anchor(&mut self) -> bool {
        let Some(event_log) = self.event_log.as_mut() else {
            return false;
        };
        match event_log.has_ready_snapshot() {
            Ok(has_ready_snapshot) => has_ready_snapshot,
            Err(err) => {
                warn!("pma-gc: failed to check ready snapshot recovery anchor: {err}");
                false
            }
        }
    }

    fn remove_pma_meta_for_gc(meta_path: &Path, context: &str) -> Result<()> {
        match fs::remove_file(meta_path) {
            Ok(()) => durability::sync_parent_dir(meta_path, context).map_err(|err| {
                CrownError::SaveError(format!(
                    "failed to sync PMA metadata parent directory for {}: {err}",
                    meta_path.display()
                ))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(CrownError::SaveError(format!(
                "failed to remove PMA metadata at {}: {err}",
                meta_path.display()
            ))),
        }
    }

    unsafe fn flip_top_frame_and_trim_free_gap(&mut self, event_num: u64) {
        let flip_start = Instant::now();
        self.context.stack.flip_top_frame(0);
        log_preserve_component(event_num, "stack_flip_top_frame", flip_start.elapsed());
        self.trim_stack_free_gap(event_num);
    }

    fn trim_stack_free_gap(&self, event_num: u64) {
        if !stack_free_gap_trim_enabled() {
            return;
        }
        let trim_start = Instant::now();
        match self
            .context
            .stack
            .advise_free_gap(NOCK_STACK_FREE_GAP_TRIM_THRESHOLD_WORDS)
        {
            Ok(advice) if advice.advised_bytes > 0 => {
                debug!(
                    event_num,
                    free_gap_words = advice.free_gap_words,
                    free_gap_mib = words_to_mib(advice.free_gap_words),
                    advised_bytes = advice.advised_bytes,
                    advised_mib = bytes_to_mib(advice.advised_bytes),
                    threshold_mib = bytes_to_mib(NOCK_STACK_FREE_GAP_TRIM_THRESHOLD_BYTES),
                    elapsed_ms = duration_ms(trim_start.elapsed()),
                    "nockstack free-gap madvise done"
                );
            }
            Ok(_) => {}
            Err(err) => {
                warn!(
                    event_num,
                    error = %err,
                    elapsed_ms = duration_ms(trim_start.elapsed()),
                    "nockstack free-gap madvise failed"
                );
            }
        }
    }

    /// Preserves leftovers after an event update.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it modifies the Serf's state directly.
    #[tracing::instrument(level = "info", skip_all)]
    pub unsafe fn preserve_event_update_leftovers(&mut self) -> Option<PmaCopyDetail> {
        self.preserve_event_update_leftovers_with_pre_persist(|_| {})
    }

    /// Preserves leftovers after an event update, with a hook after durable PMA copy succeeds
    /// but before durable PMA metadata is published.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it modifies the Serf's state directly.
    #[tracing::instrument(level = "info", skip_all)]
    pub unsafe fn preserve_event_update_leftovers_with_pre_persist<F>(
        &mut self,
        pre_persist: F,
    ) -> Option<PmaCopyDetail>
    where
        F: FnOnce(&mut Self),
    {
        let event_num = self.event_num.load(Ordering::SeqCst);
        assert!(
            self.context.scry_stack.is_direct(),
            "scry_stack must be cleared before resetting the NockStack"
        );
        if std::env::var_os("NOCK_STACK_TIMING_DETAIL").is_some() {
            let total_words = self.context.stack.arena().words() as u64;
            let least_space = self.context.stack.least_space() as u64;
            let used_words = total_words.saturating_sub(least_space);
            let used_mib = (used_words as f64 * 8.0) / (1024.0 * 1024.0);
            info!(
                "stack-usage: used_words={} used_mib={:.3} least_space_words={} total_words={} event_num={}",
                used_words, used_mib, least_space, total_words, event_num
            );
        }
        if self.pma.is_some() {
            let trace_pma = std::env::var_os("NOCK_PMA_TRACE").is_some();
            let detail_enabled = std::env::var_os("NOCK_PMA_TIMING_DETAIL").is_some();
            let mut pma = self.pma.take().expect("checked is_some");
            let mut detail = if detail_enabled {
                Some(PmaCopyDetail::default())
            } else {
                None
            };
            let arvo_segment = self.copy_durable_state_to_pma(&mut pma, trace_pma, detail.as_mut());
            log_pma_preserve_component(event_num, "pma_arvo_copy", arvo_segment);
            #[cfg(feature = "pma-assert")]
            {
                // Enforce: durable PMA state must not reference the NockStack.
                self.assert_durable_state_in_pma(&pma);
            }

            pre_persist(self);

            let persist_start = Instant::now();
            self.persist_pma_state(&pma);
            log_preserve_component(event_num, "pma_persist_state", persist_start.elapsed());

            unsafe {
                self.copy_runtime_state_to_pma(&mut pma, trace_pma, detail.as_mut());
            }
            #[cfg(feature = "pma-assert")]
            {
                self.assert_runtime_state_in_pma(&pma);
            }

            let gc_start = Instant::now();
            let (pma_after_gc, did_pma_gc) = self.maybe_pma_gc(pma);
            log_preserve_component(event_num, "pma_gc", gc_start.elapsed());
            pma = pma_after_gc;
            if did_pma_gc {
                #[cfg(feature = "pma-assert")]
                {
                    self.assert_runtime_state_in_pma(&pma);
                }
            }
            self.flip_top_frame_and_trim_free_gap(event_num);
            self.pma = Some(pma);
            detail
        } else {
            pre_persist(self);
            self.preserve_persistent_state_in_stack(event_num);
            self.flip_top_frame_and_trim_free_gap(event_num);
            None
        }
    }

    /// Returns a mutable reference to the Nock stack.
    ///
    /// # Returns
    ///
    /// A mutable reference to the `NockStack`.
    pub fn stack(&mut self) -> &mut NockStack {
        &mut self.context.stack
    }

    /// Creates a poke swap noun.
    ///
    /// # Arguments
    ///
    /// * `eve` - The event number.
    /// * `mug` - The mug value.
    /// * `ovo` - The original noun.
    /// * `fec` - The effect noun.
    ///
    /// # Returns
    ///
    /// A noun representing the poke swap.
    pub fn poke_bail(&mut self, eve: u64, mug: u64, ovo: Noun, fec: Noun) -> Noun {
        T(
            self.stack(),
            &[D(tas!(b"poke")), D(tas!(b"swap")), D(eve), D(mug), ovo, fec],
        )
    }

    /// Creates a poke bail noun.
    ///
    /// # Arguments
    ///
    /// * `lud` - The lud noun.
    ///
    /// # Returns
    ///
    /// A noun representing the poke bail.
    pub fn poke_bail_noun(&mut self, lud: Noun) -> Noun {
        T(self.stack(), &[D(tas!(b"poke")), D(tas!(b"bail")), lud])
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use blake3::hash;
    use bytes::Bytes;
    use nockvm::jets::cold::Cold;
    use nockvm::jets::hot::HotEntry;
    use nockvm::jets::warm::Warm;
    use tempfile::TempDir;

    use super::*;

    const DUMB_KERNEL_JAM: &[u8] = include_bytes!(env!("DUMB_JAM_PATH"));

    fn dummy_serf() -> Serf {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let cold = Cold::new(&mut stack);
        let hot_state: [HotEntry; 0] = [];
        let context = create_context(stack, &hot_state, cold, None, vec![]);
        let cancel_token = context.cancel_token();
        Serf {
            ker_hash: Hash::from([0; 32]),
            arvo: D(0),
            context,
            pma: None,
            pma_meta_path: None,
            pma_gc_state: None,
            snapshot_creation_enabled: false,
            rotating_snapshot_interval_event_time: None,
            cumulative_event_processing_time_since_snapshot: Duration::ZERO,
            event_log: None,
            cancel_token,
            event_num: Arc::new(AtomicU64::new(0)),
            metrics: None,
        }
    }

    fn load_jam_bytes(jam: &str) -> Vec<u8> {
        let possible_paths = [
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("test-jams")
                .join(jam),
            Path::new("open/crates/nockapp/test-jams").join(jam),
        ];
        for path in &possible_paths {
            if let Ok(bytes) = fs::read(path) {
                return bytes;
            }
        }
        panic!("Failed to read {} file from any known path", jam)
    }

    async fn setup_kernel(jam: &str) -> Kernel<SaveableCheckpoint> {
        let jam_bytes = load_jam_bytes(jam);
        Kernel::load(&jam_bytes, None, vec![], TraceOpts::default(), None)
            .await
            .expect("Could not load kernel")
    }

    fn bench_serf(kernel_bytes: &[u8]) -> Serf {
        let stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        Serf::new(
            stack,
            None,
            None,
            false,
            None,
            false,
            None,
            None,
            None::<SaveableCheckpoint>,
            kernel_bytes,
            &[],
            vec![],
            TraceOpts::default(),
            NOCK_STACK_SIZE_TINY,
        )
        .expect("bench serf should initialize")
    }

    fn kernel_state_slab_from_serf(serf: &mut Serf) -> NounSlab {
        let space = serf.context.stack.noun_space();
        let kernel_state_noun = serf
            .arvo
            .in_space(&space)
            .slot(STATE_AXIS)
            .map(|handle| handle.noun())
            .expect("resolve kernel state noun");
        let mut kernel_state_slab = NounSlab::new();
        kernel_state_slab.copy_into(kernel_state_noun, &space);
        kernel_state_slab
    }

    #[derive(Debug)]
    struct ColdPersistenceBenchReport {
        first_boot_with_cold_ready: Duration,
        load_kernel_state_into_fresh_vm: Duration,
        cold_to_noun: Duration,
        noun_to_jam: Duration,
        save_jam_to_disk: Duration,
        cue_noun_from_disk: Duration,
        reinject_cued_cold_into_vm: Duration,
        jam_bytes: usize,
    }

    impl ColdPersistenceBenchReport {
        fn print(&self) {
            println!("cold_state_persistence_bench");
            println!("  kernel_asset=assets/dumb.jam");
            println!(
                "  first_boot_with_cold_ready_ms={:.3}",
                duration_ms(self.first_boot_with_cold_ready)
            );
            println!(
                "  load_kernel_state_into_fresh_vm_ms={:.3}",
                duration_ms(self.load_kernel_state_into_fresh_vm)
            );
            println!("  cold_to_noun_ms={:.3}", duration_ms(self.cold_to_noun));
            println!("  noun_to_jam_ms={:.3}", duration_ms(self.noun_to_jam));
            println!(
                "  save_jam_to_disk_ms={:.3}",
                duration_ms(self.save_jam_to_disk)
            );
            println!(
                "  cue_noun_from_disk_ms={:.3}",
                duration_ms(self.cue_noun_from_disk)
            );
            println!(
                "  reinject_cued_cold_into_vm_ms={:.3}",
                duration_ms(self.reinject_cued_cold_into_vm)
            );
            println!("  jam_bytes={}", self.jam_bytes);
        }
    }

    #[test]
    #[ignore = "benchmark harness"]
    #[cfg_attr(miri, ignore)]
    fn cold_state_noun_jam_persistence_bench() {
        let boot_start = Instant::now();
        let mut serf = bench_serf(DUMB_KERNEL_JAM);
        let first_boot_with_cold_ready = boot_start.elapsed();
        let kernel_state_slab = kernel_state_slab_from_serf(&mut serf);

        let mut load_serf = bench_serf(DUMB_KERNEL_JAM);
        let load_state_noun = kernel_state_slab.copy_to_stack(load_serf.stack());
        let load_kernel_state_into_fresh_vm_start = Instant::now();
        let _loaded_arvo = load_serf
            .load(load_state_noun)
            .expect("load saved kernel state into fresh vm");
        let load_kernel_state_into_fresh_vm = load_kernel_state_into_fresh_vm_start.elapsed();

        let cold_to_noun_start = Instant::now();
        let cold_noun = serf.context.cold.into_noun(serf.stack());
        let cold_to_noun = cold_to_noun_start.elapsed();

        let noun_to_jam_start = Instant::now();
        let cold_jam = {
            let space = serf.context.stack.noun_space();
            NockJammer::jam(cold_noun, &space)
        };
        let noun_to_jam = noun_to_jam_start.elapsed();

        let temp_dir = TempDir::new().expect("create temp dir");
        let cold_jam_path = temp_dir.path().join("cold-state.jam");

        let save_jam_to_disk_start = Instant::now();
        durability::write_atomic(&cold_jam_path, &cold_jam, "cold_state_bench_write")
            .expect("persist cold jam");
        let save_jam_to_disk = save_jam_to_disk_start.elapsed();

        let cue_noun_from_disk_start = Instant::now();
        let jam_from_disk = Bytes::from(fs::read(&cold_jam_path).expect("read cold jam"));
        let mut cued_cold_slab: NounSlab = NounSlab::new();
        let cued_cold_noun = cued_cold_slab
            .cue_into(jam_from_disk)
            .expect("cue cold noun");
        cued_cold_slab.set_root(cued_cold_noun);
        let cue_noun_from_disk = cue_noun_from_disk_start.elapsed();

        let mut inject_stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let inject_cold = Cold::new(&mut inject_stack);
        let mut inject_context =
            create_context(inject_stack, URBIT_HOT_STATE, inject_cold, None, vec![]);

        let reinject_cued_cold_into_vm_start = Instant::now();
        let cued_cold_in_vm = cued_cold_slab.copy_to_stack(&mut inject_context.stack);
        let space = inject_context.stack.noun_space();
        let cold_vecs = Cold::from_noun(&mut inject_context.stack, &cued_cold_in_vm, &space)
            .expect("decode cold noun");
        let rebuilt_cold = Cold::from_vecs(
            &mut inject_context.stack, cold_vecs.0, cold_vecs.1, cold_vecs.2,
        );
        inject_context.cold = rebuilt_cold;
        let hot = inject_context.hot;
        let test_jets = inject_context.test_jets;
        inject_context.warm = Warm::init(
            &mut inject_context.stack, &mut inject_context.cold, &hot, &test_jets,
        );
        let reinject_cued_cold_into_vm = reinject_cued_cold_into_vm_start.elapsed();

        let reinjected_cold_noun = inject_context.cold.into_noun(&mut inject_context.stack);
        let reinjected_cold_jam = {
            let space = inject_context.stack.noun_space();
            NockJammer::jam(reinjected_cold_noun, &space)
        };
        assert!(
            !reinjected_cold_jam.is_empty(),
            "re-injected cold state should remain serializable"
        );

        let report = ColdPersistenceBenchReport {
            first_boot_with_cold_ready,
            load_kernel_state_into_fresh_vm,
            cold_to_noun,
            noun_to_jam,
            save_jam_to_disk,
            cue_noun_from_disk,
            reinject_cued_cold_into_vm,
            jam_bytes: cold_jam.len(),
        };
        report.print();
    }

    #[tokio::test]
    async fn load_allows_checkpoint_kernel_hash_mismatch() {
        let jam_bytes = load_jam_bytes("test-ker.jam");
        let kernel = setup_kernel("test-ker.jam").await;
        let mut checkpoint = kernel
            .checkpoint()
            .await
            .expect("Could not checkpoint kernel");
        let mut hasher = Hasher::new();
        hasher.update(b"mismatched-checkpoint-kernel");
        checkpoint.ker_hash = hasher.finalize();

        Kernel::load(
            &jam_bytes,
            Some(checkpoint),
            vec![],
            TraceOpts::default(),
            None,
        )
        .await
        .expect("checkpoint load should tolerate kernel hash drift");
    }

    // Convert this to an integration test and feed it the kernel.jam from Choo in CI/CD
    // https://www.youtube.com/watch?v=4m1EFMoRFvY
    // #[test]
    // #[cfg_attr(miri, ignore)]
    // fn test_kernel_boot() {
    //     let _ = setup_kernel("dumb.jam");
    // }

    // To test your own kernel, place a `kernel.jam` file in the `assets` directory
    // and uncomment the following test:
    //
    // #[test]
    // fn test_custom_kernel() {
    //     let (kernel, _temp_dir) = setup_kernel("kernel.jam");
    //     // Add your custom assertions here to test the kernel's behavior
    // }

    fn checkpoint_with_state_too_large_for_stack(state_bytes_len: usize) -> SaveableCheckpoint {
        let mut state = NounSlab::new();
        let state_bytes = Bytes::from(vec![0x41; state_bytes_len]);
        let state_root = Atom::from_bytes(&mut state, &state_bytes).as_noun();
        state.set_root(state_root);

        let mut cold_stack = NockStack::new(1024, 0);
        let cold_noun = Cold::new(&mut cold_stack).into_noun(&mut cold_stack);
        let cold_space = cold_stack.noun_space();
        let mut cold = NounSlab::new();
        let cold_root = cold.copy_into(cold_noun, &cold_space);
        cold.set_root(cold_root);

        SaveableCheckpoint {
            ker_hash: hash(b"stack-oom-repro"),
            event_num: 1,
            state,
            cold,
        }
    }

    #[tokio::test]
    async fn serf_thread_init_stack_oom_is_not_collapsed_into_oneshot_error() {
        let stack_words = 9 * 1024;
        let checkpoint = checkpoint_with_state_too_large_for_stack((stack_words - 2) * 8);
        let result = SerfThread::<SaveableCheckpoint>::new(
            Vec::new(),
            Some(checkpoint),
            Vec::new(),
            stack_words,
            None,
            Vec::new(),
            TraceOpts::default(),
        )
        .await;

        let Err(err) = result else {
            panic!("expected serf initialization to fail from stack exhaustion");
        };
        let err = err.to_string();

        assert!(
            !err.contains("oneshot channel error"),
            "serf stack exhaustion must be reported directly, not as {err:?}"
        );
        assert!(
            err.contains("Out of memory") || err.contains("allocation"),
            "expected a stack allocation error, got {err:?}"
        );
        assert!(
            err.contains("--stack-size") && err.contains("checkpoint event_num="),
            "expected operator guidance and checkpoint context, got {err:?}"
        );
    }

    /// Create a structurally-valid PMA slab plus its `.meta` sidecar recording
    /// `ker_hash`/`event_num`, as if written by some prior kernel.
    fn write_valid_pma(path: &Path, ker_hash: Hash, event_num: u64) {
        let pma_reserved_words = {
            let pma = Pma::new(1 << 10, path.to_path_buf()).expect("create test PMA");
            let pma_reserved_words =
                u64::try_from(pma.reserved_words()).expect("test PMA reservation fits in u64");
            pma.sync_all().expect("sync test PMA");
            pma_reserved_words
        };
        PmaPersistMetadata::new(ker_hash, event_num, 0, pma_reserved_words)
            .save_to_path(&pma_meta_path(path))
            .expect("write test PMA metadata");
    }

    // Regression for the reported symptom: after a kernel upgrade the hash gate
    // made *both* slabs report `None`, so selection fell through to its
    // `Slab0` default and then reported slab 0's metadata as "missing or
    // invalid" even though the live state was in slab 1.
    #[test]
    fn select_active_pma_slab_is_kernel_hash_agnostic_and_picks_latest_event() {
        let dir = TempDir::new().expect("temp dir");
        let path_0 = dir.path().join("0.pma");
        let path_1 = dir.path().join("1.pma");
        // The two slabs were written by *different* kernels.
        write_valid_pma(&path_0, hash(b"kernel-a"), 1);
        write_valid_pma(&path_1, hash(b"kernel-b"), 5);
        let paths = PmaSlabPaths {
            path_0: path_0.clone(),
            path_1: path_1.clone(),
        };
        assert!(
            matches!(select_active_pma_slab(&paths), PmaSlab::Slab1),
            "slab selection must pick the most-recent slab regardless of kernel hash"
        );
    }

    // A kernel-hash mismatch must not invalidate the PMA; the state is reused
    // and migrated by the current kernel, mirroring checkpoint/snapshot restore.
    #[test]
    fn inspect_existing_pma_is_valid_on_kernel_hash_mismatch() {
        let dir = TempDir::new().expect("temp dir");
        let path_0 = dir.path().join("0.pma");
        let path_1 = dir.path().join("1.pma");
        write_valid_pma(&path_0, hash(b"old-kernel-bytes"), 7);
        // Current kernel bytes hash to something different from the stored meta.
        let status = inspect_existing_pma(path_0.as_path(), path_1.as_path(), b"new-kernel-bytes");
        match status {
            ExistingPmaStatus::Valid { event_num, .. } => {
                assert_eq!(
                    event_num, 7,
                    "must reuse PMA state at its recorded event_num"
                );
            }
            other => {
                panic!("kernel-hash mismatch must NOT invalidate the PMA, got {other:?}")
            }
        }
    }
}

pub trait SerfCheckpoint: Send {
    fn new(
        stack: &mut NockStack,
        ker_hash: Hash,
        event_num: u64,
        kernel_state: Noun,
        cold_state: Cold,
        metrics: &Option<Arc<NockAppMetrics>>,
    ) -> Self;

    fn load(self) -> SaveableCheckpoint;
}

impl SerfCheckpoint for SaveableCheckpoint {
    fn new(
        stack: &mut NockStack,
        ker_hash: Hash,
        event_num: u64,
        kernel_state: Noun,
        cold_state: Cold,
        metrics: &Option<Arc<NockAppMetrics>>,
    ) -> Self {
        let cold_noun_start = Instant::now();
        // Cold state has nouns in it which are *not* copied in into_noun
        // TODO: FIX THIS FOOTGUN
        let cold_stack_noun = cold_state.into_noun(stack);
        let space = stack.noun_space();
        let mut cold_slab: NounSlab = NounSlab::new();
        let cold_copy = cold_slab.copy_into(cold_stack_noun, &space);
        cold_slab.set_root(cold_copy);
        let cold_noun_elapsed = cold_noun_start.elapsed();

        let state_copy_start = Instant::now();
        let mut state_slab: NounSlab = NounSlab::new();
        let state_copy = state_slab.copy_into(kernel_state, &space);
        state_slab.set_root(state_copy);
        let state_copy_elapsed = state_copy_start.elapsed();

        if let Some(metrics) = metrics {
            metrics
                .serf_loop_noun_encode_cold_state
                .add_timing(&cold_noun_elapsed);
            metrics
                .serf_loop_copy_state_noun
                .add_timing(&state_copy_elapsed);
        }
        Self {
            ker_hash,
            event_num,
            state: state_slab,
            cold: cold_slab,
        }
    }

    fn load(self) -> SaveableCheckpoint {
        self
    }
}
