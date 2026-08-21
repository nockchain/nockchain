//! Typed vLLM control plane and mining-first request scheduler.
//!
//! vLLM reports fused gate/up work over gRPC. The scheduler drains active
//! inference work and otherwise keeps a bounded idle-mining backend running. One
//! already-launched idle batch may finish after work arrives; CUDA launches are not
//! preemptible.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use ai_pow::params::{PEARL_K_MAX, PEARL_MN_MAX};
use anyhow::{bail, Result};
use nockapp_grpc_proto::pb::ai_pow::v1::inference_mining_service_server::{
    InferenceMiningService, InferenceMiningServiceServer,
};
use nockapp_grpc_proto::pb::ai_pow::v1::{
    opened_block_part, GetMiningJobRequest, GetStatusRequest, InferenceMiningStatus, MiningJob,
    NotifyWorkRequest, NotifyWorkResponse, OpenedBlockMetadata, OpenedBlockPart, OpenedTensor,
    RegisterRuntimeRequest, RegisterRuntimeResponse, SchedulerMode, SubmitOpenedBlockResponse,
    WorkPhase,
};
use tonic::{Request, Response, Status};

const PROTOCOL_VERSION: u32 = 1;
const RUNTIME_ID_BYTES: usize = 16;
const DIGEST_BYTES: usize = 32;
const CUDA_DEVICE_UUID_BYTES: usize = 16;
const HEADER_BYTES: usize = 76;
const TARGET_BYTES: usize = 32;
const MAX_TENSOR_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_OPENED_TENSOR_BYTES: u64 = 512 * 1024 * 1024;
pub const GEMMA_COMMON_DIM: u32 = 5_376;
pub const GEMMA_FUSED_OUTPUT_DIM: u32 = 43_008;
const GEMMA_MAX_TOKENS: u32 = 8_192;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WorkKey {
    runtime_id: [u8; RUNTIME_ID_BYTES],
    work_id: u64,
}

#[derive(Debug, Default)]
struct SchedulerInner {
    runtimes: HashSet<[u8; RUNTIME_ID_BYTES]>,
    active: HashSet<WorkKey>,
}

/// Shared state for the gRPC service and the dedicated idle-mining worker.
pub struct InferenceSchedulerState {
    inner: Mutex<SchedulerInner>,
    changed: Condvar,
    next_runtime: AtomicU64,
    idle_batches: AtomicU64,
    inference_batches: AtomicU64,
    opened_blocks_received: AtomicU64,
    candidate_generation: AtomicU64,
}

impl Default for InferenceSchedulerState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(SchedulerInner::default()),
            changed: Condvar::new(),
            next_runtime: AtomicU64::new(1),
            idle_batches: AtomicU64::new(0),
            inference_batches: AtomicU64::new(0),
            opened_blocks_received: AtomicU64::new(0),
            candidate_generation: AtomicU64::new(0),
        }
    }
}

impl InferenceSchedulerState {
    fn register_runtime(&self, request: RegisterRuntimeRequest) -> Result<[u8; RUNTIME_ID_BYTES]> {
        if request.protocol_version != PROTOCOL_VERSION {
            bail!(
                "unsupported inference protocol version {}; expected {PROTOCOL_VERSION}",
                request.protocol_version
            );
        }
        let checkpoint_layout_digest = fixed_bytes::<DIGEST_BYTES>(
            &request.checkpoint_layout_digest, "checkpoint_layout_digest",
        )?;
        let cuda_device_uuid =
            fixed_bytes::<CUDA_DEVICE_UUID_BYTES>(&request.cuda_device_uuid, "cuda_device_uuid")?;
        let sequence = self.next_runtime.fetch_add(1, Ordering::Relaxed);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&sequence.to_le_bytes());
        hasher.update(&request.process_id.to_le_bytes());
        hasher.update(&checkpoint_layout_digest);
        hasher.update(&cuda_device_uuid);
        let mut runtime_id = [0u8; RUNTIME_ID_BYTES];
        runtime_id.copy_from_slice(&hasher.finalize().as_bytes()[..RUNTIME_ID_BYTES]);
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("scheduler lock poisoned"))?;
        inner.runtimes.insert(runtime_id);
        Ok(runtime_id)
    }

    fn validate_runtime(&self, bytes: &[u8]) -> Result<[u8; RUNTIME_ID_BYTES]> {
        let runtime_id = fixed_bytes::<RUNTIME_ID_BYTES>(bytes, "runtime_id")?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("scheduler lock poisoned"))?;
        if !inner.runtimes.contains(&runtime_id) {
            bail!("runtime_id is not registered");
        }
        Ok(runtime_id)
    }

    fn notify_work(&self, request: NotifyWorkRequest) -> Result<u32> {
        let runtime_id = fixed_bytes::<RUNTIME_ID_BYTES>(&request.runtime_id, "runtime_id")?;
        let phase = WorkPhase::try_from(request.phase)
            .map_err(|_| anyhow::anyhow!("invalid work phase {}", request.phase))?;
        if request.work_id == 0 {
            bail!("work_id must be nonzero");
        }
        if request.token_count == 0 || request.token_count > GEMMA_MAX_TOKENS {
            bail!("token_count must be in 1..={GEMMA_MAX_TOKENS}");
        }
        if request.common_dim == 0 || request.common_dim > PEARL_K_MAX {
            bail!("common_dim must be in 1..={PEARL_K_MAX}");
        }
        if request.output_dim == 0 || request.output_dim > PEARL_MN_MAX {
            bail!("output_dim must be in 1..={PEARL_MN_MAX}");
        }
        let key = WorkKey {
            runtime_id,
            work_id: request.work_id,
        };
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("scheduler lock poisoned"))?;
        if !inner.runtimes.contains(&runtime_id) {
            bail!("runtime_id is not registered");
        }
        match phase {
            WorkPhase::Started => {
                if !inner.active.insert(key) {
                    bail!("work_id is already active");
                }
            }
            WorkPhase::Finished | WorkPhase::Failed => {
                if !inner.active.remove(&key) {
                    bail!("work_id is not active");
                }
                self.inference_batches.fetch_add(1, Ordering::Relaxed);
            }
            WorkPhase::Unspecified => bail!("work phase must be specified"),
        }
        let active = u32::try_from(inner.active.len())?;
        drop(inner);
        self.changed.notify_all();
        Ok(active)
    }

    fn active_work_items(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.active.len())
            .unwrap_or(usize::MAX)
    }

    fn wait_for_idle_or_stop(&self, stop: &AtomicBool) -> bool {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return false,
        };
        while !inner.active.is_empty() && !stop.load(Ordering::Acquire) {
            inner = match self.changed.wait(inner) {
                Ok(inner) => inner,
                Err(_) => return false,
            };
        }
        !stop.load(Ordering::Acquire)
    }

    fn status(&self) -> InferenceMiningStatus {
        let active = self.active_work_items();
        InferenceMiningStatus {
            mode: if active == 0 {
                SchedulerMode::IdleMining.into()
            } else {
                SchedulerMode::InferenceMining.into()
            },
            active_work_items: u32::try_from(active).unwrap_or(u32::MAX),
            idle_batches: self.idle_batches.load(Ordering::Relaxed),
            inference_batches: self.inference_batches.load(Ordering::Relaxed),
            candidate_generation: self.candidate_generation.load(Ordering::Acquire),
            opened_blocks_received: self.opened_blocks_received.load(Ordering::Relaxed),
        }
    }

    pub fn runtime_snapshot_count(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.runtimes.len())
            .unwrap_or(0)
    }
}

/// One bounded idle-mining launch. Implementations must not allocate on the
/// no-hit path and must return after one cancellable launch.
pub trait IdleMiningBackend: Send + Sync + 'static {
    fn mine_one_batch(&self) -> Result<()>;
}

pub struct IdleMiningWorker {
    stop: Arc<AtomicBool>,
    state: Arc<InferenceSchedulerState>,
    handle: Option<JoinHandle<Result<()>>>,
}

impl IdleMiningWorker {
    pub fn spawn<B: IdleMiningBackend>(
        state: Arc<InferenceSchedulerState>,
        backend: Arc<B>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_state = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("ai-pow-inference-idle".to_string())
            .spawn(move || {
                while worker_state.wait_for_idle_or_stop(&worker_stop) {
                    // A request may arrive immediately after this check. One already
                    // selected batch is allowed to finish; CUDA launches cannot be
                    // preempted.
                    backend.mine_one_batch()?;
                    worker_state.idle_batches.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            })
            .expect("idle mining worker thread creation");
        Self {
            stop,
            state,
            handle: Some(handle),
        }
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        self.state.changed.notify_all();
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("idle mining worker panicked"))??;
        }
        Ok(())
    }
}

impl Drop for IdleMiningWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.state.changed.notify_all();
    }
}

#[derive(Clone)]
pub struct InferenceMiningRpc {
    state: Arc<InferenceSchedulerState>,
    mining_job: Arc<Mutex<MiningJob>>,
}

impl InferenceMiningRpc {
    pub fn new(state: Arc<InferenceSchedulerState>, mining_job: MiningJob) -> Result<Self> {
        validate_mining_job(&mining_job)?;
        state
            .candidate_generation
            .store(mining_job.candidate_generation, Ordering::Release);
        Ok(Self {
            state,
            mining_job: Arc::new(Mutex::new(mining_job)),
        })
    }

    pub fn set_mining_job(&self, mining_job: MiningJob) -> Result<()> {
        validate_mining_job(&mining_job)?;
        self.state
            .candidate_generation
            .store(mining_job.candidate_generation, Ordering::Release);
        *self
            .mining_job
            .lock()
            .map_err(|_| anyhow::anyhow!("mining job lock poisoned"))? = mining_job;
        Ok(())
    }

    pub fn into_server(self) -> InferenceMiningServiceServer<Self> {
        InferenceMiningServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl InferenceMiningService for InferenceMiningRpc {
    async fn register_runtime(
        &self,
        request: Request<RegisterRuntimeRequest>,
    ) -> Result<Response<RegisterRuntimeResponse>, Status> {
        let runtime_id = self
            .state
            .register_runtime(request.into_inner())
            .map_err(invalid_argument)?;
        Ok(Response::new(RegisterRuntimeResponse {
            runtime_id: runtime_id.to_vec(),
            protocol_version: PROTOCOL_VERSION,
        }))
    }

    async fn get_mining_job(
        &self,
        request: Request<GetMiningJobRequest>,
    ) -> Result<Response<MiningJob>, Status> {
        self.state
            .validate_runtime(&request.into_inner().runtime_id)
            .map_err(unauthenticated)?;
        let job = self
            .mining_job
            .lock()
            .map_err(|_| Status::internal("mining job lock poisoned"))?
            .clone();
        Ok(Response::new(job))
    }

    async fn notify_work(
        &self,
        request: Request<NotifyWorkRequest>,
    ) -> Result<Response<NotifyWorkResponse>, Status> {
        let request = request.into_inner();
        let work_id = request.work_id;
        let active_work_items = self.state.notify_work(request).map_err(invalid_argument)?;
        Ok(Response::new(NotifyWorkResponse {
            work_id,
            active_work_items,
        }))
    }

    async fn submit_opened_block(
        &self,
        request: Request<tonic::Streaming<OpenedBlockPart>>,
    ) -> Result<Response<SubmitOpenedBlockResponse>, Status> {
        let mut stream = request.into_inner();
        let mut metadata: Option<OpenedBlockMetadata> = None;
        let mut next_offsets = [0u64; 3];
        while let Some(part) = stream.message().await? {
            match part
                .part
                .ok_or_else(|| Status::invalid_argument("opened block part missing"))?
            {
                opened_block_part::Part::Metadata(value) => {
                    if metadata.is_some() {
                        return Err(Status::invalid_argument("duplicate opened block metadata"));
                    }
                    self.state
                        .validate_runtime(&value.runtime_id)
                        .map_err(unauthenticated)?;
                    if value.noise_seed_a.len() != DIGEST_BYTES
                        || value.noise_seed_b.len() != DIGEST_BYTES
                    {
                        return Err(Status::invalid_argument("noise seeds must be 32 bytes"));
                    }
                    metadata = Some(value);
                }
                opened_block_part::Part::TensorChunk(chunk) => {
                    if metadata.is_none() {
                        return Err(Status::invalid_argument(
                            "metadata must precede tensor chunks",
                        ));
                    }
                    if chunk.data.is_empty() || chunk.data.len() > MAX_TENSOR_CHUNK_BYTES {
                        return Err(Status::invalid_argument(
                            "tensor chunk size is out of bounds",
                        ));
                    }
                    let tensor = OpenedTensor::try_from(chunk.tensor)
                        .map_err(|_| Status::invalid_argument("invalid opened tensor"))?;
                    let index = match tensor {
                        OpenedTensor::A => 0,
                        OpenedTensor::BTransposed => 1,
                        OpenedTensor::Routing => 2,
                        OpenedTensor::Unspecified => {
                            return Err(Status::invalid_argument(
                                "opened tensor must be specified",
                            ));
                        }
                    };
                    if chunk.offset != next_offsets[index] {
                        return Err(Status::invalid_argument("tensor chunks must be contiguous"));
                    }
                    next_offsets[index] = next_offsets[index]
                        .checked_add(chunk.data.len() as u64)
                        .ok_or_else(|| Status::invalid_argument("tensor length overflow"))?;
                    if next_offsets[index] > MAX_OPENED_TENSOR_BYTES {
                        return Err(Status::resource_exhausted(
                            "opened tensor exceeds byte limit",
                        ));
                    }
                }
            }
        }
        let metadata =
            metadata.ok_or_else(|| Status::invalid_argument("opened block metadata missing"))?;
        self.state
            .opened_blocks_received
            .fetch_add(1, Ordering::Relaxed);
        Ok(Response::new(SubmitOpenedBlockResponse {
            accepted: false,
            detail: format!(
                "opened witness received for generation {}; proof handoff is not configured",
                metadata.candidate_generation
            ),
        }))
    }

    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<InferenceMiningStatus>, Status> {
        Ok(Response::new(self.state.status()))
    }
}

fn validate_mining_job(job: &MiningJob) -> Result<()> {
    if job.candidate_generation == 0 {
        bail!("candidate_generation must be nonzero");
    }
    if job.incomplete_header.len() != HEADER_BYTES {
        bail!("incomplete_header must be {HEADER_BYTES} bytes");
    }
    if job.target_le.len() != TARGET_BYTES {
        bail!("target_le must be {TARGET_BYTES} bytes");
    }
    if job.certificate_version == 0 {
        bail!("certificate_version must be nonzero");
    }
    Ok(())
}

fn fixed_bytes<const N: usize>(bytes: &[u8], field: &str) -> Result<[u8; N]> {
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{field} must be {N} bytes"))
}

fn invalid_argument(error: anyhow::Error) -> Status {
    Status::invalid_argument(error.to_string())
}

fn unauthenticated(error: anyhow::Error) -> Status {
    Status::unauthenticated(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;

    fn mining_job() -> MiningJob {
        MiningJob {
            candidate_generation: 1,
            incomplete_header: vec![0x11; HEADER_BYTES],
            target_le: vec![0x22; TARGET_BYTES],
            certificate_version: 3,
        }
    }

    fn runtime_request() -> RegisterRuntimeRequest {
        RegisterRuntimeRequest {
            protocol_version: PROTOCOL_VERSION,
            checkpoint_layout_digest: vec![0x33; DIGEST_BYTES],
            cuda_device_uuid: vec![0x44; CUDA_DEVICE_UUID_BYTES],
            process_id: 7,
        }
    }

    fn work(runtime_id: &[u8], work_id: u64, phase: WorkPhase) -> NotifyWorkRequest {
        NotifyWorkRequest {
            runtime_id: runtime_id.to_vec(),
            work_id,
            phase: phase.into(),
            layer: 3,
            token_count: 128,
            common_dim: GEMMA_COMMON_DIM,
            output_dim: GEMMA_FUSED_OUTPUT_DIM,
            error: String::new(),
        }
    }

    #[test]
    fn registration_and_work_transitions_are_typed_and_bounded() {
        let state = InferenceSchedulerState::default();
        let runtime_id = state.register_runtime(runtime_request()).unwrap();
        assert_eq!(state.runtime_snapshot_count(), 1);
        assert_eq!(
            state
                .notify_work(work(&runtime_id, 1, WorkPhase::Started))
                .unwrap(),
            1
        );
        assert_eq!(state.status().mode, SchedulerMode::InferenceMining as i32);
        assert!(state
            .notify_work(work(&runtime_id, 1, WorkPhase::Started))
            .is_err());
        assert_eq!(
            state
                .notify_work(work(&runtime_id, 1, WorkPhase::Finished))
                .unwrap(),
            0
        );
        assert_eq!(state.status().mode, SchedulerMode::IdleMining as i32);
        assert_eq!(state.status().inference_batches, 1);
    }

    #[test]
    fn work_token_limit_matches_runtime_context() {
        let state = InferenceSchedulerState::default();
        let runtime_id = state.register_runtime(runtime_request()).unwrap();
        let mut maximum = work(&runtime_id, 1, WorkPhase::Started);
        maximum.token_count = GEMMA_MAX_TOKENS;
        state.notify_work(maximum).unwrap();
        state
            .notify_work(work(&runtime_id, 1, WorkPhase::Finished))
            .unwrap();

        let mut over_limit = work(&runtime_id, 2, WorkPhase::Started);
        over_limit.token_count = GEMMA_MAX_TOKENS + 1;
        assert_eq!(
            state.notify_work(over_limit).unwrap_err().to_string(),
            "token_count must be in 1..=8192"
        );
    }

    struct CountingBackend {
        batches: AtomicUsize,
    }

    impl IdleMiningBackend for CountingBackend {
        fn mine_one_batch(&self) -> Result<()> {
            self.batches.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(1));
            Ok(())
        }
    }

    #[test]
    fn idle_worker_pauses_for_inference_and_resumes_after_queue_drains() {
        let state = Arc::new(InferenceSchedulerState::default());
        let runtime_id = state.register_runtime(runtime_request()).unwrap();
        let backend = Arc::new(CountingBackend {
            batches: AtomicUsize::new(0),
        });
        let worker = IdleMiningWorker::spawn(Arc::clone(&state), Arc::clone(&backend));
        while backend.batches.load(Ordering::Relaxed) < 2 {
            std::thread::sleep(Duration::from_millis(1));
        }
        state
            .notify_work(work(&runtime_id, 1, WorkPhase::Started))
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let paused = backend.batches.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(backend.batches.load(Ordering::Relaxed), paused);
        state
            .notify_work(work(&runtime_id, 1, WorkPhase::Finished))
            .unwrap();
        while backend.batches.load(Ordering::Relaxed) == paused {
            std::thread::sleep(Duration::from_millis(1));
        }
        worker.stop().unwrap();
        assert!(state.status().idle_batches > 0);
    }

    #[tokio::test]
    async fn rpc_roundtrip_returns_registered_job_and_status() {
        let state = Arc::new(InferenceSchedulerState::default());
        let rpc = InferenceMiningRpc::new(Arc::clone(&state), mining_job()).unwrap();
        let registered = rpc
            .register_runtime(Request::new(runtime_request()))
            .await
            .unwrap()
            .into_inner();
        let job = rpc
            .get_mining_job(Request::new(GetMiningJobRequest {
                runtime_id: registered.runtime_id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(job, mining_job());
        rpc.notify_work(Request::new(work(
            &registered.runtime_id,
            9,
            WorkPhase::Started,
        )))
        .await
        .unwrap();
        let status = rpc
            .get_status(Request::new(GetStatusRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(status.mode, SchedulerMode::InferenceMining as i32);
        assert_eq!(status.active_work_items, 1);
    }

    #[test]
    fn malformed_runtime_and_job_fields_reject() {
        let state = InferenceSchedulerState::default();
        let mut malformed = runtime_request();
        malformed.checkpoint_layout_digest.pop();
        assert!(state.register_runtime(malformed).is_err());
        let mut job = mining_job();
        job.target_le.pop();
        assert!(InferenceMiningRpc::new(Arc::new(state), job).is_err());
    }
}
