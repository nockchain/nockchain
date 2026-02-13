use alloy::primitives::U256;
use nockchain_math::belt::Belt;
pub use nockchain_types::tx_engine::common::Hash as Tip5Hash;
use nockchain_types::tx_engine::common::Hash as NockPkh;
use nockchain_types::v1::Name;
pub use nockchain_types::EthAddress;
use nockvm::noun::{Noun, NounAllocator};
use noun_serde::{NounDecode, NounEncode};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use tiny_keccak::{Hasher, Keccak};

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
        // Hash as_of
        for limb in &self.as_of.0 {
            limb.0.hash(state);
        }
        // Hash name.first
        for limb in &self.name.first.0 {
            limb.0.hash(state);
        }
        // Hash name.last
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
    /// Format: as_of (40 bytes: 5 × u64 BE) || name.first (40 bytes) || name.last (40 bytes) = 120 bytes total
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(120);

        // Encode as_of (5 × u64 big-endian = 40 bytes)
        for limb in &self.as_of.0 {
            bytes.extend_from_slice(&limb.0.to_be_bytes());
        }

        // Encode name.first (5 × u64 big-endian = 40 bytes)
        for limb in &self.name.first.0 {
            bytes.extend_from_slice(&limb.0.to_be_bytes());
        }

        // Encode name.last (5 × u64 big-endian = 40 bytes)
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

        // Parse as_of (first 40 bytes)
        let as_of = Tip5Hash::from_be_limb_bytes(&bytes[0..40]).map_err(|err| err.to_string())?;

        // Parse name.first (next 40 bytes)
        let name_first =
            Tip5Hash::from_be_limb_bytes(&bytes[40..80]).map_err(|err| err.to_string())?;

        // Parse name.last (last 40 bytes)
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

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    hasher.update(data);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

/// Nicks per NOCK on Nockchain (2^16)
const NICKS_PER_NOCK: u128 = 65_536;

/// Base unit for Nock token (10^16) - Nock.sol uses 16 decimals
const NOCK_BASE_UNIT: u128 = 10_000_000_000_000_000;

/// Conversion factor: NOCK base units per nick
/// 1 nick = 10^16 / 65,536 = 152,587,890,625 NOCK base units
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

    // Convert nicks to NOCK base units to match Solidity contract
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

/// Ethereum ECDSA signature (r, s, v)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthSignatureParts {
    pub r: [u8; 32],
    pub s: [u8; 32],
    pub v: u64,
}

impl EthSignatureParts {
    pub fn validate(&self) -> Result<(), String> {
        let is_zero = |arr: &[u8; 32]| arr.iter().all(|&b| b == 0);

        if is_zero(&self.r) {
            return Err("r component cannot be zero".to_string());
        }

        if is_zero(&self.s) {
            return Err("s component cannot be zero".to_string());
        }

        if self.v != 27 && self.v != 28 {
            return Err(format!("v component must be 27 or 28, got {}", self.v));
        }

        Ok(())
    }
}

impl NounEncode for EthSignatureParts {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        let r_atom = unsafe {
            let mut ia = nockvm::noun::IndirectAtom::new_raw_bytes(allocator, 32, self.r.as_ptr());
            ia.normalize_as_atom().as_noun()
        };
        let s_atom = unsafe {
            let mut ia = nockvm::noun::IndirectAtom::new_raw_bytes(allocator, 32, self.s.as_ptr());
            ia.normalize_as_atom().as_noun()
        };
        let v_atom = nockvm::noun::Atom::new(allocator, self.v).as_noun();
        let inner = nockvm::noun::T(allocator, &[s_atom, v_atom]);
        nockvm::noun::T(allocator, &[r_atom, inner])
    }
}

impl NounDecode for EthSignatureParts {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let c0 = noun
            .as_cell()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
        let r_bytes = c0
            .head()
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?
            .to_be_bytes();
        let c1 = c0
            .tail()
            .as_cell()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
        let s_bytes = c1
            .head()
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?
            .to_be_bytes();
        let v = c1
            .tail()
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?
            .as_u64()
            .map_err(|_| noun_serde::NounDecodeError::Custom("Expected small atom".into()))?;

        fn to_fixed_32(mut b: Vec<u8>) -> [u8; 32] {
            if b.len() > 32 {
                b = b.split_off(b.len() - 32);
            } else if b.len() < 32 {
                let mut pad = vec![0u8; 32 - b.len()];
                pad.extend_from_slice(&b);
                b = pad;
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(&b);
            out
        }

        Ok(EthSignatureParts {
            r: to_fixed_32(r_bytes),
            s: to_fixed_32(s_bytes),
            v,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteArray<const N: usize>(pub [u8; N]);

impl<const N: usize> NounEncode for ByteArray<N> {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        let mut atoms = Vec::new();
        for &byte in &self.0 {
            atoms.push(nockvm::noun::Atom::new(allocator, byte as u64).as_noun());
        }

        let mut result = nockvm::noun::D(0);
        for atom in atoms.into_iter().rev() {
            result = nockvm::noun::T(allocator, &[atom, result]);
        }
        result
    }
}

impl<const N: usize> NounDecode for ByteArray<N> {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let mut bytes = Vec::new();
        let mut current = *noun;

        while let Ok(cell) = current.as_cell() {
            let head = cell.head();
            let byte = head
                .as_atom()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?
                .as_u64()
                .map_err(|_| {
                    noun_serde::NounDecodeError::Custom("Invalid byte value".to_string())
                })?;

            if byte > 255 {
                return Err(noun_serde::NounDecodeError::Custom(
                    "Byte value too large".to_string(),
                ));
            }

            bytes.push(byte as u8);
            current = cell.tail();
        }

        if let Ok(atom) = current.as_atom() {
            if atom.as_u64()? != 0 {
                return Err(noun_serde::NounDecodeError::Custom(
                    "Invalid list termination".to_string(),
                ));
            }
        } else {
            return Err(noun_serde::NounDecodeError::ExpectedAtom);
        }

        if bytes.len() != N {
            return Err(noun_serde::NounDecodeError::Custom(format!(
                "Expected {} bytes, got {}",
                N,
                bytes.len()
            )));
        }

        let mut array = [0u8; N];
        array.copy_from_slice(&bytes);
        Ok(ByteArray(array))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomBytes(pub Vec<u8>);

impl AtomBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::Deref for AtomBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[u8]> for AtomBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for AtomBytes {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl NounEncode for AtomBytes {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        if self.0.is_empty() {
            return nockvm::noun::Atom::new(allocator, 0).as_noun();
        }
        unsafe {
            let mut ia =
                nockvm::noun::IndirectAtom::new_raw_bytes(allocator, self.0.len(), self.0.as_ptr());
            ia.normalize_as_atom().as_noun()
        }
    }
}

impl NounDecode for AtomBytes {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let atom = noun
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?;
        let bytes = atom.as_ne_bytes();
        let len = bytes
            .iter()
            .rposition(|&b| b != 0)
            .map(|i| i + 1)
            .unwrap_or(0);
        Ok(Self(bytes[..len].to_vec()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchnorrSecretKey(pub [Belt; 8]);

impl SchnorrSecretKey {
    pub fn limbs(&self) -> &[Belt; 8] {
        &self.0
    }
}

impl From<[Belt; 8]> for SchnorrSecretKey {
    fn from(value: [Belt; 8]) -> Self {
        Self(value)
    }
}

impl NounEncode for SchnorrSecretKey {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        encode_belt_array(&self.0, allocator)
    }
}

impl NounDecode for SchnorrSecretKey {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        Ok(SchnorrSecretKey(decode_belt_array(noun)?))
    }
}

/// Bridge constants matching Hoon `bridge-constants` type.
/// These are static parameters that configure bridge behavior.
#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct BridgeConstants {
    /// Version tag (always 0 for now)
    pub version: u64,
    /// Minimum signatures required (default: 3)
    pub min_signers: u64,
    /// Total number of bridge nodes (default: 5)
    pub total_signers: u64,
    /// Minimum nocks for a bridge event (default: 1_000_000)
    pub minimum_event_nocks: u64,
    /// Fee per nock in nicks (default: 195)
    pub nicks_fee_per_nock: u64,
    /// Base blocks per chunk (default: 100)
    pub base_blocks_chunk: u64,
    /// Base chain start height (default: 33_387_036)
    pub base_start_height: u64,
    /// Nockchain start height (default: 25)
    pub nockchain_start_height: u64,
}

impl Default for BridgeConstants {
    fn default() -> Self {
        Self {
            version: 0,
            min_signers: 3,
            total_signers: 5,
            minimum_event_nocks: 1_000_000,
            nicks_fee_per_nock: 195,
            base_blocks_chunk: 100,
            base_start_height: 39_694_000,
            nockchain_start_height: 46_810,
        }
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BridgeCause(pub u64, pub BridgeCauseVariant);

impl BridgeCause {
    pub fn cfg_load(config: Option<NodeConfig>) -> Self {
        Self(0, BridgeCauseVariant::ConfigLoad(config))
    }

    pub fn set_constants(constants: BridgeConstants) -> Self {
        Self(0, BridgeCauseVariant::SetConstants(constants))
    }

    pub fn stop(last: StopLastBlocks) -> Self {
        Self(0, BridgeCauseVariant::Stop(last))
    }

    pub fn start() -> Self {
        Self(0, BridgeCauseVariant::Start(NullTag))
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BaseCallSigData(pub EthSignatureParts, pub AtomBytes);

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub enum BridgeCauseVariant {
    #[noun(tag = "base-blocks")]
    BaseBlocks(RawBaseBlocks),

    #[noun(tag = "nockchain-block")]
    NockchainBlock(NockchainBlockCause),

    #[noun(tag = "proposed-base-call")]
    ProposedBaseCall(ProposedBaseCallData),

    #[noun(tag = "proposed-nock-tx")]
    ProposedNockTx(nockchain_types::v1::RawTx),

    #[noun(tag = "base-call-sig")]
    BaseCallSig(BaseCallSigData),

    #[noun(tag = "cfg-load")]
    ConfigLoad(Option<NodeConfig>),

    #[noun(tag = "set-constants")]
    SetConstants(BridgeConstants),

    #[noun(tag = "stop")]
    Stop(StopLastBlocks),

    #[noun(tag = "start")]
    Start(NullTag),
}

// TODO: generalize this or move it up into the types crate
#[derive(Debug, Clone, Copy, Default)]
pub struct NullTag;

impl NounEncode for NullTag {
    fn to_noun<A: NounAllocator>(&self, _allocator: &mut A) -> Noun {
        nockvm::noun::D(0)
    }
}

impl NounDecode for NullTag {
    fn from_noun(noun: &Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let atom = noun
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?;
        if atom.as_u64()? == 0 {
            Ok(NullTag)
        } else {
            Err(noun_serde::NounDecodeError::Custom(
                "expected ~ (null), got non-zero atom".into(),
            ))
        }
    }
}

pub type RawBaseBlocks = Vec<RawBaseBlockEntry>;

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct RawBaseBlockEntry {
    pub height: u64,
    pub block_id: AtomBytes,
    pub parent_block_id: AtomBytes,
    pub txs: Vec<BaseEvent>,
}

#[derive(Clone)]
pub struct NockchainBlockCause {
    pub page_slab: nockapp::noun::slab::NounSlab<nockapp::noun::slab::NockJammer>,
    pub page_noun: nockvm::noun::Noun,
    pub txs: NockchainTxsMap,
}

impl std::fmt::Debug for NockchainBlockCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NockchainBlockCause")
            .field("txs", &self.txs)
            .finish()
    }
}

impl NockchainBlockCause {
    pub fn new(
        page_slab: nockapp::noun::slab::NounSlab<nockapp::noun::slab::NockJammer>,
        page_noun: nockvm::noun::Noun,
        txs: NockchainTxsMap,
    ) -> Self {
        Self {
            page_slab,
            page_noun,
            txs,
        }
    }
}

impl NounEncode for NockchainBlockCause {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        use nockapp::noun::NounAllocatorExt;

        let page_noun = allocator.copy_into(self.page_noun);
        let txs_noun = self.txs.to_noun(allocator);
        nockvm::noun::T(allocator, &[page_noun, txs_noun])
    }
}

impl NounDecode for NockchainBlockCause {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        use nockapp::noun::slab::{NockJammer, NounSlab};

        let cell = noun
            .as_cell()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;

        let mut page_slab: NounSlab<NockJammer> = NounSlab::new();
        let page_noun = page_slab.copy_into(cell.head());

        let txs = NockchainTxsMap::from_noun(&cell.tail())?;

        Ok(Self {
            page_slab,
            page_noun,
            txs,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NockchainTxsMap(pub Vec<(nockchain_types::tx_engine::common::TxId, Tx)>);

impl NounEncode for NockchainTxsMap {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        use nockchain_math::zoon::common::DefaultTipHasher;
        use nockchain_math::zoon::zmap;
        self.0.iter().fold(nockvm::noun::D(0), |acc, (tx_id, tx)| {
            let mut key = tx_id.to_noun(allocator);
            let mut value = tx.to_noun(allocator);
            zmap::z_map_put(allocator, &acc, &mut key, &mut value, &DefaultTipHasher)
                .expect("failed to encode txs map")
        })
    }
}

impl NounDecode for NockchainTxsMap {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        fn traverse(
            node: &nockvm::noun::Noun,
            acc: &mut Vec<(nockchain_types::tx_engine::common::TxId, Tx)>,
        ) -> Result<(), noun_serde::NounDecodeError> {
            if let Ok(atom) = node.as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(noun_serde::NounDecodeError::ExpectedCell);
            }
            let cell = node
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            let kv = cell
                .head()
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            let tx_id = nockchain_types::tx_engine::common::TxId::from_noun(&kv.head())?;
            let tx = Tx::from_noun(&kv.tail())?;
            acc.push((tx_id, tx));
            let branches = cell
                .tail()
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            traverse(&branches.head(), acc)?;
            traverse(&branches.tail(), acc)?;
            Ok(())
        }
        let mut acc = Vec::new();
        traverse(noun, &mut acc)?;
        Ok(NockchainTxsMap(acc))
    }
}

#[derive(Debug, Clone)]
pub enum Tx {
    V1(TxV1),
}

impl NounEncode for Tx {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        match self {
            Tx::V1(tx) => tx.to_noun(allocator),
        }
    }
}

impl NounDecode for Tx {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let cell = noun
            .as_cell()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
        let tag = cell
            .head()
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?
            .as_u64()
            .map_err(|_| noun_serde::NounDecodeError::Custom("tx tag too large".into()))?;
        match tag {
            1 => Ok(Tx::V1(TxV1::from_noun(noun)?)),
            _ => Err(noun_serde::NounDecodeError::Custom(format!(
                "unsupported tx version: {}",
                tag
            ))),
        }
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct TxV1 {
    pub version: u64,
    pub raw_tx: nockchain_types::v1::RawTx,
    pub total_size: u64,
    pub outputs: OutputsV1,
}

#[derive(Debug, Clone)]
pub struct OutputsV1(pub Vec<OutputV1>);

impl NounEncode for OutputsV1 {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        use nockchain_math::zoon::common::DefaultTipHasher;
        use nockchain_math::zoon::zset;
        self.0.iter().fold(nockvm::noun::D(0), |acc, output| {
            let mut value = output.to_noun(allocator);
            zset::z_set_put(allocator, &acc, &mut value, &DefaultTipHasher)
                .expect("failed to encode outputs set")
        })
    }
}

impl NounDecode for OutputsV1 {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        fn traverse(
            node: &nockvm::noun::Noun,
            acc: &mut Vec<OutputV1>,
        ) -> Result<(), noun_serde::NounDecodeError> {
            if let Ok(atom) = node.as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(noun_serde::NounDecodeError::ExpectedCell);
            }
            let cell = node
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            acc.push(OutputV1::from_noun(&cell.head())?);
            let branches = cell
                .tail()
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            traverse(&branches.head(), acc)?;
            traverse(&branches.tail(), acc)?;
            Ok(())
        }
        let mut acc = Vec::new();
        traverse(noun, &mut acc)?;
        Ok(OutputsV1(acc))
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct OutputV1 {
    pub note: nockchain_types::v1::Note,
    pub seeds: nockchain_types::v1::Seeds,
}

/// ProposedBaseCallData contains the list of eth signature requests
/// that are being proposed for signing. This matches the Hoon type:
/// `(list nock-deposit-request)`
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct ProposedBaseCallData {
    pub requests: Vec<NockDepositRequestData>,
}

#[derive(Debug, Clone)]
pub struct BaseBlockRef {
    pub height: u64,
    pub block_id: AtomBytes,
    pub parent_block_id: AtomBytes,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BaseWithdrawalEntry {
    pub base_tx_id: AtomBytes,
    pub withdrawal: Withdrawal,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct Withdrawal {
    pub base_tx_id: AtomBytes,
    pub dest: Option<Tip5Hash>,
    pub raw_amount: u64,
}

/// Deposit from unsettled-deposits in Hoon bridge state.
/// Matches the Hoon type:
/// ```hoon
/// $:  =tx-id
///     =nname
///     dest=(unit base-addr)
///     raw-amount=coins
/// ==
/// ```
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
pub struct BaseEvent {
    pub base_event_id: AtomBytes,
    pub content: BaseEventContent,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub enum BaseEventContent {
    #[noun(tag = "deposit-processed")]
    DepositProcessed {
        nock_tx_id: Tip5Hash,
        note_name: Name,
        recipient: EthAddress,
        amount: u64,
        block_height: u64,
        as_of: Tip5Hash,
        nonce: u64,
    },
    #[noun(tag = "bridge-node-updated")]
    BridgeNodeUpdated(NullTag),
    #[noun(tag = "burn-for-withdrawal")]
    BurnForWithdrawal {
        burner: EthAddress,
        amount: u64,
        lock_root: Tip5Hash,
    },
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct NodeConfig {
    pub node_id: u64,
    pub nodes: Vec<NodeInfo>,
    pub my_eth_key: AtomBytes,
    pub my_nock_key: SchnorrSecretKey,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct NodeInfo {
    pub ip: String,
    pub eth_pubkey: AtomBytes,
    /// Nockchain public key hash (PKH) - base58 encoded ~52 chars
    pub nock_pkh: NockPkh,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BridgeEffect {
    // TODO: I have no idea what the tag is doing, it doesn't seem to have an effect on the decoding result
    //#[noun(tag = "0")]
    pub version: u64,
    #[noun(flatten)]
    pub variant: BridgeEffectVariant,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BaseCallData(pub Vec<EthSignatureParts>, pub AtomBytes);

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct CommitNockDepositsData(
    pub nockchain_types::tx_engine::common::SchnorrPubkey,
    pub AtomBytes,
);

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct ProposeNockchainTxData(
    pub nockchain_types::tx_engine::common::SchnorrPubkey,
    pub AtomBytes,
);

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct StopTipBase {
    pub base_hash: Tip5Hash,
    pub height: u64,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct StopTipNock {
    pub nock_hash: Tip5Hash,
    pub height: u64,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct StopLastBlocks {
    pub base: StopTipBase,
    pub nock: StopTipNock,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct StopEffectData {
    pub reason: String,
    pub last: StopLastBlocks,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub enum BridgeEffectVariant {
    #[noun(tag = "base-call")]
    BaseCall(BaseCallData),

    #[noun(tag = "assemble-base-call")]
    AssembleBaseCall(DepositSettlementData),

    #[noun(tag = "nock-deposit-request")]
    NockDepositRequest(NockDepositRequestKernelData),

    #[noun(tag = "commit-nock-deposits")]
    CommitNockDeposits(Vec<NockDepositRequestKernelData>),

    #[noun(tag = "nockchain-tx")]
    NockchainTx(NockchainTxData),

    #[noun(tag = "propose-nockchain-tx")]
    ProposeNockchainTx(ProposeNockchainTxData),

    #[noun(tag = "grpc")]
    Grpc(GrpcEffect),

    #[noun(tag = "stop")]
    Stop(StopEffectData),
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

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct NockchainTxData {
    pub tx: nockchain_types::v1::RawTx,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub enum GrpcEffect {
    #[noun(tag = "peek")]
    Peek(GrpcPeekData),
    #[noun(tag = "call")]
    Call(GrpcCallData),
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct GrpcPeekData {
    pub pid: u64,
    pub typ: AtomBytes,
    pub path: Vec<AtomBytes>,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct GrpcCallData {
    pub ip: String,
    pub method: AtomBytes,
    pub data: AtomBytes,
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

pub type NounDigest = Tip5Hash;

pub fn zero_tip5_hash() -> Tip5Hash {
    Tip5Hash([Belt(0); 5])
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct HeightPeek {
    pub inner: Option<Option<u64>>,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct HoldInfo {
    pub hash: Tip5Hash,
    pub height: u64,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct HoldPeek {
    pub inner: Option<Option<HoldInfo>>,
}

/// Peek response for unsettled deposit lookup.
/// Matches Hoon peek response: `[~ [~ deposit]]` or `[~ ~]`
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct DepositPeek {
    pub inner: Option<Option<Deposit>>,
}

/// Peek response for count queries (deposits, withdrawals).
/// Matches Hoon peek response: `[~ ~ @ud]`
/// The structure is (unit (unit @ud))
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct CountPeek {
    pub inner: Option<Option<u64>>,
}

/// Peek response for boolean queries (hold status).
/// Matches Hoon peek response: `[~ ~ ?]`
/// The structure is (unit (unit ?))
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BoolPeek {
    pub inner: Option<Option<bool>>,
}

/// Peek response for stop-info.
/// Matches Hoon peek response: `[~ ~ stop-info]`
/// The structure is (unit (unit stop-info))
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct StopInfoPeek {
    pub inner: Option<Option<StopLastBlocks>>,
}

/// Peek response for lists of nock deposit requests.
/// Matches Hoon peek response: `[~ ~ (list nock-deposit-request)]`
/// The structure is (unit (unit (list nock-deposit-request))).
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct NockDepositRequestsPeek {
    pub inner: Option<Option<Vec<NockDepositRequestKernelData>>>,
}

/// Aggregated kernel state counts for TUI display.
#[derive(Debug, Clone, Default)]
pub struct BridgeState {
    /// Number of deposits awaiting settlement on Base
    pub unsettled_deposits: u64,
    /// Number of withdrawals awaiting settlement on Nockchain
    pub unsettled_withdrawals: u64,
    /// Latest observed Base tip hash from the driver (hex with 0x prefix).
    pub base_tip_hash: Option<String>,
    /// Next base hashchain height (kernel expects next block height).
    pub base_next_height: Option<u64>,
    /// Next nock hashchain height (kernel expects next block height).
    pub nock_next_height: Option<u64>,
    /// Whether base chain processing is held waiting for nock
    pub base_hold: bool,
    /// Whether nock chain processing is held waiting for base
    pub nock_hold: bool,
    /// Whether the kernel has latched a stop state.
    pub kernel_stopped: bool,
    /// Whether the kernel is in fakenet mode (true) or mainnet mode (false).
    /// None indicates the status hasn't been fetched yet.
    pub is_fakenet: Option<bool>,
    /// Counterparty nock height that releases the base hold.
    pub base_hold_height: Option<u64>,
    /// Counterparty base height that releases the nock hold.
    pub nock_hold_height: Option<u64>,
}

fn encode_belt_array<const N: usize, A: NounAllocator>(
    limbs: &[Belt; N],
    allocator: &mut A,
) -> Noun {
    let mut tail = limbs[N - 1].to_noun(allocator);
    for limb in limbs[..N - 1].iter().rev() {
        let head = limb.to_noun(allocator);
        tail = nockvm::noun::T(allocator, &[head, tail]);
    }
    tail
}

fn decode_belt_array<const N: usize>(
    noun: &Noun,
) -> Result<[Belt; N], noun_serde::NounDecodeError> {
    let mut result = [Belt(0); N];
    let mut current = *noun;
    for (idx, item) in result.iter_mut().enumerate() {
        if idx == N - 1 {
            *item = Belt::from_noun(&current)?;
        } else {
            let cell = current
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            *item = Belt::from_noun(&cell.head())?;
            current = cell.tail();
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::Belt;
    use noun_serde::{NounDecode, NounEncode};
    use tracing::{debug, info};

    use super::*;

    fn init_test_logging() {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();
    }

    fn sample_base_blocks_cause() -> RawBaseBlocks {
        vec![RawBaseBlockEntry {
            height: 12345,
            block_id: AtomBytes(vec![0xde, 0xad, 0xbe, 0xef]),
            parent_block_id: AtomBytes(vec![0xca, 0xfe, 0xba, 0xbe]),
            txs: vec![],
        }]
    }

    fn sample_nockchain_block_cause() -> NockchainBlockCause {
        use nockchain_types::tx_engine::common::{BigNum, CoinbaseSplit, Hash as NockHash, Page};
        use noun_serde::NounEncode;

        let page = Page {
            digest: NockHash([Belt(0); 5]),
            pow: None,
            parent: NockHash([Belt(0); 5]),
            tx_ids: vec![],
            coinbase: CoinbaseSplit::V0(vec![]),
            timestamp: 0,
            epoch_counter: 0,
            target: BigNum::from_u64(0),
            accumulated_work: BigNum::from_u64(0),
            height: 0,
            msg: vec![],
        };

        let mut page_slab: NounSlab<NockJammer> = NounSlab::new();
        let page_noun = page.to_noun(&mut page_slab);

        NockchainBlockCause::new(page_slab, page_noun, NockchainTxsMap(vec![]))
    }

    #[test]
    fn test_cause_cfg_load_none_roundtrip() {
        init_test_logging();
        info!("Starting cfg-load (None) cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let original_cause = BridgeCause::cfg_load(None);
        debug!("Created original cause with None config");

        let encoded_noun = original_cause.to_noun(&mut allocator);
        info!("Encoded cause to noun");

        let decoded_cause = BridgeCause::from_noun(&encoded_noun)
            .expect("Failed to decode cfg-load cause from noun");
        debug!("Decoded cause successfully");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::ConfigLoad(config) => {
                assert!(config.is_none(), "Config should be None");
                info!("cfg-load (None) validated successfully");
            }
            _ => panic!("Expected ConfigLoad variant"),
        }
    }

    #[test]
    fn test_cause_nockchain_block_roundtrip() {
        use nockchain_types::tx_engine::common::{BigNum, CoinbaseSplit, Hash as NockHash, Page};
        use noun_serde::NounEncode;

        init_test_logging();
        info!("Starting nockchain-block cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let digest = NockHash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let parent = NockHash([Belt(10), Belt(20), Belt(30), Belt(40), Belt(50)]);

        let page = Page {
            digest,
            pow: None,
            parent,
            tx_ids: vec![],
            coinbase: CoinbaseSplit::V0(vec![]),
            timestamp: 1234567890,
            epoch_counter: 42,
            target: BigNum::from_u64(1000),
            accumulated_work: BigNum::from_u64(5000),
            height: 100,
            msg: vec![],
        };

        let mut page_slab: NounSlab<NockJammer> = NounSlab::new();
        let page_noun = page.to_noun(&mut page_slab);

        let nockchain_block_cause =
            NockchainBlockCause::new(page_slab, page_noun, NockchainTxsMap(vec![]));
        debug!("Created nockchain-block cause with height={}", page.height);

        let inner_noun = nockchain_block_cause.to_noun(&mut allocator);
        assert!(inner_noun.is_cell(), "Cause noun should be a cell");
        info!("Nockchain-block cause created successfully");
    }

    #[test]
    fn test_cause_base_blocks_roundtrip() {
        init_test_logging();
        info!("Starting base-blocks cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let batch = sample_base_blocks_cause();
        let original_cause = BridgeCause(0, BridgeCauseVariant::BaseBlocks(batch.clone()));
        debug!("Created base-blocks cause with {} entries", batch.len());

        let encoded_noun = original_cause.to_noun(&mut allocator);
        info!("Encoded base-blocks cause to noun");

        let decoded_cause = BridgeCause::from_noun(&encoded_noun)
            .expect("Failed to decode base-blocks cause from noun");
        debug!("Decoded base-blocks cause successfully");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::BaseBlocks(decoded) => {
                assert_eq!(decoded.len(), batch.len(), "Entry count should match");
                assert_eq!(decoded[0].height, batch[0].height, "Height should match");
                assert_eq!(
                    decoded[0].block_id.0, batch[0].block_id.0,
                    "Block id bytes should match"
                );
                info!("All base-blocks fields validated successfully");
            }
            _ => panic!("Expected BaseBlocks variant"),
        }
    }

    #[test]
    fn test_cause_proposed_base_call_roundtrip() {
        init_test_logging();
        info!("Starting proposed-base-call cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let call_data = ProposedBaseCallData {
            requests: vec![NockDepositRequestData {
                tx_id: zero_tip5_hash(),
                name: Name::new(zero_tip5_hash(), zero_tip5_hash()),
                recipient: EthAddress::ZERO,
                amount: 1000,
                block_height: 100,
                as_of: zero_tip5_hash(),
                nonce: 1,
            }],
        };

        let original_cause =
            BridgeCause(0, BridgeCauseVariant::ProposedBaseCall(call_data.clone()));
        debug!(
            "Created proposed-base-call cause with {} requests",
            call_data.requests.len()
        );

        let encoded_noun = original_cause.to_noun(&mut allocator);
        info!("Encoded proposed-base-call cause to noun");

        let decoded_cause = BridgeCause::from_noun(&encoded_noun)
            .expect("Failed to decode proposed-base-call cause from noun");
        debug!("Decoded proposed-base-call cause successfully");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::ProposedBaseCall(data) => {
                assert_eq!(
                    data.requests.len(),
                    call_data.requests.len(),
                    "Request count should match"
                );
                info!("Proposed-base-call data validated successfully");
            }
            _ => panic!("Expected ProposedBaseCall variant"),
        }
    }

    #[test]
    fn test_cause_base_call_sig_roundtrip() {
        init_test_logging();
        info!("Starting base-call-sig cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let sig = EthSignatureParts {
            r: [0x33u8; 32],
            s: [0x44u8; 32],
            v: 28,
        };
        let call_data = AtomBytes(vec![0x12, 0x34, 0x56]);

        let original_cause = BridgeCause(
            0,
            BridgeCauseVariant::BaseCallSig(BaseCallSigData(sig, call_data.clone())),
        );
        debug!("Created base-call-sig cause with v={}", sig.v);

        let encoded_noun = original_cause.to_noun(&mut allocator);
        info!("Encoded base-call-sig cause to noun");

        let decoded_cause = BridgeCause::from_noun(&encoded_noun)
            .expect("Failed to decode base-call-sig cause from noun");
        debug!("Decoded base-call-sig cause successfully");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::BaseCallSig(BaseCallSigData(decoded_sig, data)) => {
                assert_eq!(decoded_sig.r, sig.r, "Signature r should match");
                assert_eq!(decoded_sig.s, sig.s, "Signature s should match");
                assert_eq!(decoded_sig.v, sig.v, "Signature v should match");
                assert_eq!(data.0, call_data.0, "Call data should match");
                info!("All base-call-sig fields validated successfully");
            }
            _ => panic!("Expected BaseCallSig variant"),
        }
    }

    #[test]
    fn test_cause_stop_roundtrip() {
        init_test_logging();
        info!("Starting stop cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let last = StopLastBlocks {
            base: StopTipBase {
                base_hash: Tip5Hash([Belt(1); 5]),
                height: 123,
            },
            nock: StopTipNock {
                nock_hash: Tip5Hash([Belt(2); 5]),
                height: 456,
            },
        };
        let original_cause = BridgeCause::stop(last.clone());

        let encoded_noun = original_cause.to_noun(&mut allocator);
        let decoded_cause =
            BridgeCause::from_noun(&encoded_noun).expect("Failed to decode stop cause from noun");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::Stop(decoded_last) => {
                assert_eq!(decoded_last.base.base_hash, last.base.base_hash);
                assert_eq!(decoded_last.base.height, last.base.height);
                assert_eq!(decoded_last.nock.nock_hash, last.nock.nock_hash);
                assert_eq!(decoded_last.nock.height, last.nock.height);
                info!("stop cause validated successfully");
            }
            _ => panic!("Expected Stop variant"),
        }
    }

    #[test]
    fn test_cause_start_roundtrip() {
        init_test_logging();
        info!("Starting start cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let original_cause = BridgeCause::start();
        let encoded_noun = original_cause.to_noun(&mut allocator);
        let decoded_cause =
            BridgeCause::from_noun(&encoded_noun).expect("Failed to decode start cause from noun");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::Start(_tag) => {
                info!("start cause validated successfully");
            }
            _ => panic!("Expected Start variant"),
        }
    }

    #[test]
    fn test_all_cause_variants_have_version_zero() {
        init_test_logging();
        info!("Testing that all cause variants preserve version 0");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let test_cases: Vec<(&str, BridgeCause)> = vec![
            (
                "cfg-load",
                BridgeCause(0, BridgeCauseVariant::ConfigLoad(None)),
            ),
            (
                "base-blocks",
                BridgeCause(
                    0,
                    BridgeCauseVariant::BaseBlocks(sample_base_blocks_cause()),
                ),
            ),
            (
                "nockchain-block",
                BridgeCause(
                    0,
                    BridgeCauseVariant::NockchainBlock(sample_nockchain_block_cause()),
                ),
            ),
            (
                "proposed-base-call",
                BridgeCause(
                    0,
                    BridgeCauseVariant::ProposedBaseCall(ProposedBaseCallData { requests: vec![] }),
                ),
            ),
            (
                "stop",
                BridgeCause(
                    0,
                    BridgeCauseVariant::Stop(StopLastBlocks {
                        base: StopTipBase {
                            base_hash: Tip5Hash([Belt(1); 5]),
                            height: 123,
                        },
                        nock: StopTipNock {
                            nock_hash: Tip5Hash([Belt(2); 5]),
                            height: 456,
                        },
                    }),
                ),
            ),
            ("start", BridgeCause(0, BridgeCauseVariant::Start(NullTag))),
        ];

        for (name, cause) in test_cases {
            debug!("Testing version for {} variant", name);
            let encoded = cause.to_noun(&mut allocator);
            let decoded = BridgeCause::from_noun(&encoded)
                .unwrap_or_else(|_| panic!("Failed to decode {} variant", name));
            assert_eq!(decoded.0, 0, "{} variant should have version 0", name);
        }

        info!("All cause variants correctly preserve version 0");
    }

    #[test]
    fn test_empty_vs_nonempty_collections() {
        init_test_logging();
        info!("Testing empty vs non-empty collection encoding");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let empty_batch: RawBaseBlocks = vec![];
        let empty_blocks = BridgeCause(0, BridgeCauseVariant::BaseBlocks(empty_batch));
        debug!("Encoding empty blocks list");
        let encoded_empty = empty_blocks.to_noun(&mut allocator);
        let decoded_empty =
            BridgeCause::from_noun(&encoded_empty).expect("Failed to decode empty blocks");

        match decoded_empty.1 {
            BridgeCauseVariant::BaseBlocks(batch) => {
                assert!(batch.is_empty(), "Blocks list should be empty");
                info!("Empty blocks list encoded/decoded correctly");
            }
            _ => panic!("Expected BaseBlocks variant"),
        }

        let nonempty_blocks = BridgeCause(
            0,
            BridgeCauseVariant::BaseBlocks(sample_base_blocks_cause()),
        );
        debug!("Encoding non-empty blocks list");
        let encoded_nonempty = nonempty_blocks.to_noun(&mut allocator);
        let decoded_nonempty =
            BridgeCause::from_noun(&encoded_nonempty).expect("Failed to decode non-empty blocks");

        match decoded_nonempty.1 {
            BridgeCauseVariant::BaseBlocks(batch) => {
                assert_eq!(batch.len(), 1, "Blocks list should contain an entry");
                info!("Non-empty blocks list encoded/decoded correctly");
            }
            _ => panic!("Expected BaseBlocks variant"),
        }
    }

    #[test]
    fn test_effect_base_call_roundtrip() {
        init_test_logging();
        info!("Starting base-call effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let sig1 = EthSignatureParts {
            r: [0x11u8; 32],
            s: [0x22u8; 32],
            v: 27,
        };
        let sig2 = EthSignatureParts {
            r: [0x33u8; 32],
            s: [0x44u8; 32],
            v: 28,
        };
        let call_data = AtomBytes(vec![0xde, 0xad, 0xbe, 0xef]);

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::BaseCall(BaseCallData(
                vec![sig1, sig2],
                call_data.clone(),
            )),
        };
        debug!("Created base-call effect with 2 signatures");

        let encoded_noun = original_effect.to_noun(&mut allocator);
        info!("Encoded base-call effect to noun");

        let decoded_effect = BridgeEffect::from_noun(&encoded_noun)
            .expect("Failed to decode base-call effect from noun");
        debug!("Decoded base-call effect successfully");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::BaseCall(BaseCallData(sigs, data)) => {
                assert_eq!(sigs.len(), 2, "Should have 2 signatures");
                assert_eq!(sigs[0], sig1, "First signature should match");
                assert_eq!(sigs[1], sig2, "Second signature should match");
                assert_eq!(data.0, call_data.0, "Call data should match");
                info!("All base-call fields validated successfully");
            }
            _ => panic!("Expected BaseCall variant"),
        }
    }

    #[test]
    fn test_effect_assemble_base_call_roundtrip() {
        init_test_logging();
        info!("Starting assemble-base-call effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let deposit_data = DepositSettlementData {
            counterpart: nockchain_types::tx_engine::common::Hash([Belt(1); 5]),
            as_of: nockchain_types::tx_engine::common::Hash([Belt(2); 5]),
            dest: AtomBytes(vec![0xaa; 20]),
            settled_amount: 67890,
            fees: vec![
                DepositSettlementFee {
                    address: AtomBytes(vec![0xbb; 20]),
                    amount: 100,
                },
                DepositSettlementFee {
                    address: AtomBytes(vec![0xcc; 20]),
                    amount: 200,
                },
            ],
            bridge_fee: 50,
        };

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::AssembleBaseCall(deposit_data.clone()),
        };
        debug!("Created assemble-base-call effect");

        let encoded_noun = original_effect.to_noun(&mut allocator);
        info!("Encoded assemble-base-call effect to noun");

        let decoded_effect = BridgeEffect::from_noun(&encoded_noun)
            .expect("Failed to decode assemble-base-call effect from noun");
        debug!("Decoded assemble-base-call effect successfully");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::AssembleBaseCall(data) => {
                assert_eq!(
                    data.counterpart, deposit_data.counterpart,
                    "Counterpart should match"
                );
                assert_eq!(data.as_of, deposit_data.as_of, "as_of should match");
                assert_eq!(data.dest, deposit_data.dest, "dest should match");
                assert_eq!(
                    data.settled_amount, deposit_data.settled_amount,
                    "settled_amount should match"
                );
                assert_eq!(data.fees, deposit_data.fees, "fees should match");
                info!("All assemble-base-call fields validated successfully");
            }
            _ => panic!("Expected AssembleBaseCall variant"),
        }
    }

    #[test]
    fn test_effect_nock_deposit_request_roundtrip() {
        init_test_logging();
        info!("Starting nock-deposit-request effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let request_data = NockDepositRequestKernelData {
            tx_id: Tip5Hash([Belt(1); 5]),
            name: Name::new(Tip5Hash([Belt(2); 5]), Tip5Hash([Belt(3); 5])),
            recipient: EthAddress([0xca; 20]),
            amount: 1000,
            block_height: 42,
            as_of: Tip5Hash([Belt(4); 5]),
        };

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::NockDepositRequest(request_data.clone()),
        };
        debug!("Created nock-deposit-request effect");

        let encoded_noun = original_effect.to_noun(&mut allocator);
        info!("Encoded nock-deposit-request effect to noun");

        let decoded_effect = BridgeEffect::from_noun(&encoded_noun)
            .expect("Failed to decode nock-deposit-request effect from noun");
        debug!("Decoded nock-deposit-request effect successfully");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::NockDepositRequest(data) => {
                assert_eq!(data.tx_id, request_data.tx_id, "tx_id should match");
                assert_eq!(data.name, request_data.name, "name should match");
                assert_eq!(
                    data.recipient, request_data.recipient,
                    "recipient should match"
                );
                assert_eq!(data.amount, request_data.amount, "amount should match");
                assert_eq!(
                    data.block_height, request_data.block_height,
                    "block_height should match"
                );
                assert_eq!(data.as_of, request_data.as_of, "as_of should match");
                info!("Nock-deposit-request data validated successfully");
            }
            _ => panic!("Expected NockDepositRequest variant"),
        }
    }

    #[test]
    fn test_effect_commit_nock_deposits_roundtrip() {
        init_test_logging();
        info!("Starting commit-nock-deposits effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let req1 = NockDepositRequestKernelData {
            tx_id: Tip5Hash([Belt(1); 5]),
            name: Name::new(Tip5Hash([Belt(2); 5]), Tip5Hash([Belt(3); 5])),
            recipient: EthAddress([0xde; 20]),
            amount: 1000,
            block_height: 42,
            as_of: Tip5Hash([Belt(4); 5]),
        };
        let req2 = NockDepositRequestKernelData {
            tx_id: Tip5Hash([Belt(5); 5]),
            name: Name::new(Tip5Hash([Belt(6); 5]), Tip5Hash([Belt(7); 5])),
            recipient: EthAddress([0xad; 20]),
            amount: 2000,
            block_height: 43,
            as_of: Tip5Hash([Belt(8); 5]),
        };
        let proposal_data = vec![req1.clone(), req2.clone()];

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::CommitNockDeposits(proposal_data.clone()),
        };
        debug!(
            "Created commit-nock-deposits effect with {} requests",
            proposal_data.len()
        );

        let encoded_noun = original_effect.to_noun(&mut allocator);
        info!("Encoded commit-nock-deposits effect to noun");
        eprintln!(
            "encoded_noun: {:?}",
            nockvm::noun::FullDebugCell(&encoded_noun.as_cell().unwrap())
        );

        let decoded_effect = BridgeEffect::from_noun(&encoded_noun)
            .expect("Failed to decode commit-nock-deposits effect from noun");
        debug!("Decoded commit-nock-deposits effect successfully");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::CommitNockDeposits(data) => {
                assert_eq!(data.len(), 2, "Should have 2 requests");
                assert_eq!(
                    data[0].tx_id, req1.tx_id,
                    "First request tx_id should match"
                );
                assert_eq!(
                    data[1].tx_id, req2.tx_id,
                    "Second request tx_id should match"
                );
                info!("All commit-nock-deposits fields validated successfully");
            }
            _ => panic!("Expected CommitNockDeposits variant"),
        }
    }

    #[test]
    fn test_effect_nockchain_tx_roundtrip() {
        use nockchain_types::tx_engine::common::Hash as TxId;
        use nockchain_types::v1::{RawTx, Spends, Version};

        init_test_logging();
        info!("Starting nockchain-tx effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let raw_tx = RawTx {
            version: Version::V1,
            id: TxId([Belt(1); 5]),
            spends: Spends(vec![]),
        };

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::NockchainTx(NockchainTxData { tx: raw_tx.clone() }),
        };
        debug!("Created nockchain-tx effect");

        let encoded_noun = original_effect.to_noun(&mut allocator);
        info!("Encoded nockchain-tx effect to noun");

        let decoded_effect = BridgeEffect::from_noun(&encoded_noun)
            .expect("Failed to decode nockchain-tx effect from noun");
        debug!("Decoded nockchain-tx effect successfully");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::NockchainTx(tx_data) => {
                assert_eq!(tx_data.tx.id, raw_tx.id, "Raw tx id should match");
                assert_eq!(
                    tx_data.tx.version, raw_tx.version,
                    "Raw tx version should match"
                );
                info!("Nockchain-tx data validated successfully");
            }
            _ => panic!("Expected NockchainTx variant"),
        }
    }

    #[test]
    fn test_effect_propose_nockchain_tx_roundtrip() {
        use nockchain_math::crypto::cheetah::{CheetahPoint, F6lt};
        use nockchain_types::tx_engine::common::SchnorrPubkey;

        init_test_logging();
        info!("Starting propose-nockchain-tx effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let pubkey = SchnorrPubkey(CheetahPoint {
            x: F6lt([Belt(3); 6]),
            y: F6lt([Belt(4); 6]),
            inf: false,
        });
        let tx_data = AtomBytes(vec![0xbe, 0xef]);

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::ProposeNockchainTx(ProposeNockchainTxData(
                pubkey.clone(),
                tx_data.clone(),
            )),
        };
        debug!("Created propose-nockchain-tx effect");

        let encoded_noun = original_effect.to_noun(&mut allocator);
        info!("Encoded propose-nockchain-tx effect to noun");

        let decoded_effect = BridgeEffect::from_noun(&encoded_noun)
            .expect("Failed to decode propose-nockchain-tx effect from noun");
        debug!("Decoded propose-nockchain-tx effect successfully");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::ProposeNockchainTx(ProposeNockchainTxData(pk, data)) => {
                assert_eq!(pk, pubkey, "Pubkey should match");
                assert_eq!(data.0, tx_data.0, "Tx data should match");
                info!("All propose-nockchain-tx fields validated successfully");
            }
            _ => panic!("Expected ProposeNockchainTx variant"),
        }
    }

    #[test]
    fn test_effect_stop_roundtrip() {
        init_test_logging();
        info!("Starting stop effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let last = StopLastBlocks {
            base: StopTipBase {
                base_hash: Tip5Hash([Belt(1); 5]),
                height: 123,
            },
            nock: StopTipNock {
                nock_hash: Tip5Hash([Belt(2); 5]),
                height: 456,
            },
        };
        let reason = "invariant violated".to_string();
        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::Stop(StopEffectData {
                reason: reason.clone(),
                last: last.clone(),
            }),
        };

        let encoded_noun = original_effect.to_noun(&mut allocator);
        let decoded_effect =
            BridgeEffect::from_noun(&encoded_noun).expect("Failed to decode stop effect from noun");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");
        match decoded_effect.variant {
            BridgeEffectVariant::Stop(data) => {
                assert_eq!(data.reason, reason);
                assert_eq!(data.last.base.base_hash, last.base.base_hash);
                assert_eq!(data.last.base.height, last.base.height);
                assert_eq!(data.last.nock.nock_hash, last.nock.nock_hash);
                assert_eq!(data.last.nock.height, last.nock.height);
                info!("stop effect validated successfully");
            }
            _ => panic!("Expected Stop variant"),
        }
    }

    #[test]
    fn test_eth_signature_validation() {
        init_test_logging();
        info!("Testing Ethereum signature validation");

        let valid_sig = EthSignatureParts {
            r: [0x11u8; 32],
            s: [0x22u8; 32],
            v: 27,
        };
        assert!(
            valid_sig.validate().is_ok(),
            "Valid signature should pass validation"
        );
        info!("Valid signature (v=27) passed validation");

        let valid_sig_v28 = EthSignatureParts {
            r: [0x11u8; 32],
            s: [0x22u8; 32],
            v: 28,
        };
        assert!(
            valid_sig_v28.validate().is_ok(),
            "Valid signature with v=28 should pass validation"
        );
        info!("Valid signature (v=28) passed validation");

        let zero_r = EthSignatureParts {
            r: [0u8; 32],
            s: [0x22u8; 32],
            v: 27,
        };
        assert!(
            zero_r.validate().is_err(),
            "Signature with zero r should fail validation"
        );
        info!("Zero r component correctly rejected");

        let zero_s = EthSignatureParts {
            r: [0x11u8; 32],
            s: [0u8; 32],
            v: 27,
        };
        assert!(
            zero_s.validate().is_err(),
            "Signature with zero s should fail validation"
        );
        info!("Zero s component correctly rejected");

        let invalid_v = EthSignatureParts {
            r: [0x11u8; 32],
            s: [0x22u8; 32],
            v: 26,
        };
        assert!(
            invalid_v.validate().is_err(),
            "Signature with invalid v should fail validation"
        );
        info!("Invalid v component (26) correctly rejected");

        let invalid_v_high = EthSignatureParts {
            r: [0x11u8; 32],
            s: [0x22u8; 32],
            v: 29,
        };
        assert!(
            invalid_v_high.validate().is_err(),
            "Signature with invalid v should fail validation"
        );
        info!("Invalid v component (29) correctly rejected");

        info!("All Ethereum signature validation tests passed");
    }

    #[test]
    fn test_edge_case_large_atom_bytes() {
        init_test_logging();
        info!("Testing large AtomBytes encoding/decoding via BaseBlocks");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        // Create a RawBaseBlockEntry with large block_id data
        let large_data = vec![0xffu8; 1024];
        let entry = RawBaseBlockEntry {
            height: 12345,
            block_id: AtomBytes(large_data.clone()),
            parent_block_id: AtomBytes(vec![0xca, 0xfe]),
            txs: vec![],
        };
        let cause = BridgeCause(0, BridgeCauseVariant::BaseBlocks(vec![entry]));

        let encoded = cause.to_noun(&mut allocator);
        let decoded = BridgeCause::from_noun(&encoded).expect("Failed to decode large AtomBytes");

        match decoded.1 {
            BridgeCauseVariant::BaseBlocks(blocks) => {
                assert_eq!(blocks.len(), 1, "Should have 1 block");
                assert_eq!(
                    blocks[0].block_id.0.len(),
                    1024,
                    "Large data should preserve length"
                );
                assert_eq!(blocks[0].block_id.0, large_data, "Large data should match");
                info!("Large AtomBytes (1024 bytes) encoded/decoded correctly");
            }
            _ => panic!("Expected BaseBlocks variant"),
        }
    }

    #[test]
    fn test_edge_case_many_signatures() {
        init_test_logging();
        info!("Testing effect with many signatures");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let mut sigs = Vec::new();
        for i in 0..10 {
            sigs.push(EthSignatureParts {
                r: [i as u8; 32],
                s: [(i + 1) as u8; 32],
                v: if i % 2 == 0 { 27 } else { 28 },
            });
        }

        let effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::BaseCall(BaseCallData(
                sigs.clone(),
                AtomBytes(vec![0xaa, 0xbb]),
            )),
        };

        let encoded = effect.to_noun(&mut allocator);
        let decoded = BridgeEffect::from_noun(&encoded)
            .expect("Failed to decode effect with many signatures");

        match decoded.variant {
            BridgeEffectVariant::BaseCall(BaseCallData(decoded_sigs, _)) => {
                assert_eq!(decoded_sigs.len(), 10, "Should have 10 signatures");
                for (i, sig) in decoded_sigs.iter().enumerate() {
                    assert_eq!(*sig, sigs[i], "Signature {} should match", i);
                }
                info!("Effect with 10 signatures encoded/decoded correctly");
            }
            _ => panic!("Expected BaseCall variant"),
        }
    }

    #[test]
    fn test_edge_case_empty_fees_map() {
        init_test_logging();
        info!("Testing DepositSettlementData with empty fees map");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let deposit_data = DepositSettlementData {
            counterpart: nockchain_types::tx_engine::common::Hash([Belt(0); 5]),
            as_of: nockchain_types::tx_engine::common::Hash([Belt(0); 5]),
            dest: AtomBytes(vec![0; 20]),
            settled_amount: 0,
            fees: Vec::new(),
            bridge_fee: 0,
        };

        let effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::AssembleBaseCall(deposit_data.clone()),
        };

        let encoded = effect.to_noun(&mut allocator);
        let decoded =
            BridgeEffect::from_noun(&encoded).expect("Failed to decode effect with empty fees");

        match decoded.variant {
            BridgeEffectVariant::AssembleBaseCall(data) => {
                assert!(data.fees.is_empty(), "Fees map should be empty");
                info!("Empty fees map encoded/decoded correctly");
            }
            _ => panic!("Expected AssembleBaseCall variant"),
        }
    }

    #[test]
    fn test_eth_signature_request_with_nockchain_hashes() {
        use nockchain_math::belt::Belt;

        init_test_logging();
        info!("Testing NockDepositRequestData with nockchain-native hashes");

        let req = NockDepositRequestData {
            tx_id: Tip5Hash([Belt(100), Belt(200), Belt(300), Belt(400), Belt(500)]),
            name: Name::new(
                Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
                Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
            ),
            recipient: EthAddress([0xaa; 20]),
            amount: 1000,
            block_height: 42,
            as_of: Tip5Hash([Belt(10), Belt(20), Belt(30), Belt(40), Belt(50)]),
            nonce: 1,
        };

        let proposal_hash = req.compute_proposal_hash();

        assert_eq!(proposal_hash.len(), 32, "Proposal hash should be 32 bytes");

        let tx_id_limbs = req.tx_id.to_array();
        let name_first_limbs = req.name.first.to_array();
        let name_last_limbs = req.name.last.to_array();
        let as_of_limbs = req.as_of.to_array();

        assert_eq!(tx_id_limbs.len(), 5, "tx_id should encode to 5 limbs");
        assert_eq!(
            name_first_limbs.len(),
            5,
            "name.first should encode to 5 limbs"
        );
        assert_eq!(
            name_last_limbs.len(),
            5,
            "name.last should encode to 5 limbs"
        );
        assert_eq!(as_of_limbs.len(), 5, "as_of should encode to 5 limbs");

        let reconstructed_tx_id = Tip5Hash::from_limbs(&tx_id_limbs);
        let reconstructed_name_first = Tip5Hash::from_limbs(&name_first_limbs);
        let reconstructed_name_last = Tip5Hash::from_limbs(&name_last_limbs);
        let reconstructed_as_of = Tip5Hash::from_limbs(&as_of_limbs);

        assert_eq!(reconstructed_tx_id, req.tx_id, "tx_id should roundtrip");
        assert_eq!(
            reconstructed_name_first, req.name.first,
            "name.first should roundtrip"
        );
        assert_eq!(
            reconstructed_name_last, req.name.last,
            "name.last should roundtrip"
        );
        assert_eq!(reconstructed_as_of, req.as_of, "as_of should roundtrip");

        info!("Nockchain-native hashes roundtrip correctly through limbs encoding");
    }

    #[test]
    fn test_deposit_id_roundtrip() {
        use nockchain_math::belt::Belt;

        init_test_logging();
        info!("Testing DepositId serialization roundtrip");

        let original = DepositId {
            as_of: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            name: Name::new(
                Tip5Hash([Belt(10), Belt(20), Belt(30), Belt(40), Belt(50)]),
                Tip5Hash([Belt(100), Belt(200), Belt(300), Belt(400), Belt(500)]),
            ),
        };

        let bytes = original.to_bytes();
        assert_eq!(bytes.len(), 120, "Should serialize to 120 bytes");

        let decoded = DepositId::from_bytes(&bytes).expect("Failed to deserialize DepositId");

        assert_eq!(decoded.as_of, original.as_of, "as_of should match");
        assert_eq!(
            decoded.name.first, original.name.first,
            "name.first should match"
        );
        assert_eq!(
            decoded.name.last, original.name.last,
            "name.last should match"
        );
        assert_eq!(decoded, original, "Full DepositId should match");
        info!("DepositId roundtrip successful");
    }

    #[test]
    fn test_deposit_id_from_effect_payload() {
        use nockchain_math::belt::Belt;

        init_test_logging();
        info!("Testing DepositId construction from NockDepositRequestData");

        let request = NockDepositRequestData {
            tx_id: Tip5Hash([Belt(1); 5]),
            name: Name::new(Tip5Hash([Belt(2); 5]), Tip5Hash([Belt(3); 5])),
            recipient: EthAddress([0xaa; 20]),
            amount: 1000,
            block_height: 42,
            as_of: Tip5Hash([Belt(4); 5]),
            nonce: 1,
        };

        let deposit_id = DepositId::from_effect_payload(&request);

        assert_eq!(
            deposit_id.as_of, request.as_of,
            "as_of should match request"
        );
        assert_eq!(deposit_id.name, request.name, "name should match request");
        info!("DepositId constructed correctly from effect payload");
    }

    #[test]
    fn test_deposit_id_hash_uniqueness() {
        use std::collections::HashSet;

        use nockchain_math::belt::Belt;

        init_test_logging();
        info!("Testing DepositId hash uniqueness");

        let id1 = DepositId {
            as_of: Tip5Hash([Belt(1); 5]),
            name: Name::new(Tip5Hash([Belt(2); 5]), Tip5Hash([Belt(3); 5])),
        };

        let id2 = DepositId {
            as_of: Tip5Hash([Belt(1); 5]),
            name: Name::new(Tip5Hash([Belt(2); 5]), Tip5Hash([Belt(4); 5])), // Different last
        };

        let id3 = DepositId {
            as_of: Tip5Hash([Belt(2); 5]), // Different as_of
            name: Name::new(Tip5Hash([Belt(2); 5]), Tip5Hash([Belt(3); 5])),
        };

        let mut set = HashSet::new();
        set.insert(id1.clone());
        set.insert(id2.clone());
        set.insert(id3.clone());

        assert_eq!(
            set.len(),
            3,
            "All three DepositIds should be unique in HashSet"
        );
        assert!(set.contains(&id1), "HashSet should contain id1");
        assert!(set.contains(&id2), "HashSet should contain id2");
        assert!(set.contains(&id3), "HashSet should contain id3");
        info!("DepositId hashing works correctly for HashMap/HashSet usage");
    }

    #[test]
    fn test_edge_case_cfg_load_some() {
        init_test_logging();
        info!("Testing cfg-load with Some(config)");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let node_info = NodeInfo {
            ip: "127.0.0.1".to_string(),
            eth_pubkey: AtomBytes(vec![0x01, 0x02]),
            // Use a valid PKH (Tip5 hash with 5 Belt limbs)
            nock_pkh: nockchain_types::tx_engine::common::Hash([
                Belt(1),
                Belt(2),
                Belt(3),
                Belt(4),
                Belt(5),
            ]),
        };

        let config = NodeConfig {
            node_id: 0,
            nodes: vec![node_info],
            my_eth_key: AtomBytes(vec![0xab, 0xcd]),
            my_nock_key: SchnorrSecretKey([Belt(42); 8]),
        };

        let cause = BridgeCause(0, BridgeCauseVariant::ConfigLoad(Some(config.clone())));

        let encoded = cause.to_noun(&mut allocator);
        let decoded =
            BridgeCause::from_noun(&encoded).expect("Failed to decode cfg-load with Some");

        match decoded.1 {
            BridgeCauseVariant::ConfigLoad(Some(decoded_config)) => {
                assert_eq!(
                    decoded_config.node_id, config.node_id,
                    "node_id should match"
                );
                assert_eq!(
                    decoded_config.nodes.len(),
                    config.nodes.len(),
                    "nodes length should match"
                );
                info!("cfg-load with Some(config) encoded/decoded correctly");
            }
            _ => panic!("Expected ConfigLoad(Some(_)) variant"),
        }
    }

    #[test]
    fn test_bridge_constants_roundtrip() {
        init_test_logging();
        info!("Starting bridge-constants roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let original = BridgeConstants::default();

        let encoded = original.to_noun(&mut allocator);
        let decoded =
            BridgeConstants::from_noun(&encoded).expect("Failed to decode BridgeConstants");

        assert_eq!(decoded.version, original.version);
        assert_eq!(decoded.min_signers, original.min_signers);
        assert_eq!(decoded.total_signers, original.total_signers);
        assert_eq!(decoded.minimum_event_nocks, original.minimum_event_nocks);
        assert_eq!(decoded.nicks_fee_per_nock, original.nicks_fee_per_nock);
        assert_eq!(decoded.base_blocks_chunk, original.base_blocks_chunk);
        assert_eq!(decoded.base_start_height, original.base_start_height);
        assert_eq!(
            decoded.nockchain_start_height,
            original.nockchain_start_height
        );

        info!("BridgeConstants roundtrip successful");
    }

    #[test]
    fn test_cause_set_constants_roundtrip() {
        init_test_logging();
        info!("Starting set-constants cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let constants = BridgeConstants::default();
        let original = BridgeCause::set_constants(constants.clone());

        let encoded = original.to_noun(&mut allocator);
        let decoded =
            BridgeCause::from_noun(&encoded).expect("Failed to decode set-constants cause");

        assert_eq!(decoded.0, 0);
        match decoded.1 {
            BridgeCauseVariant::SetConstants(c) => {
                assert_eq!(c.min_signers, constants.min_signers);
                assert_eq!(c.total_signers, constants.total_signers);
                info!("set-constants cause roundtrip successful");
            }
            _ => panic!("Expected SetConstants variant"),
        }
    }

    #[test]
    fn test_base_event_deposit_processed_roundtrip() {
        init_test_logging();
        info!("Starting base-event deposit-processed roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let event = BaseEvent {
            base_event_id: AtomBytes(vec![0x12, 0x34, 0x56, 0x78]),
            content: BaseEventContent::DepositProcessed {
                nock_tx_id: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
                note_name: Name::new(
                    Tip5Hash([Belt(10), Belt(20), Belt(30), Belt(40), Belt(50)]),
                    Tip5Hash([Belt(11), Belt(21), Belt(31), Belt(41), Belt(51)]),
                ),
                recipient: EthAddress([0xaa; 20]),
                amount: 65536, // 1 NOCK in internal units
                block_height: 12345,
                as_of: Tip5Hash([Belt(100), Belt(200), Belt(300), Belt(400), Belt(500)]),
                nonce: 42,
            },
        };

        let encoded = event.to_noun(&mut allocator);
        info!("Encoded BaseEvent with DepositProcessed to noun");

        let decoded = BaseEvent::from_noun(&encoded)
            .expect("Failed to decode BaseEvent with DepositProcessed");
        info!("Decoded BaseEvent successfully");

        assert_eq!(
            decoded.base_event_id, event.base_event_id,
            "base_event_id should match"
        );

        match (&decoded.content, &event.content) {
            (
                BaseEventContent::DepositProcessed {
                    nock_tx_id: d_tx_id,
                    note_name: d_name,
                    recipient: d_recipient,
                    amount: d_amount,
                    block_height: d_height,
                    as_of: d_as_of,
                    nonce: d_nonce,
                },
                BaseEventContent::DepositProcessed {
                    nock_tx_id: e_tx_id,
                    note_name: e_name,
                    recipient: e_recipient,
                    amount: e_amount,
                    block_height: e_height,
                    as_of: e_as_of,
                    nonce: e_nonce,
                },
            ) => {
                assert_eq!(d_tx_id, e_tx_id, "nock_tx_id should match");
                assert_eq!(d_name, e_name, "note_name should match");
                assert_eq!(d_recipient, e_recipient, "recipient should match");
                assert_eq!(d_amount, e_amount, "amount should match");
                assert_eq!(d_height, e_height, "block_height should match");
                assert_eq!(d_as_of, e_as_of, "as_of should match");
                assert_eq!(d_nonce, e_nonce, "nonce should match");
                info!("All DepositProcessed fields validated successfully");
            }
            _ => panic!("Expected DepositProcessed variant"),
        }
    }

    #[test]
    fn test_base_event_burn_for_withdrawal_roundtrip() {
        init_test_logging();
        info!("Starting base-event burn-for-withdrawal roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let event = BaseEvent {
            base_event_id: AtomBytes(vec![0xab, 0xcd, 0xef]),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xbb; 20]),
                amount: 131072, // 2 NOCK in internal units
                lock_root: Tip5Hash([Belt(111), Belt(222), Belt(333), Belt(444), Belt(555)]),
            },
        };

        let encoded = event.to_noun(&mut allocator);
        info!("Encoded BaseEvent with BurnForWithdrawal to noun");

        let decoded = BaseEvent::from_noun(&encoded)
            .expect("Failed to decode BaseEvent with BurnForWithdrawal");
        info!("Decoded BaseEvent successfully");

        assert_eq!(
            decoded.base_event_id, event.base_event_id,
            "base_event_id should match"
        );

        match (&decoded.content, &event.content) {
            (
                BaseEventContent::BurnForWithdrawal {
                    burner: d_burner,
                    amount: d_amount,
                    lock_root: d_lock_root,
                },
                BaseEventContent::BurnForWithdrawal {
                    burner: e_burner,
                    amount: e_amount,
                    lock_root: e_lock_root,
                },
            ) => {
                assert_eq!(d_burner, e_burner, "burner should match");
                assert_eq!(d_amount, e_amount, "amount should match");
                assert_eq!(d_lock_root, e_lock_root, "lock_root should match");
                info!("All BurnForWithdrawal fields validated successfully");
            }
            _ => panic!("Expected BurnForWithdrawal variant"),
        }
    }

    #[test]
    fn test_raw_base_blocks_with_events_roundtrip() {
        init_test_logging();
        info!("Starting raw-base-blocks with events roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let deposit_event = BaseEvent {
            base_event_id: AtomBytes(vec![0x01, 0x02]),
            content: BaseEventContent::DepositProcessed {
                nock_tx_id: Tip5Hash([Belt(1); 5]),
                note_name: Name::new(Tip5Hash([Belt(2); 5]), Tip5Hash([Belt(3); 5])),
                recipient: EthAddress([0xcc; 20]),
                amount: 65536,
                block_height: 100,
                as_of: Tip5Hash([Belt(4); 5]),
                nonce: 1,
            },
        };

        let withdrawal_event = BaseEvent {
            base_event_id: AtomBytes(vec![0x03, 0x04]),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xdd; 20]),
                amount: 131072,
                lock_root: Tip5Hash([Belt(5); 5]),
            },
        };

        let raw_blocks: RawBaseBlocks = vec![RawBaseBlockEntry {
            height: 12345,
            block_id: AtomBytes(vec![0xde, 0xad, 0xbe, 0xef]),
            parent_block_id: AtomBytes(vec![0xca, 0xfe, 0xba, 0xbe]),
            txs: vec![deposit_event.clone(), withdrawal_event.clone()],
        }];

        let cause = BridgeCause(0, BridgeCauseVariant::BaseBlocks(raw_blocks.clone()));
        let encoded = cause.to_noun(&mut allocator);
        info!("Encoded BridgeCause with BaseBlocks containing events");

        let decoded =
            BridgeCause::from_noun(&encoded).expect("Failed to decode BridgeCause with BaseBlocks");
        info!("Decoded BridgeCause successfully");

        match decoded.1 {
            BridgeCauseVariant::BaseBlocks(blocks) => {
                assert_eq!(blocks.len(), 1, "Should have 1 block");
                let block = &blocks[0];
                assert_eq!(block.height, 12345, "height should match");
                assert_eq!(block.txs.len(), 2, "Should have 2 events");

                // Verify first event (DepositProcessed)
                match &block.txs[0].content {
                    BaseEventContent::DepositProcessed { amount, nonce, .. } => {
                        assert_eq!(*amount, 65536, "deposit amount should match");
                        assert_eq!(*nonce, 1, "deposit nonce should match");
                    }
                    _ => panic!("First event should be DepositProcessed"),
                }

                // Verify second event (BurnForWithdrawal)
                match &block.txs[1].content {
                    BaseEventContent::BurnForWithdrawal { amount, .. } => {
                        assert_eq!(*amount, 131072, "withdrawal amount should match");
                    }
                    _ => panic!("Second event should be BurnForWithdrawal"),
                }

                info!("All BaseBlocks events validated successfully");
            }
            _ => panic!("Expected BaseBlocks variant"),
        }
    }

    #[test]
    fn test_count_peek_roundtrip() {
        init_test_logging();
        info!("Starting CountPeek roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        // Test with a value
        let original = CountPeek {
            inner: Some(Some(42)),
        };
        let encoded = original.to_noun(&mut allocator);
        let decoded = CountPeek::from_noun(&encoded).expect("Failed to decode CountPeek from noun");
        assert_eq!(
            decoded.inner,
            Some(Some(42)),
            "CountPeek value should match"
        );

        // Test with None (absent)
        let mut allocator2: NounSlab<NockJammer> = NounSlab::new();
        let original_none = CountPeek { inner: Some(None) };
        let encoded_none = original_none.to_noun(&mut allocator2);
        let decoded_none =
            CountPeek::from_noun(&encoded_none).expect("Failed to decode CountPeek None from noun");
        assert_eq!(
            decoded_none.inner,
            Some(None),
            "CountPeek None should match"
        );

        info!("CountPeek roundtrip validated successfully");
    }

    #[test]
    fn test_bool_peek_roundtrip() {
        init_test_logging();
        info!("Starting BoolPeek roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        // Test with true
        let original_true = BoolPeek {
            inner: Some(Some(true)),
        };
        let encoded_true = original_true.to_noun(&mut allocator);
        let decoded_true =
            BoolPeek::from_noun(&encoded_true).expect("Failed to decode BoolPeek true from noun");
        assert_eq!(
            decoded_true.inner,
            Some(Some(true)),
            "BoolPeek true should match"
        );

        // Test with false
        let mut allocator2: NounSlab<NockJammer> = NounSlab::new();
        let original_false = BoolPeek {
            inner: Some(Some(false)),
        };
        let encoded_false = original_false.to_noun(&mut allocator2);
        let decoded_false =
            BoolPeek::from_noun(&encoded_false).expect("Failed to decode BoolPeek false from noun");
        assert_eq!(
            decoded_false.inner,
            Some(Some(false)),
            "BoolPeek false should match"
        );

        info!("BoolPeek roundtrip validated successfully");
    }
}
