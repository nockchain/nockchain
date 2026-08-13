//! CUDA Pearl V3 search backend.
//!
//! The canonical path derives commitments, noised opened strips, rolling tile
//! state, jackpot hashes, target comparisons, and the lowest winner on the GPU.
//! Rust validates every reported winner before proof construction.

use std::ffi::{c_int, c_void};
use std::sync::{Arc, Mutex};

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::pearl_jackpot_hash;
use ai_pow::tile_hash::hash_le_target;
use anyhow::{bail, Result};
use rayon::prelude::*;

use crate::canonical::PreparedCanonicalMoeTemplate;
use crate::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};

const NO_WINNER: u32 = u32::MAX;

unsafe extern "C" {
    fn ai_pow_cuda_session_create(
        max_attempts: u32,
        h: u32,
        w: u32,
        k: u32,
        rank: u32,
        dot_product_len: u32,
        session_out: *mut *mut c_void,
    ) -> c_int;
    fn ai_pow_cuda_session_run(
        session: *mut c_void,
        a_rows: *const i8,
        b_cols: *const i8,
        attempts: u32,
        states_out: *mut i32,
    ) -> c_int;
    fn ai_pow_cuda_session_destroy(session: *mut c_void) -> c_int;
    fn ai_pow_cuda_v3_session_create(
        max_attempts: u32,
        a_matrix: *const i8,
        b_matrix: *const i8,
        sigma: *const u8,
        mu: *const u8,
        routing_data: *const u8,
        routing_data_len: u32,
        routing_offsets: *const u8,
        routing_offsets_len: u32,
        row_indices: *const u32,
        col_indices: *const u32,
        session_out: *mut *mut c_void,
    ) -> c_int;
    fn ai_pow_cuda_v3_session_search(
        session: *mut c_void,
        extranonce_start: u32,
        attempts: u32,
        target: *const u8,
        winner_local: *mut u32,
        jackpot_out: *mut u8,
    ) -> c_int;
    fn ai_pow_cuda_v3_session_destroy(session: *mut c_void) -> c_int;
    #[cfg(test)]
    fn ai_pow_cuda_v3_session_debug(
        session: *mut c_void,
        extranonce: u32,
        kappa: *mut u8,
        h_a: *mut u8,
        h_b: *mut u8,
        s_a: *mut u8,
        s_b: *mut u8,
        a_rows: *mut i8,
        b_cols: *mut i8,
        state: *mut i32,
        jackpot: *mut u8,
    ) -> c_int;
}

#[derive(Debug)]
pub struct GpuSearchBackend {
    device_ordinal: usize,
    batch_attempts: u64,
    dispatch: Mutex<CanonicalDispatch>,
}

#[derive(Debug, Default)]
struct CanonicalDispatch {
    template: Option<Arc<PreparedCanonicalMoeTemplate>>,
    session: Option<CudaSession>,
}

#[derive(Debug)]
struct CudaSession {
    raw: *mut c_void,
    canonical_v3: bool,
}

// Access to the owned CUDA stream and allocations is serialized by `dispatch`.
unsafe impl Send for CudaSession {}

impl CudaSession {
    fn generic(
        attempts: usize,
        h: usize,
        w: usize,
        k: usize,
        rank: usize,
        dot: usize,
    ) -> Result<Self, SearchBackendError> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` is writable. Dimensions are checked before crossing the ABI.
        let status = unsafe {
            ai_pow_cuda_session_create(
                u32::try_from(attempts).map_err(unavailable)?,
                u32::try_from(h).map_err(unavailable)?,
                u32::try_from(w).map_err(unavailable)?,
                u32::try_from(k).map_err(unavailable)?,
                u32::try_from(rank).map_err(unavailable)?,
                u32::try_from(dot).map_err(unavailable)?,
                &mut raw,
            )
        };
        if status != 0 {
            return Err(cuda_error("session creation", status));
        }
        Ok(Self {
            raw,
            canonical_v3: false,
        })
    }

    fn canonical(
        template: &PreparedCanonicalMoeTemplate,
        max_attempts: u32,
    ) -> Result<Self, SearchBackendError> {
        let (a, b, routing, offsets, rows, cols, sigma, mu) = template.gpu_inputs();
        let rows: &[u32; 8] = rows.try_into().map_err(|_| unavailable("canonical row count"))?;
        let cols: &[u32; 8] = cols.try_into().map_err(|_| unavailable("canonical column count"))?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: CUDA copies all fixed template inputs before returning.
        let status = unsafe {
            ai_pow_cuda_v3_session_create(
                max_attempts,
                a.as_ptr(),
                b.as_ptr(),
                sigma.as_ptr(),
                mu.as_ptr(),
                routing.as_ptr(),
                u32::try_from(routing.len()).map_err(unavailable)?,
                offsets.as_ptr(),
                u32::try_from(offsets.len()).map_err(unavailable)?,
                rows.as_ptr(),
                cols.as_ptr(),
                &mut raw,
            )
        };
        if status != 0 {
            return Err(cuda_error("V3 session creation", status));
        }
        Ok(Self {
            raw,
            canonical_v3: true,
        })
    }

    fn run_generic(
        &self,
        a_rows: &[i8],
        b_cols: &[i8],
        attempts: usize,
    ) -> Result<Vec<TileState>, SearchBackendError> {
        let mut words = vec![0i32; attempts * 16];
        // SAFETY: session dimensions define input lengths and the output has 16 words per attempt.
        let status = unsafe {
            ai_pow_cuda_session_run(
                self.raw,
                a_rows.as_ptr(),
                b_cols.as_ptr(),
                u32::try_from(attempts).map_err(unavailable)?,
                words.as_mut_ptr(),
            )
        };
        if status != 0 {
            return Err(cuda_error("batch execution", status));
        }
        Ok(words
            .chunks_exact(16)
            .map(|chunk| {
                let mut state = [0i32; 16];
                state.copy_from_slice(chunk);
                TileState(state)
            })
            .collect())
    }

    fn search_canonical(
        &self,
        start: u32,
        attempts: u32,
        threshold: &[u8; 32],
    ) -> Result<Option<(u32, [u8; 32])>, SearchBackendError> {
        let mut local = NO_WINNER;
        let mut jackpot = [0u8; 32];
        // SAFETY: all buffers remain valid for the synchronous ABI call.
        let status = unsafe {
            ai_pow_cuda_v3_session_search(
                self.raw,
                start,
                attempts,
                threshold.as_ptr(),
                &mut local,
                jackpot.as_mut_ptr(),
            )
        };
        if status != 0 {
            return Err(cuda_error("V3 search", status));
        }
        Ok((local != NO_WINNER).then_some((local, jackpot)))
    }

    #[cfg(test)]
    fn debug_canonical(&self, extranonce: u32) -> Result<CanonicalDebug, SearchBackendError> {
        let mut debug = CanonicalDebug::default();
        // SAFETY: every output buffer has the fixed ABI length.
        let status = unsafe {
            ai_pow_cuda_v3_session_debug(
                self.raw,
                extranonce,
                debug.kappa.as_mut_ptr(),
                debug.h_a.as_mut_ptr(),
                debug.h_b.as_mut_ptr(),
                debug.s_a.as_mut_ptr(),
                debug.s_b.as_mut_ptr(),
                debug.a_rows.as_mut_ptr(),
                debug.b_cols.as_mut_ptr(),
                debug.state.as_mut_ptr(),
                debug.jackpot.as_mut_ptr(),
            )
        };
        if status != 0 {
            return Err(cuda_error("V3 debug evaluation", status));
        }
        Ok(debug)
    }
}

impl Drop for CudaSession {
    fn drop(&mut self) {
        // SAFETY: `raw` is owned by this value and destroyed exactly once.
        unsafe {
            if self.canonical_v3 {
                ai_pow_cuda_v3_session_destroy(self.raw);
            } else {
                ai_pow_cuda_session_destroy(self.raw);
            }
        }
    }
}

impl GpuSearchBackend {
    pub fn new(device_ordinal: usize, batch_attempts: u64) -> Result<Self> {
        if batch_attempts == 0 {
            bail!("--gpu-batch-attempts must be nonzero");
        }
        if device_ordinal != 0 {
            bail!("the GPU backend currently supports CUDA device 0 only");
        }
        if batch_attempts > u64::from(u32::MAX) {
            bail!("--gpu-batch-attempts must fit in u32");
        }
        Ok(Self {
            device_ordinal,
            batch_attempts,
            dispatch: Mutex::new(CanonicalDispatch::default()),
        })
    }

    pub const fn device_ordinal(&self) -> usize {
        self.device_ordinal
    }

    pub const fn batch_attempts(&self) -> u64 {
        self.batch_attempts
    }
}

fn unavailable(error: impl std::fmt::Display) -> SearchBackendError {
    SearchBackendError::BackendUnavailable(error.to_string())
}

fn cuda_error(operation: &str, status: c_int) -> SearchBackendError {
    unavailable(format!("CUDA {operation} failed with error {status}"))
}

impl SearchBackend for GpuSearchBackend {
    fn search_dense(
        &self,
        template: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let attempts = usize::try_from(batch.len).map_err(unavailable)?;
        let params = template.params();
        let config = template.config();
        let h = config.rows_pattern.size()? as usize;
        let w = config.cols_pattern.size()? as usize;
        let k = params.k as usize;
        let rank = params.noise_rank as usize;
        let attempt_bytes_a = h * k;
        let attempt_bytes_b = w * k;
        let dot = config.dot_product_length()? as usize;
        let mut all_a = vec![0; attempts * attempt_bytes_a];
        let mut all_b = vec![0; attempts * attempt_bytes_b];
        all_a
            .par_chunks_mut(attempt_bytes_a)
            .zip(all_b.par_chunks_mut(attempt_bytes_b))
            .enumerate()
            .try_for_each(|(offset, (destination_a, destination_b))| {
                let ordinal = batch.start + offset as u64;
                let (row, col) = template
                    .offsets_at_ordinal(ordinal)
                    .ok_or(SearchBackendError::DenseOrdinalOutOfRange(ordinal))?;
                let mut scratch = template.scratch();
                let (a, b) = template.prepare_offset(row, col, &mut scratch)?;
                destination_a.copy_from_slice(a);
                destination_b.copy_from_slice(b);
                Ok::<_, SearchBackendError>(())
            })?;
        let session = CudaSession::generic(attempts, h, w, k, rank, dot)?;
        let states = session.run_generic(&all_a, &all_b, attempts)?;
        for (offset, state) in states.iter().enumerate() {
            let jackpot = pearl_jackpot_hash(state, &template.commitments().s_a);
            if hash_le_target(&jackpot, &batch.threshold) {
                return Ok(Some(SearchWinner {
                    ordinal: batch.start + offset as u64,
                    jackpot_hash: jackpot,
                }));
            }
        }
        Ok(None)
    }

    fn search_canonical(
        &self,
        template: Arc<PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let start = u32::try_from(batch.start)
            .map_err(|_| SearchBackendError::CanonicalOrdinalOutOfRange(batch.start))?;
        let attempts = u32::try_from(batch.len).map_err(unavailable)?;
        if u64::from(start) + u64::from(attempts) > u64::from(u32::MAX) + 1 {
            return Err(SearchBackendError::CanonicalOrdinalOutOfRange(
                batch.end_exclusive() - 1,
            ));
        }
        let mut dispatch = self
            .dispatch
            .lock()
            .map_err(|_| unavailable("GPU dispatch lock poisoned"))?;
        if dispatch
            .template
            .as_ref()
            .is_none_or(|active| !Arc::ptr_eq(active, &template))
        {
            dispatch.session = Some(CudaSession::canonical(
                &template,
                u32::try_from(self.batch_attempts).map_err(unavailable)?,
            )?);
            dispatch.template = Some(Arc::clone(&template));
        }
        let (local, gpu_jackpot) = match dispatch
            .session
            .as_ref()
            .expect("canonical session is initialized")
            .search_canonical(start, attempts, &batch.threshold)?
        {
            Some(winner) => winner,
            None => return Ok(None),
        };
        if local >= attempts {
            return Err(SearchBackendError::WinnerOutsideBatch {
                winner: batch.start + u64::from(local),
                batch_start: batch.start,
                batch_end_exclusive: batch.end_exclusive(),
            });
        }
        let ordinal = batch.start + u64::from(local);
        let extranonce = u32::try_from(ordinal)
            .map_err(|_| SearchBackendError::CanonicalOrdinalOutOfRange(ordinal))?;
        let scalar = template.evaluate(extranonce, &mut template.scratch());
        if scalar.jackpot_hash != gpu_jackpot
            || !hash_le_target(&scalar.jackpot_hash, &batch.threshold)
        {
            return Err(unavailable(format!(
                "GPU winner {ordinal} failed Pearl V3 scalar validation"
            )));
        }
        Ok(Some(SearchWinner {
            ordinal,
            jackpot_hash: scalar.jackpot_hash,
        }))
    }

    fn batch_attempts(&self) -> u64 {
        self.batch_attempts
    }
}

#[cfg(test)]
#[derive(Debug)]
struct CanonicalDebug {
    kappa: [u8; 32],
    h_a: [u8; 32],
    h_b: [u8; 32],
    s_a: [u8; 32],
    s_b: [u8; 32],
    a_rows: [i8; 8192],
    b_cols: [i8; 8192],
    state: [i32; 16],
    jackpot: [u8; 32],
}

#[cfg(test)]
impl Default for CanonicalDebug {
    fn default() -> Self {
        Self {
            kappa: [0; 32],
            h_a: [0; 32],
            h_b: [0; 32],
            s_a: [0; 32],
            s_b: [0; 32],
            a_rows: [0; 8192],
            b_cols: [0; 8192],
            state: [0; 16],
            jackpot: [0; 32],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_pow::params::MatmulParams;

    fn canonical_params() -> MatmulParams {
        MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        }
    }

    #[test]
    fn rejects_zero_batch() {
        assert!(GpuSearchBackend::new(0, 0).is_err());
    }

    #[test]
    fn rejects_unsupported_device() {
        assert!(GpuSearchBackend::new(1, 1).is_err());
    }

    #[test]
    fn canonical_v3_device_pipeline_matches_scalar() {
        let template = PreparedCanonicalMoeTemplate::new(
            &canonical_params(),
            8,
            2,
            1,
            [0x42; 32],
        )
        .expect("canonical template");
        let session = CudaSession::canonical(&template, 4).expect("CUDA V3 session");
        let debug = session.debug_canonical(7).expect("CUDA V3 evaluation");
        let mut scratch = template.scratch();
        let scalar = template.evaluate(7, &mut scratch);
        let (a_rows, b_cols) = template.prepared_strips(&scratch);
        assert_eq!(debug.kappa, scalar.commitments.kappa);
        assert_eq!(debug.h_a, scalar.commitments.h_a);
        assert_eq!(debug.h_b, scalar.commitments.h_b);
        assert_eq!(debug.s_a, scalar.commitments.s_a);
        assert_eq!(debug.s_b, scalar.commitments.s_b);
        assert_eq!(&debug.a_rows, a_rows);
        assert_eq!(&debug.b_cols, b_cols);
        assert_eq!(TileState(debug.state), scalar.tile_state);
        assert_eq!(debug.jackpot, scalar.jackpot_hash);
    }
}
