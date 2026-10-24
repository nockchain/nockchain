use crate::shared::errors::BridgeError;
use crate::shared::types::Tip5Hash;
use crate::withdrawal::types::WithdrawalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalState {
    Pending,
    Assembling,
    Prepared,
    PeerCanonical,
    Authorized,
    MempoolAccepted,
    Confirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalFallbackPolicy {
    pub assembly_timeout_blocks: u64,
    pub submission_timeout_blocks: u64,
}

pub const DEFAULT_WITHDRAWAL_FALLBACK_TIMEOUT_BLOCKS: u64 = 100;

impl Default for WithdrawalFallbackPolicy {
    fn default() -> Self {
        Self {
            assembly_timeout_blocks: DEFAULT_WITHDRAWAL_FALLBACK_TIMEOUT_BLOCKS,
            submission_timeout_blocks: DEFAULT_WITHDRAWAL_FALLBACK_TIMEOUT_BLOCKS,
        }
    }
}

impl WithdrawalState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Assembling => "assembling",
            Self::Prepared => "prepared",
            Self::PeerCanonical => "peer_canonical",
            Self::Authorized => "authorized",
            Self::MempoolAccepted => "mempool_accepted",
            Self::Confirmed => "confirmed",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, BridgeError> {
        match value {
            "pending" => Ok(Self::Pending),
            "assembling" => Ok(Self::Assembling),
            "prepared" => Ok(Self::Prepared),
            "peer_canonical" => Ok(Self::PeerCanonical),
            "authorized" => Ok(Self::Authorized),
            "mempool_accepted" => Ok(Self::MempoolAccepted),
            "confirmed" => Ok(Self::Confirmed),
            other => Err(BridgeError::Runtime(format!(
                "unknown withdrawal state: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignedWithdrawalTransactionRecord {
    pub signer_node_id: u64,
    pub created_at: i64,
    pub transaction: nockchain_types::v1::Transaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWithdrawalView {
    // A live withdrawal is an active operator attempt, not merely a tracked
    // Base burn request. Pending and Confirmed rows are intentionally excluded.
    pub id: WithdrawalId,
    pub recipient: Option<Tip5Hash>,
    pub gross_burned_amount: Option<u64>,
    pub base_batch_end: Option<u64>,
    pub withdrawal_nonce: Option<u64>,
    // `current_epoch` tracks which withdrawal attempt / tx body is active.
    pub current_epoch: u64,
    pub proposal_hash: Option<String>,
    pub peer_commit_certificate: Option<Vec<u8>>,
    // Historical field name retained for compatibility; this stores the
    // submitted raw transaction id, not the stable envelope transaction name.
    pub authorized_transaction_name: Option<String>,
    // `handoff_index` rotates responsibility within `current_epoch`; it does
    // not replace the epoch as the identifier of the active attempt.
    pub handoff_index: u64,
    pub turn_started_base_height: Option<u64>,
    pub submit_attempt_count: u64,
    pub last_submit_attempt_base_height: Option<u64>,
    pub last_submit_error: Option<String>,
    pub state: WithdrawalState,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireWithdrawalAssemblyOutcome {
    Acquired,
    Busy {
        active: WithdrawalId,
    },
    AlreadyTracked {
        id: WithdrawalId,
        state: WithdrawalState,
    },
}
