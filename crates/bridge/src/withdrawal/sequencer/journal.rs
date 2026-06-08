use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, Signature, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use serde::{Deserialize, Serialize};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

use crate::observability::metrics;
use crate::shared::errors::BridgeError;

const JOURNAL_SCHEMA_VERSION: u16 = 1;
const DEFAULT_JOURNAL_ID: &str = "default";
pub const GENESIS_EVENT_ID: &str = "genesis";
const JOURNAL_SIGNATURE_SCHEME: &str = "eth_secp256k1_recoverable";
const OBJECT_STORE_OPERATION_TIMEOUT: Duration = Duration::from_secs(15);
const OBJECT_STORE_PUT_MAX_ATTEMPTS: usize = 3;
const OBJECT_STORE_READ_MAX_ATTEMPTS: usize = 3;

pub trait SequencerJournal: Send + Sync {
    /// Returns the remote journal id if this handle writes a durable journal.
    fn journal_id(&self) -> Option<String>;

    /// Appends one sequencer journal event before the local SQLite projection is advanced.
    fn append(&self, event: &SequencerJournalRecord) -> Result<(), BridgeError>;

    /// Lists ordered journal event objects.
    fn list(&self) -> Result<Vec<SequencerJournalObjectRef>, BridgeError>;

    /// Returns the ordered journal object key/ref for a non-genesis local cursor.
    fn object_ref_for_cursor(
        &self,
        cursor: &SequencerJournalCursor,
    ) -> Result<SequencerJournalObjectRef, BridgeError>;

    /// Returns the first ordered event object after the supplied object.
    fn first_after(
        &self,
        start_after: Option<&SequencerJournalObjectRef>,
    ) -> Result<Option<SequencerJournalObjectRef>, BridgeError>;

    /// Loads one ordered journal event object.
    fn get(
        &self,
        object_ref: &SequencerJournalObjectRef,
    ) -> Result<SequencerJournalRecord, BridgeError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalRecord {
    pub schema_version: u16,
    pub journal_id: String,
    pub sequence: u64,
    #[serde(default)]
    pub record_hash: String,
    pub event_id: String,
    #[serde(default)]
    pub previous_record_hash: String,
    pub previous_event_id: String,
    pub created_at_unix_ms: i64,
    pub event_type: SequencerJournalEventType,
    pub withdrawal: SequencerJournalWithdrawal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<SequencerJournalBaseContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nockchain: Option<SequencerJournalNockchainContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<SequencerJournalProposalContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission: Option<SequencerJournalSubmissionContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<SequencerJournalConfirmationContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<SequencerJournalSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SequencerJournalRecordIdentity<'a> {
    pub schema_version: u16,
    pub journal_id: &'a str,
    pub sequence: u64,
    pub previous_record_hash: &'a str,
    pub created_at_unix_ms: i64,
    pub event_type: &'a SequencerJournalEventType,
    pub withdrawal: &'a SequencerJournalWithdrawal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<&'a SequencerJournalBaseContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nockchain: Option<&'a SequencerJournalNockchainContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<&'a SequencerJournalProposalContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission: Option<&'a SequencerJournalSubmissionContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<&'a SequencerJournalConfirmationContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalSignature {
    pub scheme: String,
    pub signer: String,
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SequencerJournalEventType {
    WithdrawalOrdered,
    ProposalCanonicalized,
    ProposalAuthorized,
    TxSubmitted,
    TxSeenMempoolAccepted,
    MempoolRetryAttempted,
    TxConfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalWithdrawal {
    pub as_of: String,
    pub base_event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawal_nonce: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burned_amount: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_batch_end: Option<u64>,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalBaseContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_batch_end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_started_base_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_submit_attempt_base_height: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalNockchainContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_tip_height_observed_by_writer: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalProposalContext {
    pub proposal_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_jam: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_inputs: Vec<SequencerJournalInputName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_certificate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_node_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalInputName {
    pub first: String,
    pub last: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalSubmissionContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted_raw_tx_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorized_raw_tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submit_attempt_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_submit_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalConfirmationContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub included_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub included_block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_block_id: Option<String>,
}

impl SequencerJournalRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new_unsequenced(
        created_at_unix_ms: i64,
        event_type: SequencerJournalEventType,
        withdrawal: SequencerJournalWithdrawal,
        base: Option<SequencerJournalBaseContext>,
        nockchain: Option<SequencerJournalNockchainContext>,
        proposal: Option<SequencerJournalProposalContext>,
        submission: Option<SequencerJournalSubmissionContext>,
        confirmation: Option<SequencerJournalConfirmationContext>,
    ) -> Result<Self, BridgeError> {
        let mut record = Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            journal_id: DEFAULT_JOURNAL_ID.to_string(),
            sequence: 0,
            record_hash: String::new(),
            event_id: String::new(),
            previous_record_hash: GENESIS_EVENT_ID.to_string(),
            previous_event_id: GENESIS_EVENT_ID.to_string(),
            created_at_unix_ms,
            event_type,
            withdrawal,
            base,
            nockchain,
            proposal,
            submission,
            confirmation,
            signature: None,
        };
        record.refresh_event_id()?;
        Ok(record)
    }

    pub fn with_journal_id(mut self, journal_id: String) -> Result<Self, BridgeError> {
        self.journal_id = journal_id;
        self.signature = None;
        self.refresh_event_id()?;
        Ok(self)
    }

    pub fn into_ordered(
        mut self,
        journal_id: String,
        sequence: u64,
        previous_event_id: String,
    ) -> Result<Self, BridgeError> {
        if sequence == 0 {
            return Err(BridgeError::Runtime(
                "sequencer journal sequence must be greater than zero".to_string(),
            ));
        }
        if previous_event_id.trim().is_empty() {
            return Err(BridgeError::Runtime(
                "sequencer journal previous_event_id cannot be empty".to_string(),
            ));
        }
        self.journal_id = journal_id;
        self.sequence = sequence;
        self.previous_record_hash = previous_event_id.clone();
        self.previous_event_id = previous_event_id;
        self.signature = None;
        self.refresh_event_id()?;
        Ok(self)
    }

    pub fn canonical_bytes_without_auth(&self) -> Result<Vec<u8>, BridgeError> {
        serde_json::to_vec(&SequencerJournalRecordIdentity {
            schema_version: self.schema_version,
            journal_id: &self.journal_id,
            sequence: self.sequence,
            previous_record_hash: &self.previous_event_id,
            created_at_unix_ms: self.created_at_unix_ms,
            event_type: &self.event_type,
            withdrawal: &self.withdrawal,
            base: self.base.as_ref(),
            nockchain: self.nockchain.as_ref(),
            proposal: self.proposal.as_ref(),
            submission: self.submission.as_ref(),
            confirmation: self.confirmation.as_ref(),
        })
        .map_err(|err| BridgeError::Runtime(format!("failed to encode journal identity: {err}")))
    }

    pub fn canonical_bytes_without_event_id(&self) -> Result<Vec<u8>, BridgeError> {
        self.canonical_bytes_without_auth()
    }

    pub fn compute_record_hash_bytes(&self) -> Result<[u8; 32], BridgeError> {
        let bytes = self.canonical_bytes_without_auth()?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }

    pub fn compute_event_id(&self) -> Result<String, BridgeError> {
        Ok(format!(
            "b3_{}",
            hex::encode(self.compute_record_hash_bytes()?)
        ))
    }

    pub fn refresh_event_id(&mut self) -> Result<(), BridgeError> {
        self.previous_record_hash = self.previous_event_id.clone();
        let event_id = self.compute_event_id()?;
        self.record_hash = event_id.clone();
        self.event_id = event_id;
        Ok(())
    }

    pub fn object_key(&self, prefix: &str) -> Result<String, BridgeError> {
        journal_event_object_key(prefix, &self.journal_id, self.sequence)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencerJournalObjectRef {
    pub key: String,
    pub sequence: u64,
}

impl SequencerJournalObjectRef {
    fn from_cursor(prefix: &str, cursor: &SequencerJournalCursor) -> Result<Self, BridgeError> {
        Ok(Self {
            key: journal_event_object_key(prefix, &cursor.journal_id, cursor.last_sequence)?,
            sequence: cursor.last_sequence,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencerJournalCursor {
    pub journal_id: String,
    pub last_sequence: u64,
    pub last_event_id: String,
}

impl SequencerJournalCursor {
    pub fn genesis(journal_id: impl Into<String>) -> Self {
        Self {
            journal_id: journal_id.into(),
            last_sequence: 0,
            last_event_id: GENESIS_EVENT_ID.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct SequencerJournalSigner {
    wallet: PrivateKeySigner,
    verifier_address: Address,
}

impl SequencerJournalSigner {
    pub fn new(signing_key: &str, verifier_address: Address) -> Result<Self, BridgeError> {
        let key = signing_key.strip_prefix("0x").unwrap_or(signing_key);
        let wallet = PrivateKeySigner::from_str(key).map_err(|err| {
            BridgeError::Config(format!("invalid sequencer journal signing key: {err}"))
        })?;
        let signer_address = wallet.address();
        if signer_address != verifier_address {
            return Err(BridgeError::Config(format!(
                "sequencer journal signing key derives {}, but config verifier is {}",
                signer_address, verifier_address
            )));
        }
        Ok(Self {
            wallet,
            verifier_address,
        })
    }

    pub fn verifier_address(&self) -> Address {
        self.verifier_address
    }

    pub fn sign_record(
        &self,
        record: &SequencerJournalRecord,
    ) -> Result<SequencerJournalRecord, BridgeError> {
        let mut signed = record.clone();
        signed.signature = None;
        signed.refresh_event_id()?;
        let hash = B256::from(signed.compute_record_hash_bytes()?);
        let signature = self.wallet.sign_hash_sync(&hash).map_err(|err| {
            BridgeError::Runtime(format!("sequencer journal record signing failed: {err}"))
        })?;
        signed.signature = Some(SequencerJournalSignature {
            scheme: JOURNAL_SIGNATURE_SCHEME.to_string(),
            signer: self.verifier_address.to_string(),
            signature: format!("0x{}", hex::encode(signature.as_bytes())),
        });
        Ok(signed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequencerJournalStartupVerification {
    Disabled,
    Verified {
        journal_id: String,
        last_sequence: u64,
        last_event_id: String,
    },
}

#[derive(Clone)]
pub struct ObjectStoreSequencerJournalConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub prefix: String,
    pub journal_id: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub verifier_address: String,
    pub signing_key: String,
}

#[derive(Clone)]
pub struct ObjectStoreSequencerJournal {
    config: ObjectStoreSequencerJournalConfig,
    client: S3Client,
    runtime: Arc<ObjectStoreSequencerJournalRuntime>,
    signer: SequencerJournalSigner,
}

struct ObjectStoreSequencerJournalRuntime {
    runtime: Mutex<Option<Runtime>>,
}

impl ObjectStoreSequencerJournalRuntime {
    fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Mutex::new(Some(runtime)),
        }
    }

    fn block_on<T>(
        &self,
        operation: &'static str,
        future: impl Future<Output = Result<T, BridgeError>> + Send + 'static,
    ) -> Result<T, BridgeError> {
        let runtime = self.runtime.lock().map_err(|_| {
            BridgeError::Runtime(format!(
                "sequencer journal {operation} runtime lock poisoned"
            ))
        })?;
        let runtime = runtime.as_ref().ok_or_else(|| {
            BridgeError::Runtime(format!(
                "sequencer journal {operation} runtime has already shut down"
            ))
        })?;
        runtime.block_on(async move {
            tokio::time::timeout(OBJECT_STORE_OPERATION_TIMEOUT, future)
                .await
                .map_err(|_| {
                    BridgeError::Runtime(format!(
                        "sequencer journal {operation} timed out after {} seconds",
                        OBJECT_STORE_OPERATION_TIMEOUT.as_secs()
                    ))
                })?
        })
    }
}

impl Drop for ObjectStoreSequencerJournalRuntime {
    fn drop(&mut self) {
        let Some(runtime) = self
            .runtime
            .get_mut()
            .ok()
            .and_then(|runtime| runtime.take())
        else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            let _ = std::thread::spawn(move || drop(runtime)).join();
        } else {
            drop(runtime);
        }
    }
}

#[derive(Clone)]
pub enum SequencerJournalHandle {
    Disabled,
    ObjectStore(Box<ObjectStoreSequencerJournal>),
    #[cfg(test)]
    Recording(RecordingSequencerJournal),
}

impl SequencerJournalHandle {
    pub fn disabled() -> Self {
        Self::Disabled
    }

    pub fn object_store(config: ObjectStoreSequencerJournalConfig) -> Result<Self, BridgeError> {
        Ok(Self::ObjectStore(Box::new(
            ObjectStoreSequencerJournal::new(config)?,
        )))
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

impl SequencerJournal for SequencerJournalHandle {
    fn journal_id(&self) -> Option<String> {
        match self {
            Self::Disabled => None,
            Self::ObjectStore(journal) => journal.journal_id(),
            #[cfg(test)]
            Self::Recording(journal) => journal.journal_id(),
        }
    }

    fn append(&self, event: &SequencerJournalRecord) -> Result<(), BridgeError> {
        let metrics = metrics::init_metrics();
        if matches!(self, Self::Disabled) {
            record_disabled_journal_append(&metrics);
            return Ok(());
        }
        let started = Instant::now();
        let result = match self {
            Self::Disabled => unreachable!("disabled journal append returns before timing"),
            Self::ObjectStore(journal) => journal.append(event),
            #[cfg(test)]
            Self::Recording(journal) => journal.append(event),
        };
        record_journal_append_result(&metrics, started, &result);
        result
    }

    fn list(&self) -> Result<Vec<SequencerJournalObjectRef>, BridgeError> {
        match self {
            Self::Disabled => Ok(Vec::new()),
            Self::ObjectStore(journal) => journal.list(),
            #[cfg(test)]
            Self::Recording(journal) => journal.list(),
        }
    }

    fn object_ref_for_cursor(
        &self,
        cursor: &SequencerJournalCursor,
    ) -> Result<SequencerJournalObjectRef, BridgeError> {
        match self {
            Self::Disabled => Err(BridgeError::Runtime(
                "sequencer journal is disabled".to_string(),
            )),
            Self::ObjectStore(journal) => journal.object_ref_for_cursor(cursor),
            #[cfg(test)]
            Self::Recording(journal) => journal.object_ref_for_cursor(cursor),
        }
    }

    fn first_after(
        &self,
        start_after: Option<&SequencerJournalObjectRef>,
    ) -> Result<Option<SequencerJournalObjectRef>, BridgeError> {
        match self {
            Self::Disabled => Ok(None),
            Self::ObjectStore(journal) => journal.first_after(start_after),
            #[cfg(test)]
            Self::Recording(journal) => journal.first_after(start_after),
        }
    }

    fn get(
        &self,
        object_ref: &SequencerJournalObjectRef,
    ) -> Result<SequencerJournalRecord, BridgeError> {
        match self {
            Self::Disabled => Err(BridgeError::Runtime(
                "sequencer journal is disabled".to_string(),
            )),
            Self::ObjectStore(journal) => journal.get(object_ref),
            #[cfg(test)]
            Self::Recording(journal) => journal.get(object_ref),
        }
    }
}

#[cfg(test)]
#[derive(Clone)]
pub struct RecordingSequencerJournal {
    state: Arc<RecordingSequencerJournalState>,
    signer: SequencerJournalSigner,
}

#[cfg(test)]
#[derive(Default)]
struct RecordingSequencerJournalState {
    records: Mutex<Vec<SequencerJournalRecord>>,
    fail: Mutex<bool>,
}

#[cfg(test)]
impl Default for RecordingSequencerJournal {
    fn default() -> Self {
        Self {
            state: Arc::new(RecordingSequencerJournalState::default()),
            signer: test_journal_signer(),
        }
    }
}

#[cfg(test)]
impl RecordingSequencerJournal {
    pub(crate) fn handle(&self) -> SequencerJournalHandle {
        SequencerJournalHandle::Recording(self.clone())
    }

    pub(crate) fn records(&self) -> Vec<SequencerJournalRecord> {
        self.state
            .records
            .lock()
            .expect("journal records lock")
            .clone()
    }

    pub(crate) fn set_fail(&self, fail: bool) {
        *self.state.fail.lock().expect("journal fail lock") = fail;
    }

    pub(crate) fn replace_records(&self, records: Vec<SequencerJournalRecord>) {
        *self.state.records.lock().expect("journal records lock") = records;
    }
}

#[cfg(test)]
impl SequencerJournal for RecordingSequencerJournal {
    fn journal_id(&self) -> Option<String> {
        Some("recording".to_string())
    }

    fn append(&self, event: &SequencerJournalRecord) -> Result<(), BridgeError> {
        if *self.state.fail.lock().expect("journal fail lock") {
            return Err(BridgeError::Runtime(
                "remote sequencer journal unavailable".to_string(),
            ));
        }
        let signed = self.signer.sign_record(event)?;
        self.state
            .records
            .lock()
            .expect("journal records lock")
            .push(signed);
        Ok(())
    }

    fn list(&self) -> Result<Vec<SequencerJournalObjectRef>, BridgeError> {
        let mut refs = self
            .records()
            .into_iter()
            .map(|record| {
                Ok(SequencerJournalObjectRef {
                    key: record.object_key("recording")?,
                    sequence: record.sequence,
                })
            })
            .collect::<Result<Vec<_>, BridgeError>>()?;
        refs.sort_by_key(|object_ref| object_ref.sequence);
        Ok(refs)
    }

    fn object_ref_for_cursor(
        &self,
        cursor: &SequencerJournalCursor,
    ) -> Result<SequencerJournalObjectRef, BridgeError> {
        if cursor.journal_id != "recording" {
            return Err(BridgeError::Runtime(format!(
                "recording sequencer journal cursor belongs to journal {}, not recording",
                cursor.journal_id
            )));
        }
        SequencerJournalObjectRef::from_cursor("recording", cursor)
    }

    fn first_after(
        &self,
        start_after: Option<&SequencerJournalObjectRef>,
    ) -> Result<Option<SequencerJournalObjectRef>, BridgeError> {
        let start_after_key = start_after.map(|object_ref| object_ref.key.as_str());
        let mut refs = self.list()?;
        refs.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(refs.into_iter().find(|object_ref| {
            start_after_key
                .map(|key| object_ref.key.as_str() > key)
                .unwrap_or(true)
        }))
    }

    fn get(
        &self,
        object_ref: &SequencerJournalObjectRef,
    ) -> Result<SequencerJournalRecord, BridgeError> {
        let record = self
            .records()
            .into_iter()
            .find(|record| record.sequence == object_ref.sequence)
            .ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "recording sequencer journal object not found: {}",
                    object_ref.key
                ))
            })?;
        verify_journal_record_auth(&record, self.signer.verifier_address())?;
        Ok(record)
    }
}

#[cfg(test)]
const TEST_JOURNAL_SIGNING_KEY: &str =
    "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";

#[cfg(test)]
fn test_journal_signer() -> SequencerJournalSigner {
    let key = TEST_JOURNAL_SIGNING_KEY
        .strip_prefix("0x")
        .unwrap_or(TEST_JOURNAL_SIGNING_KEY);
    let wallet = PrivateKeySigner::from_str(key).expect("valid test journal signing key");
    SequencerJournalSigner::new(TEST_JOURNAL_SIGNING_KEY, wallet.address())
        .expect("construct test journal signer")
}

impl ObjectStoreSequencerJournal {
    pub fn new(config: ObjectStoreSequencerJournalConfig) -> Result<Self, BridgeError> {
        if config.endpoint.trim().is_empty() {
            return Err(BridgeError::Config(
                "sequencer journal object-store endpoint cannot be empty".to_string(),
            ));
        }
        if config.bucket.trim().is_empty() {
            return Err(BridgeError::Config(
                "sequencer journal object-store bucket cannot be empty".to_string(),
            ));
        }
        if config.journal_id.trim().is_empty() {
            return Err(BridgeError::Config(
                "sequencer journal id cannot be empty".to_string(),
            ));
        }
        if config.access_key_id.trim().is_empty() || config.secret_access_key.is_empty() {
            return Err(BridgeError::Config(
                "sequencer journal object-store credentials cannot be empty".to_string(),
            ));
        }
        if config.signing_key.trim().is_empty() {
            return Err(BridgeError::Config(
                "sequencer journal signing key is required when journal is enabled".to_string(),
            ));
        }
        let verifier_address = Address::from_str(&config.verifier_address).map_err(|err| {
            BridgeError::Config(format!("invalid sequencer journal verifier address: {err}"))
        })?;
        let signer = SequencerJournalSigner::new(&config.signing_key, verifier_address)?;
        let endpoint = normalized_object_store_endpoint(&config.endpoint)?;
        let runtime = RuntimeBuilder::new_multi_thread()
            .worker_threads(2)
            .thread_name("sequencer-journal-s3")
            .enable_all()
            .build()
            .map_err(|err| {
                BridgeError::Config(format!("failed to build sequencer journal runtime: {err}"))
            })?;
        let s3_config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint)
            .region(Region::new(config.region.clone()))
            .credentials_provider(Credentials::new(
                config.access_key_id.clone(),
                config.secret_access_key.clone(),
                None,
                None,
                "sequencer-journal",
            ))
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();
        let client = S3Client::from_conf(s3_config);
        Ok(Self {
            config,
            client,
            runtime: Arc::new(ObjectStoreSequencerJournalRuntime::new(runtime)),
            signer,
        })
    }

    fn block_on_s3<T>(
        &self,
        operation: &'static str,
        future: impl Future<Output = Result<T, BridgeError>> + Send + 'static,
    ) -> Result<T, BridgeError>
    where
        T: Send + 'static,
    {
        let runtime = self.runtime.clone();
        let run = move || runtime.block_on(operation, future);
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::spawn(run).join().map_err(|_| {
                BridgeError::Runtime(format!("sequencer journal {operation} panicked"))
            })?
        } else {
            run()
        }
    }

    fn put_object(&self, key: &str, body: Vec<u8>) -> Result<(), BridgeError> {
        let client = self.client.clone();
        let bucket = self.config.bucket.clone();
        let key = key.to_string();
        self.block_on_s3("PUT", async move {
            let mut last_error = None;
            for attempt in 1..=OBJECT_STORE_PUT_MAX_ATTEMPTS {
                let put_result = client
                    .put_object()
                    .bucket(bucket.clone())
                    .key(key.clone())
                    // Journal event keys are sequence-only, so this create-only
                    // write is the atomic duplicate-sequence guard.
                    .if_none_match("*")
                    .body(ByteStream::from(body.clone()))
                    .send()
                    .await;
                match put_result {
                    Ok(_) => return Ok(()),
                    Err(err) => {
                        let put_error = err.to_string();
                        if !is_retryable_object_store_put_error(&put_error) {
                            return Err(BridgeError::Runtime(format!(
                                "sequencer journal PUT failed: {put_error}"
                            )));
                        }

                        match client
                            .get_object()
                            .bucket(bucket.clone())
                            .key(key.clone())
                            .send()
                            .await
                        {
                            Ok(response) => {
                                let data = response.body.collect().await.map_err(|body_err| {
                                    BridgeError::Runtime(format!(
                                        "failed to read sequencer journal PUT read-back body: {body_err}"
                                    ))
                                })?;
                                if data.into_bytes().as_ref() == body.as_slice() {
                                    return Ok(());
                                }
                                return Err(BridgeError::Runtime(format!(
                                    "sequencer journal PUT failed and read-back found different object at {key}: {put_error}"
                                )));
                            }
                            Err(read_err) => {
                                last_error = Some(BridgeError::Runtime(format!(
                                    "sequencer journal PUT failed: {put_error}; read-back failed: {read_err}"
                                )));
                            }
                        }

                        if attempt < OBJECT_STORE_PUT_MAX_ATTEMPTS {
                            tokio::time::sleep(object_store_retry_delay(attempt)).await;
                        }
                    }
                }
            }
            Err(last_error.unwrap_or_else(|| {
                BridgeError::Runtime("sequencer journal PUT failed".to_string())
            }))
        })
    }

    fn get_object(&self, key: &str) -> Result<Vec<u8>, BridgeError> {
        let client = self.client.clone();
        let bucket = self.config.bucket.clone();
        let key = key.to_string();
        self.block_on_s3("GET", async move {
            let mut last_error = None;
            for attempt in 1..=OBJECT_STORE_READ_MAX_ATTEMPTS {
                match client
                    .get_object()
                    .bucket(bucket.clone())
                    .key(key.clone())
                    .send()
                    .await
                {
                    Ok(response) => match response.body.collect().await {
                        Ok(data) => return Ok(data.into_bytes().to_vec()),
                        Err(err) => {
                            let error = err.to_string();
                            if !is_retryable_object_store_error(&error) {
                                return Err(BridgeError::Runtime(format!(
                                    "failed to read sequencer journal GET body: {error}"
                                )));
                            }
                            last_error = Some(BridgeError::Runtime(format!(
                                "failed to read sequencer journal GET body: {error}"
                            )));
                        }
                    },
                    Err(err) => {
                        let error = err.to_string();
                        if !is_retryable_object_store_error(&error) {
                            return Err(BridgeError::Runtime(format!(
                                "sequencer journal GET failed: {error}"
                            )));
                        }
                        last_error = Some(BridgeError::Runtime(format!(
                            "sequencer journal GET failed: {error}"
                        )));
                    }
                }
                if attempt < OBJECT_STORE_READ_MAX_ATTEMPTS {
                    tokio::time::sleep(object_store_retry_delay(attempt)).await;
                }
            }
            Err(last_error.unwrap_or_else(|| {
                BridgeError::Runtime("sequencer journal GET failed".to_string())
            }))
        })
    }

    fn list_objects(&self, prefix: &str) -> Result<Vec<SequencerJournalObjectRef>, BridgeError> {
        let client = self.client.clone();
        let bucket = self.config.bucket.clone();
        let journal_prefix = prefix.to_string();
        let configured_prefix = self.config.prefix.clone();
        let journal_id = self.config.journal_id.clone();
        self.block_on_s3("LIST", async move {
            let mut last_error = None;
            'attempts: for attempt in 1..=OBJECT_STORE_READ_MAX_ATTEMPTS {
                let mut continuation_token = None;
                let mut object_refs = Vec::new();
                loop {
                    let mut request = client
                        .list_objects_v2()
                        .bucket(bucket.clone())
                        .prefix(journal_prefix.clone());
                    if let Some(token) = continuation_token {
                        request = request.continuation_token(token);
                    }
                    let output = match request.send().await {
                        Ok(output) => output,
                        Err(err) => {
                            let error = err.to_string();
                            if !is_retryable_object_store_error(&error) {
                                return Err(BridgeError::Runtime(format!(
                                    "sequencer journal LIST failed: {error}"
                                )));
                            }
                            last_error = Some(BridgeError::Runtime(format!(
                                "sequencer journal LIST failed: {error}"
                            )));
                            if attempt < OBJECT_STORE_READ_MAX_ATTEMPTS {
                                tokio::time::sleep(object_store_retry_delay(attempt)).await;
                                continue 'attempts;
                            }
                            break;
                        }
                    };
                    for object in output.contents() {
                        let key = object.key().ok_or_else(|| {
                            BridgeError::Runtime(
                                "sequencer journal LIST returned object without key".to_string(),
                            )
                        })?;
                        let object_ref =
                            parse_journal_object_key(key, &configured_prefix, &journal_id)?;
                        object_refs.push(object_ref);
                    }
                    let next_token = output.next_continuation_token().map(str::to_string);
                    if next_token.is_none() {
                        object_refs.sort_by_key(|object_ref| object_ref.sequence);
                        return Ok(object_refs);
                    }
                    continuation_token = next_token;
                }
            }
            Err(last_error.unwrap_or_else(|| {
                BridgeError::Runtime("sequencer journal LIST failed".to_string())
            }))
        })
    }

    fn first_object_after(
        &self,
        prefix: &str,
        start_after: Option<&str>,
    ) -> Result<Option<SequencerJournalObjectRef>, BridgeError> {
        let client = self.client.clone();
        let bucket = self.config.bucket.clone();
        let journal_prefix = prefix.to_string();
        let configured_prefix = self.config.prefix.clone();
        let journal_id = self.config.journal_id.clone();
        let start_after = start_after.map(str::to_string);
        self.block_on_s3("LIST", async move {
            let mut last_error = None;
            for attempt in 1..=OBJECT_STORE_READ_MAX_ATTEMPTS {
                let mut request = client
                    .list_objects_v2()
                    .bucket(bucket.clone())
                    .prefix(journal_prefix.clone())
                    .max_keys(1);
                if let Some(start_after) = start_after.clone() {
                    request = request.start_after(start_after);
                }
                match request.send().await {
                    Ok(output) => {
                        let mut object_refs = Vec::new();
                        for object in output.contents() {
                            let key = object.key().ok_or_else(|| {
                                BridgeError::Runtime(
                                    "sequencer journal LIST returned object without key"
                                        .to_string(),
                                )
                            })?;
                            object_refs.push(parse_journal_object_key(
                                key, &configured_prefix, &journal_id,
                            )?);
                        }
                        object_refs.sort_by(|left, right| left.key.cmp(&right.key));
                        return Ok(object_refs.into_iter().next());
                    }
                    Err(err) => {
                        let error = err.to_string();
                        if !is_retryable_object_store_error(&error) {
                            return Err(BridgeError::Runtime(format!(
                                "sequencer journal LIST failed: {error}"
                            )));
                        }
                        last_error = Some(BridgeError::Runtime(format!(
                            "sequencer journal LIST failed: {error}"
                        )));
                    }
                }
                if attempt < OBJECT_STORE_READ_MAX_ATTEMPTS {
                    tokio::time::sleep(object_store_retry_delay(attempt)).await;
                }
            }
            Err(last_error.unwrap_or_else(|| {
                BridgeError::Runtime("sequencer journal LIST failed".to_string())
            }))
        })
    }
}

fn normalized_object_store_endpoint(endpoint: &str) -> Result<String, BridgeError> {
    let endpoint = endpoint.trim_end_matches('/');
    let endpoint_url = reqwest::Url::parse(endpoint)
        .map_err(|err| BridgeError::Config(format!("invalid object-store endpoint: {err}")))?;
    if endpoint_url.path() != "/" && !endpoint_url.path().is_empty() {
        return Err(BridgeError::Config(
            "sequencer journal object-store endpoint must not include a path".to_string(),
        ));
    }
    if endpoint_url.query().is_some() {
        return Err(BridgeError::Config(
            "sequencer journal object-store endpoint must not include a query string".to_string(),
        ));
    }
    Ok(endpoint.to_string())
}

fn is_retryable_object_store_put_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("dispatch failure")
        || normalized.contains("timeout")
        || normalized.contains("timed out")
        || normalized.contains("connection")
        || normalized.contains("broken pipe")
        || normalized.contains("connection reset")
}

fn is_retryable_object_store_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("dispatch failure")
        || normalized.contains("service error")
        || normalized.contains("timeout")
        || normalized.contains("timed out")
        || normalized.contains("connection")
        || normalized.contains("broken pipe")
        || normalized.contains("connection reset")
        || normalized.contains("slowdown")
        || normalized.contains("temporarily unavailable")
}

fn object_store_retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(100 * attempt as u64)
}

impl SequencerJournal for ObjectStoreSequencerJournal {
    fn journal_id(&self) -> Option<String> {
        Some(self.config.journal_id.clone())
    }

    fn append(&self, event: &SequencerJournalRecord) -> Result<(), BridgeError> {
        if event.journal_id != self.config.journal_id {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal event belongs to journal {}, not {}",
                event.journal_id, self.config.journal_id
            )));
        }
        let signed = self.signer.sign_record(event)?;
        let body = serde_json::to_vec_pretty(&signed).map_err(|err| {
            BridgeError::Runtime(format!("failed to encode sequencer journal record: {err}"))
        })?;
        self.put_object(&signed.object_key(&self.config.prefix)?, body)
    }

    fn list(&self) -> Result<Vec<SequencerJournalObjectRef>, BridgeError> {
        self.list_objects(&journal_events_prefix(
            &self.config.prefix, &self.config.journal_id,
        ))
    }

    fn object_ref_for_cursor(
        &self,
        cursor: &SequencerJournalCursor,
    ) -> Result<SequencerJournalObjectRef, BridgeError> {
        if cursor.journal_id != self.config.journal_id {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal cursor belongs to journal {}, not {}",
                cursor.journal_id, self.config.journal_id
            )));
        }
        SequencerJournalObjectRef::from_cursor(&self.config.prefix, cursor)
    }

    fn first_after(
        &self,
        start_after: Option<&SequencerJournalObjectRef>,
    ) -> Result<Option<SequencerJournalObjectRef>, BridgeError> {
        self.first_object_after(
            &journal_events_prefix(&self.config.prefix, &self.config.journal_id),
            start_after.map(|object_ref| object_ref.key.as_str()),
        )
    }

    fn get(
        &self,
        object_ref: &SequencerJournalObjectRef,
    ) -> Result<SequencerJournalRecord, BridgeError> {
        let body = self.get_object(&object_ref.key)?;
        let record: SequencerJournalRecord = serde_json::from_slice(&body).map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to decode sequencer journal object {}: {err}",
                object_ref.key
            ))
        })?;
        verify_journal_record_auth(&record, self.signer.verifier_address())?;
        Ok(record)
    }
}

fn journal_events_key_segments(prefix: &str, journal_id: &str) -> Vec<String> {
    vec![
        sanitize_key_segment(prefix),
        "v1".to_string(),
        "journals".to_string(),
        sanitize_key_segment(journal_id),
        "events".to_string(),
    ]
}

fn journal_events_prefix(prefix: &str, journal_id: &str) -> String {
    join_key_segments(journal_events_key_segments(prefix, journal_id)) + "/"
}

fn journal_event_object_key(
    prefix: &str,
    journal_id: &str,
    sequence: u64,
) -> Result<String, BridgeError> {
    if sequence == 0 {
        return Err(BridgeError::Runtime(
            "sequencer journal object key requires an ordered record".to_string(),
        ));
    }
    let event_file = format!("{sequence:020}.json");
    let mut segments = journal_events_key_segments(prefix, journal_id);
    segments.push(event_file);
    Ok(join_key_segments(segments))
}

fn parse_journal_object_key(
    key: &str,
    prefix: &str,
    journal_id: &str,
) -> Result<SequencerJournalObjectRef, BridgeError> {
    let expected_prefix = journal_events_prefix(prefix, journal_id);
    let suffix = key.strip_prefix(&expected_prefix).ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal object key {key} is outside prefix {expected_prefix}"
        ))
    })?;
    if suffix.contains('/') {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object key {key} is not an ordered event object"
        )));
    }
    let suffix = suffix.strip_suffix(".json").ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal object key {key} does not end in .json"
        ))
    })?;
    if suffix.contains('-') {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object key {key} uses unsupported event id suffix"
        )));
    }
    let sequence = suffix.parse::<u64>().map_err(|err| {
        BridgeError::Runtime(format!(
            "sequencer journal object key {key} has invalid sequence: {err}"
        ))
    })?;
    if sequence == 0 {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object key {key} has zero sequence"
        )));
    }
    Ok(SequencerJournalObjectRef {
        key: key.to_string(),
        sequence,
    })
}

pub fn verify_journal_record_hashes(record: &SequencerJournalRecord) -> Result<(), BridgeError> {
    if record.previous_record_hash.trim().is_empty() {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal previous_record_hash is missing at sequence {}",
            record.sequence
        )));
    }
    if record.previous_record_hash != record.previous_event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal previous_record_hash mismatch at sequence {}: expected {}, found {}",
            record.sequence, record.previous_event_id, record.previous_record_hash
        )));
    }
    let computed_event_id = record.compute_event_id()?;
    if record.event_id != computed_event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal event_id mismatch at sequence {}: expected {}, found {}",
            record.sequence, computed_event_id, record.event_id
        )));
    }
    if record.record_hash != record.event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal record_hash mismatch at sequence {}: expected {}, found {}",
            record.sequence, record.event_id, record.record_hash
        )));
    }
    Ok(())
}

pub fn verify_journal_record_auth(
    record: &SequencerJournalRecord,
    verifier_address: Address,
) -> Result<(), BridgeError> {
    verify_journal_record_hashes(record)?;
    let signature = record.signature.as_ref().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal signature is missing at sequence {}",
            record.sequence
        ))
    })?;
    if signature.scheme != JOURNAL_SIGNATURE_SCHEME {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal signature uses unsupported scheme {} at sequence {}",
            signature.scheme, record.sequence
        )));
    }
    let claimed_signer = Address::from_str(&signature.signer).map_err(|err| {
        BridgeError::Runtime(format!(
            "sequencer journal signature signer is invalid at sequence {}: {err}",
            record.sequence
        ))
    })?;
    if claimed_signer != verifier_address {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal signature signer mismatch at sequence {}: expected {}, found {}",
            record.sequence, verifier_address, claimed_signer
        )));
    }
    let signature_bytes = decode_hex_exact(
        "sequencer journal signature", &signature.signature, 65, record.sequence,
    )?;
    let signature = Signature::from_raw(&signature_bytes).map_err(|err| {
        BridgeError::Runtime(format!(
            "sequencer journal signature is invalid at sequence {}: {err}",
            record.sequence
        ))
    })?;
    let record_hash = B256::from(record.compute_record_hash_bytes()?);
    let recovered = signature
        .recover_address_from_prehash(&record_hash)
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "sequencer journal signature recovery failed at sequence {}: {err}",
                record.sequence
            ))
        })?;
    if recovered != verifier_address {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal signature recovered wrong signer at sequence {}: expected {}, found {}",
            record.sequence, verifier_address, recovered
        )));
    }
    Ok(())
}

fn decode_hex_exact(
    label: &str,
    value: &str,
    expected_len: usize,
    sequence: u64,
) -> Result<Vec<u8>, BridgeError> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(trimmed).map_err(|err| {
        BridgeError::Runtime(format!(
            "{label} is invalid hex at sequence {sequence}: {err}"
        ))
    })?;
    if bytes.len() != expected_len {
        return Err(BridgeError::Runtime(format!(
            "{label} has invalid length at sequence {sequence}: expected {expected_len}, got {}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

pub fn verify_journal_continuity(records: &[SequencerJournalRecord]) -> Result<(), BridgeError> {
    let mut expected_sequence = 1_u64;
    let mut previous_event_id = GENESIS_EVENT_ID.to_string();
    for record in records {
        if record.sequence != expected_sequence {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal sequence gap: expected {}, found {}",
                expected_sequence, record.sequence
            )));
        }
        if record.previous_event_id != previous_event_id {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal previous_event_id mismatch at sequence {}: expected {}, found {}",
                record.sequence, previous_event_id, record.previous_event_id
            )));
        }
        verify_journal_record_hashes(record)?;
        previous_event_id = record.event_id.clone();
        expected_sequence = expected_sequence.saturating_add(1);
    }
    Ok(())
}

pub fn verify_remote_journal<J: SequencerJournal + ?Sized>(
    journal: &J,
) -> Result<SequencerJournalCursor, BridgeError> {
    let Some(journal_id) = journal.journal_id() else {
        return Err(BridgeError::Runtime(
            "cannot verify disabled sequencer journal".to_string(),
        ));
    };

    let mut object_refs = journal.list()?;
    object_refs.sort_by_key(|object_ref| object_ref.sequence);

    let mut previous_sequence = None;
    let mut records = Vec::with_capacity(object_refs.len());
    for object_ref in object_refs {
        if let Some(previous_sequence) = previous_sequence {
            if object_ref.sequence == previous_sequence {
                return Err(BridgeError::Runtime(format!(
                    "sequencer journal duplicate sequence {} at object {}",
                    object_ref.sequence, object_ref.key
                )));
            }
        }
        previous_sequence = Some(object_ref.sequence);

        let record = journal.get(&object_ref)?;
        if record.journal_id != journal_id {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal object {} belongs to journal {}, not {}",
                object_ref.key, record.journal_id, journal_id
            )));
        }
        if record.sequence != object_ref.sequence {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal object {} sequence mismatch: key has {}, record has {}",
                object_ref.key, object_ref.sequence, record.sequence
            )));
        }
        records.push(record);
    }

    verify_journal_continuity(&records)?;
    Ok(if let Some(record) = records.last() {
        SequencerJournalCursor {
            journal_id,
            last_sequence: record.sequence,
            last_event_id: record.event_id.clone(),
        }
    } else {
        SequencerJournalCursor::genesis(journal_id)
    })
}

pub fn verify_remote_journal_tail_at_cursor<J: SequencerJournal + ?Sized>(
    journal: &J,
    local: &SequencerJournalCursor,
) -> Result<SequencerJournalStartupVerification, BridgeError> {
    let Some(journal_id) = journal.journal_id() else {
        return Err(BridgeError::Runtime(
            "cannot verify disabled sequencer journal".to_string(),
        ));
    };
    if local.journal_id != journal_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal cursor journal mismatch: local {}, remote {}",
            local.journal_id, journal_id
        )));
    }

    if local.last_sequence == 0 {
        if local.last_event_id != GENESIS_EVENT_ID {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal genesis cursor has invalid event id: expected {}, found {}",
                GENESIS_EVENT_ID, local.last_event_id
            )));
        }
        if let Some(next) = journal.first_after(None)? {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal tail verification found local cursor behind remote: local genesis, next remote object {} has sequence {}; use store startup recovery to replay successors",
                next.key, next.sequence
            )));
        }
        return Ok(SequencerJournalStartupVerification::Verified {
            journal_id,
            last_sequence: local.last_sequence,
            last_event_id: local.last_event_id.clone(),
        });
    }

    let cursor_ref = journal.object_ref_for_cursor(local)?;
    let cursor_record = journal.get(&cursor_ref).map_err(|err| {
        BridgeError::Runtime(format!(
            "sequencer journal cursor object is missing or unreadable; local cursor may be ahead of remote: sequence {}, event {}, object {}: {err}",
            local.last_sequence, local.last_event_id, cursor_ref.key
        ))
    })?;
    verify_cursor_record(&journal_id, local, &cursor_ref, &cursor_record)?;

    if let Some(next) = journal.first_after(Some(&cursor_ref))? {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal tail verification found local cursor behind remote: local sequence {}, next remote object {} has sequence {}; use store startup recovery to replay successors",
            local.last_sequence, next.key, next.sequence
        )));
    }

    Ok(SequencerJournalStartupVerification::Verified {
        journal_id,
        last_sequence: local.last_sequence,
        last_event_id: local.last_event_id.clone(),
    })
}

fn verify_cursor_record(
    journal_id: &str,
    local: &SequencerJournalCursor,
    object_ref: &SequencerJournalObjectRef,
    record: &SequencerJournalRecord,
) -> Result<(), BridgeError> {
    if object_ref.sequence != local.last_sequence {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal cursor object {} sequence mismatch: cursor has {}, key has {}",
            object_ref.key, local.last_sequence, object_ref.sequence
        )));
    }
    if record.journal_id != journal_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object {} belongs to journal {}, not {}",
            object_ref.key, record.journal_id, journal_id
        )));
    }
    if record.sequence != local.last_sequence {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object {} sequence mismatch: cursor has {}, record has {}",
            object_ref.key, local.last_sequence, record.sequence
        )));
    }
    if record.event_id != local.last_event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object {} event mismatch: cursor has {}, record has {}",
            object_ref.key, local.last_event_id, record.event_id
        )));
    }
    verify_journal_record_hashes(record)?;
    Ok(())
}

pub fn verify_local_cursor_against_remote(
    local: &SequencerJournalCursor,
    remote: &SequencerJournalCursor,
) -> Result<SequencerJournalStartupVerification, BridgeError> {
    if local.journal_id != remote.journal_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal cursor journal mismatch: local {}, remote {}",
            local.journal_id, remote.journal_id
        )));
    }
    if local.last_sequence > remote.last_sequence {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal local cursor is ahead of remote: local sequence {}, remote sequence {}",
            local.last_sequence, remote.last_sequence
        )));
    }
    if local.last_sequence < remote.last_sequence {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal local cursor is behind remote: local sequence {}, remote sequence {}; use store startup recovery to replay successors",
            local.last_sequence, remote.last_sequence
        )));
    }
    if local.last_event_id != remote.last_event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal cursor event mismatch at sequence {}: local {}, remote {}",
            local.last_sequence, local.last_event_id, remote.last_event_id
        )));
    }

    Ok(SequencerJournalStartupVerification::Verified {
        journal_id: local.journal_id.clone(),
        last_sequence: local.last_sequence,
        last_event_id: local.last_event_id.clone(),
    })
}

fn join_key_segments<I>(segments: I) -> String
where
    I: IntoIterator<Item = String>,
{
    segments
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn sanitize_key_segment(raw: &str) -> String {
    raw.trim_matches('/')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod r2_test_support {
    use std::env;
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use tokio::sync::{Mutex, MutexGuard};

    use super::*;

    pub(crate) const R2_E2E_ENABLE_ENV: &str = "BRIDGE_R2_RUN_E2E";
    const R2_E2E_KEEP_OBJECTS_ENV: &str = "BRIDGE_R2_KEEP_OBJECTS";
    const R2_E2E_URL_ENV: &str = "BRIDGE_R2_TEST_URL";
    const R2_E2E_ENDPOINT_ENV: &str = "BRIDGE_R2_TEST_ENDPOINT";
    const R2_E2E_BUCKET_ENV: &str = "BRIDGE_R2_TEST_BUCKET";
    const R2_E2E_REGION_ENV: &str = "BRIDGE_R2_TEST_REGION";
    const R2_E2E_PREFIX_ENV: &str = "BRIDGE_R2_TEST_PREFIX";
    const R2_E2E_ACCESS_KEY_ID_ENV: &str = "BRIDGE_R2_TEST_ACCESS_KEY_ID";
    const R2_E2E_SECRET_ACCESS_KEY_ENV: &str = "BRIDGE_R2_TEST_SECRET_ACCESS_KEY";
    const R2_E2E_TOKEN_ENV: &str = "BRIDGE_R2_TEST_TOKEN";

    #[derive(Debug, Clone)]
    struct R2Endpoint {
        endpoint: String,
        account_id: String,
        bucket: String,
    }

    #[derive(Debug, Clone)]
    struct R2Credentials {
        access_key_id: String,
        secret_access_key: String,
    }

    pub(crate) fn enabled() -> bool {
        enabled_env(R2_E2E_ENABLE_ENV)
    }

    fn enabled_env(name: &str) -> bool {
        matches!(env::var(name).ok().as_deref(), Some("1" | "true" | "yes"))
    }

    fn serial_lock() -> &'static Mutex<()> {
        static R2_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        R2_TEST_LOCK.get_or_init(|| Mutex::new(()))
    }

    pub(crate) fn serial_guard() -> MutexGuard<'static, ()> {
        serial_lock().blocking_lock()
    }

    pub(crate) async fn async_serial_guard() -> MutexGuard<'static, ()> {
        serial_lock().lock().await
    }

    fn required_env(name: &str) -> String {
        env::var(name).unwrap_or_else(|_| panic!("{name} must be set when {R2_E2E_ENABLE_ENV}=1"))
    }

    fn optional_env(name: &str) -> Option<String> {
        env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn endpoint() -> R2Endpoint {
        if let Some(url) = optional_env(R2_E2E_URL_ENV) {
            let parsed = reqwest::Url::parse(&url).expect("BRIDGE_R2_TEST_URL must be a valid URL");
            let host = parsed
                .host_str()
                .expect("BRIDGE_R2_TEST_URL must include a host");
            let account_id = host
                .split_once('.')
                .map(|(account_id, _)| account_id.to_string())
                .expect("BRIDGE_R2_TEST_URL host must start with the Cloudflare account id");
            let endpoint = format!("{}://{}", parsed.scheme(), host);
            let bucket = parsed.path().trim_matches('/');
            assert!(
                !bucket.is_empty() && !bucket.contains('/'),
                "BRIDGE_R2_TEST_URL must include exactly one bucket path segment"
            );
            return R2Endpoint {
                endpoint,
                account_id,
                bucket: bucket.to_string(),
            };
        }

        let endpoint = required_env(R2_E2E_ENDPOINT_ENV);
        let parsed =
            reqwest::Url::parse(&endpoint).expect("BRIDGE_R2_TEST_ENDPOINT must be a valid URL");
        let account_id = parsed
            .host_str()
            .and_then(|host| {
                host.split_once('.')
                    .map(|(account_id, _)| account_id.to_string())
            })
            .expect("BRIDGE_R2_TEST_ENDPOINT host must start with the Cloudflare account id");
        R2Endpoint {
            endpoint,
            account_id,
            bucket: required_env(R2_E2E_BUCKET_ENV),
        }
    }

    fn cloudflare_token_id(account_id: &str, token: &str) -> String {
        let url =
            format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/tokens/verify");
        let response = reqwest::blocking::Client::new()
            .get(url)
            .bearer_auth(token)
            .send()
            .expect("failed to verify Cloudflare R2 token")
            .error_for_status()
            .expect("Cloudflare R2 token verification returned an error status")
            .json::<Value>()
            .expect("Cloudflare R2 token verification returned invalid JSON");
        assert!(
            response["success"].as_bool().unwrap_or(false),
            "Cloudflare R2 token verification failed"
        );
        let status = response["result"]["status"].as_str().unwrap_or("");
        assert_eq!(status, "active", "Cloudflare R2 token is not active");
        response["result"]["id"]
            .as_str()
            .expect("Cloudflare R2 token verification did not return a token id")
            .to_string()
    }

    fn credentials(account_id: &str) -> R2Credentials {
        if let (Some(access_key_id), Some(secret_access_key)) = (
            optional_env(R2_E2E_ACCESS_KEY_ID_ENV),
            optional_env(R2_E2E_SECRET_ACCESS_KEY_ENV),
        ) {
            return R2Credentials {
                access_key_id,
                secret_access_key,
            };
        }

        if let Some(token) = optional_env(R2_E2E_TOKEN_ENV) {
            let access_key_id = cloudflare_token_id(account_id, &token);
            let secret_access_key = format!("{:x}", Sha256::digest(token.as_bytes()));
            return R2Credentials {
                access_key_id,
                secret_access_key,
            };
        }

        R2Credentials {
            access_key_id: required_env(R2_E2E_ACCESS_KEY_ID_ENV),
            secret_access_key: required_env(R2_E2E_SECRET_ACCESS_KEY_ENV),
        }
    }

    pub(crate) fn object_store_config(test_name: &str) -> ObjectStoreSequencerJournalConfig {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_millis();
        let endpoint = endpoint();
        let credentials = credentials(&endpoint.account_id);
        let prefix_root = optional_env(R2_E2E_PREFIX_ENV)
            .unwrap_or_else(|| "withdrawal-sequencer-e2e".to_string());
        let test_name = sanitize_key_segment(test_name);
        let run_id = format!("{now}-{}", std::process::id());
        ObjectStoreSequencerJournalConfig {
            endpoint: endpoint.endpoint,
            bucket: endpoint.bucket,
            region: optional_env(R2_E2E_REGION_ENV).unwrap_or_else(|| "auto".to_string()),
            prefix: format!("{prefix_root}/{test_name}/{run_id}"),
            journal_id: format!("bridge-r2-e2e-{test_name}-{run_id}"),
            access_key_id: credentials.access_key_id,
            secret_access_key: credentials.secret_access_key,
            verifier_address: test_journal_signer().verifier_address().to_string(),
            signing_key: TEST_JOURNAL_SIGNING_KEY.to_string(),
        }
    }

    pub(crate) fn redact_error(mut message: String) -> String {
        for name in [R2_E2E_ACCESS_KEY_ID_ENV, R2_E2E_SECRET_ACCESS_KEY_ENV, R2_E2E_TOKEN_ENV] {
            if let Some(value) = optional_env(name) {
                message = message.replace(&value, "<redacted>");
            }
        }
        message
    }

    pub(crate) fn expect<T>(operation: &str, result: Result<T, BridgeError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!(
                "{operation} failed against R2 object store: {}",
                redact_error(err.to_string())
            ),
        }
    }

    fn delete_object(journal: &ObjectStoreSequencerJournal, key: String) {
        let client = journal.client.clone();
        let bucket = journal.config.bucket.clone();
        let result = journal.block_on_s3("DELETE", async move {
            client
                .delete_object()
                .bucket(bucket)
                .key(key)
                .send()
                .await
                .map_err(|err| BridgeError::Runtime(format!("R2 cleanup DELETE failed: {err}")))?;
            Ok(())
        });
        if let Err(err) = result {
            eprintln!("R2 cleanup failed: {}", redact_error(err.to_string()));
        }
    }

    pub(crate) struct Cleanup {
        journal: Option<ObjectStoreSequencerJournal>,
        keys: Vec<String>,
    }

    impl Cleanup {
        pub(crate) fn new(journal: ObjectStoreSequencerJournal) -> Self {
            Self {
                journal: Some(journal),
                keys: Vec::new(),
            }
        }
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let Some(journal) = self.journal.take() else {
                return;
            };
            if enabled_env(R2_E2E_KEEP_OBJECTS_ENV) {
                eprintln!(
                    "leaving R2 journal objects for prefix {} journal {} because {}=1",
                    journal.config.prefix, journal.config.journal_id, R2_E2E_KEEP_OBJECTS_ENV
                );
                return;
            }
            let mut keys = std::mem::take(&mut self.keys);
            let cleanup = move || {
                if let Ok(object_refs) = journal.list() {
                    keys.extend(object_refs.into_iter().map(|object_ref| object_ref.key));
                }
                keys.sort();
                keys.dedup();
                for key in keys {
                    delete_object(&journal, key);
                }
            };
            if tokio::runtime::Handle::try_current().is_ok() {
                if let Err(_panic) = std::thread::spawn(cleanup).join() {
                    eprintln!("R2 cleanup panicked");
                }
            } else {
                cleanup();
            }
        }
    }
}

fn record_disabled_journal_append(metrics: &metrics::BridgeHealthMetrics) -> usize {
    metrics
        .sequencer_withdrawal_journal_append_disabled
        .increment()
}

#[derive(Debug, Eq, PartialEq)]
enum JournalAppendMetricOutcome {
    Success { previous_count: usize },
    Error { previous_count: usize },
}

fn record_journal_append_result(
    metrics: &metrics::BridgeHealthMetrics,
    started: Instant,
    result: &Result<(), BridgeError>,
) -> JournalAppendMetricOutcome {
    metrics
        .sequencer_withdrawal_journal_append_time
        .add_timing(&started.elapsed());
    match result {
        Ok(()) => JournalAppendMetricOutcome::Success {
            previous_count: metrics
                .sequencer_withdrawal_journal_append_success
                .increment(),
        },
        Err(_) => JournalAppendMetricOutcome::Error {
            previous_count: metrics
                .sequencer_withdrawal_journal_append_error
                .increment(),
        },
    }
}

#[cfg(test)]
mod tests {
    use gnort::{MetricsRegistry, RegistryConfig};
    use serde_json::Value;

    use super::{r2_test_support as r2, *};
    use crate::observability::metrics::BridgeHealthMetrics;

    #[derive(Clone)]
    struct StaticSequencerJournal {
        journal_id: Option<String>,
        object_refs: Vec<SequencerJournalObjectRef>,
        records: Vec<SequencerJournalRecord>,
    }

    impl SequencerJournal for StaticSequencerJournal {
        fn journal_id(&self) -> Option<String> {
            self.journal_id.clone()
        }

        fn append(&self, _event: &SequencerJournalRecord) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Result<Vec<SequencerJournalObjectRef>, BridgeError> {
            Ok(self.object_refs.clone())
        }

        fn object_ref_for_cursor(
            &self,
            cursor: &SequencerJournalCursor,
        ) -> Result<SequencerJournalObjectRef, BridgeError> {
            self.object_refs
                .iter()
                .find(|object_ref| object_ref.sequence == cursor.last_sequence)
                .cloned()
                .ok_or_else(|| {
                    BridgeError::Runtime(format!(
                        "missing static journal cursor object for sequence {}",
                        cursor.last_sequence
                    ))
                })
        }

        fn first_after(
            &self,
            start_after: Option<&SequencerJournalObjectRef>,
        ) -> Result<Option<SequencerJournalObjectRef>, BridgeError> {
            let start_after_key = start_after.map(|object_ref| object_ref.key.as_str());
            let mut refs = self.object_refs.clone();
            refs.sort_by(|left, right| left.key.cmp(&right.key));
            Ok(refs.into_iter().find(|object_ref| {
                start_after_key
                    .map(|key| object_ref.key.as_str() > key)
                    .unwrap_or(true)
            }))
        }

        fn get(
            &self,
            object_ref: &SequencerJournalObjectRef,
        ) -> Result<SequencerJournalRecord, BridgeError> {
            self.records
                .iter()
                .find(|record| record.sequence == object_ref.sequence)
                .cloned()
                .ok_or_else(|| {
                    BridgeError::Runtime(format!(
                        "missing static journal record for sequence {}",
                        object_ref.sequence
                    ))
                })
        }
    }

    #[derive(Clone)]
    struct TailOnlySequencerJournal {
        journal_id: String,
        object_ref: SequencerJournalObjectRef,
        record: SequencerJournalRecord,
    }

    impl SequencerJournal for TailOnlySequencerJournal {
        fn journal_id(&self) -> Option<String> {
            Some(self.journal_id.clone())
        }

        fn append(&self, _event: &SequencerJournalRecord) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Result<Vec<SequencerJournalObjectRef>, BridgeError> {
            Err(BridgeError::Runtime(
                "full journal list should not be used during startup tail verification".to_string(),
            ))
        }

        fn object_ref_for_cursor(
            &self,
            cursor: &SequencerJournalCursor,
        ) -> Result<SequencerJournalObjectRef, BridgeError> {
            if cursor.last_sequence == self.object_ref.sequence {
                Ok(self.object_ref.clone())
            } else {
                Err(BridgeError::Runtime(
                    "tail-only cursor object not found".to_string(),
                ))
            }
        }

        fn first_after(
            &self,
            start_after: Option<&SequencerJournalObjectRef>,
        ) -> Result<Option<SequencerJournalObjectRef>, BridgeError> {
            if start_after != Some(&self.object_ref) {
                return Err(BridgeError::Runtime(
                    "startup tail verification should probe after the cursor object".to_string(),
                ));
            }
            Ok(None)
        }

        fn get(
            &self,
            object_ref: &SequencerJournalObjectRef,
        ) -> Result<SequencerJournalRecord, BridgeError> {
            if object_ref.sequence == self.record.sequence {
                Ok(self.record.clone())
            } else {
                Err(BridgeError::Runtime(
                    "tail-only object not found".to_string(),
                ))
            }
        }
    }

    fn sample_record() -> SequencerJournalRecord {
        SequencerJournalRecord::new_unsequenced(
            123_000,
            SequencerJournalEventType::ProposalAuthorized,
            SequencerJournalWithdrawal {
                as_of: "asof".to_string(),
                base_event_id: "baseevent".to_string(),
                withdrawal_nonce: Some(7),
                recipient: Some("recipient".to_string()),
                burned_amount: Some(42),
                base_batch_end: Some(44),
                epoch: 3,
            },
            Some(SequencerJournalBaseContext {
                base_batch_end: Some(44),
                turn_started_base_height: Some(45),
                last_submit_attempt_base_height: None,
            }),
            Some(SequencerJournalNockchainContext {
                snapshot_height: Some(100),
                snapshot_block_id: Some("block".to_string()),
                safe_tip_height_observed_by_writer: Some(90),
            }),
            Some(SequencerJournalProposalContext {
                proposal_hash: "proposalhashvalue".to_string(),
                amount: Some(42),
                transaction_name: Some("txname".to_string()),
                transaction_jam: Some("jam".to_string()),
                selected_inputs: vec![SequencerJournalInputName {
                    first: "first".to_string(),
                    last: "last".to_string(),
                }],
                commit_certificate: None,
                signer_node_id: None,
            }),
            Some(SequencerJournalSubmissionContext {
                submitted_raw_tx_id: Some("rawid".to_string()),
                authorized_raw_tx: Some("rawtx".to_string()),
                submit_attempt_count: Some(1),
                last_submit_error: None,
            }),
            None,
        )
        .expect("sample journal record")
    }

    fn ordered_sample_record(sequence: u64, previous_hash: String) -> SequencerJournalRecord {
        sample_record()
            .into_ordered("journal".to_string(), sequence, previous_hash)
            .expect("ordered sample record")
    }

    fn other_test_journal_signer() -> SequencerJournalSigner {
        let key = "0x59c6995e998f97a5a0044966f09453892d69e3f67122e7bd1c4ef5e6d8e0e6df";
        let wallet = PrivateKeySigner::from_str(key.strip_prefix("0x").unwrap_or(key))
            .expect("valid second test journal signing key");
        SequencerJournalSigner::new(key, wallet.address()).expect("construct second signer")
    }

    fn test_metrics() -> BridgeHealthMetrics {
        BridgeHealthMetrics::register(&MetricsRegistry::new(RegistryConfig::default()))
            .expect("metrics should register")
    }

    #[test]
    fn disabled_journal_append_metric_does_not_count_as_success() {
        let metrics = test_metrics();
        assert_eq!(record_disabled_journal_append(&metrics), 0);
        assert_eq!(record_disabled_journal_append(&metrics), 1);
    }

    #[test]
    fn real_journal_append_metrics_record_success_and_error() {
        let metrics = test_metrics();
        assert_eq!(
            record_journal_append_result(&metrics, Instant::now(), &Ok(())),
            JournalAppendMetricOutcome::Success { previous_count: 0 }
        );
        assert_eq!(
            record_journal_append_result(
                &metrics,
                Instant::now(),
                &Err(BridgeError::Runtime("boom".to_string())),
            ),
            JournalAppendMetricOutcome::Error { previous_count: 0 }
        );
    }

    #[test]
    #[ignore = "requires BRIDGE_R2_RUN_E2E=1 and R2 S3-compatible credentials"]
    fn object_store_journal_roundtrips_against_r2() {
        if !r2::enabled() {
            eprintln!(
                "skipping R2 E2E test; set {}=1 to run it",
                r2::R2_E2E_ENABLE_ENV
            );
            return;
        }
        let _r2_guard = r2::serial_guard();
        let config = r2::object_store_config("journal-roundtrip");
        let journal_id = config.journal_id.clone();
        let journal =
            ObjectStoreSequencerJournal::new(config).expect("construct R2 object-store journal");
        let _cleanup = r2::Cleanup::new(journal.clone());

        let first = sample_record()
            .into_ordered(journal_id.clone(), 1, GENESIS_EVENT_ID.to_string())
            .expect("first ordered record");
        let second = sample_record()
            .into_ordered(journal_id.clone(), 2, first.event_id.clone())
            .expect("second ordered record");

        r2::expect("append first journal record", journal.append(&first));
        r2::expect("append second journal record", journal.append(&second));

        let listed = r2::expect("list journal records", journal.list());
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].sequence, 1);
        assert_eq!(listed[1].sequence, 2);
        let loaded_first = r2::expect("get first journal record", journal.get(&listed[0]));
        let loaded_second = r2::expect("get second journal record", journal.get(&listed[1]));
        assert_eq!(loaded_first.event_id, first.event_id);
        assert_eq!(loaded_second.event_id, second.event_id);
        assert!(loaded_first.signature.is_some());
        assert!(loaded_second.signature.is_some());

        let first_after = r2::expect(
            "find successor journal record",
            journal.first_after(Some(&listed[0])),
        )
        .expect("second record should follow the first");
        assert_eq!(first_after.sequence, 2);
        let tail = r2::expect("verify remote journal", verify_remote_journal(&journal));
        assert_eq!(tail.last_sequence, 2);
        assert_eq!(tail.last_event_id, second.event_id);
        r2::expect(
            "verify remote journal tail at cursor",
            verify_remote_journal_tail_at_cursor(&journal, &tail),
        );

        let duplicate = journal.append(&first);
        assert!(
            duplicate.is_err(),
            "R2 object-store journal append unexpectedly overwrote an existing sequence object"
        );
    }

    #[test]
    fn journal_record_serializes_nested_event_only_shape() {
        let value: Value = serde_json::from_slice(
            &serde_json::to_vec(&sample_record()).expect("serialize journal record"),
        )
        .expect("decode journal json");

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["record_hash"], value["event_id"]);
        assert_eq!(value["previous_record_hash"], value["previous_event_id"]);
        assert_eq!(value["event_type"], "proposal_authorized");
        assert_eq!(value["withdrawal"]["withdrawal_nonce"], 7);
        assert_eq!(value["proposal"]["proposal_hash"], "proposalhashvalue");
        assert_eq!(value["submission"]["authorized_raw_tx"], "rawtx");
        assert!(value.get("signature").is_none());
        assert!(value.get("projection").is_none());
    }

    #[test]
    fn journal_record_signature_verifies_valid_record() {
        let signer = test_journal_signer();
        let record = ordered_sample_record(1, GENESIS_EVENT_ID.to_string());
        let signed = signer.sign_record(&record).expect("sign record");

        verify_journal_record_auth(&signed, signer.verifier_address())
            .expect("signed record verifies");
        assert!(signed.signature.is_some());
    }

    #[test]
    fn journal_record_signature_rejects_tampered_payload() {
        let signer = test_journal_signer();
        let record = ordered_sample_record(1, GENESIS_EVENT_ID.to_string());
        let mut signed = signer.sign_record(&record).expect("sign record");
        signed.withdrawal.epoch = signed.withdrawal.epoch.saturating_add(1);

        let err = verify_journal_record_auth(&signed, signer.verifier_address())
            .expect_err("tampered payload should reject");

        assert!(err.to_string().contains("event_id mismatch"));
    }

    #[test]
    fn journal_record_signature_rejects_wrong_signer() {
        let expected_signer = test_journal_signer();
        let wrong_signer = other_test_journal_signer();
        let record = ordered_sample_record(1, GENESIS_EVENT_ID.to_string());
        let signed = wrong_signer
            .sign_record(&record)
            .expect("sign with wrong key");

        let err = verify_journal_record_auth(&signed, expected_signer.verifier_address())
            .expect_err("wrong signer should reject");

        assert!(err.to_string().contains("signature signer mismatch"));
    }

    #[test]
    fn journal_record_signature_rejects_missing_signature() {
        let signer = test_journal_signer();
        let mut signed = signer
            .sign_record(&ordered_sample_record(1, GENESIS_EVENT_ID.to_string()))
            .expect("sign record");
        signed.signature = None;

        let err = verify_journal_record_auth(&signed, signer.verifier_address())
            .expect_err("missing signature should reject");

        assert!(err.to_string().contains("signature is missing"));
    }

    #[test]
    fn journal_continuity_rejects_broken_previous_record_hash() {
        let first = ordered_sample_record(1, GENESIS_EVENT_ID.to_string());
        let second = ordered_sample_record(2, GENESIS_EVENT_ID.to_string());

        let err =
            verify_journal_continuity(&[first, second]).expect_err("broken previous hash rejects");

        assert!(err.to_string().contains("previous_event_id mismatch"));
    }

    #[test]
    fn journal_event_id_is_stable_for_identical_facts() {
        let left = sample_record();
        let right = sample_record();

        assert_eq!(left.event_id, right.event_id);
    }

    #[test]
    fn journal_event_id_changes_when_facts_change() {
        let left = sample_record();
        let mut right = sample_record();
        right.withdrawal.epoch = right.withdrawal.epoch.saturating_add(1);
        right.refresh_event_id().expect("refresh event id");

        assert_ne!(left.event_id, right.event_id);
    }

    #[test]
    fn journal_object_key_uses_ordered_journal_path() {
        let record = sample_record()
            .into_ordered(
                "base-84532-bridge-test".to_string(),
                42,
                GENESIS_EVENT_ID.to_string(),
            )
            .expect("order journal record");

        let key = record.object_key("bridge/test prefix").expect("object key");

        assert_eq!(
            key,
            "bridge/test_prefix/v1/journals/base-84532-bridge-test/events/00000000000000000042.json"
        );
    }

    #[test]
    fn parse_journal_object_key_roundtrips_ordered_key() {
        let record = sample_record()
            .into_ordered(
                "base-84532-bridge-test".to_string(),
                42,
                GENESIS_EVENT_ID.to_string(),
            )
            .expect("order journal record");
        let key = record.object_key("bridge/test prefix").expect("object key");

        let parsed = parse_journal_object_key(&key, "bridge/test prefix", "base-84532-bridge-test")
            .expect("parse object key");

        assert_eq!(parsed.key, key);
        assert_eq!(parsed.sequence, 42);
    }

    #[test]
    fn journal_object_key_is_sequence_authoritative() {
        let first = sample_record()
            .into_ordered(
                "base-84532-bridge-test".to_string(),
                42,
                GENESIS_EVENT_ID.to_string(),
            )
            .expect("first ordered record");
        let mut second = first.clone();
        second.event_id = "b3_retry_with_different_event_id".to_string();

        assert_eq!(
            first.object_key("bridge/test prefix").expect("first key"),
            second.object_key("bridge/test prefix").expect("second key")
        );
    }

    #[test]
    fn parse_journal_object_key_rejects_event_id_suffix() {
        let key = "bridge/test_prefix/v1/journals/base-84532-bridge-test/events/00000000000000000042-b3_legacy.json";

        let err = parse_journal_object_key(key, "bridge/test prefix", "base-84532-bridge-test")
            .expect_err("event-id suffixed object keys are unsupported");

        assert!(err.to_string().contains("unsupported event id suffix"));
    }

    #[test]
    fn verify_journal_continuity_accepts_ordered_records() {
        let first = sample_record()
            .into_ordered("journal".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("first");
        let second = sample_record()
            .into_ordered("journal".to_string(), 2, first.event_id.clone())
            .expect("second");

        verify_journal_continuity(&[first, second]).expect("continuous records");
    }

    #[test]
    fn verify_journal_continuity_rejects_previous_event_mismatch() {
        let first = sample_record()
            .into_ordered("journal".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("first");
        let second = sample_record()
            .into_ordered("journal".to_string(), 2, GENESIS_EVENT_ID.to_string())
            .expect("second");

        let err =
            verify_journal_continuity(&[first, second]).expect_err("previous id mismatch fails");

        assert!(err.to_string().contains("previous_event_id mismatch"));
    }

    #[test]
    fn verify_remote_journal_rejects_duplicate_sequence() {
        let record = sample_record()
            .into_ordered("journal".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("ordered record");
        let journal = StaticSequencerJournal {
            journal_id: Some("journal".to_string()),
            object_refs: vec![
                SequencerJournalObjectRef {
                    key: "one".to_string(),
                    sequence: 1,
                },
                SequencerJournalObjectRef {
                    key: "two".to_string(),
                    sequence: 1,
                },
            ],
            records: vec![record],
        };

        let err = verify_remote_journal(&journal).expect_err("duplicate sequence should fail");

        assert!(err.to_string().contains("duplicate sequence 1"));
    }

    #[test]
    fn verify_remote_journal_rejects_object_record_event_id_mismatch() {
        let mut record = sample_record()
            .into_ordered("journal".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("ordered record");
        record.event_id = "wrong-event-id".to_string();
        let journal = StaticSequencerJournal {
            journal_id: Some("journal".to_string()),
            object_refs: vec![SequencerJournalObjectRef {
                key: "one".to_string(),
                sequence: 1,
            }],
            records: vec![record],
        };

        let err =
            verify_remote_journal(&journal).expect_err("computed event id mismatch should fail");

        assert!(err.to_string().contains("event_id mismatch"));
    }

    #[test]
    fn verify_local_cursor_against_remote_rejects_ahead_and_behind() {
        let remote = SequencerJournalCursor {
            journal_id: "journal".to_string(),
            last_sequence: 2,
            last_event_id: "remote".to_string(),
        };

        let behind = SequencerJournalCursor {
            journal_id: "journal".to_string(),
            last_sequence: 1,
            last_event_id: "old".to_string(),
        };
        let ahead = SequencerJournalCursor {
            journal_id: "journal".to_string(),
            last_sequence: 3,
            last_event_id: "future".to_string(),
        };

        assert!(verify_local_cursor_against_remote(&behind, &remote)
            .expect_err("behind cursor should fail")
            .to_string()
            .contains("behind remote"));
        assert!(verify_local_cursor_against_remote(&ahead, &remote)
            .expect_err("ahead cursor should fail")
            .to_string()
            .contains("ahead of remote"));
    }

    #[test]
    fn verify_remote_journal_tail_at_cursor_accepts_exact_tail() {
        let record = sample_record()
            .into_ordered("journal".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("ordered record");
        let object_ref = SequencerJournalObjectRef {
            key: record.object_key("prefix").expect("object key"),
            sequence: record.sequence,
        };
        let journal = StaticSequencerJournal {
            journal_id: Some("journal".to_string()),
            object_refs: vec![object_ref],
            records: vec![record.clone()],
        };
        let cursor = SequencerJournalCursor {
            journal_id: "journal".to_string(),
            last_sequence: 1,
            last_event_id: record.event_id.clone(),
        };

        let verification = verify_remote_journal_tail_at_cursor(&journal, &cursor)
            .expect("tail cursor should verify");

        assert_eq!(
            verification,
            SequencerJournalStartupVerification::Verified {
                journal_id: "journal".to_string(),
                last_sequence: 1,
                last_event_id: record.event_id,
            }
        );
    }

    #[test]
    fn verify_remote_journal_tail_at_cursor_does_not_require_full_list() {
        let record = sample_record()
            .into_ordered("journal".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("ordered record");
        let object_ref = SequencerJournalObjectRef {
            key: record.object_key("prefix").expect("object key"),
            sequence: record.sequence,
        };
        let journal = TailOnlySequencerJournal {
            journal_id: "journal".to_string(),
            object_ref,
            record: record.clone(),
        };
        let cursor = SequencerJournalCursor {
            journal_id: "journal".to_string(),
            last_sequence: 1,
            last_event_id: record.event_id,
        };

        verify_remote_journal_tail_at_cursor(&journal, &cursor)
            .expect("tail verification should not need full journal listing");
    }

    #[test]
    fn verify_remote_journal_tail_at_cursor_rejects_successor_object() {
        let first = sample_record()
            .into_ordered("journal".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("first");
        let second = sample_record()
            .into_ordered("journal".to_string(), 2, first.event_id.clone())
            .expect("second");
        let first_ref = SequencerJournalObjectRef {
            key: first.object_key("prefix").expect("first object key"),
            sequence: first.sequence,
        };
        let second_ref = SequencerJournalObjectRef {
            key: second.object_key("prefix").expect("second object key"),
            sequence: second.sequence,
        };
        let journal = StaticSequencerJournal {
            journal_id: Some("journal".to_string()),
            object_refs: vec![first_ref, second_ref],
            records: vec![first.clone(), second],
        };
        let cursor = SequencerJournalCursor {
            journal_id: "journal".to_string(),
            last_sequence: 1,
            last_event_id: first.event_id,
        };

        let err = verify_remote_journal_tail_at_cursor(&journal, &cursor)
            .expect_err("successor object means local cursor is behind");

        assert!(err.to_string().contains("behind remote"));
    }

    #[test]
    fn verify_remote_journal_tail_at_cursor_accepts_empty_genesis() {
        let journal = StaticSequencerJournal {
            journal_id: Some("journal".to_string()),
            object_refs: Vec::new(),
            records: Vec::new(),
        };
        let cursor = SequencerJournalCursor::genesis("journal");

        let verification = verify_remote_journal_tail_at_cursor(&journal, &cursor)
            .expect("empty remote journal should accept genesis cursor");

        assert_eq!(
            verification,
            SequencerJournalStartupVerification::Verified {
                journal_id: "journal".to_string(),
                last_sequence: 0,
                last_event_id: GENESIS_EVENT_ID.to_string(),
            }
        );
    }

    #[test]
    fn verify_remote_journal_tail_at_cursor_rejects_missing_cursor_object() {
        let journal = StaticSequencerJournal {
            journal_id: Some("journal".to_string()),
            object_refs: Vec::new(),
            records: Vec::new(),
        };
        let cursor = SequencerJournalCursor {
            journal_id: "journal".to_string(),
            last_sequence: 1,
            last_event_id: "missing".to_string(),
        };

        let err = verify_remote_journal_tail_at_cursor(&journal, &cursor)
            .expect_err("missing cursor object should fail closed");

        assert!(err
            .to_string()
            .contains("missing static journal cursor object"));
    }

    #[test]
    fn object_store_endpoint_rejects_path_and_query() {
        assert!(normalized_object_store_endpoint("https://example.com/path")
            .expect_err("endpoint path should fail")
            .to_string()
            .contains("must not include a path"));
        assert!(normalized_object_store_endpoint("https://example.com?x=1")
            .expect_err("endpoint query should fail")
            .to_string()
            .contains("must not include a query string"));
    }
}
