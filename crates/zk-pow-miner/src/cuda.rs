use std::ffi::{c_char, c_int, CStr};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::AtomExt;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockvm::noun::{Atom, NounAllocator, T};
use thiserror::Error;
use tracing::{debug, info};
use zkvm_jetpack::form::belt::PRIME;

use crate::v5::{decode_digest_slab, nonce_slab, V5Job};
use crate::worker::{MineResult, SerfWorker, Worker, WorkerError, WorkerId};

const DEFAULT_BLOCKS_PER_SM: u32 = 12;
const DEFAULT_THREADS_PER_BLOCK: u32 = 512;

#[derive(Debug, Error)]
pub enum CudaError {
    #[error("CUDA call failed with error {code}: {message}")]
    Runtime { code: i32, message: String },
    #[error("CUDA returned an invalid device name")]
    BadDeviceName,
    #[error("CUDA nonce limb {index} is not a Goldilocks field element")]
    UnbasedNonce { index: usize },
    #[error("CUDA batch size must be between 1 and the Goldilocks prime minus one")]
    BadBatchSize,
}

#[derive(Clone, Copy, Debug)]
pub struct CudaSearchResult {
    pub next_nonce: [u64; 5],
    pub attempts: u64,
    pub kernel_ms: f32,
    pub found: bool,
}

pub struct CudaSession {
    raw: NonNull<ZkPowCudaSession>,
    device: u32,
    name: String,
}

unsafe impl Send for CudaSession {}

impl CudaSession {
    pub fn new(device: u32) -> Result<Self, CudaError> {
        Self::with_launch_config(device, DEFAULT_BLOCKS_PER_SM, DEFAULT_THREADS_PER_BLOCK)
    }

    pub fn with_launch_config(
        device: u32,
        blocks_per_sm: u32,
        threads_per_block: u32,
    ) -> Result<Self, CudaError> {
        let mut raw = std::ptr::null_mut();
        check(unsafe {
            zk_pow_cuda_session_create(
                device,
                blocks_per_sm,
                threads_per_block,
                std::ptr::addr_of_mut!(raw),
            )
        })?;
        let raw = NonNull::new(raw).ok_or_else(|| CudaError::Runtime {
            code: -1,
            message: "session creation returned a null pointer".to_owned(),
        })?;
        let name = device_name(device)?;
        Ok(Self { raw, device, name })
    }

    pub fn device(&self) -> u32 {
        self.device
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn search(
        &mut self,
        job: &V5Job,
        start_nonce: [u64; 5],
        attempts: u64,
    ) -> Result<CudaSearchResult, CudaError> {
        validate_nonce(start_nonce)?;
        if attempts == 0 || attempts >= PRIME {
            return Err(CudaError::BadBatchSize);
        }
        let mut result = RawCudaResult::default();
        check(unsafe {
            zk_pow_cuda_search(
                self.raw.as_ptr(),
                job,
                start_nonce.as_ptr(),
                attempts,
                std::ptr::addr_of_mut!(result),
            )
        })?;
        Ok(CudaSearchResult {
            next_nonce: result.nonce,
            attempts: result.attempts,
            kernel_ms: result.kernel_ms,
            found: result.found != 0,
        })
    }

    pub fn digest(&mut self, job: &V5Job, nonce: [u64; 5]) -> Result<[u64; 5], CudaError> {
        validate_nonce(nonce)?;
        let mut digest = [0; 5];
        check(unsafe {
            zk_pow_cuda_digest(self.raw.as_ptr(), job, nonce.as_ptr(), digest.as_mut_ptr())
        })?;
        Ok(digest)
    }
}

impl Drop for CudaSession {
    fn drop(&mut self) {
        unsafe { zk_pow_cuda_session_destroy(self.raw.as_ptr()) };
    }
}

pub fn device_count() -> Result<u32, CudaError> {
    let mut count = 0;
    check(unsafe { zk_pow_cuda_device_count(std::ptr::addr_of_mut!(count)) })?;
    Ok(count)
}

pub fn device_name(device: u32) -> Result<String, CudaError> {
    let mut name = [0 as c_char; 256];
    check(unsafe { zk_pow_cuda_device_name(device, name.as_mut_ptr(), name.len() as u32) })?;
    let name = unsafe { CStr::from_ptr(name.as_ptr()) };
    name.to_str()
        .map(str::to_owned)
        .map_err(|_| CudaError::BadDeviceName)
}

fn validate_nonce(nonce: [u64; 5]) -> Result<(), CudaError> {
    for (index, limb) in nonce.into_iter().enumerate() {
        if limb >= PRIME {
            return Err(CudaError::UnbasedNonce { index });
        }
    }
    Ok(())
}

pub(crate) struct SharedProver {
    id: WorkerId,
    worker: SerfWorker,
    gate: tokio::sync::Mutex<()>,
}

impl SharedProver {
    pub(crate) async fn spawn(
        id: WorkerId,
        hot_state: Vec<nockvm::jets::hot::HotEntry>,
    ) -> Result<Self, WorkerError> {
        Ok(Self {
            id,
            worker: SerfWorker::spawn(id, hot_state).await?,
            gate: tokio::sync::Mutex::new(()),
        })
    }

    async fn mine_attempt(
        &self,
        poke: NounSlab,
        cancel_generation: &AtomicU64,
        expected_generation: u64,
    ) -> Result<MineResult, WorkerError> {
        let _guard = self.gate.lock().await;
        if cancel_generation.load(Ordering::SeqCst) != expected_generation {
            return Err(WorkerError::Cancelled);
        }
        self.worker.mine_attempt(poke).await
    }

    fn cancel(&self) {
        self.worker.cancel();
    }
}

pub(crate) struct CudaWorker {
    id: WorkerId,
    batch_size: u64,
    session: Arc<Mutex<CudaSession>>,
    prover: Arc<SharedProver>,
    cancel_generation: AtomicU64,
}

impl CudaWorker {
    pub(crate) async fn spawn(
        id: WorkerId,
        device: u32,
        batch_size: u64,
        blocks_per_sm: u32,
        threads_per_block: u32,
        prover: Arc<SharedProver>,
    ) -> Result<Self, WorkerError> {
        if batch_size == 0 || batch_size >= PRIME {
            return Err(WorkerError::Cuda(CudaError::BadBatchSize.to_string()));
        }
        let session = tokio::task::spawn_blocking(move || {
            CudaSession::with_launch_config(device, blocks_per_sm, threads_per_block)
        })
        .await
        .map_err(|_| WorkerError::Panicked)?
        .map_err(|error| WorkerError::Cuda(error.to_string()))?;
        info!(
            worker = id,
            prover = prover.id,
            device = session.device(),
            name = session.name(),
            batch_size,
            blocks_per_sm,
            threads_per_block,
            "initialized CUDA V5 mining worker"
        );
        Ok(Self {
            id,
            batch_size,
            session: Arc::new(Mutex::new(session)),
            prover,
            cancel_generation: AtomicU64::new(0),
        })
    }
}

#[async_trait]
impl Worker for CudaWorker {
    fn id(&self) -> WorkerId {
        self.id
    }

    fn cancel(&self) {
        self.cancel_generation.fetch_add(1, Ordering::SeqCst);
        self.prover.cancel();
    }

    async fn mine_attempt(&self, poke: NounSlab) -> Result<MineResult, WorkerError> {
        let cancel_generation = self.cancel_generation.load(Ordering::SeqCst);
        let Some(attempt) = decode_v5_attempt(&poke)? else {
            debug!(worker = self.id, "delegating non-V5 proof search to Serf");
            return self
                .prover
                .mine_attempt(poke, &self.cancel_generation, cancel_generation)
                .await;
        };
        if attempt.job.pow_len > 64 {
            debug!(
                worker = self.id,
                pow_len = attempt.job.pow_len,
                "delegating oversized V5 puzzle to Serf"
            );
            return self
                .prover
                .mine_attempt(poke, &self.cancel_generation, cancel_generation)
                .await;
        }
        let session = Arc::clone(&self.session);
        let batch_size = self.batch_size;
        let job = attempt.job;
        let nonce = attempt.nonce;
        let result = tokio::task::spawn_blocking(move || {
            session
                .lock()
                .map_err(|_| CudaError::Runtime {
                    code: -1,
                    message: "CUDA session lock was poisoned".to_owned(),
                })?
                .search(&job, nonce, batch_size)
        })
        .await
        .map_err(|_| WorkerError::Panicked)?
        .map_err(|error| WorkerError::Cuda(error.to_string()))?;
        if self.cancel_generation.load(Ordering::SeqCst) != cancel_generation {
            return Err(WorkerError::Cancelled);
        }

        let hash_rate = if result.kernel_ms > 0.0 {
            result.attempts as f64 * 1_000.0 / f64::from(result.kernel_ms)
        } else {
            0.0
        };
        debug!(
            worker = self.id,
            attempts = result.attempts,
            kernel_ms = result.kernel_ms,
            hashes_per_second = hash_rate,
            found = result.found,
            "completed CUDA V5 nonce batch"
        );

        if !result.found {
            return Ok(MineResult::Retry {
                next_nonce: nonce_slab(result.next_nonce),
            });
        }

        let digest = attempt.job.digest(result.next_nonce);
        if !attempt.job.meets_target(&digest) {
            return Err(WorkerError::CudaDigestMismatch);
        }
        let winning_poke = replace_poke_nonce(&poke, result.next_nonce)?;
        match self
            .prover
            .mine_attempt(winning_poke, &self.cancel_generation, cancel_generation)
            .await?
        {
            MineResult::Success {
                hash_slab,
                poke_slab,
            } => {
                let proved_digest = decode_digest_slab(&hash_slab)
                    .map_err(|error| WorkerError::Candidate(error.to_string()))?;
                if proved_digest != digest {
                    return Err(WorkerError::CudaDigestMismatch);
                }
                Ok(MineResult::Success {
                    hash_slab,
                    poke_slab,
                })
            }
            MineResult::Retry { .. } => Err(WorkerError::CudaFalsePositive),
        }
    }
}

#[derive(Clone, Copy)]
struct V5Attempt {
    job: V5Job,
    nonce: [u64; 5],
}

fn decode_v5_attempt(poke: &NounSlab) -> Result<Option<V5Attempt>, WorkerError> {
    let space = poke.noun_space();
    let root = unsafe { *poke.root() };
    let parts = match root.in_space(&space).uncell::<5>() {
        Ok(parts) => parts,
        Err(_) => return Ok(None),
    };
    if parts[0].as_atom().and_then(|atom| atom.as_u64()) != Ok(5) {
        return Ok(None);
    }
    let pow_len = parts[4]
        .as_atom()
        .map_err(|_| WorkerError::Candidate("V5 pow-len is not an atom".to_owned()))?
        .as_u64()
        .map_err(|_| WorkerError::Candidate("V5 pow-len does not fit in u64".to_owned()))?;

    let copy_part = |noun: nockvm::noun::Noun| {
        let mut slab = NounSlab::new();
        let root = slab.copy_into(noun, &space);
        slab.set_root(root);
        slab
    };
    let commitment_slab = copy_part(parts[1].noun());
    let nonce_slab = copy_part(parts[2].noun());
    let target_slab = copy_part(parts[3].noun());
    let job = V5Job::from_candidate_parts(&commitment_slab, &target_slab, pow_len)
        .map_err(|error| WorkerError::Candidate(error.to_string()))?;
    let nonce = decode_digest_slab(&nonce_slab)
        .map_err(|error| WorkerError::Candidate(error.to_string()))?;
    Ok(Some(V5Attempt { job, nonce }))
}

fn replace_poke_nonce(poke: &NounSlab, nonce: [u64; 5]) -> Result<NounSlab, WorkerError> {
    let space = poke.noun_space();
    let root = unsafe { *poke.root() };
    let parts = root
        .in_space(&space)
        .uncell::<5>()
        .map_err(|_| WorkerError::Candidate("candidate poke is not a five-tuple".to_owned()))?;
    let mut slab = NounSlab::new();
    let version = slab.copy_into(parts[0].noun(), &space);
    let commitment = slab.copy_into(parts[1].noun(), &space);
    let target = slab.copy_into(parts[3].noun(), &space);
    let pow_len = slab.copy_into(parts[4].noun(), &space);
    let nonce_atoms = nonce.map(|limb| {
        <Atom as AtomExt>::from_value(&mut slab, limb)
            .expect("based nonce limb fits in an atom")
            .as_noun()
    });
    let nonce = T(&mut slab, &nonce_atoms);
    let root = T(&mut slab, &[version, commitment, nonce, target, pow_len]);
    slab.set_root(root);
    Ok(slab)
}

fn check(code: c_int) -> Result<(), CudaError> {
    if code == 0 {
        return Ok(());
    }
    let message = unsafe {
        let pointer = zk_pow_cuda_error_string(code);
        if pointer.is_null() {
            "unknown CUDA error".to_owned()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        }
    };
    Err(CudaError::Runtime { code, message })
}

#[repr(C)]
struct ZkPowCudaSession {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
struct RawCudaResult {
    nonce: [u64; 5],
    attempts: u64,
    kernel_ms: f32,
    found: u32,
}

unsafe extern "C" {
    fn zk_pow_cuda_device_count(count: *mut u32) -> c_int;
    fn zk_pow_cuda_device_name(device: u32, name: *mut c_char, name_len: u32) -> c_int;
    fn zk_pow_cuda_session_create(
        device: u32,
        blocks_per_sm: u32,
        threads_per_block: u32,
        session: *mut *mut ZkPowCudaSession,
    ) -> c_int;
    fn zk_pow_cuda_session_destroy(session: *mut ZkPowCudaSession);
    fn zk_pow_cuda_search(
        session: *mut ZkPowCudaSession,
        job: *const V5Job,
        start_nonce: *const u64,
        attempts: u64,
        result: *mut RawCudaResult,
    ) -> c_int;
    fn zk_pow_cuda_digest(
        session: *mut ZkPowCudaSession,
        job: *const V5Job,
        nonce: *const u64,
        digest: *mut u64,
    ) -> c_int;
    fn zk_pow_cuda_error_string(error: c_int) -> *const c_char;
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuda_digest_matches_cpu_oracle() {
        let mut session = CudaSession::new(0).expect("CUDA device 0");
        for pow_len in [1, 2, 4, 16, 64] {
            let job = V5Job::new([1, 2, 3, 4, 5], [PRIME - 1; 5], pow_len).expect("job");
            for nonce in [[0, 0, 0, 0, 0], [6, 7, 8, 9, 10], [PRIME - 1, 0, 0, 0, 0]] {
                assert_eq!(
                    session.digest(&job, nonce).expect("CUDA digest"),
                    job.digest(nonce),
                    "pow_len={pow_len}, nonce={nonce:?}"
                );
            }
        }
    }

    #[test]
    fn cuda_search_finds_first_easy_target_nonce() {
        let mut session = CudaSession::new(0).expect("CUDA device 0");
        let job = V5Job::new([1, 2, 3, 4, 5], [PRIME - 1; 5], 64).expect("job");
        let start = [PRIME - 2, 9, 8, 7, 6];
        let result = session.search(&job, start, 64).expect("search");
        assert!(result.found);
        assert_eq!(result.next_nonce, start);
        assert_eq!(result.attempts, 64);
    }

    #[test]
    fn cuda_search_returns_lowest_winning_offset_across_nonce_carry() {
        use crate::v5::{add_nonce, digest_le};

        let mut session = CudaSession::new(0).expect("CUDA device 0");
        let start = [PRIME - 1, 9, 8, 7, 6];
        let mut job = V5Job::new([1, 2, 3, 4, 5], [0; 5], 64).expect("job");
        let mut target = [PRIME - 1; 5];
        let mut expected_offset = 0;
        for offset in 0..64 {
            let digest = job.digest(add_nonce(start, offset));
            if digest != target && digest_le(&digest, &target) {
                target = digest;
                expected_offset = offset;
            }
        }
        assert_ne!(expected_offset, 0, "fixture must exercise nonce carry");

        job.target = target;
        let result = session.search(&job, start, 64).expect("search");
        assert!(result.found);
        assert_eq!(
            result.next_nonce,
            add_nonce(start, expected_offset),
            "CUDA must return the first nonce meeting the target"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cuda_worker_proves_gpu_winner_with_hoon_kernel() {
        let commitment = [1, 2, 3, 4, 5];
        let nonce = [6, 7, 8, 9, 10];
        let candidate = synthetic_candidate(5, commitment, ibig::UBig::from(1u8) << 400, 2);
        let worker = spawn_cuda_worker().await;
        let poke = crate::worker::build_candidate_poke(&candidate, nonce_slab(nonce));
        let expected = V5Job::new(commitment, [PRIME - 1; 5], 2)
            .expect("job")
            .digest(nonce);
        match worker.mine_attempt(poke).await.expect("mining attempt") {
            MineResult::Success { hash_slab, .. } => {
                assert_eq!(
                    decode_digest_slab(&hash_slab).expect("proved digest"),
                    expected
                );
            }
            MineResult::Retry { .. } => panic!("trivial target rejected CUDA winner"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cuda_worker_delegates_legacy_version_to_serf() {
        let nonce = [6, 7, 8, 9, 10];
        let candidate = synthetic_candidate(3, [1, 2, 3, 4, 5], ibig::UBig::from(1u8) << 400, 1);
        let worker = spawn_cuda_worker().await;
        let poke = crate::worker::build_candidate_poke(&candidate, nonce_slab(nonce));
        match worker
            .mine_attempt(poke)
            .await
            .expect("legacy mining attempt")
        {
            MineResult::Success { .. } => {}
            MineResult::Retry { .. } => panic!("trivial target rejected legacy proof"),
        }
    }

    async fn spawn_cuda_worker() -> CudaWorker {
        let prover = Arc::new(
            SharedProver::spawn(0, zkvm_jetpack::hot::produce_prover_hot_state())
                .await
                .expect("shared prover"),
        );
        CudaWorker::spawn(
            0, 0, 1, DEFAULT_BLOCKS_PER_SM, DEFAULT_THREADS_PER_BLOCK, prover,
        )
        .await
        .expect("CUDA worker")
    }

    fn synthetic_candidate(
        version_value: u64,
        commitment: [u64; 5],
        target_value: ibig::UBig,
        pow_len: u64,
    ) -> nockchain_mining_common::MiningCandidate {
        use nockchain_mining_common::{MiningCandidate, MiningCandidateKind};
        use nockvm::noun::D;

        let mut version = NounSlab::new();
        version.set_root(D(version_value));
        let mut block_header = NounSlab::new();
        let header_atoms = commitment.map(D);
        let header = T(&mut block_header, &header_atoms);
        block_header.set_root(header);
        let mut target = NounSlab::new();
        let target_noun = bignum_to_noun(&mut target, &target_value);
        target.set_root(target_noun);
        MiningCandidate {
            kind: MiningCandidateKind::Zk,
            version,
            block_header,
            target,
            pow_len,
        }
    }

    fn bignum_to_noun(slab: &mut NounSlab, value: &ibig::UBig) -> nockvm::noun::Noun {
        use nockvm::noun::D;
        use nockvm_macros::tas;

        let mut list = D(0);
        let bytes = value.to_le_bytes();
        for chunk in bytes.chunks(4).rev() {
            let mut padded = [0u8; 4];
            padded[..chunk.len()].copy_from_slice(chunk);
            let atom = <Atom as AtomExt>::from_value(slab, u64::from(u32::from_le_bytes(padded)))
                .expect("target limb atom")
                .as_noun();
            list = T(slab, &[atom, list]);
        }
        T(slab, &[D(tas!(b"bn")), list])
    }
}
