//! CUDA ticket-search backend for the standalone AI-PoW miner.
//!
//! CUDA contexts, streams, and work sessions are owned by this excluded crate.
//! The normal miner never resolves or links CUDA dependencies.

use std::sync::{Arc, Mutex};

use ai_pow_miner::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};
use anyhow::{bail, Context, Result};
use cuda_core::{CudaContext, CudaStream};

/// Smallest GPU architecture that supports the required signed INT8 MMA path.
pub const MINIMUM_COMPUTE_CAPABILITY: (i32, i32) = (8, 0);

/// Persistently owns the CUDA context and stream used for one miner process.
///
/// A work session is keyed by the complete prepared-work fingerprint. Replacing
/// that fingerprint invalidates device state before a new candidate can launch.
#[derive(Debug)]
pub struct CudaSearchBackend {
    context: Arc<CudaContext>,
    stream: Arc<CudaStream>,
    gpu_batch_attempts: u64,
    session: Mutex<SessionState>,
}

#[derive(Debug, Default)]
struct SessionState {
    work_fingerprint: Option<[u8; 32]>,
}

impl CudaSearchBackend {
    /// Open a dedicated CUDA stream on a supported device.
    pub fn new(device_ordinal: usize, gpu_batch_attempts: u64) -> Result<Self> {
        if gpu_batch_attempts == 0 {
            bail!("--gpu-batch-attempts must be nonzero");
        }
        let context = CudaContext::new(device_ordinal)
            .with_context(|| format!("could not initialize CUDA device {device_ordinal}"))?;
        let actual = context
            .compute_capability()
            .context("could not query CUDA compute capability")?;
        if actual < MINIMUM_COMPUTE_CAPABILITY {
            bail!(
                "CUDA device {device_ordinal} has compute capability {}.{}, but AI-PoW requires {}.{}",
                actual.0,
                actual.1,
                MINIMUM_COMPUTE_CAPABILITY.0,
                MINIMUM_COMPUTE_CAPABILITY.1,
            );
        }
        let stream = context
            .new_stream()
            .context("could not create dedicated CUDA search stream")?;
        Ok(Self {
            context,
            stream,
            gpu_batch_attempts,
            session: Mutex::new(SessionState::default()),
        })
    }

    /// CUDA launch capacity. Cancellation is observed between these batches.
    pub const fn gpu_batch_attempts(&self) -> u64 {
        self.gpu_batch_attempts
    }

    /// Selected CUDA device ordinal.
    pub fn device_ordinal(&self) -> usize {
        self.context.ordinal()
    }
    /// Associate this process-local CUDA session with prepared work.
    ///
    /// A changed fingerprint must invalidate every work-specific device buffer
    /// before its replacement is uploaded.
    pub fn begin_work_session(&self, work_fingerprint: [u8; 32]) -> Result<bool> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("CUDA work-session lock was poisoned"))?;
        if session.work_fingerprint == Some(work_fingerprint) {
            return Ok(false);
        }
        session.work_fingerprint = Some(work_fingerprint);
        Ok(true)
    }

    /// Synchronize the dedicated stream before discarding a stale session.
    pub fn invalidate_work_session(&self) -> Result<()> {
        self.stream
            .synchronize()
            .context("could not synchronize CUDA search stream")?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("CUDA work-session lock was poisoned"))?;
        session.work_fingerprint = None;
        Ok(())
    }
}

impl SearchBackend for CudaSearchBackend {
    fn search_dense(
        &self,
        _template: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
        _batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        Err(SearchBackendError::BackendUnavailable(
            "CUDA dense ticket kernels are unavailable".to_string(),
        ))
    }

    fn search_canonical(
        &self,
        _template: Arc<ai_pow_miner::canonical::PreparedCanonicalMoeTemplate>,
        _batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        Err(SearchBackendError::BackendUnavailable(
            "CUDA canonical ticket kernels are unavailable".to_string(),
        ))
    }
}
