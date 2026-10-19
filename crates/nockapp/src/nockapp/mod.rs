// pub(crate) mod actors;
pub mod artifact;
pub mod driver;
pub mod error;
pub mod export;
pub mod metrics;
pub mod outbound;
pub mod save;
pub mod test;
pub mod wire;

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use driver::{
    IOAction, IODriverFn, NativeOwnPokeResult, NativePokeObserver, NockAppHandle, PokeResult,
};
pub use error::NockAppError;
use futures::stream::StreamExt;
use metrics::*;
use nockvm::noun::{NounAllocator, SIG};
use signal_hook::consts::signal::*;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook_tokio::Signals;
use tokio::select;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::task::TaskTracker;
use tracing::{debug, error, instrument, trace, warn};
use wire::WireRepr;

use crate::kernel::form::Kernel;
use crate::noun::slab::{Jammer, NockJammer, NounSlab};
use crate::save::SaveableCheckpoint;

type NockAppResult = Result<(), NockAppError>;

async fn complete_poke(
    poke_future: impl Future<Output = NativeOwnPokeResult>,
    ack_channel: tokio::sync::oneshot::Sender<PokeResult>,
    effect_broadcast: Arc<broadcast::Sender<NounSlab>>,
    observer: Option<NativePokeObserver>,
) {
    // No additional clock samples or result inspection on the ordinary path.
    let started_us = observer.as_ref().map(NativePokeObserver::time_us);
    let poke_result = poke_future.await;
    let completed_us = observer.as_ref().map(NativePokeObserver::time_us);
    if let (Some(observer), Some(started_us), Some(completed_us)) =
        (observer, started_us, completed_us)
    {
        if !ack_channel.is_closed() {
            observer.complete(started_us, completed_us, &poke_result);
        }
    }
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
}

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
    metrics: Arc<NockAppMetrics>,
    /// Signals handled by the work loop
    signals: Signals,
    _phantom: std::marker::PhantomData<J>,
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
    fn metrics_enabled() -> bool {
        if std::env::var_os("NOCKAPP_DISABLE_METRICS").is_some() {
            return false;
        }
        if std::env::var_os("GNORT_DISABLE").is_some() {
            return false;
        }
        std::env::var_os("RUST_TEST_THREADS").is_none()
    }

    pub fn take_pma_timing_samples(&self) -> Option<Vec<(Duration, Duration)>> {
        self.kernel.take_pma_timing_samples()
    }

    pub fn take_pma_timing_samples_detailed(
        &self,
    ) -> Option<Vec<crate::kernel::form::PmaTimingSample>> {
        self.kernel.take_pma_timing_samples_detailed()
    }

    /// Produces a checkpoint of the current kernel state.
    pub fn checkpoint(&self) -> impl Future<Output = crate::Result<SaveableCheckpoint>> {
        self.kernel.checkpoint()
    }

    /// This constructs a Tokio interval, even though it doesn't look like it, a Tokio runtime is _required_.
    pub async fn new<F, U, E>(kernel_from_boot: F) -> Result<Self, NockAppError>
    where
        F: FnOnce(Arc<NockAppMetrics>) -> U,
        U: Future<Output = Result<Kernel<SaveableCheckpoint>, E>>,
        NockAppError: From<E>,
    {
        // let cancel_token = tokio_util::sync::CancellationToken::new();
        let metrics = if Self::metrics_enabled() {
            match gnort::GnortClient::default() {
                Ok(client) => {
                    let registry = gnort::MetricsRegistry::new(
                        gnort::RegistryConfig::default().with_client(client),
                    );
                    Arc::new(
                        NockAppMetrics::register(&registry).expect("Failed to register metrics!"),
                    )
                }
                Err(err) => {
                    warn!("Failed to initialize metrics client, disabling metrics: {err}");
                    Arc::new(NockAppMetrics::default())
                }
            }
        } else {
            Arc::new(NockAppMetrics::default())
        };
        let mut kernel = kernel_from_boot(metrics.clone()).await?;
        // important: we are tracking this separately here because
        // what matters is the last poke *we* received an ack for. Using
        // the Arc in the serf would result in a race condition!

        let (action_channel_sender, action_channel) = mpsc::channel(100);
        let (effect_broadcast_sender, _) = broadcast::channel(1024);
        let effect_broadcast = Arc::new(effect_broadcast_sender);
        let tasks = TaskTracker::new();
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
            metrics,
            signals,
            _phantom: std::marker::PhantomData,
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
        let res = self
            .kernel
            .peek(path.clone())
            .await
            .map_err(NockAppError::CrownError);
        trace!("Peeked noun: {:?}", res);
        res
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

        let space = res.noun_space();
        let root_cell = unsafe { res.root().in_space(&space).as_cell()? };
        let tail = root_cell.tail().noun();
        if unsafe { tail.raw_equals(&SIG) } {
            Ok(None)
        } else {
            let res_noun = tail.in_space(&space).as_cell()?.tail().noun();
            let mut slab = NounSlab::new();
            slab.copy_into(res_noun, &space);
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

    pub async fn export(&self) -> Result<crate::kernel::form::LoadState, NockAppError> {
        Ok(self.kernel.export().await?)
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
                observer,
            } => {
                self.handle_poke(wire, poke, ack_channel, timeout, observer)
                    .await
            }
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
        observer: Option<NativePokeObserver>,
    ) {
        if let Some(timeout) = timeout {
            let poke_future = self.kernel.poke_timeout(wire, cause, timeout);
            let effect_broadcast = self.effect_broadcast.clone();
            drop(self.tasks.spawn(complete_poke(
                poke_future, ack_channel, effect_broadcast, observer,
            )));
        } else {
            let poke_future = self.kernel.poke(wire, cause);
            let effect_broadcast = self.effect_broadcast.clone();
            drop(self.tasks.spawn(complete_poke(
                poke_future, ack_channel, effect_broadcast, observer,
            )));
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

        let exit = self.exit.clone();
        self.tasks.spawn(async move {
            let shutdown_result = if code == EXIT_OK {
                Ok(())
            } else {
                Err(NockAppError::Exit(code))
            };
            if let Err(e) = exit.shutdown(shutdown_result).await {
                error!("Error sending shutdown: {e:}")
            }
        });
        Ok(NockAppRun::Pending)
    }
}

#[cfg(test)]
mod native_poke_tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex as StdMutex;

    use nockvm::noun::{D, T};
    use tokio::sync::oneshot;

    use super::*;

    fn clock() -> u64 {
        static TIME: AtomicU64 = AtomicU64::new(0);
        TIME.fetch_add(1, Ordering::SeqCst)
    }

    fn effects(value: Option<u64>) -> NounSlab {
        let mut slab = NounSlab::new();
        let root = value.map_or(D(0), |value| T(&mut slab, &[D(value), D(0)]));
        slab.set_root(root);
        slab
    }

    fn contains_effect(result: &NativeOwnPokeResult, value: u64) -> bool {
        result.as_ref().is_ok_and(|effects| {
            let space = effects.noun_space();
            unsafe { effects.root() }
                .in_space(&space)
                .list_iter()
                .any(|effect| effect.as_atom().and_then(|atom| atom.as_u64()).ok() == Some(value))
        })
    }

    #[tokio::test]
    async fn concurrent_pokes_observe_only_their_own_effects_before_ack_and_broadcast() {
        let (broadcast, mut broadcast_receiver) = broadcast::channel(8);
        let broadcast = Arc::new(broadcast);
        let observations = Arc::new(StdMutex::new(Vec::new()));
        let (first_result, first_future) = oneshot::channel();
        let (second_result, second_future) = oneshot::channel();
        let (first_ack, first_receiver) = oneshot::channel();
        let (second_ack, second_receiver) = oneshot::channel();
        let first_receiver = Arc::new(StdMutex::new(first_receiver));
        let second_receiver = Arc::new(StdMutex::new(second_receiver));
        let first_observer = {
            let observations = observations.clone();
            let receiver = first_receiver.clone();
            NativePokeObserver::new(clock, move |started, completed, result| {
                assert!(started <= completed);
                assert!(matches!(
                    receiver.lock().expect("ack receiver").try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                observations
                    .lock()
                    .expect("observations")
                    .push((1, contains_effect(result, 77)));
            })
        };
        let second_observer = {
            let observations = observations.clone();
            let receiver = second_receiver.clone();
            let mut effects_receiver = broadcast.subscribe();
            NativePokeObserver::new(clock, move |started, completed, result| {
                assert!(started <= completed);
                assert!(matches!(
                    receiver.lock().expect("ack receiver").try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                assert!(matches!(
                    effects_receiver.try_recv(),
                    Err(broadcast::error::TryRecvError::Empty)
                ));
                observations
                    .lock()
                    .expect("observations")
                    .push((2, contains_effect(result, 77)));
            })
        };
        let first_task = tokio::spawn(complete_poke(
            async move { first_future.await.expect("first result") },
            first_ack,
            broadcast.clone(),
            Some(first_observer),
        ));
        let second_task = tokio::spawn(complete_poke(
            async move { second_future.await.expect("second result") },
            second_ack,
            broadcast,
            Some(second_observer),
        ));

        second_result
            .send(Ok(effects(Some(77))))
            .expect("second result");
        second_task.await.expect("second completion");
        first_result.send(Ok(effects(None))).expect("first result");
        first_task.await.expect("first completion");

        assert_eq!(
            *observations.lock().expect("observations"),
            [(2, true), (1, false)]
        );
        assert!(matches!(
            first_receiver.lock().expect("first ack").try_recv(),
            Ok(PokeResult::Ack)
        ));
        assert!(matches!(
            second_receiver.lock().expect("second ack").try_recv(),
            Ok(PokeResult::Ack)
        ));
        let effect = broadcast_receiver.try_recv().expect("own broadcast effect");
        let space = effect.noun_space();
        assert_eq!(
            unsafe { effect.root() }
                .in_space(&space)
                .as_atom()
                .expect("effect atom")
                .as_u64()
                .expect("effect value"),
            77
        );
        assert!(matches!(
            broadcast_receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn failed_poke_observes_error_then_nacks_without_broadcasting() {
        let (broadcast, mut effects_receiver) = broadcast::channel(8);
        let (ack, receiver) = oneshot::channel();
        let receiver = Arc::new(StdMutex::new(receiver));
        let observed = Arc::new(AtomicBool::new(false));
        let observer = {
            let observed = observed.clone();
            let receiver = receiver.clone();
            NativePokeObserver::new(clock, move |started, completed, result| {
                assert!(started <= completed);
                assert!(matches!(result, Err(crate::CrownError::InvalidKernelInput)));
                assert!(matches!(
                    receiver.lock().expect("ack receiver").try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                observed.store(true, Ordering::SeqCst);
            })
        };
        complete_poke(
            async { Err(crate::CrownError::InvalidKernelInput) },
            ack,
            Arc::new(broadcast),
            Some(observer),
        )
        .await;
        assert!(observed.load(Ordering::SeqCst));
        assert!(matches!(
            receiver.lock().expect("ack").try_recv(),
            Ok(PokeResult::Nack)
        ));
        assert!(matches!(
            effects_receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn lost_ack_receiver_suppresses_observer_but_preserves_effect_delivery() {
        let (broadcast, mut effects_receiver) = broadcast::channel(8);
        let (ack, receiver) = oneshot::channel();
        drop(receiver);
        complete_poke(
            async { Ok(effects(Some(77))) },
            ack,
            Arc::new(broadcast),
            Some(NativePokeObserver::new(clock, |_, _, _| {
                panic!("lost acknowledgement must not be observed")
            })),
        )
        .await;
        let effect = effects_receiver
            .try_recv()
            .expect("effect delivery survives lost ack");
        let space = effect.noun_space();
        assert_eq!(
            unsafe { effect.root() }
                .in_space(&space)
                .as_atom()
                .expect("effect atom")
                .as_u64()
                .expect("effect value"),
            77
        );
    }

    #[tokio::test]
    async fn cancelled_round_trip_never_calls_completion_observer() {
        let (broadcast, _effects_receiver) = broadcast::channel(8);
        let (ack, mut receiver) = oneshot::channel();
        let (result_sender, result_receiver) = oneshot::channel::<NativeOwnPokeResult>();
        let task = tokio::spawn(complete_poke(
            async move { result_receiver.await.expect("native result") },
            ack,
            Arc::new(broadcast),
            Some(NativePokeObserver::new(clock, |_, _, _| {
                panic!("cancelled round trip must not complete")
            })),
        ));
        tokio::task::yield_now().await;
        task.abort();
        assert!(task.await.expect_err("cancelled task").is_cancelled());
        assert!(result_sender.send(Ok(effects(Some(77)))).is_err());
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }
}
