use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use nockchain_math::belt::PRIME;
use nockchain_types::tx_engine::common::Name;
use nockchain_types::v1::{FirstName, Lock, SpendCondition};
use wallet_tx_builder::types::RawNoteDataEntry;

use crate::shared::errors::BridgeError;
use crate::shared::nockchain::fetch_private_blockchain_constants;
use crate::shared::types::{BaseEventId, Tip5Hash};
use crate::withdrawal::assembly::{
    plan_withdrawal_build, WithdrawalAssemblyPlannerConfig, WithdrawalBuildPlanningError,
};
use crate::withdrawal::proposals::TrackedWithdrawalRequest;
use crate::withdrawal::snapshot::{BridgeNoteSnapshotService, BridgeOwnedNoteSelectors};
use crate::withdrawal::types::WithdrawalId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicWithdrawalQuote {
    pub gross_amount_nicks: u64,
    pub bridge_fee_nicks: u64,
    pub transaction_fee_nicks: u64,
    pub net_payout_nicks: u64,
    pub snapshot_height: u64,
    pub snapshot_block_id: Tip5Hash,
    pub reserved_input_count: u64,
    pub observed_at_unix_ms: i64,
}

#[async_trait]
pub trait WithdrawalQuotePort: Send + Sync {
    async fn quote(
        &self,
        gross_amount_nicks: u64,
        destination_lock_root: Tip5Hash,
        reserved_inputs: &[Name],
    ) -> Result<PublicWithdrawalQuote, BridgeError>;
}

#[derive(Clone)]
pub struct NockchainWithdrawalQuoteService {
    private_nockchain_endpoint: String,
    snapshot_service: Arc<BridgeNoteSnapshotService>,
    spend_authority_lock_root: Tip5Hash,
    spend_authority_spend_condition: SpendCondition,
    nicks_fee_per_nock: u64,
}

impl NockchainWithdrawalQuoteService {
    pub fn new_private(
        private_nockchain_endpoint: String,
        spend_authority_lock_root: Tip5Hash,
        spend_authority_spend_condition: SpendCondition,
        nicks_fee_per_nock: u64,
        nockchain_confirmation_depth: u64,
        stale_after: Duration,
    ) -> Result<Self, BridgeError> {
        let first_name = FirstName::from_lock_root(&spend_authority_lock_root)
            .map_err(|error| {
                BridgeError::Config(format!(
                    "failed to derive bridge quote first-name selector: {error}"
                ))
            })?
            .into_hash()
            .to_base58();
        let snapshot_service = BridgeNoteSnapshotService::new_private(
            private_nockchain_endpoint.clone(),
            BridgeOwnedNoteSelectors {
                first_names: vec![first_name],
            },
            stale_after,
        )
        .with_nockchain_confirmation_depth(nockchain_confirmation_depth);
        Ok(Self {
            private_nockchain_endpoint,
            snapshot_service: Arc::new(snapshot_service),
            spend_authority_lock_root,
            spend_authority_spend_condition,
            nicks_fee_per_nock,
        })
    }
}

#[async_trait]
impl WithdrawalQuotePort for NockchainWithdrawalQuoteService {
    async fn quote(
        &self,
        gross_amount_nicks: u64,
        destination_lock_root: Tip5Hash,
        reserved_inputs: &[Name],
    ) -> Result<PublicWithdrawalQuote, BridgeError> {
        self.snapshot_service.refresh().await?;
        let snapshot = self
            .snapshot_service
            .spendable_snapshot(reserved_inputs)
            .ok_or_else(|| {
                BridgeError::Runtime(
                    "authoritative withdrawal quote has no safe bridge note snapshot".into(),
                )
            })?;
        let blockchain_constants =
            fetch_private_blockchain_constants(&self.private_nockchain_endpoint).await?;
        let planner = WithdrawalAssemblyPlannerConfig {
            spend_authority_lock_root: self.spend_authority_lock_root.clone(),
            spend_authority_spend_condition: self.spend_authority_spend_condition.clone(),
            refund_lock_root: self.spend_authority_lock_root.clone(),
            refund_note_data: vec![RawNoteDataEntry::from_lock(Lock::SpendCondition(
                self.spend_authority_spend_condition.clone(),
            ))],
            nicks_fee_per_nock: self.nicks_fee_per_nock,
            blockchain_constants: blockchain_constants.clone(),
            bythos_phase: blockchain_constants.bythos_phase,
            base_fee: blockchain_constants.base_fee,
            input_fee_divisor: blockchain_constants.input_fee_divisor,
            min_fee: blockchain_constants.note_data.min_fee,
        };
        let request = TrackedWithdrawalRequest {
            id: WithdrawalId {
                as_of: Tip5Hash::from_limbs(&[PRIME - 1; 5]),
                base_event_id: BaseEventId(vec![0xff; BaseEventId::LEN]),
            },
            recipient: destination_lock_root,
            amount: gross_amount_nicks,
            base_batch_end: u64::MAX,
            withdrawal_nonce: 0,
        };
        let build = plan_withdrawal_build(&request, &snapshot, &planner).map_err(|error| {
            match error {
                WithdrawalBuildPlanningError::InsufficientFunds {
                    selected_total,
                    required,
                } => BridgeError::Runtime(format!(
                    "authoritative withdrawal quote has insufficient safe unreserved liquidity: selected {selected_total}, required {required}"
                )),
                WithdrawalBuildPlanningError::Bridge(error) => error,
            }
        })?;
        let bridge_fee_nicks = wallet_tx_builder::fee::compute_bridge_fee(
            build.burned_amount, self.nicks_fee_per_nock,
        );
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                BridgeError::Runtime(format!("system time before unix epoch: {error}"))
            })
            .and_then(|duration| {
                i64::try_from(duration.as_millis()).map_err(|error| {
                    BridgeError::ValueConversion(format!("quote timestamp overflow: {error}"))
                })
            })?;
        Ok(PublicWithdrawalQuote {
            gross_amount_nicks,
            bridge_fee_nicks,
            transaction_fee_nicks: build.fee,
            net_payout_nicks: build.net_amount,
            snapshot_height: snapshot.metadata.height.0 .0,
            snapshot_block_id: snapshot.metadata.block_id,
            reserved_input_count: u64::try_from(reserved_inputs.len()).map_err(|error| {
                BridgeError::ValueConversion(format!("reserved input count overflow: {error}"))
            })?,
            observed_at_unix_ms,
        })
    }
}
