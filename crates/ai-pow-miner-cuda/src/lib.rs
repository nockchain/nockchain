//! CUDA GEMM search backend for the standalone AI-PoW miner.
//!
//! Attempt-bound commitments, V3 salted seeds, and noise are derived by the
//! consensus-matched Rust implementation. CUDA computes the opened noised GEMM
//! and Pearl rolling tile state. Every winner is rechecked by the miner's scalar
//! oracle before proof construction or submission.

use std::ffi::{c_int, c_void};
use std::sync::{Arc, Mutex};

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::pearl_jackpot_hash;
use ai_pow::tile_hash::hash_le_target;
use ai_pow_miner::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};
use anyhow::{bail, Result};

unsafe extern "C" {
    fn ai_pow_cuda_tile_state(
        a_rows: *const i8,
        b_cols: *const i8,
        h: u32,
        w: u32,
        k: u32,
        rank: u32,
        dot_product_len: u32,
        state_out: *mut i32,
        stream: *mut c_void,
    ) -> c_int;
}

#[derive(Debug)]
pub struct CudaSearchBackend {
    device_ordinal: usize,
    gpu_batch_attempts: u64,
    dispatch: Mutex<()>,
}

impl CudaSearchBackend {
    pub fn new(device_ordinal: usize, gpu_batch_attempts: u64) -> Result<Self> {
        if gpu_batch_attempts == 0 {
            bail!("--gpu-batch-attempts must be nonzero");
        }
        if device_ordinal != 0 {
            bail!("the CUDA backend currently supports device ordinal 0 only");
        }
        Ok(Self {
            device_ordinal,
            gpu_batch_attempts,
            dispatch: Mutex::new(()),
        })
    }

    pub const fn gpu_batch_attempts(&self) -> u64 {
        self.gpu_batch_attempts
    }

    pub const fn device_ordinal(&self) -> usize {
        self.device_ordinal
    }

    fn tile_state(
        a_rows: &[i8],
        b_cols: &[i8],
        h: usize,
        w: usize,
        k: usize,
        rank: usize,
        dot_product_len: usize,
    ) -> Result<TileState, SearchBackendError> {
        let mut state = [0i32; 16];
        // SAFETY: slices are readable for the dimensions passed here, state is
        // writable, and the C wrapper synchronizes the default stream before
        // returning. Shape products are validated by prepared Rust templates.
        let status = unsafe {
            ai_pow_cuda_tile_state(
                a_rows.as_ptr(),
                b_cols.as_ptr(),
                u32::try_from(h).map_err(unavailable)?,
                u32::try_from(w).map_err(unavailable)?,
                u32::try_from(k).map_err(unavailable)?,
                u32::try_from(rank).map_err(unavailable)?,
                u32::try_from(dot_product_len).map_err(unavailable)?,
                state.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(SearchBackendError::BackendUnavailable(format!(
                "CUDA tile kernel failed with error {status}"
            )));
        }
        Ok(TileState(state))
    }
}

fn unavailable(error: impl std::fmt::Display) -> SearchBackendError {
    SearchBackendError::BackendUnavailable(error.to_string())
}

impl SearchBackend for CudaSearchBackend {
    fn search_dense(
        &self,
        template: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let _dispatch = self.dispatch.lock().map_err(|_| {
            SearchBackendError::BackendUnavailable("CUDA dispatch lock was poisoned".to_owned())
        })?;
        let params = template.params();
        let config = template.config();
        let h = config
            .rows_pattern
            .size()
            .map_err(SearchBackendError::DenseEvaluation)? as usize;
        let w = config
            .cols_pattern
            .size()
            .map_err(SearchBackendError::DenseEvaluation)? as usize;
        let dot = config
            .dot_product_length()
            .map_err(SearchBackendError::DenseEvaluation)? as usize;
        let mut scratch = template.scratch();
        for ordinal in batch.start..batch.end_exclusive() {
            let (t_rows, t_cols) = template
                .offsets_at_ordinal(ordinal)
                .ok_or(SearchBackendError::DenseOrdinalOutOfRange(ordinal))?;
            let (a_rows, b_cols) = template.prepare_offset(t_rows, t_cols, &mut scratch)?;
            let state = Self::tile_state(
                a_rows, b_cols, h, w, params.k as usize, params.noise_rank as usize, dot,
            )?;
            let jackpot = pearl_jackpot_hash(&state, &template.commitments().s_a);
            if hash_le_target(&jackpot, &batch.threshold) {
                return Ok(Some(SearchWinner {
                    ordinal,
                    jackpot_hash: jackpot,
                }));
            }
        }
        Ok(None)
    }

    fn search_canonical(
        &self,
        template: Arc<ai_pow_miner::canonical::PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let _dispatch = self.dispatch.lock().map_err(|_| {
            SearchBackendError::BackendUnavailable("CUDA dispatch lock was poisoned".to_owned())
        })?;
        let config = template.config();
        let params_k = config.common_dim as usize;
        let rank = config.rank as usize;
        let (routing, inner, local_b, _, _) = template.schedule();
        let h = routing
            .outer_indices(0, inner)
            .map_err(|error| unavailable(error))?
            .len();
        let w = local_b.len();
        let mut scratch = template.scratch();
        for ordinal in batch.start..batch.end_exclusive() {
            let extranonce = u32::try_from(ordinal)
                .map_err(|_| SearchBackendError::CanonicalOrdinalOutOfRange(ordinal))?;
            let commitments = template.prepare_attempt(extranonce, &mut scratch);
            let (a_rows, b_cols) = template.prepared_strips(&scratch);
            let state = Self::tile_state(a_rows, b_cols, h, w, params_k, rank, params_k)?;
            let jackpot = pearl_jackpot_hash(&state, &commitments.s_a);
            if hash_le_target(&jackpot, &batch.threshold) {
                return Ok(Some(SearchWinner {
                    ordinal,
                    jackpot_hash: jackpot,
                }));
            }
        }
        Ok(None)
    }
}
