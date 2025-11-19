#![allow(dead_code)]
use std::any::Any;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use blake3::{Hash, Hasher};
use byteorder::{LittleEndian, WriteBytesExt};
use nockvm::hamt::Hamt;
use nockvm::interpreter::{self, interpret, Error, Mote, NockCancelToken};
use nockvm::jets::cold::{Cold, Nounable};
use nockvm::jets::hot::{HotEntry, URBIT_HOT_STATE};
use nockvm::jets::nock::util::mook;
use nockvm::mem::NockStack;
use nockvm::mug::met3_usize;
use nockvm::noun::{Atom, Cell, DirectAtom, IndirectAtom, Noun, Slots, D, T};
use nockvm::trace::{path_to_cord, write_serf_trace_safe};
use nockvm_macros::tas;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Duration;
use tracing::{debug, warn};

use crate::kernel::boot::TraceOpts;
use crate::metrics::NockAppMetrics;
use crate::nockapp::wire::{wire_to_noun, WireRepr};
use crate::noun::slab::NounSlab;
use crate::noun::slam;
use crate::save::SaveableCheckpoint;
use crate::utils::{
    create_context, current_da, NOCK_STACK_SIZE, NOCK_STACK_SIZE_HUGE, NOCK_STACK_SIZE_LARGE,
    NOCK_STACK_SIZE_MEDIUM, NOCK_STACK_SIZE_SMALL, NOCK_STACK_SIZE_TINY,
};
use crate::{AtomExt, CrownError, IndirectAtomExt, NounExt, Result, ToBytesExt};

pub(crate) const STATE_AXIS: u64 = 6;
const LOAD_AXIS: u64 = 4;
const PEEK_AXIS: u64 = 22;
const POKE_AXIS: u64 = 23;

const SERF_FINISHED_INTERVAL: Duration = Duration::from_millis(100);
const SERF_THREAD_STACK_SIZE: usize = 256 * 1024 * 1024; // 8MB

pub struct LoadState {
    pub ker_hash: Hash,
    pub event_num: u64,
    pub kernel_state: NounSlab,
}

// Actions to request of the serf thread
pub enum SerfAction<C> {
    // Make a CheckPoint
    Checkpoint {
        result: oneshot::Sender<C>,
    },
    Import {
        state: LoadState,
        result: oneshot::Sender<Result<()>>,
    },
    Export {
        result: oneshot::Sender<Result<LoadState>>,
    },
    // Get the state noun of the kernel as a slab
    GetKernelStateSlab {
        result: oneshot::Sender<Result<NounSlab>>,
    },
    // Get the cold state as a NounSlab
    GetColdStateSlab {
        result: oneshot::Sender<NounSlab>,
    },
    // Run a peek
    Peek {
        ovo: NounSlab,
        result: oneshot::Sender<Result<NounSlab>>,
    },
    // Run a poke
    //
    // TODO: send back the event number after each poke
    Poke {
        wire: WireRepr,
        cause: NounSlab,
        result: oneshot::Sender<Result<NounSlab>>,
        result_ack: oneshot::Receiver<()>,
    },
    // Provide metrics
    ProvideMetrics {
        metrics: Arc<NockAppMetrics>,
        result: oneshot::Sender<()>,
    },
    // Stop the loop
    Stop,
}

pub struct SerfThread<C> {
    handle: Option<std::thread::JoinHandle<()>>,
    action_sender: mpsc::Sender<SerfAction<C>>,
    pub cancel_token: NockCancelToken,
    inhibit: Arc<AtomicBool>,
    pub event_number: Arc<AtomicU64>,
}

impl<C: SerfCheckpoint + Send + 'static> SerfThread<C> {
    pub async fn new(
        kernel_bytes: Vec<u8>,
        checkpoint: Option<C>,
        constant_hot_state: Vec<HotEntry>,
        nock_stack_size: usize,
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let (action_sender, action_receiver) = mpsc::channel(1);
        let (event_number_sender, event_number_receiver) = oneshot::channel();
        let (cancel_token_sender, cancel_token_receiver) = oneshot::channel();
        let inhibit = Arc::new(AtomicBool::new(false));
        let inhibit_clone = inhibit.clone();
        let handle = std::thread::Builder::new()
            .name("serf".to_string())
            .stack_size(SERF_THREAD_STACK_SIZE)
            .spawn(move || {
                let stack = NockStack::new(nock_stack_size, 0);
                let serf = Serf::new(
                    stack, checkpoint, &kernel_bytes, &constant_hot_state, test_jets, trace,
                );
                event_number_sender
                    .send(serf.event_num.clone())
                    .expect("Could not send event number out of serf thread");
                cancel_token_sender
                    .send(serf.context.cancel_token())
                    .expect("Could not send cancel token out of serf thread");
                serf_loop(serf, action_receiver, inhibit_clone);
            })?;

        let event_number = event_number_receiver.await?;
        let cancel_token = cancel_token_receiver.await?;
        Ok(SerfThread {
            inhibit,
            handle: Some(handle),
            action_sender,
            event_number,
            cancel_token,
        })
    }
}

impl<C> SerfThread<C> {
    pub(crate) fn provide_metrics(
        &mut self,
        metrics: Arc<NockAppMetrics>,
    ) -> impl Future<Output = Result<()>> {
        let action_sender = self.action_sender.clone();
        let (result, result_recv) = oneshot::channel();
        async move {
            action_sender
                .send(SerfAction::ProvideMetrics { metrics, result })
                .await?;
            Ok(result_recv.await?)
        }
    }

    pub(crate) fn stop(&mut self) -> impl Future<Output = Result<()>> {
        let action_sender = self.action_sender.clone();
        let cancel_token = self.cancel_token.clone();
        let join_handle = self.handle.take().expect("Serf join handle already taken.");
        let tokio_join_handle = tokio::task::spawn_blocking(move || join_handle.join());
        self.inhibit.store(true, Ordering::SeqCst);
        async move {
            cancel_token.cancel();
            action_sender
                .send(SerfAction::Stop)
                .await
                .expect("Failed to send stop action");
            match tokio_join_handle.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(CrownError::Unknown(format!("Serf thread panicked: {e:?}"))),
                Err(e) => Err(CrownError::JoinError(e)),
            }
        }
    }

    pub(crate) fn join(&mut self) -> Result<(), Box<dyn Any + Send + 'static>> {
        self.handle
            .take()
            .expect("Serf thread already joined")
            .join()
    }

    pub(crate) async fn get_kernel_state_slab(&self) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        self.action_sender
            .send(SerfAction::GetKernelStateSlab { result })
            .await?;
        result_fut.await?
    }

    pub(crate) async fn get_cold_state_slab(&self) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        self.action_sender
            .send(SerfAction::GetColdStateSlab { result })
            .await?;
        Ok(result_fut.await?)
    }

    pub(crate) fn peek(&self, ovo: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender.send(SerfAction::Peek { ovo, result }).await?;
            result_fut.await?
        }
    }

    // We are very carefully ensuring that the future does not contain the &self reference, to allow spawning a task without lifetime issues
    pub fn poke(&self, wire: WireRepr, cause: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        let (result, result_fut) = oneshot::channel();
        let (result_ack_sender, result_ack) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender
                .send(SerfAction::Poke {
                    wire,
                    cause,
                    result,
                    result_ack,
                })
                .await?;
            let res = result_fut.await?;
            let _ = result_ack_sender.send(());
            res
        }
    }

    pub fn poke_timeout(
        &self,
        wire: WireRepr,
        cause: NounSlab,
        timeout: Duration,
    ) -> impl Future<Output = Result<NounSlab>> {
        let (result, result_fut) = oneshot::channel();
        let (result_ack_sender, result_ack) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        let cancel = self.cancel_token.clone();
        let timer = tokio::time::sleep(timeout);
        let cancel_task = tokio::spawn(async move {
            timer.await;
            cancel.cancel();
        });
        async move {
            action_sender
                .send(SerfAction::Poke {
                    wire,
                    cause,
                    result,
                    result_ack,
                })
                .await?;
            let res = result_fut.await?;
            cancel_task.abort();
            let _ = cancel_task.await;
            let _ = result_ack_sender.send(());
            res
        }
    }

    pub(crate) fn poke_sync(&self, wire: WireRepr, cause: NounSlab) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        let (result_ack_sender, result_ack) = oneshot::channel();
        self.action_sender.blocking_send(SerfAction::Poke {
            wire,
            cause,
            result,
            result_ack,
        })?;
        let res = result_fut.blocking_recv()?;
        let _ = result_ack_sender.send(());
        res
    }

    pub(crate) fn peek_sync(&self, ovo: NounSlab) -> Result<NounSlab> {
        let (result, result_fut) = oneshot::channel();
        self.action_sender
            .blocking_send(SerfAction::Peek { ovo, result })?;
        result_fut.blocking_recv()?
    }

    pub(crate) fn checkpoint(&self) -> impl Future<Output = Result<C>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender
                .send(SerfAction::Checkpoint { result })
                .await?;
            Ok(result_fut.await?)
        }
    }

    pub fn import(&self, state: LoadState) -> impl Future<Output = Result<()>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender
                .send(SerfAction::Import { state, result })
                .await?;
            result_fut.await?
        }
    }

    pub fn export(&self) -> impl Future<Output = Result<LoadState>> {
        let (result, result_fut) = oneshot::channel();
        let action_sender = self.action_sender.clone();
        async move {
            action_sender.send(SerfAction::Export { result }).await?;
            result_fut.await?
        }
    }
}

fn serf_loop<C: SerfCheckpoint>(
    mut serf: Serf,
    mut action_receiver: mpsc::Receiver<SerfAction<C>>,
    inhibit: Arc<AtomicBool>,
) {
    loop {
        let start = std::time::Instant::now();
        let Some(action) = action_receiver.blocking_recv() else {
            break;
        };
        let recv_elapsed = start.elapsed();
        if let Some(nockapp_metrics) = &serf.metrics {
            nockapp_metrics
                .serf_loop_blocking_recv
                .add_timing(&recv_elapsed);
        };
        let action_start = std::time::Instant::now();
        match action {
            SerfAction::Stop => {
                break;
            }
            SerfAction::Export { result } => {
                let kernel_state_noun = serf.arvo.slot(STATE_AXIS);
                let kernel_state = kernel_state_noun.map_or_else(
                    |err| Err(CrownError::from(err)),
                    |noun| {
                        let mut slab = NounSlab::new();
                        slab.copy_into(noun);
                        Ok(slab)
                    },
                );
                let load_state = kernel_state.map(|kernel_state| LoadState {
                    kernel_state,
                    ker_hash: serf.ker_hash,
                    event_num: serf.event_num.load(Ordering::SeqCst),
                });
                let _ = result.send(load_state).inspect_err(|_err| {
                    debug!("Failed to send to dropped channel");
                });
            }
            SerfAction::Import { state, result } => {
                let state_noun = state.kernel_state.copy_to_stack(serf.stack());
                let arvo = serf.load(state_noun);
                match arvo {
                    Err(e) => {
                        let _ = result.send(Err(e)).map_err(|err| {
                            debug!("Tried to send to dropped channel: {:?}", err);
                        });
                    }
                    Ok(arvo) => {
                        if serf.ker_hash != state.ker_hash {
                            debug!(
                                "Importing state from kernel hash {} into kernel hash {}",
                                state.ker_hash, serf.ker_hash
                            );
                        }
                        unsafe {
                            serf.event_update(state.event_num, arvo);
                            serf.preserve_event_update_leftovers();
                        }
                        let _ = result.send(Ok(())).map_err(|err| {
                            debug!("Tried to send to dropped channel: {:?}", err);
                        });
                    }
                }
            }
            SerfAction::GetKernelStateSlab { result } => {
                let kernel_state_noun = serf.arvo.slot(STATE_AXIS);
                let kernel_state_slab = kernel_state_noun.map_or_else(
                    |err| Err(CrownError::from(err)),
                    |noun| {
                        let mut slab = NounSlab::new();
                        slab.copy_into(noun);
                        Ok(slab)
                    },
                );
                let _ = result.send(kernel_state_slab).inspect_err(|_e| {
                    debug!("Tried to send to dropped result channel");
                });
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_get_kernel_state_slab
                        .add_timing(&action_elapsed);
                };
            }
            SerfAction::GetColdStateSlab { result } => {
                let cold_state_noun = serf.context.cold.into_noun(serf.stack());
                let cold_state_slab = {
                    let mut slab = NounSlab::new();
                    slab.copy_into(cold_state_noun);
                    slab
                };
                let _ = result.send(cold_state_slab).inspect_err(|_e| {
                    debug!("Could not send cold state to dropped channel.");
                });
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_get_cold_state_slab
                        .add_timing(&action_elapsed);
                };
            }
            SerfAction::Checkpoint { result } => {
                let metrics_checkpoint = serf.metrics.clone();
                let checkpoint = create_checkpoint(&mut serf, &metrics_checkpoint);
                //result.send(checkpoint).expect("Could not send checkpoint");
                if result.send(checkpoint).is_err() {
                    debug!(
                        "Checkpoint receiver dropped before receiving result - likely timed out"
                    );
                };
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_checkpoint
                        .add_timing(&action_elapsed);
                };
            }
            SerfAction::Peek { ovo, result } => {
                if inhibit.load(Ordering::SeqCst) {
                    let _ = result
                        .send(Err(CrownError::Unknown("Serf stopping".to_string())))
                        .inspect_err(|_e| {
                            debug!("Tried to send inhibited peek state to dropped channel");
                        });
                } else {
                    let ovo_noun = ovo.copy_to_stack(serf.stack());
                    let noun_res = serf.peek(ovo_noun);
                    let noun_slab_res = noun_res.map(|noun| {
                        let mut slab = NounSlab::new();
                        slab.copy_into(noun);
                        slab
                    });
                    let _ = result.send(noun_slab_res).inspect_err(|_e| {
                        debug!("Tried to send peek state to dropped channel");
                    });
                };
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics.serf_loop_peek.add_timing(&action_elapsed);
                };
            }
            SerfAction::Poke {
                wire,
                cause,
                result,
                result_ack,
            } => {
                if inhibit.load(Ordering::SeqCst) {
                    let _ = result
                        .send(Err(CrownError::Unknown("Serf stopping".to_string())))
                        .inspect_err(|_e| {
                            debug!("Failed to send inihibited poke result from serf thread");
                        });
                } else {
                    let cause_noun = cause.copy_to_stack(serf.stack());
                    let noun_res = serf.poke(wire, cause_noun);
                    let noun_slab_res = noun_res.map(|noun| {
                        let mut slab = NounSlab::new();
                        slab.copy_into(noun);
                        slab
                    });
                    let _ = result.send(noun_slab_res).inspect_err(|_e| {
                        debug!("Failed to send poke result from serf thread");
                    });
                };
                let _ = result_ack.blocking_recv().inspect_err(|_e| {
                    debug!("Failed to receive result ack in serf thread");
                });
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics.serf_loop_poke.add_timing(&action_elapsed);
                };
            }
            SerfAction::ProvideMetrics { metrics, result } => {
                serf.metrics = Some(metrics);
                let _ = result.send(()).inspect_err(|_e| {
                    debug!("Failed to send metric-provision result from serf thread");
                });
                let action_elapsed = action_start.elapsed();
                if let Some(nockapp_metrics) = &serf.metrics {
                    nockapp_metrics
                        .serf_loop_provide_metrics
                        .add_timing(&action_elapsed);
                };
            }
        };
        let elapsed = start.elapsed();
        if let Some(nockapp_metrics) = &serf.metrics {
            nockapp_metrics.serf_loop_all.add_timing(&elapsed);
        };
    }
}

fn create_checkpoint<C: SerfCheckpoint>(
    serf: &mut Serf,
    metrics: &Option<Arc<NockAppMetrics>>,
) -> C {
    let ker_hash = serf.ker_hash;
    let event_num = serf.event_num.load(Ordering::SeqCst);
    let ker_state = serf.arvo.slot(STATE_AXIS).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    let cold_state = serf.context.cold;

    C::new(
        serf.stack(),
        ker_hash,
        event_num,
        ker_state,
        cold_state,
        metrics,
    )
}

/// Represents a Sword kernel, containing a Serf and snapshot location.
pub struct Kernel<C> {
    /// The Serf managing the interface to the Sword.
    pub(crate) serf: SerfThread<C>,
}

impl<C: SerfCheckpoint + 'static> Kernel<C> {
    /// Loads a kernel with a custom hot state.
    ///
    /// # Arguments
    ///
    /// * `snap_dir` - Directory for storing snapshots.
    /// * `kernel` - Byte slice containing the kernel as a jammed noun.
    /// * `hot_state` - Custom hot state entries.
    /// * `trace` - Whether to enable tracing.
    ///
    /// # Returns
    ///
    /// A new `Kernel` instance.
    pub async fn load_with_hot_state(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE, test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_tiny(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_TINY, test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_small(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_SMALL, test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_medium(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_MEDIUM, test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_large(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_LARGE, test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    pub async fn load_with_hot_state_huge(
        kernel: &[u8],
        checkpoint: Option<C>,
        hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        let kernel_vec = Vec::from(kernel);
        let hot_state_vec = Vec::from(hot_state);
        let serf = SerfThread::new(
            kernel_vec, checkpoint, hot_state_vec, NOCK_STACK_SIZE_HUGE, test_jets, trace,
        )
        .await?;
        Ok(Self { serf })
    }

    /// Loads a kernel with default hot state.
    ///
    /// # Arguments
    ///
    /// * `snap_dir` - Directory for storing snapshots.
    /// * `kernel` - Byte slice containing the kernel code.
    /// * `trace` - Whether to enable tracing.
    ///
    /// # Returns
    ///
    /// A new `Kernel` instance.
    pub async fn load(
        kernel: &[u8],
        checkpoint: Option<C>,
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Result<Self> {
        Self::load_with_hot_state(kernel, checkpoint, &Vec::new(), test_jets, trace).await
    }

    /// Produces a checkpoint of the kernel state.
    pub fn checkpoint(&self) -> impl Future<Output = Result<C>> {
        self.serf.checkpoint()
    }
}

impl<C> Kernel<C> {
    // We are very carefully ensuring the future does not contain the "self" reference to ensure no lifetime issues when spawning tasks
    pub fn poke(&self, wire: WireRepr, cause: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        self.serf.poke(wire, cause)
    }

    pub fn poke_sync(&self, wire: WireRepr, cause: NounSlab) -> Result<NounSlab> {
        self.serf.poke_sync(wire, cause)
    }

    pub fn peek_sync(&self, ovo: NounSlab) -> Result<NounSlab> {
        self.serf.peek_sync(ovo)
    }

    pub fn poke_timeout(
        &self,
        wire: WireRepr,
        cause: NounSlab,
        timeout: Duration,
    ) -> impl Future<Output = Result<NounSlab>> {
        self.serf.poke_timeout(wire, cause, timeout)
    }

    // We are very carefully ensuring the future does not contain the "self" reference to ensure no lifetime issues when spawning tasks
    #[tracing::instrument(name = "crown::Kernel::peek", skip_all)]
    pub(crate) fn peek(&self, ovo: NounSlab) -> impl Future<Output = Result<NounSlab>> {
        self.serf.peek(ovo)
    }

    pub fn import(&self, state: LoadState) -> impl Future<Output = Result<()>> {
        self.serf.import(state)
    }

    pub fn export(&self) -> impl Future<Output = Result<LoadState>> {
        self.serf.export()
    }

    pub(crate) fn provide_metrics(
        &mut self,
        metrics: Arc<NockAppMetrics>,
    ) -> impl Future<Output = Result<()>> {
        self.serf.provide_metrics(metrics)
    }
}

/// Represents the Serf, which maintains context and provides an interface to
/// the Sword.
pub struct Serf {
    /// Hash of boot kernel
    pub ker_hash: Hash,
    /// The current Arvo state.
    pub arvo: Noun,
    /// The interpreter context.
    pub context: interpreter::Context,
    /// Cancellation
    pub cancel_token: NockCancelToken,
    /// The current event number.
    pub event_num: Arc<AtomicU64>,
    /// A metrics
    pub metrics: Option<Arc<NockAppMetrics>>,
}

impl Serf {
    /// Creates a new Serf instance.
    ///
    /// # Arguments
    ///
    /// * `stack` - The Nock stack.
    /// * `checkpoint` - Optional checkpoint to restore from.
    /// * `kernel_bytes` - Byte slice containing the kernel code.
    /// * `constant_hot_state` - Custom hot state entries.
    /// * `trace_info` - Optional nockvm tracing implementation.
    ///
    /// # Returns
    ///
    /// A new `Serf` instance.
    fn new<C: SerfCheckpoint>(
        mut stack: NockStack,
        checkpoint: Option<C>,
        kernel_bytes: &[u8],
        constant_hot_state: &[HotEntry],
        test_jets: Vec<NounSlab>,
        trace: TraceOpts,
    ) -> Self {
        let hot_state = [URBIT_HOT_STATE, constant_hot_state].concat();

        let mut hasher = Hasher::new();
        hasher.update(kernel_bytes);
        let ker_hash = hasher.finalize();

        let (maybe_state, cold, event_num_raw) = if let Some(c) = checkpoint {
            let saveable = c.load();

            let ker_state = saveable.state.copy_to_stack(&mut stack);
            let cold_noun = saveable.cold.copy_to_stack(&mut stack);
            let cold_vecs = Cold::from_noun(&mut stack, &cold_noun)
                .expect("Could not load cold state from snapshot");
            let cold = Cold::from_vecs(&mut stack, cold_vecs.0, cold_vecs.1, cold_vecs.2);
            if saveable.ker_hash != ker_hash {
                debug!(
                    "Loading snapshot from kernel {} into kernel {}",
                    saveable.ker_hash, ker_hash
                );
            }
            (Some(ker_state), cold, saveable.event_num)
        } else {
            (None, Cold::new(&mut stack), 0)
        };

        let event_num = Arc::new(AtomicU64::new(event_num_raw));

        let mut context = create_context(stack, &hot_state, cold, trace.into(), test_jets);
        let cancel_token = context.cancel_token();

        let mut arvo = {
            let kernel_trap = Noun::cue_bytes_slice(&mut context.stack, kernel_bytes)
                .expect("invalid kernel jam");
            let fol = T(&mut context.stack, &[D(9), D(2), D(0), D(1)]);

            if context.trace_info.is_some() {
                let start = Instant::now();
                let arvo = interpret(&mut context, kernel_trap, fol).unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                write_serf_trace_safe(&mut context, "boot", start);
                arvo
            } else {
                interpret(&mut context, kernel_trap, fol).unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                })
            }
        };

        let mut serf = Self {
            ker_hash,
            arvo,
            context,
            event_num,
            cancel_token,
            metrics: None,
        };

        if let Some(kernel_state) = maybe_state {
            arvo = serf.load(kernel_state).expect("serf: load failed");
        }

        unsafe {
            serf.event_update(event_num_raw, arvo);
            serf.preserve_event_update_leftovers();
        }
        serf
    }

    /// Performs a peek operation on the Arvo state.
    ///
    /// # Arguments
    ///
    /// * `ovo` - The peek request noun.
    ///
    /// # Returns
    ///
    /// Result containing the peeked data or an error.
    #[tracing::instrument(skip_all)]
    pub fn peek(&mut self, ovo: Noun) -> Result<Noun> {
        if self.context.trace_info.is_some() {
            let trace_name = "peek";
            let start = Instant::now();
            let slam_res = self.slam(PEEK_AXIS, ovo);
            write_serf_trace_safe(&mut self.context, trace_name, start);

            slam_res
        } else {
            self.slam(PEEK_AXIS, ovo)
        }
    }

    /// Generates a goof (error) noun.
    ///
    /// # Arguments
    ///
    /// * `mote` - The error mote.
    /// * `traces` - Trace information.
    ///
    /// # Returns
    ///
    /// A noun representing the error.
    pub fn goof(&mut self, mote: Mote, traces: Noun) -> Noun {
        let tone = Cell::new(&mut self.context.stack, D(2), traces);
        let tang = mook(&mut self.context, tone, false)
            .expect("serf: goof: +mook crashed on bail")
            .tail();
        T(&mut self.context.stack, &[D(mote as u64), tang])
    }

    /// Performs a load operation on the Arvo state.
    ///
    /// # Arguments
    ///
    /// * `old` - The state to load.
    ///
    /// # Returns
    ///
    /// Result containing the loaded kernel or an error.
    pub fn load(&mut self, old: Noun) -> Result<Noun> {
        match self.soft(old, LOAD_AXIS, Some("load".to_string())) {
            Ok(res) => Ok(res),
            Err(goof) => {
                self.print_goof(goof);
                Err(CrownError::SerfLoadError)
            }
        }
    }

    pub fn print_goof(&mut self, goof: Noun) {
        let tang = goof
            .as_cell()
            .expect("print goof: expected goof to be a cell")
            .tail();
        tang.list_iter().for_each(|tank: Noun| {
            //  TODO: Slogger should be emitting Results in case of failure
            self.context.slogger.slog(&mut self.context.stack, 1, tank);
        });
    }

    /// Performs a poke operation on the Arvo state.
    ///
    /// # Arguments
    ///
    /// * `job` - The poke job noun.
    ///
    /// # Returns
    ///
    /// Result containing the poke response or an error.
    #[tracing::instrument(level = "info", skip_all)]
    pub fn do_poke(&mut self, job: Noun) -> Result<Noun> {
        match self.soft(job, POKE_AXIS, Some("poke".to_string())) {
            Ok(res) => {
                let cell = res.as_cell().expect("serf: poke: +slam returned atom");
                let mut fec = cell.head();
                let eve = self.event_num.load(Ordering::SeqCst);

                unsafe {
                    self.event_update(eve + 1, cell.tail());
                    self.stack().preserve(&mut fec);
                    self.preserve_event_update_leftovers();
                }
                Ok(fec)
            }
            Err(goof) => self.poke_swap(job, goof),
        }
    }

    /// Slams (applies) a gate at a specific axis of Arvo.
    ///
    /// # Arguments
    ///
    /// * `axis` - The axis to slam.
    /// * `ovo` - The sample noun.
    ///
    /// # Returns
    ///
    /// Result containing the slammed result or an error.
    pub fn slam(&mut self, axis: u64, ovo: Noun) -> Result<Noun> {
        let arvo = self.arvo;
        slam(&mut self.context, arvo, axis, ovo, self.metrics.clone())
    }

    /// Performs a "soft" computation, handling errors gracefully.
    ///
    /// # Arguments
    ///
    /// * `ovo` - The input noun.
    /// * `axis` - The axis to slam.
    /// * `trace_name` - Optional name for tracing.
    ///
    /// # Returns
    ///
    /// Result containing the computed noun or an error noun.
    fn soft(&mut self, ovo: Noun, axis: u64, trace_name: Option<String>) -> Result<Noun, Noun> {
        let slam_res = if self.context.trace_info.is_some() {
            let start = Instant::now();
            let slam_res = self.slam(axis, ovo);
            write_serf_trace_safe(
                &mut self.context,
                trace_name.as_ref().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                }),
                start,
            );

            slam_res
        } else {
            self.slam(axis, ovo)
        };

        match slam_res {
            Ok(res) => Ok(res),
            Err(error) => match error {
                CrownError::InterpreterError(e) => {
                    let (mote, traces) = match e.0 {
                        Error::Deterministic(mote, traces)
                        | Error::NonDeterministic(mote, traces) => (mote, traces),
                        Error::ScryBlocked(_) | Error::ScryCrashed(_) => {
                            panic!("serf: soft: .^ invalid outside of virtual Nock")
                        }
                    };

                    Err(self.goof(mote, traces))
                }
                _ => Err(D(0)),
            },
        }
    }

    /// Plays a list of events.
    ///
    /// # Arguments
    ///
    /// * `lit` - The list of events to play.
    ///
    /// # Returns
    ///
    /// Result containing the final Arvo state or an error.
    fn play_list(&mut self, mut lit: Noun) -> Result<Noun> {
        let mut eve = self.event_num.load(Ordering::SeqCst);
        while let Ok(cell) = lit.as_cell() {
            let ovo = cell.head();
            lit = cell.tail();
            let trace_name = if self.context.trace_info.is_some() {
                Some(format!("play [{}]", eve))
            } else {
                None
            };

            match self.soft(ovo, POKE_AXIS, trace_name) {
                Ok(res) => {
                    let arvo = res.as_cell()?.tail();
                    eve += 1;

                    unsafe {
                        self.event_update(eve, arvo);
                        self.context.stack.preserve(&mut lit);
                        self.preserve_event_update_leftovers();
                    }
                }
                Err(goof) => {
                    return Err(CrownError::KernelError(Some(goof)));
                }
            }
        }
        Ok(self.arvo)
    }

    /// Handles a poke error by swapping in a new event.
    ///
    /// # Arguments
    ///
    /// * `job` - The original poke job.
    /// * `goof` - The error noun.
    ///
    /// # Returns
    ///
    /// Result containing the new event or an error.
    fn poke_swap(&mut self, job: Noun, goof: Noun) -> Result<Noun> {
        let stack = &mut self.context.stack;
        self.context.cache = Hamt::<Noun>::new(stack);
        let job_cell = job.as_cell().expect("serf: poke: job not a cell");
        // job data is job without event_num
        let job_data = job_cell
            .tail()
            .as_cell()
            .expect("serf: poke: data not a cell");
        //  job input is job without event_num or wire
        let job_input = job_data.tail();
        let wire = T(stack, &[D(0), D(tas!(b"arvo")), D(0)]);
        let crud = DirectAtom::new_panic(tas!(b"crud"));
        let event_num = D(self.event_num.load(Ordering::SeqCst) + 1);

        let mut ovo = T(stack, &[event_num, wire, goof, job_input]);
        let trace_name = if self.context.trace_info.is_some() {
            Some(Self::poke_trace_name(
                &mut self.context.stack,
                wire,
                crud.as_atom(),
            ))
        } else {
            None
        };

        match self.soft(ovo, POKE_AXIS, trace_name) {
            Ok(res) => {
                let cell = res.as_cell().expect("serf: poke: crud +slam returned atom");
                let mut fec = cell.head();
                let eve = self.event_num.load(Ordering::SeqCst);

                unsafe {
                    self.event_update(eve + 1, cell.tail());
                    self.context.stack.preserve(&mut ovo);
                    self.context.stack.preserve(&mut fec);
                    self.preserve_event_update_leftovers();
                }
                Ok(fec)
            }
            Err(goof_crud) => Err(CrownError::KernelError(Some(goof_crud))),
        }
    }

    /// Generates a trace name for a poke operation.
    ///
    /// # Arguments
    ///
    /// * `stack` - The Nock stack.
    /// * `wire` - The wire noun.
    /// * `vent` - The vent atom.
    ///
    /// # Returns
    ///
    /// A string representing the trace name.
    fn poke_trace_name(stack: &mut NockStack, wire: Noun, vent: Atom) -> String {
        let wpc = path_to_cord(stack, wire);
        let wpc_len = met3_usize(wpc);
        let wpc_bytes = &wpc.as_ne_bytes()[0..wpc_len];
        let wpc_str = match std::str::from_utf8(wpc_bytes) {
            Ok(valid) => valid,
            Err(error) => {
                let (valid, _) = wpc_bytes.split_at(error.valid_up_to());
                unsafe { std::str::from_utf8_unchecked(valid) }
            }
        };

        let vc_len = met3_usize(vent);
        let vc_bytes = &vent.as_ne_bytes()[0..vc_len];
        let vc_str = match std::str::from_utf8(vc_bytes) {
            Ok(valid) => valid,
            Err(error) => {
                let (valid, _) = vc_bytes.split_at(error.valid_up_to());
                unsafe { std::str::from_utf8_unchecked(valid) }
            }
        };

        format!("poke [{} {}]", wpc_str, vc_str)
    }

    /// Performs a poke operation with a given cause.
    ///
    /// # Arguments
    ///
    /// * `wire` - The wire noun.
    /// * `cause` - The cause noun.
    ///
    /// # Returns
    ///
    /// Result containing the poke response or an error.
    #[tracing::instrument(level = "info", skip_all, fields(
        src = wire.source
    ))]
    pub fn poke(&mut self, wire: WireRepr, cause: Noun) -> Result<Noun> {
        let random_bytes = rand::random::<u64>();
        let bytes = random_bytes.as_bytes()?;
        let eny: Atom = Atom::from_bytes(&mut self.context.stack, &bytes);
        let our = <nockvm::noun::Atom as AtomExt>::from_value(&mut self.context.stack, 0)?; // Using 0 as default value
        let mut t_vec: Vec<u8> = vec![];
        t_vec.write_u128::<LittleEndian>(current_da().0)?;
        let now: Atom = <IndirectAtom as IndirectAtomExt>::from_bytes(
            &mut self.context.stack,
            t_vec.as_slice(),
        );

        let event_num = D(self.event_num.load(Ordering::SeqCst) + 1);
        let base_wire_noun = wire_to_noun(&mut self.context.stack, &wire);
        let wire = T(&mut self.context.stack, &[D(tas!(b"poke")), base_wire_noun]);
        let poke = T(
            &mut self.context.stack,
            &[event_num, wire, eny.as_noun(), our.as_noun(), now.as_noun(), cause],
        );

        self.do_poke(poke)
    }

    /// Updates the Serf's state after an event.
    ///
    /// # Arguments
    ///
    /// * `new_event_num` - The new event number.
    /// * `new_arvo` - The new Arvo state.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it modifies the Serf's state directly.
    #[tracing::instrument(level = "info", skip_all)]
    pub unsafe fn event_update(&mut self, new_event_num: u64, new_arvo: Noun) {
        self.arvo = new_arvo;
        self.event_num.store(new_event_num, Ordering::SeqCst);

        self.context.cache = Hamt::new(&mut self.context.stack);
        self.context.scry_stack = D(0);
    }

    /// Preserves leftovers after an event update.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it modifies the Serf's state directly.
    #[tracing::instrument(level = "info", skip_all)]
    pub unsafe fn preserve_event_update_leftovers(&mut self) {
        let stack = &mut self.context.stack;
        stack.preserve(&mut self.context.warm);
        stack.preserve(&mut self.context.test_jets);
        stack.preserve(&mut self.context.hot);
        stack.preserve(&mut self.context.cache);
        stack.preserve(&mut self.context.cold);
        stack.preserve(&mut self.arvo);
        stack.flip_top_frame(0);
    }

    /// Returns a mutable reference to the Nock stack.
    ///
    /// # Returns
    ///
    /// A mutable reference to the `NockStack`.
    pub fn stack(&mut self) -> &mut NockStack {
        &mut self.context.stack
    }

    /// Creates a poke swap noun.
    ///
    /// # Arguments
    ///
    /// * `eve` - The event number.
    /// * `mug` - The mug value.
    /// * `ovo` - The original noun.
    /// * `fec` - The effect noun.
    ///
    /// # Returns
    ///
    /// A noun representing the poke swap.
    pub fn poke_bail(&mut self, eve: u64, mug: u64, ovo: Noun, fec: Noun) -> Noun {
        T(
            self.stack(),
            &[D(tas!(b"poke")), D(tas!(b"swap")), D(eve), D(mug), ovo, fec],
        )
    }

    /// Creates a poke bail noun.
    ///
    /// # Arguments
    ///
    /// * `lud` - The lud noun.
    ///
    /// # Returns
    ///
    /// A noun representing the poke bail.
    pub fn poke_bail_noun(&mut self, lud: Noun) -> Noun {
        T(self.stack(), &[D(tas!(b"poke")), D(tas!(b"bail")), lud])
    }
}

fn slot(noun: Noun, axis: u64) -> Result<Noun> {
    Ok(noun.slot(axis)?)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    async fn setup_kernel(jam: &str) -> Kernel<SaveableCheckpoint> {
        let jam_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("assets")
            .join(jam);
        let jam_bytes =
            fs::read(jam_path).unwrap_or_else(|_| panic!("Failed to read {} file", jam));
        Kernel::load(&jam_bytes, None, vec![], TraceOpts::default())
            .await
            .expect("Could not load kernel")
    }

    // Convert this to an integration test and feed it the kernel.jam from Choo in CI/CD
    // https://www.youtube.com/watch?v=4m1EFMoRFvY
    // #[test]
    // #[cfg_attr(miri, ignore)]
    // fn test_kernel_boot() {
    //     let _ = setup_kernel("dumb.jam");
    // }

    // To test your own kernel, place a `kernel.jam` file in the `assets` directory
    // and uncomment the following test:
    //
    // #[test]
    // fn test_custom_kernel() {
    //     let (kernel, _temp_dir) = setup_kernel("kernel.jam");
    //     // Add your custom assertions here to test the kernel's behavior
    // }
}

pub trait SerfCheckpoint: Send {
    fn new(
        stack: &mut NockStack,
        ker_hash: Hash,
        event_num: u64,
        kernel_state: Noun,
        cold_state: Cold,
        metrics: &Option<Arc<NockAppMetrics>>,
    ) -> Self;

    fn load(self) -> SaveableCheckpoint;
}

impl SerfCheckpoint for SaveableCheckpoint {
    fn new(
        stack: &mut NockStack,
        ker_hash: Hash,
        event_num: u64,
        kernel_state: Noun,
        cold_state: Cold,
        metrics: &Option<Arc<NockAppMetrics>>,
    ) -> Self {
        let cold_noun_start = Instant::now();
        // Cold state has nouns in it which are *not* copied in into_noun
        // TODO: FIX THIS FOOTGUN
        let cold_stack_noun = cold_state.into_noun(stack);
        let mut cold_slab: NounSlab = NounSlab::new();
        let cold_copy = cold_slab.copy_into(cold_stack_noun);
        cold_slab.set_root(cold_copy);
        let cold_noun_elapsed = cold_noun_start.elapsed();

        let state_copy_start = Instant::now();
        let mut state_slab: NounSlab = NounSlab::new();
        let state_copy = state_slab.copy_into(kernel_state);
        state_slab.set_root(state_copy);
        let state_copy_elapsed = state_copy_start.elapsed();

        if let Some(metrics) = metrics {
            metrics
                .serf_loop_noun_encode_cold_state
                .add_timing(&cold_noun_elapsed);
            metrics
                .serf_loop_copy_state_noun
                .add_timing(&state_copy_elapsed);
        }
        Self {
            ker_hash,
            event_num,
            state: state_slab,
            cold: cold_slab,
        }
    }

    fn load(self) -> SaveableCheckpoint {
        self
    }
}
