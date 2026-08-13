//! CUDA GEMM search backend.
//!
//! Rust owns Pearl V3 transcript derivation, salted seeds, noise, scheduling,
//! target checks, and scalar winner rechecks. CUDA computes batches of opened
//! noised GEMMs and Pearl rolling tile states in one launch.

use std::ffi::{c_int, c_void};
use std::sync::{Arc, Mutex};

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::pearl_jackpot_hash;
use ai_pow::tile_hash::hash_le_target;
use anyhow::{bail, Result};
use rayon::prelude::*;

use crate::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};

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
}

#[derive(Debug)]
pub struct GpuSearchBackend {
    device_ordinal: usize,
    batch_attempts: u64,
    dispatch: Mutex<()>,
}

#[derive(Debug)]
struct CudaBatchSession {
    raw: *mut c_void,
}

impl CudaBatchSession {
    fn new(
        attempts: usize,
        h: usize,
        w: usize,
        k: usize,
        rank: usize,
        dot: usize,
    ) -> Result<Self, SearchBackendError> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` is writable, and all values are checked before crossing
        // the ABI. A successful call returns sole ownership of the session.
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
        Ok(Self { raw })
    }

    fn run(
        &mut self,
        a_rows: &[i8],
        b_cols: &[i8],
        attempts: usize,
    ) -> Result<Vec<TileState>, SearchBackendError> {
        let mut words = vec![0i32; attempts * 16];
        // SAFETY: the session owns device storage sized for `attempts`; both
        // input slices contain the packed shape passed at creation; the output
        // holds exactly 16 words per attempt. The call synchronizes its stream.
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
}

impl Drop for CudaBatchSession {
    fn drop(&mut self) {
        // SAFETY: `raw` is owned by this value and destroyed exactly once.
        unsafe {
            ai_pow_cuda_session_destroy(self.raw);
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
            dispatch: Mutex::new(()),
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
        let _guard = self
            .dispatch
            .lock()
            .map_err(|_| unavailable("GPU dispatch lock poisoned"))?;
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
        let states =
            CudaBatchSession::new(attempts, h, w, k, rank, dot)?.run(&all_a, &all_b, attempts)?;
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
        template: Arc<crate::canonical::PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let _guard = self
            .dispatch
            .lock()
            .map_err(|_| unavailable("GPU dispatch lock poisoned"))?;
        let attempts = usize::try_from(batch.len).map_err(unavailable)?;
        let config = template.config();
        let (_, inner, local_b, _, _) = template.schedule();
        let h = inner.len();
        let w = local_b.len();
        let k = config.common_dim as usize;
        let rank = config.rank as usize;
        let attempt_bytes_a = h * k;
        let attempt_bytes_b = w * k;
        let mut all_a = vec![0; attempts * attempt_bytes_a];
        let mut all_b = vec![0; attempts * attempt_bytes_b];
        let mut keys = vec![[0; 32]; attempts];
        all_a
            .par_chunks_mut(attempt_bytes_a)
            .zip(all_b.par_chunks_mut(attempt_bytes_b))
            .zip(keys.par_iter_mut())
            .enumerate()
            .try_for_each(|(offset, ((destination_a, destination_b), key))| {
                let ordinal = batch.start + offset as u64;
                let extranonce = u32::try_from(ordinal)
                    .map_err(|_| SearchBackendError::CanonicalOrdinalOutOfRange(ordinal))?;
                let mut scratch = template.scratch();
                let commitments = template.prepare_attempt(extranonce, &mut scratch);
                let (a, b) = template.prepared_strips(&scratch);
                destination_a.copy_from_slice(a);
                destination_b.copy_from_slice(b);
                *key = commitments.s_a;
                Ok::<_, SearchBackendError>(())
            })?;
        let states =
            CudaBatchSession::new(attempts, h, w, k, rank, k)?.run(&all_a, &all_b, attempts)?;
        for (offset, (state, key)) in states.iter().zip(&keys).enumerate() {
            let jackpot = pearl_jackpot_hash(state, key);
            if hash_le_target(&jackpot, &batch.threshold) {
                return Ok(Some(SearchWinner {
                    ordinal: batch.start + offset as u64,
                    jackpot_hash: jackpot,
                }));
            }
        }
        Ok(None)
    }

    fn batch_attempts(&self) -> u64 {
        self.batch_attempts
    }
}

#[cfg(test)]
mod tests {
    use ai_pow::params::MatmulParams;

    use super::*;

    #[test]
    fn canonical_gpu_batch_matches_scalar_oracle() {
        let params = MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let template = Arc::new(
            crate::canonical::PreparedCanonicalMoeTemplate::new(&params, 8, 2, 1, [0x42; 32])
                .expect("canonical template"),
        );
        let expected = template.evaluate(9, &mut template.scratch()).jackpot_hash;
        let mut threshold = expected;
        threshold[0] = threshold[0].saturating_add(1);
        let winner = GpuSearchBackend::new(0, 8)
            .expect("GPU backend")
            .search_canonical(
                Arc::clone(&template),
                SearchBatch::new(9, 1, threshold).expect("search batch"),
            )
            .expect("GPU search")
            .expect("winner");

        assert_eq!(winner.ordinal, 9);
        assert_eq!(winner.jackpot_hash, expected);

        let no_winner = GpuSearchBackend::new(0, 8)
            .expect("GPU backend")
            .search_canonical(
                template,
                SearchBatch::new(7, 8, [0; 32]).expect("search batch"),
            )
            .expect("GPU search");
        assert!(no_winner.is_none());
    }
}
