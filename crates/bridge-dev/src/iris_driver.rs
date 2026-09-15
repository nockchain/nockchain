use std::str::FromStr;
use std::time::Duration;

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::rpc::types::eth::RawLog;
use async_trait::async_trait;
use bridge::shared::base::{
    burn_for_withdrawal_signature_hash, decode_burn_for_withdrawal_log_with_calldata,
    parse_withdrawal_burn_calldata, withdrawal_burn_selector,
};
use bridge::shared::e2e_environment::BASE_SEPOLIA_E2E_CHAIN_ID;
use bridge::shared::types::{WITHDRAWAL_POLICY_V1_ID, WITHDRAWAL_WIRE_V1_ID};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::time::timeout;

use crate::base_backend::{BaseBackend, BaseBackendError, TransactionReceiptFacts};
use crate::client_driver::{
    build_proof, policy_facts, validate_request, verify_reference_bytes, ClientDriverError,
    WithdrawalClientDriver, WithdrawalClientMode, WithdrawalClientOutput, WithdrawalClientRequest,
};
use crate::iris_artifact::IrisArtifact;

const REQUEST_PROTOCOL: &str = "iris-withdrawal-e2e-request-v1";
const RESULT_PROTOCOL: &str = "iris-withdrawal-e2e-result-v1";
const DEFAULT_DRIVER_TIMEOUT: Duration = Duration::from_secs(30);

pub struct IrisSdkDriver {
    artifact: IrisArtifact,
    timeout: Duration,
}

impl IrisSdkDriver {
    pub fn new(artifact: IrisArtifact) -> Self {
        Self {
            artifact,
            timeout: DEFAULT_DRIVER_TIMEOUT,
        }
    }

    pub fn with_timeout(
        artifact: IrisArtifact,
        timeout: Duration,
    ) -> Result<Self, ClientDriverError> {
        if timeout.is_zero() {
            return Err(ClientDriverError::InvalidRequest(
                "Iris driver timeout must be positive",
            ));
        }
        Ok(Self { artifact, timeout })
    }
}

#[async_trait]
impl WithdrawalClientDriver for IrisSdkDriver {
    async fn encode(
        &self,
        request: &WithdrawalClientRequest,
    ) -> Result<WithdrawalClientOutput, ClientDriverError> {
        validate_request(request)?;
        let policy = policy_facts(request.amount_base_units)?;
        let artifact = &self.artifact.facts;
        let request_json = json!({
            "protocol": REQUEST_PROTOCOL,
            "sdk_metadata": {
                "package_name": artifact.package_name,
                "package_version": artifact.package_version,
                "revision": artifact.git_revision,
            },
            "nock_token_address": format!("{:#x}", request.nock_token),
            "burner_address": format!("{:#x}", request.burner),
            "amount_base_units": request.amount_base_units.to_string(),
            "destination": {
                "kind": request.destination_kind,
                "value": request.destination_value,
            },
            "expected": {
                "wire_protocol": WITHDRAWAL_WIRE_V1_ID,
                "withdrawal_policy": WITHDRAWAL_POLICY_V1_ID,
                "nock_token_address": format!("{:#x}", request.nock_token),
                "burner_address": format!("{:#x}", request.burner),
            },
        });
        let response = timeout(
            self.timeout,
            self.artifact.driver.invoke_json(&request_json),
        )
        .await
        .map_err(|_| ClientDriverError::IrisDriver("timed out".to_owned()))?
        .map_err(|error| ClientDriverError::IrisDriver(error.to_string()))?;
        let envelope: IrisResponseEnvelope = serde_json::from_value(response)
            .map_err(|error| ClientDriverError::InvalidIrisResponse(error.to_string()))?;
        let response = match envelope {
            IrisResponseEnvelope::Success(response) if response.ok => response,
            IrisResponseEnvelope::Failure(response) if !response.ok => {
                if response.protocol != RESULT_PROTOCOL {
                    return Err(ClientDriverError::InvalidIrisResponse(
                        "failure response protocol differs".to_owned(),
                    ));
                }
                return Err(ClientDriverError::IrisFailure {
                    code: response.error.code,
                    message: response.error.message,
                });
            }
            _ => {
                return Err(ClientDriverError::InvalidIrisResponse(
                    "response ok discriminator is inconsistent".to_owned(),
                ))
            }
        };
        validate_response_identity(&response, request, &policy, artifact)?;
        let calldata = parse_calldata(&response.calldata, response.calldata_byte_length)?;
        verify_reference_bytes(request, &calldata)?;
        let (decoded_amount, commitment, decoded_root) = parse_withdrawal_burn_calldata(&calldata)
            .map_err(|error| ClientDriverError::RustDecode(error.to_string()))?;
        if decoded_amount != request.amount_base_units
            || decoded_root != request.expected_lock_root
            || format!("{commitment:#x}") != response.commitment
        {
            return Err(ClientDriverError::BindingMismatch);
        }
        let proof = build_proof(
            WithdrawalClientMode::IrisSdk,
            true,
            request,
            &policy,
            commitment,
            &calldata,
            Some(artifact.clone()),
        );
        if proof.calldata_hex != response.calldata {
            return Err(ClientDriverError::InvalidIrisResponse(
                "calldata is not canonical lowercase hex".to_owned(),
            ));
        }
        Ok(WithdrawalClientOutput {
            calldata: Bytes::from(calldata),
            proof,
        })
    }
}

fn validate_response_identity(
    response: &IrisSuccessResponse,
    request: &WithdrawalClientRequest,
    policy: &crate::client_driver::PolicyFacts,
    artifact: &crate::iris_artifact::IrisArtifactFacts,
) -> Result<(), ClientDriverError> {
    let expected_selector = format!("0x{}", hex::encode(withdrawal_burn_selector()));
    let expected_limbs = request
        .expected_lock_root
        .to_array()
        .map(|limb| limb.to_string());
    if response.protocol != RESULT_PROTOCOL
        || response.sdk_metadata.package_name != artifact.package_name
        || response.sdk_metadata.package_version != artifact.package_version
        || response.sdk_metadata.revision != artifact.git_revision
        || response.wire_protocol != WITHDRAWAL_WIRE_V1_ID
        || response.withdrawal_policy != WITHDRAWAL_POLICY_V1_ID
        || response.selector != expected_selector
        || response.destination.kind != request.destination_kind
        || response.destination.normalized != request.destination_value
        || response.destination.lock_root != request.expected_lock_root.to_base58()
        || response.destination.lock_root_limbs != expected_limbs
        || response.amount.base_units != request.amount_base_units.to_string()
        || response.amount.nicks != policy.amount_nicks.to_string()
        || response.amount.bridge_fee_nicks != policy.bridge_fee_nicks.to_string()
        || response.amount.net_after_bridge_fee_nicks
            != policy.net_after_bridge_fee_nicks.to_string()
        || !response.self_validation.valid
        || response.self_validation.decoded_wire_protocol != WITHDRAWAL_WIRE_V1_ID
        || response.self_validation.decoded_amount_base_units
            != request.amount_base_units.to_string()
        || response.self_validation.decoded_commitment != response.commitment
        || response.self_validation.decoded_lock_root_limbs != expected_limbs
    {
        return Err(ClientDriverError::InvalidIrisResponse(
            "identity, policy, destination, amount, or self-validation facts differ".to_owned(),
        ));
    }
    let commitment = B256::from_str(&response.commitment).map_err(|_| {
        ClientDriverError::InvalidIrisResponse("commitment is not bytes32 hex".to_owned())
    })?;
    if commitment == B256::ZERO {
        return Err(ClientDriverError::InvalidIrisResponse(
            "commitment must be nonzero".to_owned(),
        ));
    }
    let amount = U256::from_str(&response.amount.base_units).map_err(|_| {
        ClientDriverError::InvalidIrisResponse("amount is not decimal uint256".to_owned())
    })?;
    if amount != request.amount_base_units {
        return Err(ClientDriverError::InvalidIrisResponse(
            "amount parsing differs".to_owned(),
        ));
    }
    Ok(())
}

fn parse_calldata(value: &str, declared_length: usize) -> Result<Vec<u8>, ClientDriverError> {
    let digits = value.strip_prefix("0x").ok_or_else(|| {
        ClientDriverError::InvalidIrisResponse("calldata must be 0x hex".to_owned())
    })?;
    let calldata = hex::decode(digits).map_err(|_| {
        ClientDriverError::InvalidIrisResponse("calldata contains invalid hex".to_owned())
    })?;
    if calldata.len() != declared_length {
        return Err(ClientDriverError::InvalidIrisResponse(
            "declared calldata length differs from bytes".to_owned(),
        ));
    }
    Ok(calldata)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IrisResponseEnvelope {
    Success(IrisSuccessResponse),
    Failure(IrisFailureResponse),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrisSuccessResponse {
    protocol: String,
    ok: bool,
    sdk_metadata: IrisSdkMetadata,
    wire_protocol: String,
    withdrawal_policy: String,
    selector: String,
    destination: IrisDestinationFacts,
    amount: IrisAmountFacts,
    commitment: String,
    calldata: String,
    calldata_byte_length: usize,
    self_validation: IrisSelfValidation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrisSdkMetadata {
    package_name: String,
    package_version: String,
    revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrisDestinationFacts {
    kind: String,
    normalized: String,
    lock_root: String,
    lock_root_limbs: [String; 5],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrisAmountFacts {
    base_units: String,
    nicks: String,
    bridge_fee_nicks: String,
    net_after_bridge_fee_nicks: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrisSelfValidation {
    valid: bool,
    decoded_wire_protocol: String,
    decoded_amount_base_units: String,
    decoded_commitment: String,
    decoded_lock_root_limbs: [String; 5],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrisFailureResponse {
    protocol: String,
    ok: bool,
    error: IrisFailureFacts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrisFailureFacts {
    code: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BurnEventProof {
    pub nock_token: String,
    pub burner: String,
    pub amount_base_units: String,
    pub amount_nicks: String,
    pub commitment: String,
    pub lock_root: String,
    pub log_index: u64,
    pub base_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BurnSubmissionProof {
    pub transaction_hash: B256,
    pub block_number: u64,
    pub mined_from: Address,
    pub mined_to: Address,
    pub mined_input_hex: String,
    pub client: crate::client_driver::ClientEncodingProof,
    pub event: BurnEventProof,
    pub receipt: TransactionReceiptFacts,
}

pub async fn submit_withdrawal_burn(
    backend: &BaseBackend,
    request: &WithdrawalClientRequest,
    output: WithdrawalClientOutput,
    require_official_iris: bool,
    receipt_timeout: Duration,
) -> Result<BurnSubmissionProof, BurnSubmissionError> {
    validate_submission_binding(request, &output, require_official_iris)?;
    if receipt_timeout.is_zero() {
        return Err(BurnSubmissionError::InvalidRequest(
            "receipt timeout must be positive",
        ));
    }
    let chain_id = backend.chain_id().await?;
    if chain_id != BASE_SEPOLIA_E2E_CHAIN_ID {
        return Err(BurnSubmissionError::ChainChanged {
            expected: BASE_SEPOLIA_E2E_CHAIN_ID,
            observed: chain_id,
        });
    }
    let transaction_hash = backend
        .send_transaction(request.burner, request.nock_token, output.calldata.clone())
        .await?;
    let receipt = backend
        .wait_for_receipt(transaction_hash, receipt_timeout)
        .await?;
    if receipt.transaction_hash != transaction_hash {
        return Err(BurnSubmissionError::ReceiptReplacement {
            submitted: transaction_hash,
            observed: receipt.transaction_hash,
        });
    }
    if !receipt.success {
        return Err(BurnSubmissionError::ReceiptReverted(transaction_hash));
    }
    let mined = backend.transaction(transaction_hash).await?.ok_or(
        BurnSubmissionError::MissingMinedTransaction(transaction_hash),
    )?;
    if mined.hash != transaction_hash
        || mined.from != request.burner
        || mined.to != Some(request.nock_token)
        || mined.input != output.calldata
        || mined.block_number != Some(receipt.block_number)
    {
        return Err(BurnSubmissionError::MinedTransactionMismatch);
    }
    let event = verify_burn_event(&receipt, request, &mined.input, &output.proof)?;
    Ok(BurnSubmissionProof {
        transaction_hash,
        block_number: receipt.block_number,
        mined_from: mined.from,
        mined_to: mined
            .to
            .ok_or(BurnSubmissionError::MinedTransactionMismatch)?,
        mined_input_hex: format!("0x{}", hex::encode(&mined.input)),
        client: output.proof,
        event,
        receipt,
    })
}

pub fn verify_burn_event(
    receipt: &TransactionReceiptFacts,
    request: &WithdrawalClientRequest,
    mined_input: &[u8],
    client_proof: &crate::client_driver::ClientEncodingProof,
) -> Result<BurnEventProof, BurnSubmissionError> {
    let signature = burn_for_withdrawal_signature_hash();
    let matching_logs = receipt
        .logs
        .iter()
        .filter(|log| log.address == request.nock_token && log.topics.first() == Some(&signature))
        .collect::<Vec<_>>();
    if matching_logs.len() != 1 {
        return Err(BurnSubmissionError::EventCount(matching_logs.len()));
    }
    let event_log = matching_logs[0];
    let raw_log = RawLog {
        address: event_log.address,
        topics: event_log.topics.clone(),
        data: event_log.data.clone(),
    };
    let decoded = decode_burn_for_withdrawal_log_with_calldata(
        &raw_log,
        &receipt.transaction_hash,
        Some(event_log.log_index),
        request.nock_token,
        mined_input,
    )
    .map_err(|error| BurnSubmissionError::EventDecode(error.to_string()))?;
    let decoded_burner = Address::from(decoded.burner.0);
    let (amount_base_units, commitment, lock_root) = parse_withdrawal_burn_calldata(mined_input)
        .map_err(|error| BurnSubmissionError::EventDecode(error.to_string()))?;
    if decoded_burner != request.burner
        || decoded.amount.to_string() != client_proof.amount_nicks
        || decoded.lock_root != request.expected_lock_root
        || lock_root != request.expected_lock_root
        || amount_base_units != request.amount_base_units
        || format!("{commitment:#x}") != client_proof.commitment
    {
        return Err(BurnSubmissionError::DifferentialMismatch);
    }
    Ok(BurnEventProof {
        nock_token: format!("{:#x}", request.nock_token),
        burner: format!("{:#x}", request.burner),
        amount_base_units: amount_base_units.to_string(),
        amount_nicks: decoded.amount.to_string(),
        commitment: format!("{commitment:#x}"),
        lock_root: lock_root.to_base58(),
        log_index: event_log.log_index,
        base_event_id: format!("0x{}", hex::encode(decoded.base_event_id.0)),
    })
}

fn validate_submission_binding(
    request: &WithdrawalClientRequest,
    output: &WithdrawalClientOutput,
    require_official_iris: bool,
) -> Result<(), BurnSubmissionError> {
    verify_reference_bytes(request, &output.calldata)
        .map_err(|error| BurnSubmissionError::ClientBinding(error.to_string()))?;
    let proof = &output.proof;
    if proof.nock_token != format!("{:#x}", request.nock_token)
        || proof.burner != format!("{:#x}", request.burner)
        || proof.amount_base_units != request.amount_base_units.to_string()
        || proof.lock_root != request.expected_lock_root.to_base58()
        || proof.calldata_hex != format!("0x{}", hex::encode(&output.calldata))
        || proof.calldata_byte_length != output.calldata.len()
    {
        return Err(BurnSubmissionError::ClientBinding(
            "client proof no longer matches burn request".to_owned(),
        ));
    }
    if require_official_iris
        && (!proof.official_client
            || proof.client_mode != WithdrawalClientMode::IrisSdk
            || proof.artifact.is_none())
    {
        return Err(BurnSubmissionError::OfficialIrisRequired);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum BurnSubmissionError {
    #[error("invalid burn submission request: {0}")]
    InvalidRequest(&'static str),
    #[error("Base chain id changed after encode: expected {expected}, observed {observed}")]
    ChainChanged { expected: u64, observed: u64 },
    #[error("client binding validation failed: {0}")]
    ClientBinding(String),
    #[error("official fork E2E requires Iris SDK output with immutable artifact identity")]
    OfficialIrisRequired,
    #[error("submitted transaction {submitted:#x} was replaced by receipt {observed:#x}")]
    ReceiptReplacement { submitted: B256, observed: B256 },
    #[error("withdrawal burn transaction reverted: {0:#x}")]
    ReceiptReverted(B256),
    #[error("mined withdrawal transaction is missing: {0:#x}")]
    MissingMinedTransaction(B256),
    #[error("mined transaction from/to/input/block differs from encoded submission")]
    MinedTransactionMismatch,
    #[error("expected exactly one BurnForWithdrawal event, observed {0}")]
    EventCount(usize),
    #[error("production Rust event decoder rejected burn: {0}")]
    EventDecode(String),
    #[error("Iris response, mined input, contract event, and Rust decoder differ")]
    DifferentialMismatch,
    #[error(transparent)]
    Backend(#[from] BaseBackendError),
}
