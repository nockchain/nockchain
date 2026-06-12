use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{error, info};

use crate::shared::config::WithdrawalActivationCutoff;
use crate::shared::errors::BridgeError;
use crate::shared::kernel_projection::KernelProjectionPosition;
use crate::shared::stop::StopHandle;
use crate::withdrawal::assembly::{
    recover_pending_base_block_commit_after_activation, repair_pending_base_block_commit,
    restore_tracked_withdrawal_requests, run_withdrawal_assembly_loop, run_withdrawal_signing_loop,
    withdrawal_activation_readiness, WithdrawalActivationReadiness, WithdrawalAssemblyContext,
    WithdrawalAssemblyLoopPolicy, WithdrawalKernelPort, WithdrawalSigningContext,
    WithdrawalSigningLoopPolicy,
};
use crate::withdrawal::proposals::WithdrawalProposalRegistry;
use crate::withdrawal::submission::{
    run_withdrawal_submission_loop, WithdrawalSequencerPort, WithdrawalSubmissionContext,
    WithdrawalSubmissionLoopPolicy,
};

pub struct WithdrawalRuntimeContext<K, S> {
    pub assembly: WithdrawalAssemblyContext<K>,
    pub signing: WithdrawalSigningContext<K>,
    pub submission: WithdrawalSubmissionContext<S>,
    pub activation_cutoff: WithdrawalActivationCutoff,
    pub stop: StopHandle,
    pub assembly_policy: WithdrawalAssemblyLoopPolicy,
    pub signing_policy: WithdrawalSigningLoopPolicy,
    pub submission_policy: WithdrawalSubmissionLoopPolicy,
}

pub struct WithdrawalRuntimeHandles {
    pub activation_restore: JoinHandle<()>,
    pub assembly: JoinHandle<()>,
    pub signing: JoinHandle<()>,
    pub submission: JoinHandle<()>,
}

const WITHDRAWAL_ACTIVATION_RECHECK_INTERVAL: Duration = Duration::from_secs(30);

pub async fn bootstrap_runtime<K: WithdrawalKernelPort>(
    kernel: &K,
    proposal_registry: &WithdrawalProposalRegistry,
    activation_cutoff: WithdrawalActivationCutoff,
    repair_stale_pending_base_commit: bool,
) -> Result<u64, BridgeError> {
    if repair_stale_pending_base_commit {
        if repair_pending_base_block_commit(kernel).await? {
            info!(
                target: "bridge.withdrawal",
                "repaired stale pending base block commit for watcher replay"
            );
        }
    } else {
        recover_pending_base_block_commit_after_activation(
            kernel, proposal_registry, activation_cutoff,
        )
        .await?;
    }
    restore_tracked_withdrawal_requests(kernel, proposal_registry, activation_cutoff).await
}

pub fn spawn_runtime_loops<K, S>(
    context: WithdrawalRuntimeContext<K, S>,
) -> WithdrawalRuntimeHandles
where
    K: WithdrawalKernelPort + 'static,
    S: WithdrawalSequencerPort + 'static,
{
    let WithdrawalRuntimeContext {
        assembly,
        signing,
        submission,
        activation_cutoff,
        stop,
        assembly_policy,
        signing_policy,
        submission_policy,
    } = context;
    let assembly_stop = stop.clone();
    let signing_stop = stop.clone();
    let submission_stop = stop.clone();
    let activation_stop = stop;

    let (activation_tx, activation_rx) = watch::channel(false);
    let activation_kernel = assembly.kernel.clone();
    let activation_registry = assembly.proposal_registry.clone();
    let activation_restore = tokio::spawn(async move {
        run_withdrawal_activation_restore_loop(
            activation_kernel, activation_registry, activation_cutoff, activation_stop,
            activation_tx,
        )
        .await;
    });

    let assembly_activation = activation_rx.clone();
    let assembly = tokio::spawn(async move {
        if await_withdrawal_activation(assembly_activation).await {
            run_withdrawal_assembly_loop(assembly, assembly_stop, assembly_policy).await;
        }
    });
    let signing_activation = activation_rx.clone();
    let signing = tokio::spawn(async move {
        if await_withdrawal_activation(signing_activation).await {
            run_withdrawal_signing_loop(signing, signing_stop, signing_policy).await;
        }
    });
    let submission = tokio::spawn(async move {
        if await_withdrawal_activation(activation_rx).await {
            run_withdrawal_submission_loop(submission, submission_stop, submission_policy).await;
        }
    });

    WithdrawalRuntimeHandles {
        activation_restore,
        assembly,
        signing,
        submission,
    }
}

async fn run_withdrawal_activation_restore_loop<K: WithdrawalKernelPort>(
    kernel: Arc<K>,
    proposal_registry: Arc<WithdrawalProposalRegistry>,
    activation_cutoff: WithdrawalActivationCutoff,
    stop: StopHandle,
    activation_tx: watch::Sender<bool>,
) {
    let mut last_wait_position = None;
    loop {
        if stop.is_stopped() {
            sleep(WITHDRAWAL_ACTIVATION_RECHECK_INTERVAL).await;
            continue;
        }
        match withdrawal_activation_readiness(
            kernel.as_ref(),
            proposal_registry.as_ref(),
            activation_cutoff,
        )
        .await
        {
            Ok(WithdrawalActivationReadiness::Ready(_)) => {}
            Ok(WithdrawalActivationReadiness::Waiting(snapshot)) => {
                log_activation_wait(
                    &mut last_wait_position, &snapshot.current_position, activation_cutoff,
                );
                sleep(WITHDRAWAL_ACTIVATION_RECHECK_INTERVAL).await;
                continue;
            }
            Err(err) => {
                error!(
                    target: "bridge.withdrawal",
                    error = %err,
                    "failed to inspect withdrawal activation state"
                );
                sleep(WITHDRAWAL_ACTIVATION_RECHECK_INTERVAL).await;
                continue;
            }
        }

        match restore_tracked_withdrawal_requests(
            kernel.as_ref(),
            proposal_registry.as_ref(),
            activation_cutoff,
        )
        .await
        {
            Ok(_) => match proposal_registry.load_kernel_projection_cursor().await {
                Ok(Some(_)) => {
                    let _ = activation_tx.send(true);
                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    error!(
                        target: "bridge.withdrawal",
                        error = %err,
                        "failed to inspect withdrawal projection cursor during activation"
                    );
                }
            },
            Err(err) => {
                error!(
                    target: "bridge.withdrawal",
                    error = %err,
                    "withdrawal activation restore failed"
                );
            }
        }
        sleep(WITHDRAWAL_ACTIVATION_RECHECK_INTERVAL).await;
    }
}

fn log_activation_wait(
    last_wait_position: &mut Option<KernelProjectionPosition>,
    current_position: &KernelProjectionPosition,
    activation_cutoff: WithdrawalActivationCutoff,
) {
    if last_wait_position.as_ref() == Some(current_position) {
        return;
    }
    info!(
        target: "bridge.withdrawal",
        kernel_base_next_height = current_position.base_next_height,
        kernel_nock_next_height = current_position.nock_next_height,
        activation_nock_next_height = activation_cutoff.nock_next_height,
        "waiting for withdrawal activation cutoff before starting withdrawal loops"
    );
    *last_wait_position = Some(current_position.clone());
}

async fn await_withdrawal_activation(mut activation_rx: watch::Receiver<bool>) -> bool {
    if *activation_rx.borrow_and_update() {
        return true;
    }
    loop {
        if activation_rx.changed().await.is_err() {
            return false;
        }
        if *activation_rx.borrow_and_update() {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use nockchain_math::belt::Belt;
    use tempfile::tempdir;

    use super::*;
    use crate::shared::kernel_projection::{KernelProjectionCursor, KernelProjectionPosition};
    use crate::shared::types::{AtomBytes, BaseBlockCommitAck, PendingBaseBlockCommit, Tip5Hash};
    use crate::withdrawal::proposals::WithdrawalProjectionStore;
    use crate::withdrawal::types::{
        CreateWithdrawalTxData, NockWithdrawalRequestKernelData, WithdrawalProposalData,
    };

    struct BootstrapKernel {
        pending: Mutex<Option<PendingBaseBlockCommit>>,
        acks: Mutex<Vec<BaseBlockCommitAck>>,
        base_next_height: Option<u64>,
        nock_next_height: Option<u64>,
        base_history: Vec<NockWithdrawalRequestKernelData>,
    }

    #[async_trait]
    impl WithdrawalKernelPort for BootstrapKernel {
        async fn poke_create_withdrawal_tx(
            &self,
            _request: CreateWithdrawalTxData,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn poke_sign_tx(&self, _proposal: WithdrawalProposalData) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(self.base_next_height)
        }

        async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(self.nock_next_height)
        }

        async fn peek_base_hashchain_withdrawals_since_height(
            &self,
            start_height: u64,
        ) -> Result<Vec<NockWithdrawalRequestKernelData>, BridgeError> {
            Ok(self
                .base_history
                .iter()
                .filter(|request| request.base_batch_end >= start_height)
                .cloned()
                .collect())
        }

        async fn peek_pending_base_block_commit(
            &self,
        ) -> Result<Option<PendingBaseBlockCommit>, BridgeError> {
            Ok(self.pending.lock().expect("pending lock").clone())
        }

        async fn poke_base_block_withdrawals_committed(
            &self,
            ack: BaseBlockCommitAck,
        ) -> Result<(), BridgeError> {
            self.acks.lock().expect("ack lock").push(ack);
            self.pending.lock().expect("pending lock").take();
            Ok(())
        }

        async fn poke_repair_pending_base_block_commit(
            &self,
            ack: BaseBlockCommitAck,
        ) -> Result<(), BridgeError> {
            let pending = self.pending.lock().expect("pending lock").take();
            assert_eq!(pending.as_ref().map(PendingBaseBlockCommit::ack), Some(ack));
            Ok(())
        }
    }

    fn sample_request() -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 1).collect()),
            recipient: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            amount: 11,
            base_batch_end: 57_600,
            as_of: Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        }
    }

    async fn open_registry() -> (tempfile::TempDir, WithdrawalProposalRegistry) {
        let dir = tempdir().expect("tempdir");
        let projection_path: PathBuf = dir.path().join("withdrawal-local-state.sqlite");
        let projection_store = WithdrawalProjectionStore::open(projection_path)
            .await
            .expect("open withdrawal projection store");
        (
            dir,
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(Arc::new(
                projection_store,
            )),
        )
    }

    async fn set_cursor(
        registry: &WithdrawalProposalRegistry,
        base_next_height: u64,
        nock_next_height: u64,
    ) {
        registry
            .set_kernel_projection_cursor(KernelProjectionCursor::from_position(
                KernelProjectionPosition {
                    base_next_height,
                    base_tip_hash: None,
                    nock_next_height,
                    nock_tip_hash: None,
                },
            ))
            .await
            .expect("set kernel projection cursor");
    }

    #[tokio::test]
    async fn bootstrap_returns_restored_active_count_after_pending_recovery() {
        let (_dir, registry) = open_registry().await;
        let request = sample_request();
        let pending = PendingBaseBlockCommit {
            blocks_hash: request.as_of.clone(),
            first_height: request.base_batch_end,
            last_height: request.base_batch_end,
            withdrawals: vec![request.clone()],
        };
        let kernel = BootstrapKernel {
            pending: Mutex::new(Some(pending.clone())),
            acks: Mutex::new(Vec::new()),
            base_next_height: Some(request.base_batch_end.saturating_add(1)),
            nock_next_height: Some(0),
            base_history: vec![request],
        };
        set_cursor(&registry, pending.first_height, 0).await;

        let restored = bootstrap_runtime(
            &kernel,
            &registry,
            WithdrawalActivationCutoff {
                nock_next_height: 0,
            },
            false,
        )
        .await
        .expect("bootstrap runtime");
        assert_eq!(restored, 1);
        assert_eq!(
            kernel.acks.lock().expect("ack lock").as_slice(),
            &[pending.ack()]
        );
    }

    #[tokio::test]
    async fn bootstrap_start_repair_rewinds_pending_without_ack_or_tracking() {
        let (_dir, registry) = open_registry().await;
        let request = sample_request();
        let pending = PendingBaseBlockCommit {
            blocks_hash: request.as_of.clone(),
            first_height: request.base_batch_end,
            last_height: request.base_batch_end,
            withdrawals: vec![request],
        };
        let kernel = BootstrapKernel {
            pending: Mutex::new(Some(pending.clone())),
            acks: Mutex::new(Vec::new()),
            base_next_height: Some(pending.first_height),
            nock_next_height: Some(0),
            base_history: vec![],
        };

        let restored = bootstrap_runtime(
            &kernel,
            &registry,
            WithdrawalActivationCutoff {
                nock_next_height: 0,
            },
            true,
        )
        .await
        .expect("bootstrap runtime");
        assert_eq!(restored, 0);
        assert!(kernel.acks.lock().expect("ack lock").is_empty());
        assert!(kernel.pending.lock().expect("pending lock").is_none());
        assert!(registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests")
            .is_empty());
    }
}
