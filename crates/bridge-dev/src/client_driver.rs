use alloy::primitives::{Address, Bytes, U256};
use async_trait::async_trait;
use bridge::shared::base::{
    encode_withdrawal_burn_calldata, parse_withdrawal_burn_calldata, withdrawal_burn_commitment,
    WITHDRAWAL_BURN_CALLDATA_LEN,
};
use bridge::shared::types::{
    Tip5Hash, WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK,
    WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK, WITHDRAWAL_POLICY_V1_ID,
    WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS, WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK,
    WITHDRAWAL_WIRE_V1_ID,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::iris_artifact::{IrisArtifact, IrisArtifactFacts};
use crate::iris_driver::IrisSdkDriver;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalClientMode {
    RustReference,
    IrisSdk,
}

impl From<crate::e2e::E2eClientMode> for WithdrawalClientMode {
    fn from(mode: crate::e2e::E2eClientMode) -> Self {
        match mode {
            crate::e2e::E2eClientMode::RustReference => Self::RustReference,
            crate::e2e::E2eClientMode::Iris => Self::IrisSdk,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalClientRequest {
    pub nock_token: Address,
    pub burner: Address,
    pub amount_base_units: U256,
    pub destination_kind: String,
    pub destination_value: String,
    pub expected_lock_root: Tip5Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientEncodingProof {
    pub client_mode: WithdrawalClientMode,
    pub official_client: bool,
    pub wire_protocol: String,
    pub withdrawal_policy: String,
    pub nock_token: String,
    pub burner: String,
    pub amount_base_units: String,
    pub amount_nicks: String,
    pub bridge_fee_nicks: String,
    pub net_after_bridge_fee_nicks: String,
    pub destination_kind: String,
    pub destination_value: String,
    pub lock_root: String,
    pub lock_root_limbs: [String; 5],
    pub commitment: String,
    pub calldata_hex: String,
    pub calldata_byte_length: usize,
    pub artifact: Option<IrisArtifactFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalClientOutput {
    pub calldata: Bytes,
    pub proof: ClientEncodingProof,
}

#[async_trait]
pub trait WithdrawalClientDriver: Send + Sync {
    async fn encode(
        &self,
        request: &WithdrawalClientRequest,
    ) -> Result<WithdrawalClientOutput, ClientDriverError>;
}

pub enum SelectedWithdrawalClient {
    RustReference(RustReferenceDriver),
    IrisSdk(IrisSdkDriver),
}

impl SelectedWithdrawalClient {
    pub fn select(
        mode: WithdrawalClientMode,
        iris_artifact: Option<IrisArtifact>,
    ) -> Result<Self, ClientDriverError> {
        match (mode, iris_artifact) {
            (WithdrawalClientMode::RustReference, _) => {
                Ok(Self::RustReference(RustReferenceDriver))
            }
            (WithdrawalClientMode::IrisSdk, Some(artifact)) => {
                Ok(Self::IrisSdk(IrisSdkDriver::new(artifact)))
            }
            (WithdrawalClientMode::IrisSdk, None) => Err(ClientDriverError::MissingIrisArtifact),
        }
    }
}

#[async_trait]
impl WithdrawalClientDriver for SelectedWithdrawalClient {
    async fn encode(
        &self,
        request: &WithdrawalClientRequest,
    ) -> Result<WithdrawalClientOutput, ClientDriverError> {
        match self {
            Self::RustReference(driver) => driver.encode(request).await,
            Self::IrisSdk(driver) => driver.encode(request).await,
        }
    }
}

pub struct RustReferenceDriver;

#[async_trait]
impl WithdrawalClientDriver for RustReferenceDriver {
    async fn encode(
        &self,
        request: &WithdrawalClientRequest,
    ) -> Result<WithdrawalClientOutput, ClientDriverError> {
        validate_request(request)?;
        let policy = policy_facts(request.amount_base_units)?;
        let calldata = encode_withdrawal_burn_calldata(
            request.nock_token, request.burner, request.amount_base_units,
            &request.expected_lock_root,
        );
        let (decoded_amount, commitment, decoded_root) = parse_withdrawal_burn_calldata(&calldata)
            .map_err(|error| ClientDriverError::RustDecode(error.to_string()))?;
        if decoded_amount != request.amount_base_units || decoded_root != request.expected_lock_root
        {
            return Err(ClientDriverError::RustReferenceMismatch);
        }
        Ok(WithdrawalClientOutput {
            proof: build_proof(
                WithdrawalClientMode::RustReference,
                false,
                request,
                &policy,
                commitment,
                &calldata,
                None,
            ),
            calldata,
        })
    }
}

pub(crate) struct PolicyFacts {
    pub amount_nicks: u64,
    pub bridge_fee_nicks: u64,
    pub net_after_bridge_fee_nicks: u64,
}

pub(crate) fn validate_request(request: &WithdrawalClientRequest) -> Result<(), ClientDriverError> {
    if [request.nock_token, request.burner].contains(&Address::ZERO) {
        return Err(ClientDriverError::InvalidRequest(
            "token and burner must be nonzero",
        ));
    }
    if !matches!(request.destination_kind.as_str(), "lock_root" | "v1_pkh")
        || request.destination_value.trim().is_empty()
    {
        return Err(ClientDriverError::InvalidRequest(
            "destination kind/value are invalid",
        ));
    }
    if request.destination_kind == "lock_root"
        && request.destination_value != request.expected_lock_root.to_base58()
    {
        return Err(ClientDriverError::InvalidRequest(
            "direct destination does not match expected lock root",
        ));
    }
    Ok(())
}

pub(crate) fn policy_facts(amount_base_units: U256) -> Result<PolicyFacts, ClientDriverError> {
    let base_units_per_nick = U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK);
    if amount_base_units == U256::ZERO || amount_base_units % base_units_per_nick != U256::ZERO {
        return Err(ClientDriverError::InvalidAmount);
    }
    let nicks = amount_base_units / base_units_per_nick;
    if nicks > U256::from(u64::MAX) {
        return Err(ClientDriverError::InvalidAmount);
    }
    let amount_nicks = nicks.to::<u64>();
    let minimum_nicks = WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS
        .checked_mul(WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK)
        .ok_or(ClientDriverError::InvalidAmount)?;
    if amount_nicks < minimum_nicks {
        return Err(ClientDriverError::InvalidAmount);
    }
    let started_nocks = amount_nicks
        .checked_add(WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK - 1)
        .ok_or(ClientDriverError::InvalidAmount)?
        / WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK;
    let bridge_fee_nicks = started_nocks
        .checked_mul(WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK)
        .ok_or(ClientDriverError::InvalidAmount)?;
    let net_after_bridge_fee_nicks = amount_nicks
        .checked_sub(bridge_fee_nicks)
        .ok_or(ClientDriverError::InvalidAmount)?;
    if net_after_bridge_fee_nicks == 0 {
        return Err(ClientDriverError::InvalidAmount);
    }
    Ok(PolicyFacts {
        amount_nicks,
        bridge_fee_nicks,
        net_after_bridge_fee_nicks,
    })
}

pub(crate) fn build_proof(
    mode: WithdrawalClientMode,
    official_client: bool,
    request: &WithdrawalClientRequest,
    policy: &PolicyFacts,
    commitment: alloy::primitives::B256,
    calldata: &[u8],
    artifact: Option<IrisArtifactFacts>,
) -> ClientEncodingProof {
    ClientEncodingProof {
        client_mode: mode,
        official_client,
        wire_protocol: WITHDRAWAL_WIRE_V1_ID.to_owned(),
        withdrawal_policy: WITHDRAWAL_POLICY_V1_ID.to_owned(),
        nock_token: format!("{:#x}", request.nock_token),
        burner: format!("{:#x}", request.burner),
        amount_base_units: request.amount_base_units.to_string(),
        amount_nicks: policy.amount_nicks.to_string(),
        bridge_fee_nicks: policy.bridge_fee_nicks.to_string(),
        net_after_bridge_fee_nicks: policy.net_after_bridge_fee_nicks.to_string(),
        destination_kind: request.destination_kind.clone(),
        destination_value: request.destination_value.clone(),
        lock_root: request.expected_lock_root.to_base58(),
        lock_root_limbs: request
            .expected_lock_root
            .to_array()
            .map(|limb| limb.to_string()),
        commitment: format!("{commitment:#x}"),
        calldata_hex: format!("0x{}", hex::encode(calldata)),
        calldata_byte_length: calldata.len(),
        artifact,
    }
}

pub(crate) fn verify_reference_bytes(
    request: &WithdrawalClientRequest,
    calldata: &[u8],
) -> Result<(), ClientDriverError> {
    if calldata.len() != WITHDRAWAL_BURN_CALLDATA_LEN {
        return Err(ClientDriverError::InvalidCalldataLength(calldata.len()));
    }
    let reference = encode_withdrawal_burn_calldata(
        request.nock_token, request.burner, request.amount_base_units, &request.expected_lock_root,
    );
    if reference.as_ref() != calldata {
        return Err(ClientDriverError::IrisRustDivergence);
    }
    let (_, commitment, _) = parse_withdrawal_burn_calldata(calldata)
        .map_err(|error| ClientDriverError::RustDecode(error.to_string()))?;
    let expected = withdrawal_burn_commitment(
        request.nock_token, request.burner, request.amount_base_units, &request.expected_lock_root,
    );
    if commitment != expected {
        return Err(ClientDriverError::BindingMismatch);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ClientDriverError {
    #[error("invalid withdrawal client request: {0}")]
    InvalidRequest(&'static str),
    #[error("withdrawal amount violates policy v1")]
    InvalidAmount,
    #[error("Iris SDK mode requires an immutable Iris artifact")]
    MissingIrisArtifact,
    #[error("Iris driver failed: {0}")]
    IrisDriver(String),
    #[error("Iris driver returned a failure {code}: {message}")]
    IrisFailure { code: String, message: String },
    #[error("Iris response is invalid: {0}")]
    InvalidIrisResponse(String),
    #[error("calldata length must be 116 bytes, observed {0}")]
    InvalidCalldataLength(usize),
    #[error("production Rust decoder rejected calldata: {0}")]
    RustDecode(String),
    #[error("Rust reference encoder failed its own decode")]
    RustReferenceMismatch,
    #[error("Iris and Rust reference calldata differ")]
    IrisRustDivergence,
    #[error("calldata does not bind the requested token/burner/amount/destination")]
    BindingMismatch,
}
