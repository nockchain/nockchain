//! CUDA GEMM search backend.
//!
//! Rust owns Pearl V3 transcript derivation, salted seeds, noise, scheduling,
//! target checks, and scalar winner rechecks. CUDA computes only the opened
//! noised GEMM and Pearl rolling tile state.

use std::ffi::{c_int, c_void};
use std::sync::{Arc, Mutex};

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::pearl_jackpot_hash;
use ai_pow::tile_hash::hash_le_target;
use anyhow::{bail, Result};

use crate::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};

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
pub struct GpuSearchBackend {
    device_ordinal: usize,
    batch_attempts: u64,
    dispatch: Mutex<()>,
}

impl GpuSearchBackend {
    pub fn new(device_ordinal: usize, batch_attempts: u64) -> Result<Self> {
        if batch_attempts == 0 {
            bail!("--gpu-batch-attempts must be nonzero");
        }
        if device_ordinal != 0 {
            bail!("the GPU backend currently supports CUDA device 0 only");
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

    fn tile_state(
        a_rows: &[i8],
        b_cols: &[i8],
        h: usize,
        w: usize,
        k: usize,
        rank: usize,
        dot: usize,
    ) -> Result<TileState, SearchBackendError> {
        let mut state = [0i32; 16];
        // SAFETY: prepared templates validate every shape and slice length. The
        // C wrapper synchronizes before returning and does not retain pointers.
        let status = unsafe {
            ai_pow_cuda_tile_state(
                a_rows.as_ptr(),
                b_cols.as_ptr(),
                u32::try_from(h).map_err(unavailable)?,
                u32::try_from(w).map_err(unavailable)?,
                u32::try_from(k).map_err(unavailable)?,
                u32::try_from(rank).map_err(unavailable)?,
                u32::try_from(dot).map_err(unavailable)?,
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
        let params = template.params();
        let config = template.config();
        let h = config.rows_pattern.size()? as usize;
        let w = config.cols_pattern.size()? as usize;
        let dot = config.dot_product_length()? as usize;
        let mut scratch = template.scratch();
        for ordinal in batch.start..batch.end_exclusive() {
            let (row, col) = template
                .offsets_at_ordinal(ordinal)
                .ok_or(SearchBackendError::DenseOrdinalOutOfRange(ordinal))?;
            let (a, b) = template.prepare_offset(row, col, &mut scratch)?;
            let state = Self::tile_state(
                a, b, h, w, params.k as usize, params.noise_rank as usize, dot,
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
        template: Arc<crate::canonical::PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let _guard = self
            .dispatch
            .lock()
            .map_err(|_| unavailable("GPU dispatch lock poisoned"))?;
        let config = template.config();
        let (_, inner, local_b, _, _) = template.schedule();
        let h = inner.len();
        let w = local_b.len();
        let k = config.common_dim as usize;
        let rank = config.rank as usize;
        let mut scratch = template.scratch();
        for ordinal in batch.start..batch.end_exclusive() {
            let extranonce = u32::try_from(ordinal)
                .map_err(|_| SearchBackendError::CanonicalOrdinalOutOfRange(ordinal))?;
            let commitments = template.prepare_attempt(extranonce, &mut scratch);
            let (a, b) = template.prepared_strips(&scratch);
            let state = Self::tile_state(a, b, h, w, k, rank, k)?;
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

#[cfg(test)]
mod tests {
    use ai_pow::params::MatmulParams;

    use super::*;

    #[test]
    fn canonical_gpu_search_matches_scalar_oracle() {
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
        let expected = template.evaluate(7, &mut template.scratch()).jackpot_hash;
        let winner = GpuSearchBackend::new(0, 1)
            .expect("GPU backend")
            .search_canonical(
                template,
                SearchBatch::new(7, 1, [0xff; 32]).expect("search batch"),
            )
            .expect("GPU search")
            .expect("winner");

        assert_eq!(winner.ordinal, 7);
        assert_eq!(winner.jackpot_hash, expected);
    }
}
