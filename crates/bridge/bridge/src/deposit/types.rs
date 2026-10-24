use alloy::primitives::U256;
use nockchain_types::tx_engine::common::Hash as Tip5Hash;
use nockchain_types::v1::Name;
use nockchain_types::EthAddress;
use noun_serde::{NounDecode, NounEncode};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;

use crate::shared::types::{keccak256, AtomBytes};

/// Unique identifier for a deposit across all nodes.
/// Derived from the effect payload: (as_of, name).
/// This is used as a key for signature aggregation in the ProposalCache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositId {
    /// Hashchain hash from effect payload
    pub as_of: Tip5Hash,
    /// Note name with first/last from effect payload
    pub name: Name,
}

impl std::hash::Hash for DepositId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for limb in &self.as_of.0 {
            limb.0.hash(state);
        }
        for limb in &self.name.first.0 {
            limb.0.hash(state);
        }
        for limb in &self.name.last.0 {
            limb.0.hash(state);
        }
    }
}

impl DepositId {
    /// Construct a DepositId from an NockDepositRequestData effect payload.
    pub fn from_effect_payload(request: &NockDepositRequestData) -> Self {
        Self {
            as_of: request.as_of.clone(),
            name: request.name.clone(),
        }
    }

    /// Serialize DepositId to bytes for storage or transmission.
    /// Format: as_of (40 bytes: 5 × u64 BE) || name.first (40 bytes) || name.last (40 bytes).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(120);

        for limb in &self.as_of.0 {
            bytes.extend_from_slice(&limb.0.to_be_bytes());
        }
        for limb in &self.name.first.0 {
            bytes.extend_from_slice(&limb.0.to_be_bytes());
        }
        for limb in &self.name.last.0 {
            bytes.extend_from_slice(&limb.0.to_be_bytes());
        }

        bytes
    }

    /// Deserialize DepositId from bytes.
    /// Expects exactly 120 bytes: as_of (40) || name.first (40) || name.last (40)
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 120 {
            return Err(format!(
                "expected 120 bytes for DepositId, got {}",
                bytes.len()
            ));
        }

        let as_of = Tip5Hash::from_be_limb_bytes(&bytes[0..40]).map_err(|err| err.to_string())?;
        let name_first =
            Tip5Hash::from_be_limb_bytes(&bytes[40..80]).map_err(|err| err.to_string())?;
        let name_last =
            Tip5Hash::from_be_limb_bytes(&bytes[80..120]).map_err(|err| err.to_string())?;

        Ok(Self {
            as_of,
            name: Name::new(name_first, name_last),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignatureSet {
    pub eth_signatures: Vec<ByteBuf>,
    pub nock_signatures: Vec<ByteBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DepositSubmission {
    pub tx_id: Tip5Hash,
    /// First component of the nname (hash of the lock)
    pub name_first: Tip5Hash,
    /// Last component of the nname (hash of the source)
    pub name_last: Tip5Hash,
    pub recipient: [u8; 20],
    pub amount: u128,
    pub block_height: u64,
    pub as_of: Tip5Hash,
    pub nonce: u64,
    pub signatures: SignatureSet,
}

/// Nicks per NOCK on Nockchain (2^16).
const NICKS_PER_NOCK: u128 = 65_536;

/// Base unit for Nock token (10^16) - Nock.sol uses 16 decimals.
const NOCK_BASE_UNIT: u128 = 10_000_000_000_000_000;

/// Conversion factor: NOCK base units per nick.
/// 1 nick = 10^16 / 65,536 = 152,587,890,625 NOCK base units.
const NOCK_BASE_PER_NICK: u128 = NOCK_BASE_UNIT / NICKS_PER_NOCK;

/// Compute proposal hash:
/// `keccak256(abi.encodePacked(txId[0..4], name_first[0..4], name_last[0..4], recipient, amount, blockHeight, asOf[0..4], nonce))`
///
/// NOTE: `amount` is in nicks (Nockchain internal units), but the hash is computed
/// with the amount converted to NOCK base units to match the Solidity contract.
#[allow(clippy::too_many_arguments)]
pub fn compute_proposal_hash(
    tx_id: &[u64; 5],
    name_first: &[u64; 5],
    name_last: &[u64; 5],
    recipient: &[u8; 20],
    amount: u64,
    block_height: u64,
    as_of: &[u64; 5],
    nonce: u64,
) -> [u8; 32] {
    let mut encoded = Vec::new();

    for limb in tx_id {
        encoded.extend_from_slice(&limb.to_be_bytes());
    }
    for limb in name_first {
        encoded.extend_from_slice(&limb.to_be_bytes());
    }
    for limb in name_last {
        encoded.extend_from_slice(&limb.to_be_bytes());
    }
    encoded.extend_from_slice(recipient);

    let amount_nock = U256::from(amount) * U256::from(NOCK_BASE_PER_NICK);
    encoded.extend_from_slice(&amount_nock.to_be_bytes::<32>());

    let block_height_u256 = U256::from(block_height);
    encoded.extend_from_slice(&block_height_u256.to_be_bytes::<32>());

    for limb in as_of {
        encoded.extend_from_slice(&limb.to_be_bytes());
    }

    let nonce_u256 = U256::from(nonce);
    encoded.extend_from_slice(&nonce_u256.to_be_bytes::<32>());

    keccak256(&encoded)
}

/// ProposedBaseCallData contains the list of eth signature requests
/// that are being proposed for signing. This matches the Hoon type:
/// `(list nock-deposit-request)`
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct ProposedBaseCallData {
    pub requests: Vec<NockDepositRequestData>,
}

/// Nock deposit request data matching Hoon `nock-deposit-request`:
/// `[tx-id=tx-id:t name=nname:t recipient=base-addr amount=@ block-height=@ as-of=nock-hash nonce=@]`
///
/// This structure contains all fields needed to compute the keccak256 hash
/// that will be signed. The hash is computed over the ABI-encoded tuple of
/// these fields: `keccak256(abi.encode(txId, name.first, name.last, recipient, amount, blockHeight, asOf, nonce))`
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct NockDepositRequestData {
    pub tx_id: Tip5Hash,
    pub name: Name,
    pub recipient: EthAddress,
    pub amount: u64,
    pub block_height: u64,
    pub as_of: Tip5Hash,
    pub nonce: u64,
}

impl NockDepositRequestData {
    pub fn compute_proposal_hash(&self) -> [u8; 32] {
        let tx_id_limbs = self.tx_id.to_array();
        let name_first_limbs = self.name.first.to_array();
        let name_last_limbs = self.name.last.to_array();
        let as_of_limbs = self.as_of.to_array();

        compute_proposal_hash(
            &tx_id_limbs,
            &name_first_limbs,
            &name_last_limbs,
            self.recipient.as_bytes(),
            self.amount,
            self.block_height,
            &as_of_limbs,
            self.nonce,
        )
    }
}

/// Nock deposit request as emitted by the kernel (nonce-free).
///
/// Matches the Hoon type:
/// `[tx-id=tx-id:t name=nname:t recipient=base-addr amount=@ block-height=@ as-of=nock-hash]`
///
/// The Rust runtime assigns `nonce` deterministically and constructs the final
/// `NockDepositRequestData` used for proposal hashing, signing, and contract submission.
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct NockDepositRequestKernelData {
    pub tx_id: Tip5Hash,
    pub name: Name,
    pub recipient: EthAddress,
    pub amount: u64,
    pub block_height: u64,
    pub as_of: Tip5Hash,
}

/// Deposit from unsettled-deposits in Hoon bridge state.
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct Deposit {
    pub tx_id: Tip5Hash,
    pub nname: Tip5Hash,
    pub dest: Option<EthAddress>,
    pub raw_amount: u64,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BaseDepositSettlementEntry {
    pub base_tx_id: AtomBytes,
    pub settlement: DepositSettlement,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct DepositSettlement {
    pub base_tx_id: AtomBytes,
    pub data: DepositSettlementData,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct DepositSettlementData {
    pub counterpart: nockchain_types::tx_engine::common::TxId,
    pub as_of: nockchain_types::tx_engine::common::Hash,
    pub dest: AtomBytes,
    pub settled_amount: u64,
    pub fees: Vec<DepositSettlementFee>,
    pub bridge_fee: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct DepositSettlementFee {
    pub address: AtomBytes,
    pub amount: u64,
}
