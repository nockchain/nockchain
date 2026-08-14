//! Isolated RTX 5090 dense Pearl ticket-search session.
//!
//! The peak path accepts precomputed noised matrices in row-major `A` and
//! transposed row-major `B` form. One ticket is one 16-by-16 output tile. The
//! session owns all device allocations until drop.

use std::ffi::{c_int, c_void};
use std::sync::{Arc, Mutex};

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::PreparedPearlPatternJob;
use ai_pow::tile_hash::hash_le_target;
use anyhow::{bail, Result};

#[cfg(feature = "node")]
use crate::canonical::PreparedCanonicalMoeTemplate;
use crate::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};

pub const PEAK_K: usize = 8192;
pub const PEAK_RANK: usize = 512;
pub const PEAK_TILE: usize = 16;
const NO_WINNER: u64 = u64::MAX;

#[repr(C)]
struct FfiSearchResult {
    winner_ordinal: u64,
    jackpot: [u8; 32],
    kernel_ms: f32,
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

#[derive(Default)]
struct PeakDispatch {
    template: Option<Arc<PreparedPearlPatternJob>>,
    session: Option<PeakCudaSession>,
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
