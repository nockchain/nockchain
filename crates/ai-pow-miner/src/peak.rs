//! Isolated RTX 5090 dense Pearl ticket-search session.
//!
//! The peak path accepts precomputed noised matrices in row-major `A` and
//! transposed row-major `B` form. One ticket is one 16-by-16 output tile. The
//! session owns all device allocations until drop.

use std::ffi::{c_int, c_void};
use std::sync::{Arc, Mutex};

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::{PearlWorkCommitments, PreparedPearlPatternJob};
use ai_pow::tile_hash::hash_le_target;
use anyhow::{bail, Result};
use rayon::prelude::*;

#[cfg(feature = "node")]
use crate::canonical::{PreparedCanonicalDenseSearch, PreparedCanonicalMoeTemplate};
#[cfg(feature = "node")]
use crate::search::PeakSearchOutcome;
use crate::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};
use crate::PEAK_PRODUCTION_PARAMS;

pub const PEAK_K: usize = 8192;
pub const PEAK_RANK: usize = 512;
pub const PEAK_TILE: usize = 16;
const NO_WINNER: u64 = u64::MAX;
const MAX_PEAK_GPU_DEVICES: usize = 8;

#[repr(C)]
struct FfiSearchResult {
    winner_ordinal: u64,
    jackpot: [u8; 32],
    kernel_ms: f32,
}
#[repr(C)]
struct FfiPrepareResult {
    kappa: [u8; 32],
    h_a: [u8; 32],
    h_b: [u8; 32],
    s_a: [u8; 32],
    s_b: [u8; 32],
    commitment_ms: f32,
    noise_ms: f32,
}
#[repr(C)]
struct FfiKernelInfo {
    sm_count: u32,
    threads_per_cta: u32,
    active_ctas_per_sm: u32,
    registers_per_thread: u32,
    static_shared_bytes: u64,
    dynamic_shared_bytes: u64,
}

unsafe extern "C" {
    fn ai_pow_cuda_peak_kernel_info(device_ordinal: u32, info_out: *mut FfiKernelInfo) -> c_int;
    fn ai_pow_cuda_peak_session_create(
        device_ordinal: u32,
        m: u32,
        n: u32,
        k: u32,
        rank: u32,
        tile: u32,
        a_prime: *const i8,
        b_prime: *const i8,
        pow_key: *const u8,
        session_out: *mut *mut c_void,
    ) -> c_int;
    fn ai_pow_cuda_peak_source_session_create(
        device_ordinal: u32,
        m: u32,
        n: u32,
        k: u32,
        rank: u32,
        tile: u32,
        a: *const i8,
        b: *const i8,
        session_out: *mut *mut c_void,
    ) -> c_int;
    fn ai_pow_cuda_peak_session_prepare(
        session: *mut c_void,
        sigma: *const u8,
        mu: *const u8,
        result_out: *mut FfiPrepareResult,
    ) -> c_int;
    fn ai_pow_cuda_peak_session_search(
        session: *mut c_void,
        ordinal_start: u64,
        ordinal_count: u64,
        target: *const u8,
        result_out: *mut FfiSearchResult,
    ) -> c_int;
    fn ai_pow_cuda_peak_session_debug(
        session: *mut c_void,
        ordinal: u64,
        state_out: *mut i32,
        jackpot_out: *mut u8,
    ) -> c_int;
    fn ai_pow_cuda_peak_session_destroy(session: *mut c_void) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakSearchResult {
    pub winner: Option<u64>,
    pub jackpot: [u8; 32],
    pub kernel_ms: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeakDebugResult {
    pub state: TileState,
    pub jackpot: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakPreparation {
    pub commitments: PearlWorkCommitments,
    pub commitment_ms: f32,
    pub noise_ms: f32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeakKernelInfo {
    pub sm_count: u32,
    pub threads_per_cta: u32,
    pub active_ctas_per_sm: u32,
    pub registers_per_thread: u32,
    pub static_shared_bytes: u64,
    pub dynamic_shared_bytes: u64,
}

pub struct PeakCudaSession {
    raw: *mut c_void,
    m: usize,
    n: usize,
    total_tickets: u64,
}

// A session is owned by one worker. The CUDA stream and allocations move with it.
unsafe impl Send for PeakCudaSession {}

impl PeakCudaSession {
    pub fn kernel_info(device_ordinal: usize) -> Result<PeakKernelInfo> {
        let device_ordinal = u32::try_from(device_ordinal)?;
        let mut info = FfiKernelInfo {
            sm_count: 0,
            threads_per_cta: 0,
            active_ctas_per_sm: 0,
            registers_per_thread: 0,
            static_shared_bytes: 0,
            dynamic_shared_bytes: 0,
        };
        // SAFETY: `info` remains valid for the synchronous query.
        let status = unsafe { ai_pow_cuda_peak_kernel_info(device_ordinal, &mut info) };
        check_cuda("query peak CUDA kernel", status)?;
        Ok(PeakKernelInfo {
            sm_count: info.sm_count,
            threads_per_cta: info.threads_per_cta,
            active_ctas_per_sm: info.active_ctas_per_sm,
            registers_per_thread: info.registers_per_thread,
            static_shared_bytes: info.static_shared_bytes,
            dynamic_shared_bytes: info.dynamic_shared_bytes,
        })
    }

    pub fn new(
        device_ordinal: usize,
        m: usize,
        n: usize,
        a_prime: &[i8],
        b_prime: &[i8],
        pow_key: &[u8; 32],
    ) -> Result<Self> {
        if m == 0 || n == 0 || m % 256 != 0 || n % 128 != 0 {
            bail!("peak shape requires nonzero m%256==0 and n%128==0");
        }
        let a_len = m
            .checked_mul(PEAK_K)
            .ok_or_else(|| anyhow::anyhow!("peak A length overflow"))?;
        let b_len = n
            .checked_mul(PEAK_K)
            .ok_or_else(|| anyhow::anyhow!("peak B length overflow"))?;
        if a_prime.len() != a_len || b_prime.len() != b_len {
            bail!(
                "peak matrix lengths must be m*k={} and n*k={}, got {} and {}",
                a_len,
                b_len,
                a_prime.len(),
                b_prime.len()
            );
        }
        let total_tickets = u64::try_from(m / PEAK_TILE)?
            .checked_mul(u64::try_from(n / PEAK_TILE)?)
            .ok_or_else(|| anyhow::anyhow!("peak ticket count overflow"))?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: all slices have the validated fixed lengths. CUDA copies them
        // before this synchronous call returns and initializes `raw` on success.
        let status = unsafe {
            ai_pow_cuda_peak_session_create(
                u32::try_from(device_ordinal)?,
                u32::try_from(m)?,
                u32::try_from(n)?,
                PEAK_K as u32,
                PEAK_RANK as u32,
                PEAK_TILE as u32,
                a_prime.as_ptr(),
                b_prime.as_ptr(),
                pow_key.as_ptr(),
                &mut raw,
            )
        };
        check_cuda("peak session creation", status)?;
        if raw.is_null() {
            bail!("peak session creation returned a null session");
        }
        Ok(Self {
            raw,
            m,
            n,
            total_tickets,
        })
    }

    pub fn new_source(
        device_ordinal: usize,
        m: usize,
        n: usize,
        a: &[i8],
        b: &[i8],
    ) -> Result<Self> {
        if m == 0 || n == 0 || m % 256 != 0 || n % 128 != 0 {
            bail!("peak shape requires nonzero m%256==0 and n%128==0");
        }
        let a_len = m
            .checked_mul(PEAK_K)
            .ok_or_else(|| anyhow::anyhow!("peak source A length overflow"))?;
        let b_len = n
            .checked_mul(PEAK_K)
            .ok_or_else(|| anyhow::anyhow!("peak source B length overflow"))?;
        if a.len() != a_len || b.len() != b_len {
            bail!(
                "peak source matrix lengths must be m*k={} and n*k={}, got {} and {}",
                a_len,
                b_len,
                a.len(),
                b.len()
            );
        }
        let total_tickets = u64::try_from(m / PEAK_TILE)?
            .checked_mul(u64::try_from(n / PEAK_TILE)?)
            .ok_or_else(|| anyhow::anyhow!("peak ticket count overflow"))?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: the source slices have the validated fixed lengths. CUDA
        // copies them before this synchronous call returns.
        let status = unsafe {
            ai_pow_cuda_peak_source_session_create(
                u32::try_from(device_ordinal)?,
                u32::try_from(m)?,
                u32::try_from(n)?,
                PEAK_K as u32,
                PEAK_RANK as u32,
                PEAK_TILE as u32,
                a.as_ptr(),
                b.as_ptr(),
                &mut raw,
            )
        };
        check_cuda("peak source session creation", status)?;
        if raw.is_null() {
            bail!("peak source session creation returned a null session");
        }
        Ok(Self {
            raw,
            m,
            n,
            total_tickets,
        })
    }

    pub fn prepare(&mut self, sigma: &[u8], mu: &[u8]) -> Result<PeakPreparation> {
        if sigma.len() != 76 || mu.len() != 52 {
            bail!(
                "peak transcript lengths must be sigma=76 and mu=52, got {} and {}",
                sigma.len(),
                mu.len()
            );
        }
        let mut result = FfiPrepareResult {
            kappa: [0; 32],
            h_a: [0; 32],
            h_b: [0; 32],
            s_a: [0; 32],
            s_b: [0; 32],
            commitment_ms: 0.0,
            noise_ms: 0.0,
        };
        // SAFETY: both transcript slices have their ABI-required fixed lengths.
        // The session is exclusively borrowed for the synchronous preparation.
        let status = unsafe {
            ai_pow_cuda_peak_session_prepare(self.raw, sigma.as_ptr(), mu.as_ptr(), &mut result)
        };
        check_cuda("peak transcript preparation", status)?;
        Ok(PeakPreparation {
            commitments: PearlWorkCommitments {
                kappa: result.kappa,
                h_a: result.h_a,
                h_b: result.h_b,
                s_a: result.s_a,
                s_b: result.s_b,
            },
            commitment_ms: result.commitment_ms,
            noise_ms: result.noise_ms,
        })
    }

    pub const fn m(&self) -> usize {
        self.m
    }

    pub const fn n(&self) -> usize {
        self.n
    }

    pub const fn total_tickets(&self) -> u64 {
        self.total_tickets
    }

    pub fn search(
        &mut self,
        ordinal_start: u64,
        ordinal_count: u64,
        target: &[u8; 32],
    ) -> Result<PeakSearchResult> {
        if ordinal_count == 0
            || ordinal_start >= self.total_tickets
            || ordinal_count > self.total_tickets - ordinal_start
        {
            bail!("peak search range is outside the prepared ticket space");
        }
        let mut raw_result = FfiSearchResult {
            winner_ordinal: NO_WINNER,
            jackpot: [0; 32],
            kernel_ms: 0.0,
        };
        // SAFETY: the session is exclusively borrowed. The fixed buffers remain
        // valid for the synchronous ABI call.
        let status = unsafe {
            ai_pow_cuda_peak_session_search(
                self.raw,
                ordinal_start,
                ordinal_count,
                target.as_ptr(),
                &mut raw_result,
            )
        };
        check_cuda("peak search", status)?;
        Ok(PeakSearchResult {
            winner: (raw_result.winner_ordinal != NO_WINNER).then_some(raw_result.winner_ordinal),
            jackpot: raw_result.jackpot,
            kernel_ms: raw_result.kernel_ms,
        })
    }

    pub fn debug_ticket(&mut self, ordinal: u64) -> Result<PeakDebugResult> {
        if ordinal >= self.total_tickets {
            bail!("peak debug ordinal is outside the prepared ticket space");
        }
        let mut state = [0i32; 16];
        let mut jackpot = [0u8; 32];
        // SAFETY: the session is exclusively borrowed and both output buffers
        // have their ABI-required fixed lengths.
        let status = unsafe {
            ai_pow_cuda_peak_session_debug(
                self.raw,
                ordinal,
                state.as_mut_ptr(),
                jackpot.as_mut_ptr(),
            )
        };
        check_cuda("peak ticket debug", status)?;
        Ok(PeakDebugResult {
            state: TileState(state),
            jackpot,
        })
    }
}

/// Opt-in dense search backend for the fixed peak geometry.
pub struct PeakSearchBackend {
    device_ordinal: usize,
    dispatch: Mutex<PeakDispatch>,
}

/// One ordered dense peak search distributed across independent devices.
pub struct MultiGpuPeakSearchBackend {
    backends: Vec<PeakSearchBackend>,
}

#[derive(Default)]
struct PeakDispatch {
    template: Option<Arc<PreparedPearlPatternJob>>,
    session: Option<PeakCudaSession>,
    peak_template: Option<Arc<PreparedCanonicalDenseSearch>>,
    peak_session: Option<PeakCudaSession>,
    peak_preparation: Option<PeakPreparation>,
}

impl PeakSearchBackend {
    pub fn new(device_ordinal: usize) -> Self {
        Self {
            device_ordinal,
            dispatch: Mutex::new(PeakDispatch::default()),
        }
    }

    pub const fn device_ordinal(&self) -> usize {
        self.device_ordinal
    }

    fn validate_template(template: &PreparedPearlPatternJob) -> Result<(), SearchBackendError> {
        let params = template.params();
        if params.k as usize != PEAK_K
            || params.noise_rank as usize != PEAK_RANK
            || params.tile as usize != PEAK_TILE
            || params.m == 0
            || params.n == 0
            || params.m % 256 != 0
            || params.n % 128 != 0
        {
            return Err(unavailable(format!(
                "peak backend requires k={PEAK_K}, rank={PEAK_RANK}, tile={PEAK_TILE}, m%256=0, and n%128=0"
            )));
        }
        let rows = template.config().rows_pattern.to_list_bounded(PEAK_TILE)?;
        let cols = template.config().cols_pattern.to_list_bounded(PEAK_TILE)?;
        if rows.len() != PEAK_TILE
            || cols.len() != PEAK_TILE
            || !rows.iter().copied().eq(0..PEAK_TILE as u32)
            || !cols.iter().copied().eq(0..PEAK_TILE as u32)
        {
            return Err(unavailable(
                "peak backend requires contiguous 16-element row and column patterns",
            ));
        }
        if !template
            .row_offsets()
            .iter()
            .copied()
            .eq((0..params.m).step_by(PEAK_TILE))
            || !template
                .col_offsets()
                .iter()
                .copied()
                .eq((0..params.n).step_by(PEAK_TILE))
        {
            return Err(unavailable(
                "peak backend requires complete non-overlapping tile offsets",
            ));
        }
        Ok(())
    }

    #[cfg(feature = "node")]
    fn validate_peak_template(
        template: &PreparedCanonicalDenseSearch,
    ) -> Result<(), SearchBackendError> {
        let params = template.params();
        if params.k as usize != PEAK_K
            || params.noise_rank as usize != PEAK_RANK
            || params.tile as usize != PEAK_TILE
            || params.m == 0
            || params.n == 0
            || params.m % 256 != 0
            || params.n % 128 != 0
        {
            return Err(unavailable(format!(
                "peak backend requires k={PEAK_K}, rank={PEAK_RANK}, tile={PEAK_TILE}, m%256=0, and n%128=0"
            )));
        }
        let rows = template.config().rows_pattern.to_list_bounded(PEAK_TILE)?;
        let cols = template.config().cols_pattern.to_list_bounded(PEAK_TILE)?;
        if rows.len() != PEAK_TILE
            || cols.len() != PEAK_TILE
            || !rows.iter().copied().eq(0..PEAK_TILE as u32)
            || !cols.iter().copied().eq(0..PEAK_TILE as u32)
        {
            return Err(unavailable(
                "peak backend requires contiguous 16-element row and column patterns",
            ));
        }
        let expected = u64::from(params.m / params.tile)
            .checked_mul(u64::from(params.n / params.tile))
            .ok_or_else(|| unavailable("peak ticket count overflow"))?;
        if template.total_tickets() != expected {
            return Err(unavailable(
                "peak backend requires complete non-overlapping tile offsets",
            ));
        }
        Ok(())
    }
}

impl MultiGpuPeakSearchBackend {
    pub fn all_visible() -> Result<Self> {
        let count =
            crate::gpu::GpuSearchBackend::available_device_count()?.min(MAX_PEAK_GPU_DEVICES);
        Self::new((0..count).collect())
    }

    pub fn new(device_ordinals: Vec<usize>) -> Result<Self> {
        if device_ordinals.is_empty() {
            bail!("--cuda-devices must select at least one CUDA device");
        }
        if device_ordinals.len() > MAX_PEAK_GPU_DEVICES {
            bail!("--cuda-devices supports at most {MAX_PEAK_GPU_DEVICES} devices");
        }
        let visible = crate::gpu::GpuSearchBackend::available_device_count()?;
        for (index, &ordinal) in device_ordinals.iter().enumerate() {
            if ordinal >= visible {
                bail!("CUDA device {ordinal} is not visible; visible device count is {visible}");
            }
            if device_ordinals[..index].contains(&ordinal) {
                bail!("--cuda-devices contains duplicate device {ordinal}");
            }
        }
        Ok(Self {
            backends: device_ordinals
                .into_iter()
                .map(PeakSearchBackend::new)
                .collect(),
        })
    }

    pub fn device_ordinals(&self) -> Vec<usize> {
        self.backends
            .iter()
            .map(PeakSearchBackend::device_ordinal)
            .collect()
    }

    pub fn preflight(&self, a: &[i8], b: &[i8]) -> Result<()> {
        self.backends.par_iter().try_for_each(|backend| {
            let session = PeakCudaSession::new_source(
                backend.device_ordinal, PEAK_PRODUCTION_PARAMS.m as usize,
                PEAK_PRODUCTION_PARAMS.n as usize, a, b,
            )?;
            let mut dispatch = backend
                .dispatch
                .lock()
                .map_err(|_| anyhow::anyhow!("peak CUDA dispatch lock was poisoned"))?;
            dispatch.peak_template = None;
            dispatch.peak_preparation = None;
            dispatch.peak_session = Some(session);
            Ok(())
        })
    }
}

impl SearchBackend for PeakSearchBackend {
    fn search_dense(
        &self,
        template: Arc<PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let mut dispatch = self
            .dispatch
            .lock()
            .map_err(|_| unavailable("peak CUDA dispatch lock was poisoned"))?;
        let replace = dispatch
            .template
            .as_ref()
            .is_none_or(|current| !Arc::ptr_eq(current, &template));
        if replace {
            dispatch.session = None;
            dispatch.template = None;
            Self::validate_template(&template)?;
            let params = template.params();
            let (a_prime, b_prime) = template.prepared_matrices();
            let session = PeakCudaSession::new(
                self.device_ordinal,
                params.m as usize,
                params.n as usize,
                a_prime,
                b_prime,
                &template.commitments().s_a,
            )
            .map_err(unavailable)?;
            dispatch.template = Some(Arc::clone(&template));
            dispatch.session = Some(session);
        }
        let result = dispatch
            .session
            .as_mut()
            .ok_or_else(|| unavailable("peak CUDA session is unavailable"))?
            .search(batch.start, batch.len, &batch.threshold)
            .map_err(unavailable)?;
        let Some(ordinal) = result.winner else {
            return Ok(None);
        };
        if ordinal < batch.start || ordinal >= batch.end_exclusive() {
            return Err(SearchBackendError::WinnerOutsideBatch {
                winner: ordinal,
                batch_start: batch.start,
                batch_end_exclusive: batch.end_exclusive(),
            });
        }
        let (t_rows, t_cols) = template
            .offsets_at_ordinal(ordinal)
            .ok_or(SearchBackendError::DenseOrdinalOutOfRange(ordinal))?;
        let scalar = template.evaluate(t_rows, t_cols, &mut template.scratch())?;
        if scalar.jackpot_hash != result.jackpot
            || !hash_le_target(&scalar.jackpot_hash, &batch.threshold)
        {
            return Err(unavailable(format!(
                "peak CUDA winner {ordinal} failed scalar validation"
            )));
        }
        Ok(Some(SearchWinner {
            ordinal,
            jackpot_hash: scalar.jackpot_hash,
        }))
    }

    #[cfg(feature = "node")]
    fn search_peak(
        &self,
        template: Arc<PreparedCanonicalDenseSearch>,
        batch: SearchBatch,
    ) -> Result<PeakSearchOutcome, SearchBackendError> {
        let mut dispatch = self
            .dispatch
            .lock()
            .map_err(|_| unavailable("peak CUDA dispatch lock was poisoned"))?;
        let replace = dispatch
            .peak_template
            .as_ref()
            .is_none_or(|current| !Arc::ptr_eq(current, &template));
        if replace {
            Self::validate_peak_template(&template)?;
            let preparation = dispatch
                .peak_session
                .as_mut()
                .ok_or_else(|| unavailable("peak source session is unavailable"))?
                .prepare(template.sigma(), template.mu())
                .map_err(unavailable)?;
            dispatch.peak_template = Some(Arc::clone(&template));
            dispatch.peak_preparation = Some(preparation);
        }
        let preparation = dispatch
            .peak_preparation
            .ok_or_else(|| unavailable("peak transcript preparation is unavailable"))?;
        let result = dispatch
            .peak_session
            .as_mut()
            .ok_or_else(|| unavailable("peak source session is unavailable"))?
            .search(batch.start, batch.len, &batch.threshold)
            .map_err(unavailable)?;
        let winner = match result.winner {
            Some(ordinal) => {
                if ordinal < batch.start || ordinal >= batch.end_exclusive() {
                    return Err(SearchBackendError::WinnerOutsideBatch {
                        winner: ordinal,
                        batch_start: batch.start,
                        batch_end_exclusive: batch.end_exclusive(),
                    });
                }
                if !hash_le_target(&result.jackpot, &batch.threshold) {
                    return Err(unavailable(format!(
                        "peak CUDA winner {ordinal} is above the dispatched threshold"
                    )));
                }
                Some(SearchWinner {
                    ordinal,
                    jackpot_hash: result.jackpot,
                })
            }
            None => None,
        };
        Ok(PeakSearchOutcome {
            winner,
            commitments: preparation.commitments,
            commitment_ms: preparation.commitment_ms,
            noise_ms: preparation.noise_ms,
        })
    }

    #[cfg(feature = "node")]
    fn search_canonical(
        &self,
        _: Arc<PreparedCanonicalMoeTemplate>,
        _: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        Err(unavailable(
            "peak dense backend does not accept canonical MoE jobs",
        ))
    }

    fn batch_attempts(&self) -> u64 {
        u64::MAX
    }
}

impl SearchBackend for MultiGpuPeakSearchBackend {
    fn search_dense(
        &self,
        template: Arc<PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let active = self
            .backends
            .len()
            .min(usize::try_from(batch.len).unwrap_or(usize::MAX));
        self.backends[..active]
            .par_iter()
            .enumerate()
            .map(|(index, backend)| {
                backend.search_dense(
                    Arc::clone(&template),
                    peak_partition_for_device(batch, active, index),
                )
            })
            .try_reduce(|| None, |left, right| Ok(lower_peak_winner(left, right)))
    }

    #[cfg(feature = "node")]
    fn search_peak(
        &self,
        template: Arc<PreparedCanonicalDenseSearch>,
        batch: SearchBatch,
    ) -> Result<PeakSearchOutcome, SearchBackendError> {
        let active = self
            .backends
            .len()
            .min(usize::try_from(batch.len).unwrap_or(usize::MAX));
        let outcomes = self.backends[..active]
            .par_iter()
            .enumerate()
            .map(|(index, backend)| {
                backend.search_peak(
                    Arc::clone(&template),
                    peak_partition_for_device(batch, active, index),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut outcomes = outcomes.into_iter();
        let mut merged = outcomes
            .next()
            .ok_or_else(|| unavailable("peak multi-GPU search has no active device"))?;
        for outcome in outcomes {
            if outcome.commitments != merged.commitments {
                return Err(unavailable(
                    "peak devices derived different attempt transcripts",
                ));
            }
            merged.winner = lower_peak_winner(merged.winner, outcome.winner);
            merged.commitment_ms = merged.commitment_ms.max(outcome.commitment_ms);
            merged.noise_ms = merged.noise_ms.max(outcome.noise_ms);
        }
        Ok(merged)
    }

    #[cfg(feature = "node")]
    fn search_canonical(
        &self,
        _: Arc<PreparedCanonicalMoeTemplate>,
        _: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        Err(unavailable(
            "peak dense backend does not accept canonical MoE jobs",
        ))
    }

    fn batch_attempts(&self) -> u64 {
        u64::MAX
    }
}

fn peak_partition_for_device(batch: SearchBatch, active: usize, index: usize) -> SearchBatch {
    let active = active as u64;
    let index = index as u64;
    let base = batch.len / active;
    let remainder = batch.len % active;
    SearchBatch {
        start: batch.start + base * index + index.min(remainder),
        len: base + u64::from(index < remainder),
        threshold: batch.threshold,
    }
}

fn lower_peak_winner(
    left: Option<SearchWinner>,
    right: Option<SearchWinner>,
) -> Option<SearchWinner> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left.ordinal <= right.ordinal {
            left
        } else {
            right
        }),
        (Some(winner), None) | (None, Some(winner)) => Some(winner),
        (None, None) => None,
    }
}

fn unavailable(error: impl std::fmt::Display) -> SearchBackendError {
    SearchBackendError::BackendUnavailable(error.to_string())
}

impl Drop for PeakCudaSession {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: this object uniquely owns the opaque session.
            let _ = unsafe { ai_pow_cuda_peak_session_destroy(self.raw) };
            self.raw = std::ptr::null_mut();
        }
    }
}

fn check_cuda(operation: &str, status: c_int) -> Result<()> {
    if status != 0 {
        bail!("{operation} failed with CUDA status {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ai_pow::pearl_compat::pearl_jackpot_hash;

    use super::*;

    #[test]
    fn multi_gpu_partitions_are_adjacent_and_cover_the_batch() {
        let batch = SearchBatch::new(100, 10, [0; 32]).expect("batch");
        let partitions: Vec<_> = (0..3)
            .map(|index| peak_partition_for_device(batch, 3, index))
            .collect();
        assert_eq!(
            partitions
                .iter()
                .map(|part| (part.start, part.len))
                .collect::<Vec<_>>(),
            vec![(100, 4), (104, 3), (107, 3)]
        );
        assert_eq!(partitions[0].start, batch.start);
        assert_eq!(
            partitions.last().expect("last").end_exclusive(),
            batch.end_exclusive()
        );
    }

    #[test]
    fn multi_gpu_winner_reduction_uses_the_global_lowest_ordinal() {
        let higher = SearchWinner {
            ordinal: 29,
            jackpot_hash: [0x29; 32],
        };
        let lower = SearchWinner {
            ordinal: 17,
            jackpot_hash: [0x17; 32],
        };
        assert_eq!(lower_peak_winner(Some(higher), Some(lower)), Some(lower));
        assert_eq!(lower_peak_winner(None, Some(lower)), Some(lower));
        assert_eq!(lower_peak_winner(None, None), None);
    }

    #[cfg(feature = "node")]
    #[test]
    #[ignore = "requires two compatible CUDA devices"]
    fn multi_gpu_search_returns_global_lowest_winner() {
        let params = ai_pow::params::MatmulParams {
            m: 256,
            k: PEAK_K as u32,
            n: 128,
            noise_rank: PEAK_RANK as u32,
            tile: PEAK_TILE as u32,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let (a, b) = ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);
        let template = crate::canonical::PreparedCanonicalDenseTemplate::new(
            &params,
            [0x5a; 32],
            Arc::new(a),
            Arc::new(b),
        )
        .expect("dense template");
        let prepared = Arc::new(template.prepare(0).expect("prepared template"));
        let backend = MultiGpuPeakSearchBackend::new(vec![0, 1]).expect("two CUDA devices");

        for start in [0, 17, 64] {
            let winner = backend
                .search_dense(
                    Arc::clone(&prepared),
                    SearchBatch::new(start, 128 - start, [0xff; 32]).expect("maximum-target batch"),
                )
                .expect("multi-GPU search")
                .expect("maximum target always wins");
            assert_eq!(winner.ordinal, start);
        }
        assert!(backend
            .search_dense(
                prepared,
                SearchBatch::new(0, 128, [0; 32]).expect("zero-target batch"),
            )
            .expect("zero-target search")
            .is_none());
    }

    fn fixture(m: usize, n: usize) -> (Vec<i8>, Vec<i8>, [u8; 32]) {
        let mut state = 0x0123_4567_89ab_cdefu64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 32) as u8 & 0x7f) as i8 - 64
        };
        let a = (0..m * PEAK_K).map(|_| next()).collect();
        let b = (0..n * PEAK_K).map(|_| next()).collect();
        let key = std::array::from_fn(|index| (index as u8).wrapping_mul(17).wrapping_add(3));
        (a, b, key)
    }

    fn scalar_ticket(a: &[i8], b: &[i8], n: usize, ordinal: u64) -> TileState {
        let col_tiles = n / PEAK_TILE;
        let row_tile = ordinal as usize / col_tiles;
        let col_tile = ordinal as usize % col_tiles;
        let mut cells = [0i32; PEAK_TILE * PEAK_TILE];
        let mut state = [0i32; 16];
        for step in 0..PEAK_K / PEAK_RANK {
            for row in 0..PEAK_TILE {
                let a_base = (row_tile * PEAK_TILE + row) * PEAK_K + step * PEAK_RANK;
                for col in 0..PEAK_TILE {
                    let b_base = (col_tile * PEAK_TILE + col) * PEAK_K + step * PEAK_RANK;
                    let mut delta = 0i32;
                    for index in 0..PEAK_RANK {
                        delta += i32::from(a[a_base + index]) * i32::from(b[b_base + index]);
                    }
                    let cell = row * PEAK_TILE + col;
                    cells[cell] = cells[cell].saturating_add(delta);
                }
            }
            state[step] = cells
                .iter()
                .fold(0u32, |value, cell| value ^ (*cell as u32)) as i32;
        }
        TileState(state)
    }

    fn little_endian_predecessor(mut value: [u8; 32]) -> [u8; 32] {
        for byte in &mut value {
            if *byte != 0 {
                *byte -= 1;
                return value;
            }
            *byte = 0xff;
        }
        panic!("zero has no unsigned predecessor");
    }

    #[test]
    fn peak_device_transcript_matches_scalar() {
        let (a, b, key) = fixture(256, 128);
        let mut session =
            PeakCudaSession::new(0, 256, 128, &a, &b, &key).expect("peak CUDA session");
        for ordinal in [0, 1, 63, 127] {
            let device = session.debug_ticket(ordinal).expect("device ticket");
            let scalar = scalar_ticket(&a, &b, 128, ordinal);
            assert_eq!(device.state, scalar, "ordinal {ordinal}");
            assert_eq!(device.jackpot, pearl_jackpot_hash(&scalar, &key));
        }
    }

    #[cfg(feature = "node")]
    #[test]
    #[ignore = "requires a compatible CUDA device"]
    fn peak_source_session_matches_complete_scalar_transcript() {
        let params = ai_pow::params::MatmulParams {
            m: 256,
            k: PEAK_K as u32,
            n: 128,
            noise_rank: PEAK_RANK as u32,
            tile: PEAK_TILE as u32,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let (a, b) = ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);
        let a = Arc::new(a);
        let b = Arc::new(b);
        let template = crate::canonical::PreparedCanonicalDenseTemplate::new(
            &params,
            [0x5a; 32],
            Arc::clone(&a),
            Arc::clone(&b),
        )
        .expect("dense template");
        let mut session =
            PeakCudaSession::new_source(0, params.m as usize, params.n as usize, &a, &b)
                .expect("peak source session");
        for extranonce in [0, 1, u32::MAX - 1, u32::MAX] {
            let scalar = template.prepare(extranonce).expect("scalar preparation");
            let device = session
                .prepare(scalar.sigma(), scalar.mu())
                .expect("device preparation");
            assert_eq!(
                device.commitments,
                *scalar.commitments(),
                "extranonce {extranonce}"
            );
            for ordinal in [0, scalar.row_offsets().len() as u64 - 1, 127] {
                let (t_rows, t_cols) = scalar
                    .offsets_at_ordinal(ordinal)
                    .expect("scalar ticket offsets");
                let expected = scalar
                    .evaluate(t_rows, t_cols, &mut scalar.scratch())
                    .expect("scalar ticket");
                let actual = session.debug_ticket(ordinal).expect("device ticket");
                assert_eq!(
                    actual.state, expected.tile_state,
                    "extranonce {extranonce}, ordinal {ordinal}"
                );
                assert_eq!(
                    actual.jackpot, expected.jackpot_hash,
                    "extranonce {extranonce}, ordinal {ordinal}"
                );
            }
        }
    }

    #[test]
    fn peak_device_matches_one_thousand_deterministic_tickets() {
        const TICKET_COUNT: usize = 1_000;
        let (a, b, key) = fixture(2_048, 256);
        let mut session =
            PeakCudaSession::new(0, 2_048, 256, &a, &b, &key).expect("peak CUDA session");
        let total_tickets = session.total_tickets();
        let mut ordinals = Vec::with_capacity(TICKET_COUNT);
        ordinals.extend([0, 1, 127, 128, 129, 1_023, 1_024, total_tickets - 1]);
        let mut state = 0xd1b5_4a32_d192_ed03u64;
        while ordinals.len() < TICKET_COUNT {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ordinals.push(state % total_tickets);
        }

        for ordinal in ordinals {
            let scalar = scalar_ticket(&a, &b, 256, ordinal);
            let jackpot = pearl_jackpot_hash(&scalar, &key);
            let device = session.debug_ticket(ordinal).expect("device ticket");
            assert_eq!(device.state, scalar, "ordinal {ordinal}");
            assert_eq!(device.jackpot, jackpot, "ordinal {ordinal}");
            let lower_target = little_endian_predecessor(jackpot);
            for repetition in 0..3 {
                let hit = session
                    .search(ordinal, 1, &jackpot)
                    .expect("exact-target search");
                assert_eq!(
                    hit.winner,
                    Some(ordinal),
                    "ordinal {ordinal}, repetition {repetition}"
                );
                assert_eq!(
                    hit.jackpot, jackpot,
                    "ordinal {ordinal}, repetition {repetition}"
                );
                let miss = session
                    .search(ordinal, 1, &lower_target)
                    .expect("predecessor-target search");
                assert_eq!(
                    miss.winner, None,
                    "ordinal {ordinal}, repetition {repetition}"
                );
            }
        }
    }

    #[test]
    fn peak_search_returns_lowest_winner_and_no_hit() {
        let (a, b, key) = fixture(256, 128);
        let mut session =
            PeakCudaSession::new(0, 256, 128, &a, &b, &key).expect("peak CUDA session");
        let maximum = session
            .search(0, session.total_tickets(), &[0xff; 32])
            .expect("maximum-target search");
        assert_eq!(maximum.winner, Some(0));
        assert_eq!(
            maximum.jackpot,
            session.debug_ticket(0).expect("winner debug").jackpot
        );
        let zero = session
            .search(0, session.total_tickets(), &[0; 32])
            .expect("zero-target search");
        assert_eq!(zero.winner, None);
    }

    #[test]
    fn peak_search_honors_adjacent_ranges_across_persistent_tiles() {
        let (a, b, key) = fixture(4_096, 4_096);
        let mut session =
            PeakCudaSession::new(0, 4_096, 4_096, &a, &b, &key).expect("peak CUDA session");
        let midpoint = session.total_tickets() / 2;
        for (start, len) in [(0, midpoint), (midpoint, session.total_tickets() - midpoint)] {
            let result = session
                .search(start, len, &[0xff; 32])
                .expect("maximum-target range search");
            assert_eq!(result.winner, Some(start));
        }
    }
}
