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
const MAX_GPU_DEVICES: usize = 8;

unsafe extern "C" {
    fn ai_pow_cuda_device_count(count_out: *mut u32) -> c_int;
    fn ai_pow_cuda_session_create(
        device_ordinal: u32,
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
        device_ordinal: u32,
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
        capture_debug: u32,
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

pub struct GpuSearchBackend {
    device_ordinal: usize,
    batch_attempts: u64,
    dispatch: Mutex<CanonicalDispatch>,
}

/// One ordered CUDA search spread across independent devices.
pub struct MultiGpuSearchBackend {
    backends: Vec<GpuSearchBackend>,
    batch_attempts_per_device: u64,
    batch_attempts: u64,
}

#[derive(Default)]
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
        device_ordinal: usize,
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
                u32::try_from(device_ordinal).map_err(unavailable)?,
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
        device_ordinal: usize,
    ) -> Result<Self, SearchBackendError> {
        let (a, b, routing, offsets, rows, cols, sigma, mu) = template.gpu_inputs();
        let rows: &[u32; 8] = rows
            .try_into()
            .map_err(|_| unavailable("canonical row count"))?;
        let cols: &[u32; 8] = cols
            .try_into()
            .map_err(|_| unavailable("canonical column count"))?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: CUDA copies all fixed template inputs before returning.
        let status = unsafe {
            ai_pow_cuda_v3_session_create(
                u32::try_from(device_ordinal).map_err(unavailable)?,
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
                0,
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
    pub fn available_device_count() -> Result<usize> {
        let mut count = 0u32;
        // SAFETY: `count` is a valid writable output.
        let status = unsafe { ai_pow_cuda_device_count(&mut count) };
        if status != 0 {
            bail!("CUDA device enumeration failed with error {status}");
        }
        if count == 0 {
            bail!("no CUDA devices are visible");
        }
        Ok(count as usize)
    }

    pub fn new(device_ordinal: usize, batch_attempts: u64) -> Result<Self> {
        if batch_attempts == 0 {
            bail!("--gpu-batch-attempts must be nonzero");
        }
        if batch_attempts > u64::from(u32::MAX) {
            bail!("--gpu-batch-attempts must fit in u32");
        }
        let device_count = Self::available_device_count()?;
        if device_ordinal >= device_count {
            bail!(
                "CUDA device {device_ordinal} is not visible; visible ordinals are 0..{}",
                device_count - 1
            );
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

impl MultiGpuSearchBackend {
    pub fn all_visible(batch_attempts_per_device: u64) -> Result<Self> {
        let count = GpuSearchBackend::available_device_count()?.min(MAX_GPU_DEVICES);
        Self::new((0..count).collect(), batch_attempts_per_device)
    }

    pub fn new(device_ordinals: Vec<usize>, batch_attempts_per_device: u64) -> Result<Self> {
        if device_ordinals.is_empty() {
            bail!("--cuda-devices must select at least one CUDA device");
        }
        if device_ordinals.len() > MAX_GPU_DEVICES {
            bail!("--cuda-devices supports at most {MAX_GPU_DEVICES} devices");
        }
        for (index, &ordinal) in device_ordinals.iter().enumerate() {
            if device_ordinals[..index].contains(&ordinal) {
                bail!("--cuda-devices contains duplicate device {ordinal}");
            }
        }
        let batch_attempts = batch_attempts_per_device
            .checked_mul(device_ordinals.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("combined GPU batch size overflows u64"))?;
        let backends = device_ordinals
            .into_iter()
            .map(|ordinal| GpuSearchBackend::new(ordinal, batch_attempts_per_device))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            backends,
            batch_attempts_per_device,
            batch_attempts,
        })
    }

    pub fn device_ordinals(&self) -> Vec<usize> {
        self.backends
            .iter()
            .map(GpuSearchBackend::device_ordinal)
            .collect()
    }

    pub const fn batch_attempts_per_device(&self) -> u64 {
        self.batch_attempts_per_device
    }
}

fn active_device_count(batch: SearchBatch, device_count: usize) -> usize {
    device_count.min(usize::try_from(batch.len).unwrap_or(usize::MAX))
}

fn partition_for_device(batch: SearchBatch, active: usize, index: usize) -> SearchBatch {
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

fn lower_winner(left: Option<SearchWinner>, right: Option<SearchWinner>) -> Option<SearchWinner> {
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
        let session = CudaSession::generic(self.device_ordinal, attempts, h, w, k, rank, dot)?;
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
                self.device_ordinal,
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

impl SearchBackend for MultiGpuSearchBackend {
    fn search_dense(
        &self,
        template: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let active = active_device_count(batch, self.backends.len());
        self.backends[..active]
            .par_iter()
            .enumerate()
            .map(|(index, backend)| {
                backend.search_dense(
                    Arc::clone(&template),
                    partition_for_device(batch, active, index),
                )
            })
            .try_reduce(|| None, |left, right| Ok(lower_winner(left, right)))
    }

    fn search_canonical(
        &self,
        template: Arc<PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let active = active_device_count(batch, self.backends.len());
        self.backends[..active]
            .par_iter()
            .enumerate()
            .map(|(index, backend)| {
                backend.search_canonical(
                    Arc::clone(&template),
                    partition_for_device(batch, active, index),
                )
            })
            .try_reduce(|| None, |left, right| Ok(lower_winner(left, right)))
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
    use ai_pow::params::MatmulParams;

    use super::*;

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
    fn rejects_non_visible_device() {
        let count = GpuSearchBackend::available_device_count().expect("visible CUDA devices");
        assert!(GpuSearchBackend::new(count, 1).is_err());
    }

    #[test]
    fn multi_gpu_partitions_preserve_the_ordered_batch() {
        let batch = SearchBatch::new(41, 19, [0x5a; 32]).expect("search batch");
        let active = active_device_count(batch, 8);
        let partitions = (0..active)
            .map(|index| partition_for_device(batch, active, index))
            .collect::<Vec<_>>();
        assert_eq!(partitions.len(), 8);
        assert_eq!(partitions.first().expect("first").start, batch.start);
        assert_eq!(
            partitions.last().expect("last").end_exclusive(),
            batch.end_exclusive()
        );
        assert_eq!(
            partitions
                .iter()
                .map(|partition| partition.len)
                .sum::<u64>(),
            batch.len
        );
        assert!(partitions
            .windows(2)
            .all(|pair| pair[0].end_exclusive() == pair[1].start));
    }

    #[test]
    fn multi_gpu_reduction_returns_the_global_lowest_winner() {
        let lowest = [
            Some(SearchWinner {
                ordinal: 900,
                jackpot_hash: [9; 32],
            }),
            None,
            Some(SearchWinner {
                ordinal: 100,
                jackpot_hash: [1; 32],
            }),
        ]
        .into_iter()
        .fold(None, lower_winner)
        .expect("winner");
        assert_eq!(lowest.ordinal, 100);
        assert_eq!(lowest.jackpot_hash, [1; 32]);
    }
    #[test]
    fn multi_gpu_canonical_search_returns_global_lowest_winner() {
        let device_count =
            GpuSearchBackend::available_device_count().expect("visible CUDA devices");
        if device_count < 2 {
            return;
        }
        let template = Arc::new(
            PreparedCanonicalMoeTemplate::new(&canonical_params(), 8, 2, 1, [0x4d; 32])
                .expect("canonical template"),
        );
        let backend = MultiGpuSearchBackend::all_visible(4).expect("multi-GPU backend");
        let batch_len = u64::try_from(device_count.min(8)).expect("device count") * 2;
        let winner = backend
            .search_canonical(
                Arc::clone(&template),
                SearchBatch::new(41, batch_len, [u8::MAX; 32]).expect("maximum target batch"),
            )
            .expect("maximum target search")
            .expect("first ordinal is a winner");
        assert_eq!(winner.ordinal, 41);
        assert_eq!(
            winner.jackpot_hash,
            template.evaluate(41, &mut template.scratch()).jackpot_hash
        );
        assert!(backend
            .search_canonical(
                template,
                SearchBatch::new(57, batch_len, [0; 32]).expect("zero target batch"),
            )
            .expect("zero target search")
            .is_none());
    }

    #[test]
    fn canonical_v3_device_pipeline_matches_scalar() {
        let template = PreparedCanonicalMoeTemplate::new(&canonical_params(), 8, 2, 1, [0x42; 32])
            .expect("canonical template");
        let session = CudaSession::canonical(&template, 4, 0).expect("CUDA V3 session");
        for extranonce in [0, 1, 7, u32::MAX - 1, u32::MAX] {
            let debug = session
                .debug_canonical(extranonce)
                .expect("CUDA V3 evaluation");
            let mut scratch = template.scratch();
            let scalar = template.evaluate(extranonce, &mut scratch);
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

    #[test]
    fn canonical_search_obeys_targets_and_session_lifetime() {
        let first = Arc::new(
            PreparedCanonicalMoeTemplate::new(&canonical_params(), 8, 2, 1, [0x24; 32])
                .expect("first canonical template"),
        );
        let second = Arc::new(
            PreparedCanonicalMoeTemplate::new(&canonical_params(), 8, 2, 1, [0x25; 32])
                .expect("second canonical template"),
        );
        let backend = GpuSearchBackend::new(0, 4).expect("GPU backend");
        for (template, start) in
            [(Arc::clone(&first), 41), (Arc::clone(&first), 45), (Arc::clone(&second), 49)]
        {
            let winner = backend
                .search_canonical(
                    Arc::clone(&template),
                    SearchBatch::new(start, 4, [u8::MAX; 32]).expect("maximum target batch"),
                )
                .expect("maximum target search")
                .expect("ordinal zero is a winner");
            assert_eq!(winner.ordinal, start);
            assert_eq!(
                winner.jackpot_hash,
                template
                    .evaluate(start as u32, &mut template.scratch())
                    .jackpot_hash
            );
        }

        assert!(backend
            .search_canonical(
                second,
                SearchBatch::new(53, 4, [0; 32]).expect("zero target batch"),
            )
            .expect("zero target search")
            .is_none());
    }

    #[test]
    #[ignore = "real compact recursive proof generation is opt-in"]
    fn gpu_winner_builds_and_verifies_production_certificate() {
        use crate::canonical::prove_canonical_moe_block_at_with_verifier_context;
        use crate::certificate_noun::{
            build_ai_pow_pearl_merge_moe_artifact_noun_from_node, verify_ai_pow_block_artifact_jam,
            AiPowBlockVerifyOutcome, AiProofNode, CertificateNounLimits,
        };

        let params = canonical_params();
        let commit = [0x42; 32];
        let consensus_target = ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET;
        let work_factor =
            ai_pow::difficulty::shape_work_factor(8, 8, 1024).expect("canonical work factor");
        let threshold =
            ai_pow::difficulty::effective_jackpot_threshold(&consensus_target, work_factor)
                .expect("effective jackpot threshold");
        let template = Arc::new(
            PreparedCanonicalMoeTemplate::new(&params, 8, 2, 1, commit)
                .expect("canonical template"),
        );
        let backend = GpuSearchBackend::new(0, 1024).expect("GPU backend");
        let winner = backend
            .search_canonical(
                Arc::clone(&template),
                SearchBatch::new(0, 1024, threshold).expect("production-target batch"),
            )
            .expect("GPU search")
            .expect("GPU batch contains a production-target ticket");
        let extranonce = u32::try_from(winner.ordinal).expect("canonical extranonce");
        assert_eq!(
            winner.jackpot_hash,
            template
                .evaluate(extranonce, &mut template.scratch())
                .jackpot_hash
        );

        let (block, verifier_context) = prove_canonical_moe_block_at_with_verifier_context(
            &params, 8, 2, 1, commit, extranonce,
        )
        .expect("compact recursive proof");
        assert_eq!(block.jackpot_hash, winner.jackpot_hash);
        let AiProofNode::Bytes(certificate_bytes) = &block.certificate.certificate else {
            panic!("production compact certificate must use the canonical byte node");
        };
        let decoded_certificate =
            ai_pow_zk::recursion::decode_compact_batch_recursive_certificate(certificate_bytes)
                .expect("canonical compact certificate bytes");
        assert_eq!(
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&decoded_certificate)
                .expect("encode compact certificate"),
            *certificate_bytes
        );

        let artifact = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("production MoE artifact");
        let expected_digest_bytes =
            ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
                verifier_context.verifier_key_digest(),
            );
        let outcome = verify_ai_pow_block_artifact_jam(
            &artifact.jam(),
            CertificateNounLimits::default(),
            &commit,
            &consensus_target,
            4096,
            &verifier_context,
            &expected_digest_bytes,
        )
        .expect("production V3 verifier accepts the GPU winner certificate");
        assert!(matches!(outcome, AiPowBlockVerifyOutcome::Moe(_)));
    }
}
