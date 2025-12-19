// pub(crate) mod actors;
pub mod driver;
pub mod error;
pub mod export;
pub(crate) mod metrics;
pub mod save;
pub mod test;
pub mod wire;

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use driver::{IOAction, IODriverFn, NockAppHandle, PokeResult};
pub use error::NockAppError;
use futures::future::{pending, Either};
use futures::stream::StreamExt;
use metrics::*;
use nockvm::noun::SIG;
use signal_hook::consts::signal::*;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook_tokio::Signals;
use tokio::select;
use tokio::sync::{broadcast, mpsc, Mutex, OwnedMutexGuard};
use tokio::time::{interval_at, Duration, Instant, Interval};
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, instrument, trace, warn};
use wire::WireRepr;

use crate::kernel::form::Kernel;
use crate::noun::slab::{Jammer, NockJammer, NounSlab};
use crate::save::{SaveableCheckpoint, Saver};

type NockAppResult = Result<(), NockAppError>;

// Error code constants for process exit and signal handling
// These numbers correspond to the standard Unix-style exit codes
// Exit code = 128 + signal number
/// Clean exit, no error
pub const EXIT_OK: usize = 0;
/// Unknown signal or error
pub const EXIT_UNKNOWN: usize = 1;
/// SIGHUP: Terminal closed or controlling process died
pub const EXIT_SIGHUP: usize = 129;
/// SIGINT: Keyboard interrupt (C-c)
pub const EXIT_SIGINT: usize = 130;
/// SIGQUIT: Quit from keyboard (core dump)
pub const EXIT_SIGQUIT: usize = 131;
/// SIGTERM: Termination signal from OS or process manager
pub const EXIT_SIGTERM: usize = 143;

pub struct NockApp<J = NockJammer> {
    /// Nock kernel
    pub(crate) kernel: Kernel<SaveableCheckpoint>,
    /// Current join handles for IO drivers (parallel to `drivers`)
    pub(crate) tasks: tokio_util::task::TaskTracker,
    /// Exit state object
    exit: NockAppExit,
    /// Exit state receiver
    exit_recv: tokio::sync::mpsc::Receiver<NockAppExitStatus>,
    /// Exit status
    exit_status: AtomicBool,
    /// Abort immediately on signal
    abort_immediately: AtomicBool,
    /// IO action channel
    action_channel: mpsc::Receiver<IOAction>,
    /// IO action channel sender
    action_channel_sender: mpsc::Sender<IOAction>,
    /// Effect broadcast channel
    effect_broadcast: Arc<broadcast::Sender<NounSlab>>,
    /// Save interval
    save_interval: Option<Interval>,
    /// Mutex to ensure only one save at a time
    pub(crate) save_mutex: Arc<Mutex<Saver<J>>>,
    metrics: Arc<NockAppMetrics>,
    /// Signals handled by the work loop
    signals: Signals,
}

pub enum NockAppRun {
    Pending,
    Done,
}

pub enum NockAppExitStatus {
    Exit(usize),
    Shutdown(NockAppResult),
    Done(NockAppResult),
}

#[derive(Clone)]
pub struct NockAppExit {
    sender: tokio::sync::mpsc::Sender<NockAppExitStatus>,
}

impl NockAppExit {
    pub fn new() -> (Self, tokio::sync::mpsc::Receiver<NockAppExitStatus>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        (NockAppExit { sender }, receiver)
    }

    pub fn exit(&self, code: usize) -> impl std::future::Future<Output = NockAppResult> {
        trace!("NockAppExit exit()");
        let sender = self.sender.clone();
        async move {
            sender
                .send(NockAppExitStatus::Exit(code))
                .await
                .map_err(|_| NockAppError::ChannelClosedError)?;
            Ok(())
        }
    }

    fn shutdown(&self, res: NockAppResult) -> impl Future<Output = NockAppResult> {
        trace!("NockAppExit shutdown()");
        let sender = self.sender.clone();
        async move {
            sender
                .send(NockAppExitStatus::Shutdown(res))
                .await
                .map_err(|_| NockAppError::ChannelClosedError)?;
            Ok(())
        }
    }

    fn done(&self, res: NockAppResult) -> impl Future<Output = NockAppResult> {
        trace!("NockAppExit done()");
        let sender = self.sender.clone();
        async move {
            sender
                .send(NockAppExitStatus::Done(res))
                .await
                .map_err(|_| NockAppError::ChannelClosedError)?;
            Ok(())
        }
    }
}

impl<J: Jammer + Send + 'static> NockApp<J> {
    /// This constructs a Tokio interval, even though it doesn't look like it, a Tokio runtime is _required_.
    pub async fn new<F, U, E>(
        kernel_from_checkpoint: F,
        snapshot_path: &PathBuf,
        save_interval_duration: Option<Duration>,
    ) -> Result<Self, NockAppError>
    where
        F: FnOnce(Option<SaveableCheckpoint>) -> U,
        U: Future<Output = Result<Kernel<SaveableCheckpoint>, E>>,
        NockAppError: From<E>,
    {
        // let cancel_token = tokio_util::sync::CancellationToken::new();
        let metrics = Arc::new(
            NockAppMetrics::register(gnort::global_metrics_registry())
                .expect("Failed to register metrics!"),
        );
        let (saver, checkpoint) = Saver::<J>::try_load(snapshot_path, Some(metrics.clone()))
            .await
            .expect("Failed to set up snapshotting");
        let save_mutex = Arc::new(Mutex::new(saver));
        let mut kernel = kernel_from_checkpoint(checkpoint).await?;
        // important: we are tracking this separately here because
        // what matters is the last poke *we* received an ack for. Using
        // the Arc in the serf would result in a race condition!

        let (action_channel_sender, action_channel) = mpsc::channel(100);
        let (effect_broadcast_sender, _) = broadcast::channel(100);
        let effect_broadcast = Arc::new(effect_broadcast_sender);
        // let tasks = Arc::new(Mutex::new(TaskJoinSet::new()));
        // let tasks = TaskJoinSet::new();
        // let tasks = Arc::new(TaskJoinSet::new());
        let tasks = TaskTracker::new();
        let save_interval = save_interval_duration.map(|duration| {
            info!("Nockapp save interval duration: {:?}", duration);
            let first_tick_at = Instant::now() + duration;
            let mut interval = interval_at(first_tick_at, duration);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip); // important so we don't stack ticks when lagging
            interval
        });
        if save_interval.is_none() {
            info!("Nockapp save interval disabled; periodic saves off");
        }
        let exit_status = AtomicBool::new(false);
        let abort_immediately = AtomicBool::new(false);

        kernel
            .provide_metrics(metrics.clone())
            .await
            .expect("Failed to provide metrics to kernel");

        let signals = Signals::new([TERM_SIGNALS, &[SIGHUP]].concat())
            .expect("Failed to create signal handler");

        let (exit, exit_recv) = NockAppExit::new();
        Ok(Self {
            kernel,
            tasks,
            abort_immediately,
            exit,
            exit_recv,
            exit_status,
            action_channel,
            action_channel_sender,
            effect_broadcast,
            save_interval,
            save_mutex,
            // cancel_token,
            metrics,
            signals,
        })
    }

    pub fn get_handle(&self) -> NockAppHandle {
        NockAppHandle {
            io_sender: self.action_channel_sender.clone(),
            effect_sender: self.effect_broadcast.clone(),
            effect_receiver: Mutex::new(self.effect_broadcast.subscribe()),
            metrics: self.metrics.clone(),
            exit: self.exit.clone(),
        }
    }

    /// Assume at-least-once processing and track the state necessary to know whether
    /// all critical IO actions have been performed correctly or not from the jammed state.
    #[tracing::instrument(skip(self, driver))]
    pub async fn add_io_driver(&mut self, driver: IODriverFn) {
        let io_sender = self.action_channel_sender.clone();
        let effect_sender = self.effect_broadcast.clone();
        let effect_receiver = Mutex::new(self.effect_broadcast.subscribe());
        let metrics = self.metrics.clone();
        let exit = self.exit.clone();
        let fut = driver(NockAppHandle {
            io_sender,
            effect_sender,
            effect_receiver,
            metrics,
            exit,
        });
        // TODO: Stop using the task tracker for user code?
        self.tasks.spawn(fut);
        debug!("Added IO driver");
    }

    /// Assume at-least-once processing and track the state necessary to know whether
    /// all critical IO actions have been performed correctly or not from the jammed state.
    #[tracing::instrument(skip(self, driver))]
    pub async fn add_io_driver_(
        &mut self,
        driver: IODriverFn,
    ) -> tokio::sync::mpsc::Sender<IOAction> {
        let io_sender = self.action_channel_sender.clone();
        let effect_sender = self.effect_broadcast.clone();
        let effect_receiver = Mutex::new(self.effect_broadcast.subscribe());
        let metrics = self.metrics.clone();
        let exit = self.exit.clone();
        let fut = driver(NockAppHandle {
            io_sender,
            effect_sender,
            effect_receiver,
            metrics,
            exit,
        });
        // TODO: Stop using the task tracker for user code?
        self.tasks.spawn(fut);
        let io_sender = self.action_channel_sender.clone();
        debug!("Added IO driver");
        io_sender
    }

    /// Purely for testing purposes (injecting delays) for now.
    #[instrument(skip(self, f, save_permit))]
    pub(crate) async fn save_f(
        &mut self,
        f: impl std::future::Future<Output = ()> + Send + 'static,
        mut save_permit: OwnedMutexGuard<Saver<J>>,
    ) -> Result<tokio::task::JoinHandle<NockAppResult>, NockAppError> {
        let checkpoint_fut = self.kernel.checkpoint();
        let metrics = self.metrics.clone();

        trace!("Spawning save task from save_f");
        let join_handle = self.tasks.spawn(async move {
            f.await;
            trace!("Save task from save_f: f.await done");
            let checkpoint = checkpoint_fut.await?;
            trace!("Save task from save_f: checkpoint_fut.await done");
            save_permit.save(checkpoint, metrics).await?;
            trace!("Save task from save_f: save_permit.save done");

            drop(save_permit);
            Ok::<(), NockAppError>(())
        });
        // We don't want to close and re-open the tasktracker from multiple places
        // so we're just returning the join_handle to let the caller decide.
        Ok(join_handle)
    }

    /// Except in tests, save should only be called by the permit handler.
    pub(crate) async fn save(&mut self, save_permit: OwnedMutexGuard<Saver<J>>) -> NockAppResult {
        let _join_handle = self.save_f(async {}, save_permit).await?;
        Ok(())
    }

    pub async fn save_locked(&mut self) -> NockAppResult {
        trace!("save_locked: locking save_mutex");
        let guard = self.save_mutex.clone().lock_owned().await;
        trace!("save_locked: save_mutex locked");
        self.save(guard).await.map_err(|e| {
            error!("Failed to save: {:?}", e);
            e
        })?;
        Ok(())
    }

    /// Save the kernel to disk, blocking operation
    pub async fn save_blocking(&mut self) -> NockAppResult {
        trace!("save_blocking: locking save_mutex");
        let guard = self.save_mutex.clone().lock_owned().await;
        trace!("save_blocking: save_mutex locked");
        let join_handle = self.save_f(async {}, guard).await?;
        join_handle.await.map_err(NockAppError::JoinError)??;
        Ok(())
    }

    /// Peek at a noun in the kernel, blocking operation
    #[tracing::instrument(skip(self, path))]
    pub fn peek_sync(&mut self, path: NounSlab) -> Result<NounSlab, NockAppError> {
        trace!("Peeking at noun: {:?}", path);
        let res = self.kernel.peek_sync(path)?;
        trace!("Peeked noun: {:?}", res);
        Ok(res)
    }

    #[tracing::instrument(skip(self, path))]
    pub async fn peek(&mut self, path: NounSlab) -> Result<NounSlab, NockAppError> {
        trace!("Peeking at noun: {:?}", path);
        let res = self.kernel.peek(path).await?;
        trace!("Peeked noun: {:?}", res);
        Ok(res)
    }

    // Peek at a noun in the kernel with result munging. A `~`, which denotes an invalid
    // poke path results in an error while [~ ~] denoting missing data results in a Ok(None).
    #[tracing::instrument(skip(self, path))]
    pub async fn peek_handle(&mut self, path: NounSlab) -> Result<Option<NounSlab>, NockAppError> {
        trace!("Peeking at noun: {:?}", path);
        let res = self.kernel.peek(path).await?;
        trace!("Peeked noun: {:?}", res);
        if unsafe { res.root().raw_equals(&SIG) } {
            return Err(NockAppError::PeekFailed);
        }

        let tail = unsafe { res.root().as_cell()?.tail() };
        if unsafe { tail.raw_equals(&SIG) } {
            Ok(None)
        } else {
            let res_noun = tail.as_cell()?.tail();
            let mut slab = NounSlab::new();
            slab.copy_into(res_noun);
            Ok(Some(slab))
        }
    }

    /// Poke at a noun in the kernel, blocking operation
    #[tracing::instrument(skip(self, wire, cause))]
    pub fn poke_sync(
        &mut self,
        wire: WireRepr,
        cause: NounSlab,
    ) -> Result<Vec<NounSlab>, NockAppError> {
        // let wire_noun = wire.copy_to_stack(self.kernel.serf.stack());
        let effects_slab = self.kernel.poke_sync(wire, cause)?;
        Ok(effects_slab.to_vec())
    }

    #[tracing::instrument(skip(self, wire, cause))]
    pub async fn poke(
        &mut self,
        wire: WireRepr,
        cause: NounSlab,
    ) -> Result<Vec<NounSlab>, NockAppError> {
        let effects_slab = self.kernel.poke(wire, cause).await?;
        Ok(effects_slab.to_vec())
    }

    pub async fn poke_timeout(
        &mut self,
        wire: WireRepr,
        cause: NounSlab,
        timeout: Duration,
    ) -> Result<Vec<NounSlab>, NockAppError> {
        let effects_slab = self.kernel.poke_timeout(wire, cause, timeout).await?;
        Ok(effects_slab.to_vec())
    }

    /// Runs until the nockapp is done (returns exit 0 or an error)
    /// TODO: we should print most errors rather than exiting immediately
    #[instrument(skip(self))]
    pub async fn run(&mut self) -> NockAppResult {
        // Reset NockApp for next run
        // self.reset();
        // debug!("Reset NockApp for next run");
        loop {
            let work_res = self.work().await;
            match work_res {
                Ok(nockapp_run) => match nockapp_run {
                    crate::nockapp::NockAppRun::Pending => {
                        continue;
                    }
                    crate::nockapp::NockAppRun::Done => break Ok(()),
                },
                Err(NockAppError::Exit(code)) => {
                    if code == 0 {
                        // zero is success, we're simply done.
                        debug!("nockapp exited successfully with code: {}", code);
                        break Ok(());
                    } else {
                        error!("nockapp exited with error code: {}", code);
                        break Err(NockAppError::Exit(code));
                    }
                }
                Err(e) => {
                    error!("Got error running nockapp: {:?}", e);
                    break Err(e);
                }
            };
        }
    }

    async fn work(&mut self) -> Result<NockAppRun, NockAppError> {
        // Track SIGINT (C-c) presses for immediate termination
        // Fires when there is a save interval tick *and* an available permit in the save semaphore
        let save_ready = if let Some(interval) = self.save_interval.as_mut() {
            let save_mutex = self.save_mutex.clone();
            Either::Left(async move {
                interval.tick().await;
                trace!("save_interval tick: locking save_mutex");
                let guard = save_mutex.lock_owned().await;
                trace!("save_interval tick: save_mutex locked");
                guard
            })
        } else {
            Either::Right(pending::<OwnedMutexGuard<Saver<J>>>())
        };
        select!(
            exit_status_res = self.exit_recv.recv() => {
                let Some(exit_status) = exit_status_res else {
                    error!("Exit channel closed");
                    return Err(NockAppError::ChannelClosedError)
                };
                match exit_status {
                    NockAppExitStatus::Exit(code) => {
                        self.metrics.handle_exit.increment();
                        self.handle_exit(code).await
                    },
                    NockAppExitStatus::Shutdown(res) => {
                        self.metrics.handle_shutdown.increment();
                        let stop_fut = self.kernel.serf.stop();
                        let exit = self.exit.clone();
                        self.tasks.spawn(async move {
                            if let Err(e) = stop_fut.await {
                                if let Err(e) = exit.done(Err(NockAppError::from(e))).await {
                                    error!("Error completing shutdown: {e}");
                                }
                            } else if let Err(e) = exit.done(res).await {
                                error!("Error completing shutdown: {e}");
                            }
                        });
                        Ok(NockAppRun::Pending)
                    },
                    NockAppExitStatus::Done(res) => {
                        match res {
                            Ok(()) => {
                                debug!("Shutdown triggered, exiting");
                                Ok(NockAppRun::Done)
                            },
                            Err(e) => {
                                error!("Shutdown triggered with error: {:?}", e);
                                Err(e)
                            }
                        }
                    },
                }
            },
            save_guard = save_ready => {
                self.metrics.handle_save_permit_res.increment();
                self.handle_save_permit_res(save_guard).await
            },
            maybe_signal = self.signals.next() => {
                debug!("Signal received");
                if let Some(signal) = maybe_signal {
                    let (code, explanation) = match signal {
                        SIGINT => (EXIT_SIGINT, "SIGINT (C-c): Keyboard interrupt."),
                        SIGTERM => (EXIT_SIGTERM, "SIGTERM: Termination signal from OS or process manager."),
                        SIGQUIT => (EXIT_SIGQUIT, "SIGQUIT: Quit from keyboard (core dump)."),
                        SIGHUP => (EXIT_SIGHUP, "SIGHUP: Terminal closed or controlling process died."),
                        _ => (EXIT_UNKNOWN, "Unknown signal: default error code 1."),
                    };
                    self.metrics.handle_exit.increment();
                    debug!("Received signal {signal}, code {code}: {explanation}");
                    loop {
                        if !self.abort_immediately.load(Ordering::SeqCst) {
                            if self.abort_immediately.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                                trace!("Exiting due to signal {signal}");
                                let exit_fut = self.exit.exit(code);
                                self.tasks.spawn(exit_fut);
                                break Ok(NockAppRun::Pending);
                            }
                        } else {
                            std::process::exit(
                                code.try_into().expect("exit code should fit in i32"),
                            );
                        }
                    }
                } else {
                    error!("Signal stream ended unexpectedly");
                    Err(NockAppError::ChannelClosedError)
                }
            },
            action_res = self.action_channel.recv() => {
                trace!("Action channel received");
                self.metrics.handle_action.increment();
                match action_res {
                    Some(action) => {
                        self.handle_action(action).await;
                        Ok(NockAppRun::Pending)
                    }
                    None => {
                        error!("Action channel closed prematurely");
                        Err(NockAppError::ChannelClosedError)
                    }
                }
            }
        )
    }

    #[instrument(skip_all, level = "trace")]
    async fn handle_save_permit_res(
        &mut self,
        save_guard: OwnedMutexGuard<Saver<J>>,
    ) -> Result<NockAppRun, NockAppError> {
        //  Check if we should write in the first place
        let curr_event_num = self.kernel.serf.event_number.load(Ordering::SeqCst);
        if !save_guard.save_needed(curr_event_num) {
            return Ok(NockAppRun::Pending);
        }

        let res = self.save(save_guard).await;

        res.map(|_| NockAppRun::Pending)
    }

    #[instrument(skip_all)]
    async fn handle_action(&self, action: IOAction) {
        // Stop processing events if we are exiting
        if self.exit_status.load(Ordering::SeqCst) {
            if let IOAction::Poke { .. } = action {
                self.metrics.poke_during_exit.increment();
                debug!("Poked during exit. Ignoring.")
            } else {
                self.metrics.peek_during_exit.increment();
                debug!("Peeked during exit. Ignoring.")
            }
            return;
        }
        match action {
            IOAction::Poke {
                wire,
                poke,
                ack_channel,
                timeout,
            } => self.handle_poke(wire, poke, ack_channel, timeout).await,
            IOAction::Peek {
                path,
                result_channel,
            } => self.handle_peek(path, result_channel).await,
        }
    }

    #[instrument(skip_all)]
    async fn handle_poke(
        &self,
        wire: WireRepr,
        cause: NounSlab,
        ack_channel: tokio::sync::oneshot::Sender<PokeResult>,
        timeout: Option<Duration>,
    ) {
        if let Some(timeout) = timeout {
            let poke_future = self.kernel.poke_timeout(wire, cause, timeout);
            let effect_broadcast = self.effect_broadcast.clone();
            drop(self.tasks.spawn(async move {
                let poke_result = poke_future.await;
                match poke_result {
                    Ok(effects) => {
                        let _ = ack_channel.send(PokeResult::Ack);
                        for effect_slab in effects.to_vec() {
                            let _ = effect_broadcast.send(effect_slab);
                        }
                    }
                    Err(_) => {
                        let _ = ack_channel.send(PokeResult::Nack);
                    }
                }
            }));
        } else {
            let poke_future = self.kernel.poke(wire, cause);
            let effect_broadcast = self.effect_broadcast.clone();
            drop(self.tasks.spawn(async move {
                let poke_result = poke_future.await;
                match poke_result {
                    Ok(effects) => {
                        let _ = ack_channel.send(PokeResult::Ack);
                        for effect_slab in effects.to_vec() {
                            let _ = effect_broadcast.send(effect_slab);
                        }
                    }
                    Err(_) => {
                        let _ = ack_channel.send(PokeResult::Nack);
                    }
                }
            }));
        }
    }

    #[instrument(skip_all)]
    async fn handle_peek(
        &self,
        path: NounSlab,
        result_channel: tokio::sync::oneshot::Sender<Option<NounSlab>>,
    ) {
        let peek_future = self.kernel.peek(path);
        drop(self.tasks.spawn(async move {
            let peek_res = peek_future.await;

            match peek_res {
                Ok(res_slab) => {
                    let _ = result_channel.send(Some(res_slab));
                }
                Err(e) => {
                    error!("Peek error: {:?}", e);
                    let _ = result_channel.send(None);
                }
            }
        }));
    }

    // TODO: We should explicitly kick off a save somehow
    // TOOD: :>) spawn a task which awaits the signal stream and if there is a SIGINT, then call std::process::exit(1)
    #[instrument(skip_all)]
    async fn handle_exit(&mut self, code: usize) -> Result<NockAppRun, NockAppError> {
        // We should only run handle_exit once, break out if we are already exiting.
        loop {
            if self.exit_status.load(Ordering::SeqCst) {
                return Ok(NockAppRun::Pending);
            } else if self
                .exit_status
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }

        let exit_event_num = self.kernel.serf.event_number.load(Ordering::SeqCst);
        debug!(
            "Exit request received, waiting for save checkpoint with event_num {} (code {})",
            exit_event_num, code
        );

        let waiter_mutex_arc = self.save_mutex.clone();
        let waiter = {
            trace!("Waiting for save event_num {}", exit_event_num);
            let mut guard = waiter_mutex_arc.lock().await;
            trace!("Locked save mutex for event_num {}", exit_event_num);
            let oneshot = guard.wait_for_snapshot(exit_event_num).await;
            trace!("Acquired the oneshot for snapshot on save event_num {}", exit_event_num);
            drop(guard);
            oneshot
        };

        // Force an immediate save to ensure we have the latest state
        debug!(
            "Exit signal received with code {}, forcing immediate save",
            code
        );
        if let Err(e) = self.save_locked().await {
            error!(
                "Failed to save during exit: {:?} - continuing with shutdown anyway",
                e
            );
        }

        // let cancel_token = self.cancel_token.clone();
        let exit = self.exit.clone();
        // self.tasks.close();
        // self.tasks.wait().await;
        // recv from the watch channel until we reach the exit event_num, wrapped up in a future
        // that will send the shutdown result when we're done.
        // TODO: Break this out as a separate select! handler with no spawn
        self.tasks.spawn(async move {
            debug!("Waiting for save event_num {}", exit_event_num);
            let result = waiter.await;
            if let Err(e) = result {
                error!("Error waiting for snapshot: {e}");
                panic!("Error waiting for snapshot: {e}");
            };
            debug!("Save event_num reached, finishing with code {}", code);
            let shutdown_result = if code == EXIT_OK {
                Ok(())
            } else {
                Err(NockAppError::Exit(code))
            };
            // Ensure we send the shutdown result before canceling so that
            // we don't get a race condition where the yielded result is
            // "canceled" instead of the actual result.
            debug!("Sending shutdown result");
            if let Err(e) = exit.shutdown(shutdown_result).await {
                error!("Error sending shutdown: {e:}")
            }
        });
        Ok(NockAppRun::Pending)
    }
}
