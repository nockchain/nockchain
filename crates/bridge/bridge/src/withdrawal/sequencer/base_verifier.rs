use std::fmt;
use std::sync::Arc;

use alloy::consensus::Transaction as _;
use alloy::primitives::{Address, Bytes, B256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::eth::{Filter, RawLog};
use alloy::transports::ws::WsConnect;
use async_trait::async_trait;
use backon::Retryable;
use op_alloy::network::Optimism;
use tracing::{info, info_span, warn, Instrument};

use crate::core::loop_policy::BaseObserverLoopPolicy;
use crate::shared::base::{
    burn_for_withdrawal_signature_hash, compute_base_event_id,
    decode_burn_for_withdrawal_log_with_calldata, fetch_base_block_info,
    is_explicitly_refunded_withdrawal_burn, validate_base_log_block_hash,
    BurnForWithdrawalDecodeError,
};
use crate::shared::errors::BridgeError;
use crate::shared::types::BaseEventId;
use crate::withdrawal::proposals::TrackedWithdrawalRequest;
use crate::withdrawal::sequencer::base_height::SequencerBaseHeightTracker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequencerBaseWithdrawalRejection {
    BaseHeightUnavailable,
    EventAboveConfirmed {
        base_batch_end: u64,
        confirmed_base_height: u64,
    },
    MissingBaseEventId {
        base_event_id_hex: String,
        batch_start: u64,
        batch_end: u64,
    },
    ExplicitlyRefunded {
        base_event_id_hex: String,
    },
    EventOutsideClaimedBatchWindow {
        event_block: u64,
        batch_start: u64,
        batch_end: u64,
    },
    WrongContractAddress {
        expected: Address,
        actual: Address,
    },
    NotBurnForWithdrawal {
        reason: String,
    },
    AmountNotDivisible {
        amount_raw: String,
    },
    AmountOverflow {
        nicks: String,
    },
    InvalidCalldataTrailer {
        reason: String,
    },
    WrongLockRoot,
    WrongAmount {
        expected_nicks: u64,
        actual_nicks: u64,
    },
    RpcFailure {
        error: String,
    },
}

impl fmt::Display for SequencerBaseWithdrawalRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BaseHeightUnavailable => write!(
                f,
                "sequencer base height watcher has not observed a confirmed Base height yet"
            ),
            Self::EventAboveConfirmed {
                base_batch_end,
                confirmed_base_height,
            } => write!(
                f,
                "withdrawal base_batch_end {base_batch_end} is above confirmed Base height {confirmed_base_height}"
            ),
            Self::MissingBaseEventId {
                base_event_id_hex,
                batch_start,
                batch_end,
            } => write!(
                f,
                "withdrawal burn base_event_id {base_event_id_hex} was not found in confirmed Base batch {batch_start}..={batch_end}"
            ),
            Self::ExplicitlyRefunded { base_event_id_hex } => write!(
                f,
                "withdrawal burn base_event_id {base_event_id_hex} was explicitly refunded and is ignored"
            ),
            Self::EventOutsideClaimedBatchWindow {
                event_block,
                batch_start,
                batch_end,
            } => write!(
                f,
                "withdrawal burn event block {event_block} is outside claimed Base batch {batch_start}..={batch_end}"
            ),
            Self::WrongContractAddress { expected, actual } => write!(
                f,
                "withdrawal burn log came from contract {actual:?}, expected {expected:?}"
            ),
            Self::NotBurnForWithdrawal { reason } => {
                write!(f, "matching Base log is not Nock::BurnForWithdrawal: {reason}")
            }
            Self::AmountNotDivisible { amount_raw } => write!(
                f,
                "BurnForWithdrawal amount {amount_raw} is not exactly divisible by NOCK_BASE_PER_NICK"
            ),
            Self::AmountOverflow { nicks } => {
                write!(f, "BurnForWithdrawal amount {nicks} nicks overflows u64")
            }
            Self::InvalidCalldataTrailer { reason } => {
                write!(f, "BurnForWithdrawal calldata trailer is invalid: {reason}")
            }
            Self::WrongLockRoot => write!(
                f,
                "BurnForWithdrawal lockRoot does not match tracked withdrawal recipient"
            ),
            Self::WrongAmount {
                expected_nicks,
                actual_nicks,
            } => write!(
                f,
                "BurnForWithdrawal amount {actual_nicks} nicks does not match tracked amount {expected_nicks} nicks"
            ),
            Self::RpcFailure { error } => {
                write!(f, "failed to verify withdrawal burn against Base RPC: {error}")
            }
        }
    }
}

impl std::error::Error for SequencerBaseWithdrawalRejection {}

pub(crate) fn sequencer_base_event_id_hex(base_event_id: &BaseEventId) -> String {
    format!("0x{}", hex::encode(&base_event_id.0))
}

#[async_trait]
pub trait SequencerBaseWithdrawalVerifier: Send + Sync {
    async fn verify(
        &self,
        tracked: &TrackedWithdrawalRequest,
    ) -> Result<(), SequencerBaseWithdrawalRejection>;
}

#[derive(Debug, Clone)]
struct SequencerBaseLog {
    block_number: u64,
    transaction_hash: B256,
    log_index: Option<u64>,
    transaction_input: Bytes,
    raw: RawLog,
}

#[async_trait]
trait SequencerBaseLogSource: Send + Sync {
    async fn burn_logs(
        &self,
        batch_start: u64,
        batch_end: u64,
    ) -> Result<Vec<SequencerBaseLog>, BridgeError>;
}

pub struct SequencerBaseRpcWithdrawalVerifier {
    base_height_tracker: Arc<SequencerBaseHeightTracker>,
    base_blocks_chunk: u64,
    nock_contract_address: Address,
    log_source: Arc<dyn SequencerBaseLogSource>,
}

impl SequencerBaseRpcWithdrawalVerifier {
    pub async fn connect(
        ws_url: String,
        nock_contract_address: Address,
        base_height_tracker: Arc<SequencerBaseHeightTracker>,
        base_blocks_chunk: u64,
    ) -> Result<Self, BridgeError> {
        if base_blocks_chunk == 0 {
            return Err(BridgeError::Config(
                "base_blocks_chunk must be greater than 0".into(),
            ));
        }
        let policy = BaseObserverLoopPolicy::default();
        let connect = || async {
            ProviderBuilder::<_, _, Optimism>::default()
                .connect_ws(WsConnect::new(ws_url.clone()))
                .await
        };
        let provider = connect
            .retry(policy.rpc_retry.exponential_builder())
            .notify(|err, dur| {
                warn!(
                    target: "nockchain.withdrawal_sequencer.base_verifier",
                    error = %err,
                    backoff_secs = dur.as_secs(),
                    "failed to connect sequencer Base withdrawal verifier, will retry"
                );
            })
            .await
            .map(|provider| provider.erased())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect sequencer Base withdrawal verifier at {ws_url}: {err}"
                ))
            })?;
        Ok(Self::with_log_source(
            base_height_tracker,
            base_blocks_chunk,
            nock_contract_address,
            Arc::new(RpcSequencerBaseLogSource {
                provider,
                nock_contract_address,
            }),
        ))
    }

    fn with_log_source(
        base_height_tracker: Arc<SequencerBaseHeightTracker>,
        base_blocks_chunk: u64,
        nock_contract_address: Address,
        log_source: Arc<dyn SequencerBaseLogSource>,
    ) -> Self {
        Self {
            base_height_tracker,
            base_blocks_chunk,
            nock_contract_address,
            log_source,
        }
    }

    fn claimed_batch_window(&self, base_batch_end: u64) -> (u64, u64) {
        let batch_start = base_batch_end.saturating_sub(self.base_blocks_chunk.saturating_sub(1));
        (batch_start, base_batch_end)
    }
}

#[async_trait]
impl SequencerBaseWithdrawalVerifier for SequencerBaseRpcWithdrawalVerifier {
    async fn verify(
        &self,
        tracked: &TrackedWithdrawalRequest,
    ) -> Result<(), SequencerBaseWithdrawalRejection> {
        let confirmed_base_height = self
            .base_height_tracker
            .latest_confirmed_base_height()
            .ok_or(SequencerBaseWithdrawalRejection::BaseHeightUnavailable)?;
        if tracked.base_batch_end > confirmed_base_height {
            return Err(SequencerBaseWithdrawalRejection::EventAboveConfirmed {
                base_batch_end: tracked.base_batch_end,
                confirmed_base_height,
            });
        }

        let (batch_start, batch_end) = self.claimed_batch_window(tracked.base_batch_end);
        let logs = self
            .log_source
            .burn_logs(batch_start, batch_end)
            .instrument(info_span!(
                "sequencer_verify_base_withdrawal",
                withdrawal_nonce = tracked.withdrawal_nonce,
                base_batch_end = tracked.base_batch_end
            ))
            .await
            .map_err(|err| SequencerBaseWithdrawalRejection::RpcFailure {
                error: err.to_string(),
            })?;

        for log in logs {
            let base_event_id = compute_base_event_id(&log.transaction_hash, log.log_index);
            if base_event_id != tracked.id.base_event_id {
                continue;
            }
            if is_explicitly_refunded_withdrawal_burn(
                &base_event_id, &log.transaction_hash, log.log_index,
            ) {
                return Err(SequencerBaseWithdrawalRejection::ExplicitlyRefunded {
                    base_event_id_hex: sequencer_base_event_id_hex(&base_event_id),
                });
            }
            if log.raw.address != self.nock_contract_address {
                return Err(SequencerBaseWithdrawalRejection::WrongContractAddress {
                    expected: self.nock_contract_address,
                    actual: log.raw.address,
                });
            }
            if log.block_number < batch_start || log.block_number > batch_end {
                return Err(
                    SequencerBaseWithdrawalRejection::EventOutsideClaimedBatchWindow {
                        event_block: log.block_number,
                        batch_start,
                        batch_end,
                    },
                );
            }
            let decoded = decode_burn_for_withdrawal_log_with_calldata(
                &log.raw,
                &log.transaction_hash,
                log.log_index,
                self.nock_contract_address,
                log.transaction_input.as_ref(),
            )
            .map_err(decode_error_to_rejection)?;
            if decoded.lock_root != tracked.recipient {
                return Err(SequencerBaseWithdrawalRejection::WrongLockRoot);
            }
            if decoded.amount != tracked.amount {
                return Err(SequencerBaseWithdrawalRejection::WrongAmount {
                    expected_nicks: tracked.amount,
                    actual_nicks: decoded.amount,
                });
            }
            info!(
                target: "nockchain.withdrawal_sequencer.base_verifier",
                withdrawal_nonce = tracked.withdrawal_nonce,
                base_batch_end = tracked.base_batch_end,
                "accepted withdrawal burn from Base RPC; withdrawal id.as_of is bridge kernel context, not sequencer identity"
            );
            return Ok(());
        }

        Err(SequencerBaseWithdrawalRejection::MissingBaseEventId {
            base_event_id_hex: sequencer_base_event_id_hex(&tracked.id.base_event_id),
            batch_start,
            batch_end,
        })
    }
}

fn decode_error_to_rejection(
    err: BurnForWithdrawalDecodeError,
) -> SequencerBaseWithdrawalRejection {
    let reason = err.to_string();
    match err {
        BurnForWithdrawalDecodeError::NotBurnForWithdrawal(reason) => {
            SequencerBaseWithdrawalRejection::NotBurnForWithdrawal { reason }
        }
        BurnForWithdrawalDecodeError::AmountNotDivisible { amount_raw } => {
            SequencerBaseWithdrawalRejection::AmountNotDivisible {
                amount_raw: amount_raw.to_string(),
            }
        }
        BurnForWithdrawalDecodeError::AmountOverflow { nicks } => {
            SequencerBaseWithdrawalRejection::AmountOverflow {
                nicks: nicks.to_string(),
            }
        }
        BurnForWithdrawalDecodeError::MissingCalldataTrailer { .. }
        | BurnForWithdrawalDecodeError::MalformedCalldata { .. }
        | BurnForWithdrawalDecodeError::CalldataAmountMismatch { .. }
        | BurnForWithdrawalDecodeError::CalldataCommitmentMismatch { .. }
        | BurnForWithdrawalDecodeError::CommitmentMismatch { .. }
        | BurnForWithdrawalDecodeError::InvalidLockRoot { .. } => {
            SequencerBaseWithdrawalRejection::InvalidCalldataTrailer { reason }
        }
    }
}

struct RpcSequencerBaseLogSource {
    provider: DynProvider<Optimism>,
    nock_contract_address: Address,
}

#[async_trait]
impl SequencerBaseLogSource for RpcSequencerBaseLogSource {
    async fn burn_logs(
        &self,
        batch_start: u64,
        batch_end: u64,
    ) -> Result<Vec<SequencerBaseLog>, BridgeError> {
        let filter = Filter::new()
            .address(self.nock_contract_address)
            .event_signature(burn_for_withdrawal_signature_hash())
            .from_block(batch_start)
            .to_block(batch_end);
        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .map_err(|err| BridgeError::BaseBridgeMonitoring(err.to_string()))?;
        let block_info = fetch_base_block_info(&self.provider, batch_start, batch_end).await?;
        let mut out = Vec::with_capacity(logs.len());
        for log in logs {
            let block_number = log.block_number.ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring("Base burn log missing block number".into())
            })?;
            validate_base_log_block_hash(
                &block_info, batch_start, batch_end, block_number, log.block_hash,
            )?;
            let transaction_hash = log.transaction_hash.ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring("Base burn log missing transaction hash".into())
            })?;
            let tx = self
                .provider
                .get_transaction_by_hash(transaction_hash)
                .await
                .map_err(|err| BridgeError::BaseBridgeMonitoring(err.to_string()))?
                .ok_or_else(|| {
                    BridgeError::BaseBridgeMonitoring(format!(
                        "Base burn transaction {transaction_hash:?} unavailable"
                    ))
                })?;
            out.push(SequencerBaseLog {
                block_number,
                transaction_hash,
                log_index: log.log_index,
                transaction_input: tx.input().clone(),
                raw: RawLog {
                    address: log.address(),
                    topics: log.topics().to_vec(),
                    data: log.data().data.clone(),
                },
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use alloy::primitives::{Bytes, U256};

    use super::*;
    use crate::shared::base::{
        encode_withdrawal_burn_calldata, NOCK_BASE_PER_NICK, WITHDRAWAL_BURN_BASE_CALLDATA_LEN,
    };
    use crate::shared::types::{BaseEventId, Tip5Hash};
    use crate::withdrawal::types::WithdrawalId;

    struct MockLogSource {
        logs: Mutex<Vec<SequencerBaseLog>>,
        error: Mutex<Option<String>>,
    }

    #[async_trait]
    impl SequencerBaseLogSource for MockLogSource {
        async fn burn_logs(
            &self,
            _batch_start: u64,
            _batch_end: u64,
        ) -> Result<Vec<SequencerBaseLog>, BridgeError> {
            if let Some(error) = self.error.lock().expect("mock error lock").clone() {
                return Err(BridgeError::Runtime(error));
            }
            Ok(self.logs.lock().expect("mock logs lock").clone())
        }
    }

    fn b256_from_u64(value: u64) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        B256::from(bytes)
    }

    fn address_from_u64(value: u64) -> Address {
        let mut bytes = [0u8; 20];
        bytes[12..].copy_from_slice(&value.to_be_bytes());
        Address::from(bytes)
    }

    fn address_topic(addr: Address) -> B256 {
        let mut topic = [0u8; 32];
        topic[12..].copy_from_slice(addr.as_slice());
        B256::from(topic)
    }

    fn tip5_from_b256(value: B256) -> Tip5Hash {
        let bytes: [u8; 32] = value.as_slice().try_into().expect("B256 is 32 bytes");
        Tip5Hash::from_be_bytes(&bytes)
    }

    fn burn_log(block_number: u64, amount_raw: U256, lock_root: B256) -> SequencerBaseLog {
        let tx_hash = b256_from_u64(0xabc0 + block_number);
        let log_index = Some(3);
        let nock_contract_address = Address::ZERO;
        let burner = address_from_u64(0xbeef);
        let recipient = tip5_from_b256(lock_root);
        let calldata =
            encode_withdrawal_burn_calldata(nock_contract_address, burner, amount_raw, &recipient);
        let commitment = B256::from_slice(&calldata[36..68]);
        let topics = vec![burn_for_withdrawal_signature_hash(), address_topic(burner), commitment];
        SequencerBaseLog {
            block_number,
            transaction_hash: tx_hash,
            log_index,
            transaction_input: calldata,
            raw: RawLog {
                address: nock_contract_address,
                topics,
                data: Bytes::from(amount_raw.to_be_bytes::<32>().to_vec()),
            },
        }
    }

    fn tracked_for(
        log: &SequencerBaseLog,
        amount: u64,
        lock_root: B256,
    ) -> TrackedWithdrawalRequest {
        TrackedWithdrawalRequest {
            id: WithdrawalId {
                as_of: tip5_from_b256(b256_from_u64(0x7777)),
                base_event_id: compute_base_event_id(&log.transaction_hash, log.log_index),
            },
            recipient: tip5_from_b256(lock_root),
            amount,
            base_batch_end: 109,
            withdrawal_nonce: 7,
        }
    }

    fn verifier_with_logs(
        confirmed_height: u64,
        base_blocks_chunk: u64,
        logs: Vec<SequencerBaseLog>,
    ) -> SequencerBaseRpcWithdrawalVerifier {
        let tracker = Arc::new(SequencerBaseHeightTracker::default());
        tracker.record_confirmed_base_height(confirmed_height);
        SequencerBaseRpcWithdrawalVerifier::with_log_source(
            tracker,
            base_blocks_chunk,
            Address::ZERO,
            Arc::new(MockLogSource {
                logs: Mutex::new(logs),
                error: Mutex::new(None),
            }),
        )
    }

    #[tokio::test]
    async fn verifier_accepts_matching_confirmed_burn_for_withdrawal() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        verifier.verify(&tracked).await.expect("verified burn");
    }

    #[tokio::test]
    async fn verifier_rejects_explicitly_refunded_withdrawal_burn() {
        let lock_root = b256_from_u64(0x1234);
        let mut log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        log.transaction_hash = B256::from_slice(
            &hex::decode("fa0b8e4134a387440a99544114578397d52542cea306d6b9adea801407e3123f")
                .expect("refunded tx hash hex"),
        );
        log.log_index = Some(243);
        let tracked = tracked_for(&log, 42, lock_root);
        assert_eq!(
            sequencer_base_event_id_hex(&tracked.id.base_event_id),
            "0x45cfbf831f2abf377164f857a2bc47338fcaa8f4f12a5986a3ba9bef35afeabd"
        );
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("refunded burn should be ignored");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::ExplicitlyRefunded { .. }
        ));
        assert!(err.to_string().contains("explicitly refunded"));
    }

    #[tokio::test]
    async fn verifier_rejects_matching_event_from_wrong_contract_address() {
        let lock_root = b256_from_u64(0x1234);
        let mut log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        log.raw.address = address_from_u64(0x9999);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("wrong contract address");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::WrongContractAddress { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_burn_without_full_lock_root_trailer() {
        let lock_root = b256_from_u64(0x1234);
        let mut log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        log.transaction_input =
            Bytes::from(log.transaction_input[..WITHDRAWAL_BURN_BASE_CALLDATA_LEN].to_vec());
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("missing trailer");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::InvalidCalldataTrailer { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_missing_base_event_id() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let mut tracked = tracked_for(&log, 42, lock_root);
        tracked.id.base_event_id = BaseEventId(vec![0xff; 32]);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier.verify(&tracked).await.expect_err("missing event");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::MissingBaseEventId { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_wrong_lock_root() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let mut tracked = tracked_for(&log, 42, lock_root);
        tracked.recipient = tip5_from_b256(b256_from_u64(0x9999));
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("wrong lock root");
        assert_eq!(err, SequencerBaseWithdrawalRejection::WrongLockRoot);
    }

    #[tokio::test]
    async fn verifier_rejects_wrong_amount() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(41u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier.verify(&tracked).await.expect_err("wrong amount");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::WrongAmount {
                expected_nicks: 42,
                actual_nicks: 41
            }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_amount_not_divisible_by_nock_base_per_nick() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(NOCK_BASE_PER_NICK) + U256::from(1u64),
            lock_root,
        );
        let tracked = tracked_for(&log, 1, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("fractional nick");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::AmountNotDivisible { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_event_above_confirmed_base_height() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(108, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("above confirmed");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::EventAboveConfirmed {
                base_batch_end: 109,
                confirmed_base_height: 108
            }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_event_outside_claimed_batch_window() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            99,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier.verify(&tracked).await.expect_err("outside batch");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::EventOutsideClaimedBatchWindow {
                event_block: 99,
                batch_start: 100,
                batch_end: 109
            }
        ));
    }

    #[tokio::test]
    async fn verifier_fails_closed_on_log_source_error() {
        let tracker = Arc::new(SequencerBaseHeightTracker::default());
        tracker.record_confirmed_base_height(109);
        let verifier = SequencerBaseRpcWithdrawalVerifier::with_log_source(
            tracker,
            10,
            Address::ZERO,
            Arc::new(MockLogSource {
                logs: Mutex::new(Vec::new()),
                error: Mutex::new(Some("rpc unavailable".into())),
            }),
        );
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);

        let err = verifier.verify(&tracked).await.expect_err("rpc failure");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::RpcFailure { .. }
        ));
    }
}
