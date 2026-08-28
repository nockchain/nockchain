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
use ai_pow::pearl_compat::{
    PearlMiningConfig, PEARL_INCOMPLETE_BLOCK_HEADER_SIZE, PEARL_MINING_CONFIG_SIZE,
};
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
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status};

use crate::canonical::canonical_dense_mining_config;
use crate::gemma4::GEMMA4_NATIVE_PARAMS;

const PROTOCOL_VERSION: u32 = 2;
const RUNTIME_ID_BYTES: usize = 16;
const DIGEST_BYTES: usize = 32;
const CUDA_DEVICE_UUID_BYTES: usize = 16;
const HEADER_BYTES: usize = 76;
const TARGET_BYTES: usize = 32;
const MAX_TENSOR_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_OPENED_TENSOR_BYTES: u64 = 512 * 1024 * 1024;
pub const GEMMA_COMMON_DIM: u32 = 5_376;
const GEMMA_NOISE_RANK: u32 = 128;
const GEMMA_ROW_ALIGNMENT: u32 = 256;
const GEMMA_TILE: u32 = 16;
const GEMMA_MAX_PADDED_ROWS: u32 = 4_096;
pub const GEMMA_FUSED_OUTPUT_DIM: u32 = 43_008;
const GEMMA_MAX_TOKENS: u32 = 8_192;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WorkKey {
    runtime_id: [u8; RUNTIME_ID_BYTES],
    work_id: u64,
}

#[derive(Debug)]
pub struct OpenedDenseWitness {
    pub candidate_generation: u64,
    pub work_id: u64,
    pub extranonce: u32,
    pub a_row_indices: Vec<u32>,
    pub b_column_indices: Vec<u32>,
    pub noise_seed_a: [u8; DIGEST_BYTES],
    pub noise_seed_b: [u8; DIGEST_BYTES],
    pub noise_rank: u32,
    pub a_rows: u32,
    pub b_columns: u32,
    pub common_dim: u32,
    pub a: Arc<Vec<i8>>,
    pub b_transposed: Arc<Vec<i8>>,
}

pub struct InferenceProofRequest {
    pub witness: OpenedDenseWitness,
    pub response: oneshot::Sender<Result<String, String>>,
}

pub type InferenceProofSender = mpsc::Sender<InferenceProofRequest>;

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
    production_enabled: AtomicBool,
    node_connected: AtomicBool,
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
            production_enabled: AtomicBool::new(false),
            node_connected: AtomicBool::new(false),
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
            production_enabled: self.production_enabled.load(Ordering::Acquire),
            node_connected: self.node_connected.load(Ordering::Acquire),
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
    proof_sender: Option<InferenceProofSender>,
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
            proof_sender: None,
        })
    }

    pub fn with_proof_sender(mut self, proof_sender: InferenceProofSender) -> Self {
        self.proof_sender = Some(proof_sender);
        self
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
    pub fn candidate_generation(&self) -> u64 {
        self.state.candidate_generation.load(Ordering::Acquire)
    }

    pub fn mining_job(&self) -> Result<MiningJob> {
        self.mining_job
            .lock()
            .map(|job| job.clone())
            .map_err(|_| anyhow::anyhow!("mining job lock poisoned"))
    }

    pub fn set_production_enabled(&self, enabled: bool) {
        self.state
            .production_enabled
            .store(enabled, Ordering::Release);
    }

    pub fn set_node_connected(&self, connected: bool) {
        self.state
            .node_connected
            .store(connected, Ordering::Release);
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
        let mut buffers: [Vec<u8>; 3] = std::array::from_fn(|_| Vec::new());
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
                    let current_generation =
                        self.state.candidate_generation.load(Ordering::Acquire);
                    if value.candidate_generation != current_generation {
                        return Err(Status::failed_precondition(format!(
                            "stale candidate generation {}; current generation is {current_generation}",
                            value.candidate_generation
                        )));
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
                    if chunk.offset != buffers[index].len() as u64 {
                        return Err(Status::invalid_argument("tensor chunks must be contiguous"));
                    }
                    let next_length = buffers[index]
                        .len()
                        .checked_add(chunk.data.len())
                        .ok_or_else(|| Status::invalid_argument("tensor length overflow"))?;
                    if next_length as u64 > MAX_OPENED_TENSOR_BYTES {
                        return Err(Status::resource_exhausted(
                            "opened tensor exceeds byte limit",
                        ));
                    }
                    buffers[index].extend_from_slice(&chunk.data);
                }
            }
        }
        let metadata =
            metadata.ok_or_else(|| Status::invalid_argument("opened block metadata missing"))?;
        let witness = validate_opened_dense_witness(metadata, buffers).map_err(invalid_argument)?;
        let current_generation = self.state.candidate_generation.load(Ordering::Acquire);
        if witness.candidate_generation != current_generation {
            return Err(Status::failed_precondition(format!(
                "candidate generation changed to {current_generation} while receiving the opened witness"
            )));
        }
        self.state
            .opened_blocks_received
            .fetch_add(1, Ordering::Relaxed);

        let Some(proof_sender) = &self.proof_sender else {
            return Ok(Response::new(SubmitOpenedBlockResponse {
                accepted: false,
                detail: format!(
                    "opened witness received for generation {}; proof handoff is not configured",
                    witness.candidate_generation
                ),
            }));
        };
        let (response, response_rx) = oneshot::channel();
        proof_sender
            .send(InferenceProofRequest { witness, response })
            .await
            .map_err(|_| Status::unavailable("proof worker is unavailable"))?;
        let proof_result = response_rx
            .await
            .map_err(|_| Status::unavailable("proof worker dropped the response"))?;
        let (accepted, detail) = match proof_result {
            Ok(detail) => (true, detail),
            Err(detail) => (false, detail),
        };
        Ok(Response::new(SubmitOpenedBlockResponse {
            accepted,
            detail,
        }))
    }

    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<InferenceMiningStatus>, Status> {
        Ok(Response::new(self.state.status()))
    }
}

fn validate_opened_dense_witness(
    metadata: OpenedBlockMetadata,
    buffers: [Vec<u8>; 3],
) -> Result<OpenedDenseWitness> {
    validate_opened_dense_witness_for_output(metadata, buffers, GEMMA_FUSED_OUTPUT_DIM)
}

fn validate_opened_dense_witness_for_output(
    metadata: OpenedBlockMetadata,
    buffers: [Vec<u8>; 3],
    expected_output_dim: u32,
) -> Result<OpenedDenseWitness> {
    if metadata.noise_rank != GEMMA_NOISE_RANK {
        bail!("noise_rank must be {GEMMA_NOISE_RANK}");
    }
    if metadata.common_dim != GEMMA_COMMON_DIM {
        bail!("common_dim must be {GEMMA_COMMON_DIM}");
    }
    if metadata.b_columns != expected_output_dim {
        bail!("b_columns must be {expected_output_dim}");
    }
    if metadata.a_rows == 0
        || metadata.a_rows > GEMMA_MAX_PADDED_ROWS
        || !metadata.a_rows.is_multiple_of(GEMMA_ROW_ALIGNMENT)
    {
        bail!(
            "a_rows must be a nonzero multiple of {GEMMA_ROW_ALIGNMENT} no greater than {GEMMA_MAX_PADDED_ROWS}"
        );
    }
    if !metadata.b_columns.is_multiple_of(GEMMA_TILE) {
        bail!("b_columns must be a multiple of {GEMMA_TILE}");
    }
    validate_opened_indices(&metadata.a_row_indices, metadata.a_rows, "a_row_indices")?;
    validate_opened_indices(
        &metadata.b_column_indices, metadata.b_columns, "b_column_indices",
    )?;
    if !buffers[2].is_empty() {
        bail!("dense opened witnesses must not contain routing data");
    }

    let expected_a = checked_tensor_elements(metadata.a_rows, metadata.common_dim, "A")?;
    let expected_b =
        checked_tensor_elements(metadata.b_columns, metadata.common_dim, "B-transposed")?;
    if buffers[0].len() != expected_a {
        bail!(
            "A tensor length {} does not match shape {}x{} ({expected_a} bytes)",
            buffers[0].len(),
            metadata.a_rows,
            metadata.common_dim
        );
    }
    if buffers[1].len() != expected_b {
        bail!(
            "B-transposed tensor length {} does not match shape {}x{} ({expected_b} bytes)",
            buffers[1].len(),
            metadata.b_columns,
            metadata.common_dim
        );
    }
    validate_int7(&buffers[0], "A")?;
    validate_int7(&buffers[1], "B-transposed")?;

    let noise_seed_a = fixed_bytes(&metadata.noise_seed_a, "noise_seed_a")?;
    let noise_seed_b = fixed_bytes(&metadata.noise_seed_b, "noise_seed_b")?;
    let [a_bytes, b_bytes, _routing] = buffers;
    Ok(OpenedDenseWitness {
        candidate_generation: metadata.candidate_generation,
        work_id: metadata.work_id,
        a_row_indices: metadata.a_row_indices,
        extranonce: metadata.extranonce,
        b_column_indices: metadata.b_column_indices,
        noise_seed_a,
        noise_seed_b,
        noise_rank: metadata.noise_rank,
        a_rows: metadata.a_rows,
        b_columns: metadata.b_columns,
        common_dim: metadata.common_dim,
        a: Arc::new(bytes_into_i8(a_bytes)),
        b_transposed: Arc::new(bytes_into_i8(b_bytes)),
    })
}

fn validate_opened_indices(indices: &[u32], dimension: u32, field: &str) -> Result<()> {
    if indices.len() != GEMMA_TILE as usize {
        bail!("{field} must contain exactly {GEMMA_TILE} indices");
    }
    let start = indices[0];
    if !start.is_multiple_of(GEMMA_TILE) {
        bail!("{field} must start on a {GEMMA_TILE}-element tile boundary");
    }
    if start
        .checked_add(GEMMA_TILE)
        .is_none_or(|end| end > dimension)
    {
        bail!("{field} exceeds its tensor dimension");
    }
    if indices
        .iter()
        .copied()
        .enumerate()
        .any(|(offset, value)| value != start + offset as u32)
    {
        bail!("{field} must describe one contiguous tile");
    }
    Ok(())
}

fn checked_tensor_elements(rows: u32, columns: u32, field: &str) -> Result<usize> {
    let elements = (rows as u64)
        .checked_mul(columns as u64)
        .ok_or_else(|| anyhow::anyhow!("{field} tensor length overflow"))?;
    if elements > MAX_OPENED_TENSOR_BYTES {
        bail!("{field} tensor exceeds byte limit");
    }
    usize::try_from(elements).map_err(|_| anyhow::anyhow!("{field} tensor length overflow"))
}

fn validate_int7(bytes: &[u8], field: &str) -> Result<()> {
    if let Some((offset, value)) = bytes
        .iter()
        .copied()
        .map(|value| value as i8)
        .enumerate()
        .find(|(_, value)| !(-64..=63).contains(value))
    {
        bail!("{field} tensor byte {offset} has non-INT7 value {value}");
    }
    Ok(())
}

fn bytes_into_i8(mut bytes: Vec<u8>) -> Vec<i8> {
    let pointer = bytes.as_mut_ptr().cast::<i8>();
    let length = bytes.len();
    let capacity = bytes.capacity();
    std::mem::forget(bytes);
    // SAFETY: u8 and i8 have identical size and alignment. The original
    // allocation is transferred exactly once and keeps its length and capacity.
    unsafe { Vec::from_raw_parts(pointer, length, capacity) }
}

/// Build the fixed Gemma 4 mining statement sent to the inference runtime.
pub fn build_gemma4_mining_job(
    candidate_generation: u64,
    incomplete_header: [u8; PEARL_INCOMPLETE_BLOCK_HEADER_SIZE],
    nockchain_target: [u8; TARGET_BYTES],
    certificate_version: u32,
) -> Result<MiningJob> {
    let config = canonical_dense_mining_config(&GEMMA4_NATIVE_PARAMS)?;
    let mining_config = config
        .to_bytes()
        .map_err(|error| anyhow::anyhow!("encode Gemma mining config: {error:?}"))?;
    let work_factor = config
        .shape_work_factor()
        .map_err(|error| anyhow::anyhow!("derive Gemma work factor: {error:?}"))?;
    let effective_target =
        ai_pow::difficulty::effective_jackpot_threshold(&nockchain_target, work_factor)
            .map_err(|error| anyhow::anyhow!("adjust Gemma target: {error:?}"))?;
    let job = MiningJob {
        candidate_generation,
        incomplete_header: incomplete_header.to_vec(),
        effective_target_le: effective_target.to_vec(),
        certificate_version,
        mining_config: mining_config.to_vec(),
    };
    validate_mining_job(&job)?;
    Ok(job)
}

fn validate_mining_job(job: &MiningJob) -> Result<()> {
    if job.candidate_generation == 0 {
        bail!("candidate_generation must be nonzero");
    }
    if job.incomplete_header.len() != HEADER_BYTES {
        bail!("incomplete_header must be {HEADER_BYTES} bytes");
    }
    if job.effective_target_le.len() != TARGET_BYTES {
        bail!("effective_target_le must be {TARGET_BYTES} bytes");
    }
    if job.certificate_version == 0 {
        bail!("certificate_version must be nonzero");
    }
    if job.mining_config.len() != PEARL_MINING_CONFIG_SIZE {
        bail!("mining_config must be {PEARL_MINING_CONFIG_SIZE} bytes");
    }
    let config = PearlMiningConfig::from_bytes(&job.mining_config)
        .map_err(|error| anyhow::anyhow!("invalid mining_config: {error:?}"))?;
    let expected = canonical_dense_mining_config(&GEMMA4_NATIVE_PARAMS)?;
    if config != expected {
        bail!("mining_config does not match the canonical Gemma 4 profile");
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
        let mut target = [0u8; TARGET_BYTES];
        target[0] = 1;
        build_gemma4_mining_job(1, [0x11; HEADER_BYTES], target, 3).expect("canonical mining job")
    }

    #[test]
    fn gemma4_mining_job_bytes_are_canonical() {
        let job = mining_job();
        assert_eq!(
            hex::encode(job.mining_config),
            "0015000080000000000f00000000000f000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            hex::encode(job.effective_target_le),
            "0000150000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn gemma4_mining_job_rejects_unrepresentable_effective_target() {
        let error = build_gemma4_mining_job(1, [0x11; HEADER_BYTES], [u8::MAX; TARGET_BYTES], 3)
            .expect_err("over-band target must fail closed");
        assert!(error.to_string().contains("adjust Gemma target"));
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
        rpc.set_production_enabled(true);
        rpc.set_node_connected(true);
        let status = rpc
            .get_status(Request::new(GetStatusRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(status.mode, SchedulerMode::InferenceMining as i32);
        assert_eq!(status.active_work_items, 1);
        assert!(status.production_enabled);
        assert!(status.node_connected);
    }

    #[test]
    fn malformed_runtime_and_job_fields_reject() {
        let state = InferenceSchedulerState::default();
        let mut malformed = runtime_request();
        malformed.checkpoint_layout_digest.pop();
        assert!(state.register_runtime(malformed).is_err());

        let mut job = mining_job();
        job.effective_target_le.pop();
        assert!(InferenceMiningRpc::new(Arc::new(state), job).is_err());

        let mut job = mining_job();
        job.mining_config[9] = 7;
        assert!(
            InferenceMiningRpc::new(Arc::new(InferenceSchedulerState::default()), job).is_err()
        );
    }

    fn opened_metadata(output_dim: u32) -> OpenedBlockMetadata {
        OpenedBlockMetadata {
            runtime_id: vec![0x55; RUNTIME_ID_BYTES],
            candidate_generation: 1,
            work_id: 9,
            extranonce: 0,
            a_row_indices: (0..GEMMA_TILE).collect(),
            b_column_indices: (16..16 + GEMMA_TILE).collect(),
            noise_seed_a: vec![0x66; DIGEST_BYTES],
            noise_seed_b: vec![0x77; DIGEST_BYTES],
            noise_rank: GEMMA_NOISE_RANK,
            a_rows: GEMMA_ROW_ALIGNMENT,
            b_columns: output_dim,
            common_dim: GEMMA_COMMON_DIM,
        }
    }

    fn opened_buffers(output_dim: u32) -> [Vec<u8>; 3] {
        [
            vec![0; GEMMA_ROW_ALIGNMENT as usize * GEMMA_COMMON_DIM as usize],
            vec![0; output_dim as usize * GEMMA_COMMON_DIM as usize],
            Vec::new(),
        ]
    }

    #[test]
    fn dense_witness_reconstruction_enforces_shapes_and_preserves_signed_bytes() {
        const TEST_OUTPUT_DIM: u32 = 128;
        let mut buffers = opened_buffers(TEST_OUTPUT_DIM);
        buffers[0][0] = (-64i8) as u8;
        buffers[1][0] = 63;
        let witness = validate_opened_dense_witness_for_output(
            opened_metadata(TEST_OUTPUT_DIM),
            buffers,
            TEST_OUTPUT_DIM,
        )
        .unwrap();
        assert_eq!(witness.a[0], -64);
        assert_eq!(witness.b_transposed[0], 63);
        assert_eq!(witness.a_row_indices, (0..GEMMA_TILE).collect::<Vec<_>>());
        assert_eq!(
            witness.b_column_indices,
            (16..16 + GEMMA_TILE).collect::<Vec<_>>()
        );
    }

    #[test]
    fn dense_witness_reconstruction_rejects_malformed_inputs() {
        const TEST_OUTPUT_DIM: u32 = 128;

        let mut metadata = opened_metadata(TEST_OUTPUT_DIM);
        metadata.a_row_indices[3] = 99;
        assert!(validate_opened_dense_witness_for_output(
            metadata,
            opened_buffers(TEST_OUTPUT_DIM),
            TEST_OUTPUT_DIM,
        )
        .unwrap_err()
        .to_string()
        .contains("contiguous"));

        let mut buffers = opened_buffers(TEST_OUTPUT_DIM);
        buffers[1].pop();
        assert!(validate_opened_dense_witness_for_output(
            opened_metadata(TEST_OUTPUT_DIM),
            buffers,
            TEST_OUTPUT_DIM,
        )
        .unwrap_err()
        .to_string()
        .contains("does not match shape"));

        let mut buffers = opened_buffers(TEST_OUTPUT_DIM);
        buffers[0][0] = 64;
        assert!(validate_opened_dense_witness_for_output(
            opened_metadata(TEST_OUTPUT_DIM),
            buffers,
            TEST_OUTPUT_DIM,
        )
        .unwrap_err()
        .to_string()
        .contains("non-INT7"));

        let mut buffers = opened_buffers(TEST_OUTPUT_DIM);
        buffers[2].push(1);
        assert!(validate_opened_dense_witness_for_output(
            opened_metadata(TEST_OUTPUT_DIM),
            buffers,
            TEST_OUTPUT_DIM,
        )
        .unwrap_err()
        .to_string()
        .contains("routing"));
    }
}
