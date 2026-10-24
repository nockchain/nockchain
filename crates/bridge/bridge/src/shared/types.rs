use std::fmt;

use ibig::UBig;
use nockchain_math::belt::{Belt, PRIME};
pub use nockchain_types::tx_engine::common::Hash as Tip5Hash;
use nockchain_types::tx_engine::common::Hash as NockPkh;
use nockchain_types::v1::Name;
pub use nockchain_types::EthAddress;
use nockvm::noun::{Noun, NounAllocator, NounSpace};
use noun_serde::{NounDecode, NounEncode};
use num_bigint::BigUint;
use tiny_keccak::{Hasher, Keccak};

use crate::deposit::types::{Deposit, NockDepositRequestKernelData, ProposedBaseCallData};
use crate::withdrawal::types::{
    CreateWithdrawalTxData, NockWithdrawalRequestKernelData, WithdrawalProposalData,
};

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    hasher.update(data);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
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
            let space = allocator.noun_space();
            ia.normalize_as_atom(&space).as_noun()
        };
        let s_atom = unsafe {
            let mut ia = nockvm::noun::IndirectAtom::new_raw_bytes(allocator, 32, self.s.as_ptr());
            let space = allocator.noun_space();
            ia.normalize_as_atom(&space).as_noun()
        };
        let v_atom = nockvm::noun::Atom::new(allocator, self.v).as_noun();
        let inner = nockvm::noun::T(allocator, &[s_atom, v_atom]);
        nockvm::noun::T(allocator, &[r_atom, inner])
    }
}

impl NounDecode for EthSignatureParts {
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        let c0 = noun
            .in_space(space)
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
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        let mut bytes = Vec::new();
        let mut current = *noun;

        while let Ok(cell) = current.in_space(space).as_cell() {
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
            current = cell.tail().noun();
        }

        if let Ok(atom) = current.in_space(space).as_atom() {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AtomBytes(pub Vec<u8>);

impl AtomBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Converts this atom's little-endian bytes into base-`PRIME` belt digits.
    pub fn to_belt_digits(&self) -> Vec<Belt> {
        let mut remaining = UBig::from_le_bytes(&self.0);
        if remaining == UBig::from(0u8) {
            return vec![Belt(0)];
        }

        let prime = UBig::from(PRIME);
        let mut digits = Vec::new();
        while remaining != UBig::from(0u8) {
            let rem = &remaining % &prime;
            digits.push(Belt(
                u64::try_from(rem).expect("base-prime digit should fit into u64"),
            ));
            remaining /= &prime;
        }
        digits
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
            let space = allocator.noun_space();
            ia.normalize_as_atom(&space).as_noun()
        }
    }
}

impl NounDecode for AtomBytes {
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        let atom = noun
            .in_space(space)
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BaseEventId(pub Vec<u8>);

impl BaseEventId {
    pub const LEN: usize = 32;

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn to_belt_digits(&self) -> Vec<Belt> {
        AtomBytes(self.0.clone()).to_belt_digits()
    }
}

impl std::ops::Deref for BaseEventId {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[u8]> for BaseEventId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for BaseEventId {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<[u8; 32]> for BaseEventId {
    fn from(value: [u8; 32]) -> Self {
        Self(value.to_vec())
    }
}

impl From<AtomBytes> for BaseEventId {
    fn from(value: AtomBytes) -> Self {
        Self(value.0)
    }
}

impl From<BaseEventId> for AtomBytes {
    fn from(value: BaseEventId) -> Self {
        Self(value.0)
    }
}

impl NounEncode for BaseEventId {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        AtomBytes(self.0.clone()).to_noun(allocator)
    }
}

impl NounDecode for BaseEventId {
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        let mut bytes = AtomBytes::from_noun(noun, space)?.0;
        if bytes.len() > Self::LEN {
            return Err(noun_serde::NounDecodeError::Custom(format!(
                "expected base_event_id atom to fit in {} bytes, got {significant_len}",
                Self::LEN,
                significant_len = bytes.len()
            )));
        }

        bytes.resize(Self::LEN, 0);
        Ok(Self(bytes))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SchnorrSecretKey(pub [Belt; 8]);

impl fmt::Debug for SchnorrSecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SchnorrSecretKey(<redacted>)")
    }
}

impl SchnorrSecretKey {
    pub fn limbs(&self) -> &[Belt; 8] {
        &self.0
    }

    pub fn to_big_uint(&self) -> BigUint {
        let radix = BigUint::from(1u64 << 32);
        let mut result = BigUint::from(0u8);
        let mut power = BigUint::from(1u8);
        for belt in &self.0 {
            result += BigUint::from(belt.0) * &power;
            power *= &radix;
        }
        result
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
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        Ok(SchnorrSecretKey(decode_belt_array(noun, space)?))
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
    /// Minimum nocks for a bridge event (default: 100_000)
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
            minimum_event_nocks: 100_000,
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

    pub fn set_blockchain_constants(constants: nockchain_types::BlockchainConstants) -> Self {
        Self(0, BridgeCauseVariant::SetBlockchainConstants(constants))
    }

    pub fn stop(last: StopLastBlocks) -> Self {
        Self(0, BridgeCauseVariant::Stop(last))
    }

    pub fn start() -> Self {
        Self(0, BridgeCauseVariant::Start(NullTag))
    }

    pub fn base_block_withdrawals_committed(ack: BaseBlockCommitAck) -> Self {
        Self(0, BridgeCauseVariant::BaseBlockWithdrawalsCommitted(ack))
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BaseCallSigData(pub EthSignatureParts, pub AtomBytes);

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct BaseBlockCommitAck {
    pub blocks_hash: Tip5Hash,
    pub first_height: u64,
    pub last_height: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct PendingBaseBlockCommit {
    pub blocks_hash: Tip5Hash,
    pub first_height: u64,
    pub last_height: u64,
    pub withdrawals: Vec<NockWithdrawalRequestKernelData>,
}

impl PendingBaseBlockCommit {
    pub fn ack(&self) -> BaseBlockCommitAck {
        BaseBlockCommitAck {
            blocks_hash: self.blocks_hash.clone(),
            first_height: self.first_height,
            last_height: self.last_height,
        }
    }
}

impl From<&PendingBaseBlockCommit> for BaseBlockCommitAck {
    fn from(pending: &PendingBaseBlockCommit) -> Self {
        pending.ack()
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub enum BridgeCauseVariant {
    #[noun(tag = "base-blocks")]
    BaseBlocks(RawBaseBlocks),

    #[noun(tag = "base-block-withdrawals-committed")]
    BaseBlockWithdrawalsCommitted(BaseBlockCommitAck),

    #[noun(tag = "nockchain-block")]
    NockchainBlock(NockchainBlockCause),

    #[noun(tag = "create-withdrawal-tx")]
    CreateWithdrawalTx(CreateWithdrawalTxData),

    #[noun(tag = "sign-tx")]
    SignTx(WithdrawalProposalData),

    #[noun(tag = "proposed-base-call")]
    ProposedBaseCall(ProposedBaseCallData),

    #[noun(tag = "proposed-nock-tx")]
    ProposedNockTx(WithdrawalProposalData),

    #[noun(tag = "base-call-sig")]
    BaseCallSig(BaseCallSigData),

    #[noun(tag = "cfg-load")]
    ConfigLoad(Option<NodeConfig>),

    #[noun(tag = "set-constants")]
    SetConstants(BridgeConstants),

    #[noun(tag = "set-blockchain-constants")]
    SetBlockchainConstants(nockchain_types::BlockchainConstants),

    #[noun(tag = "stop")]
    Stop(StopLastBlocks),

    #[noun(tag = "start")]
    Start(NullTag),
}

// TODO: generalize this or move it up into the types crate
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullTag;

impl NounEncode for NullTag {
    fn to_noun<A: NounAllocator>(&self, _allocator: &mut A) -> Noun {
        nockvm::noun::D(0)
    }
}

impl NounDecode for NullTag {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, noun_serde::NounDecodeError> {
        let atom = noun
            .in_space(space)
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

        let space = self.page_slab.noun_space();
        let page_noun = allocator.copy_into(self.page_noun, &space);
        let txs_noun = self.txs.to_noun(allocator);
        nockvm::noun::T(allocator, &[page_noun, txs_noun])
    }
}

impl NounDecode for NockchainBlockCause {
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        use nockapp::noun::slab::{NockJammer, NounSlab};

        let cell = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;

        let mut page_slab: NounSlab<NockJammer> = NounSlab::new();
        let page_noun = page_slab.copy_into(cell.head().noun(), space);

        let txs = NockchainTxsMap::from_noun(&cell.tail().noun(), space)?;

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
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        fn traverse(
            node: &nockvm::noun::Noun,
            space: &NounSpace,
            acc: &mut Vec<(nockchain_types::tx_engine::common::TxId, Tx)>,
        ) -> Result<(), noun_serde::NounDecodeError> {
            if let Ok(atom) = node.in_space(space).as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(noun_serde::NounDecodeError::ExpectedCell);
            }
            let cell = node
                .in_space(space)
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            let kv = cell
                .head()
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            let tx_id =
                nockchain_types::tx_engine::common::TxId::from_noun(&kv.head().noun(), space)?;
            let tx = Tx::from_noun(&kv.tail().noun(), space)?;
            acc.push((tx_id, tx));
            let branches = cell
                .tail()
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            traverse(&branches.head().noun(), space, acc)?;
            traverse(&branches.tail().noun(), space, acc)?;
            Ok(())
        }
        let mut acc = Vec::new();
        traverse(noun, space, &mut acc)?;
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
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        let cell = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
        let tag = cell
            .head()
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?
            .as_u64()
            .map_err(|_| noun_serde::NounDecodeError::Custom("tx tag too large".into()))?;
        match tag {
            1 => Ok(Tx::V1(TxV1::from_noun(noun, space)?)),
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
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        fn traverse(
            node: &nockvm::noun::Noun,
            space: &NounSpace,
            acc: &mut Vec<OutputV1>,
        ) -> Result<(), noun_serde::NounDecodeError> {
            if let Ok(atom) = node.in_space(space).as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(noun_serde::NounDecodeError::ExpectedCell);
            }
            let cell = node
                .in_space(space)
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            acc.push(OutputV1::from_noun(&cell.head().noun(), space)?);
            let branches = cell
                .tail()
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            traverse(&branches.head().noun(), space, acc)?;
            traverse(&branches.tail().noun(), space, acc)?;
            Ok(())
        }
        let mut acc = Vec::new();
        traverse(noun, space, &mut acc)?;
        Ok(OutputsV1(acc))
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct OutputV1 {
    pub note: nockchain_types::v1::Note,
    pub seeds: nockchain_types::v1::Seeds,
}

#[derive(Debug, Clone)]
pub struct BaseBlockRef {
    pub height: u64,
    pub block_id: AtomBytes,
    pub parent_block_id: AtomBytes,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BaseEvent {
    pub base_event_id: BaseEventId,
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

#[derive(Clone, NounEncode, NounDecode)]
pub struct NodeConfig {
    pub node_id: u64,
    pub nodes: Vec<NodeInfo>,
    pub bridge_lock_root: Tip5Hash,
    pub my_eth_key: AtomBytes,
    pub my_nock_key: SchnorrSecretKey,
}

impl fmt::Debug for NodeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeConfig")
            .field("node_id", &self.node_id)
            .field("nodes", &self.nodes)
            .field("bridge_lock_root", &self.bridge_lock_root)
            .field("my_eth_key", &"<redacted>")
            .field("my_nock_key", &self.my_nock_key)
            .finish()
    }
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
    #[noun(tag = "create-withdrawal-txs")]
    CreateWithdrawalTxs(Vec<NockWithdrawalRequestKernelData>),

    #[noun(tag = "base-block-withdrawals-pending")]
    BaseBlockWithdrawalsPending(PendingBaseBlockCommit),

    #[noun(tag = "withdrawal-proposal-built")]
    WithdrawalProposalBuilt(WithdrawalProposalData),

    #[noun(tag = "withdrawal-tx-signed")]
    WithdrawalTxSigned(WithdrawalProposalData),

    #[noun(tag = "commit-nock-deposits")]
    CommitNockDeposits(Vec<NockDepositRequestKernelData>),

    #[noun(tag = "grpc")]
    Grpc(GrpcEffect),

    #[noun(tag = "stop")]
    Stop(StopEffectData),
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

/// Peek response for lists of nock withdrawal requests.
/// Matches Hoon peek response: `[~ ~ (list nock-withdrawal-request)]`
/// The structure is (unit (unit (list nock-withdrawal-request))).
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct NockWithdrawalRequestsPeek {
    pub inner: Option<Option<Vec<NockWithdrawalRequestKernelData>>>,
}

/// Peek response for a pending Base batch waiting for withdrawal DB commit ack.
/// Matches Hoon peek response: `[~ ~ pending-base-block-withdrawals]`.
/// The structure is (unit (unit pending-base-block-withdrawals)).
#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct PendingBaseBlockCommitPeek {
    pub inner: Option<Option<PendingBaseBlockCommit>>,
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
    space: &NounSpace,
) -> Result<[Belt; N], noun_serde::NounDecodeError> {
    let mut result = [Belt(0); N];
    let mut current = *noun;
    for (idx, item) in result.iter_mut().enumerate() {
        if idx == N - 1 {
            *item = Belt::from_noun(&current, space)?;
        } else {
            let cell = current
                .in_space(space)
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::ExpectedCell)?;
            *item = Belt::from_noun(&cell.head().noun(), space)?;
            current = cell.tail().noun();
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
    use crate::deposit::types::{
        DepositId, NockDepositRequestData, NockDepositRequestKernelData, ProposedBaseCallData,
    };
    use crate::withdrawal::types::{
        CreateWithdrawalTxData, NockWithdrawalRequestKernelData, SelectedWithdrawalNoteData,
        WithdrawalId, WithdrawalProposalData, WithdrawalSnapshot,
    };

    fn init_test_logging() {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();
    }

    fn sample_base_block_commit_ack() -> BaseBlockCommitAck {
        BaseBlockCommitAck {
            blocks_hash: Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]),
            first_height: 100,
            last_height: 199,
        }
    }

    fn sample_pending_base_block_commit() -> PendingBaseBlockCommit {
        PendingBaseBlockCommit {
            blocks_hash: Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]),
            first_height: 100,
            last_height: 199,
            withdrawals: vec![NockWithdrawalRequestKernelData {
                base_event_id: BaseEventId(
                    (0..32).map(|offset| 0x44_u8.wrapping_add(offset)).collect(),
                ),
                recipient: Tip5Hash([Belt(51), Belt(52), Belt(53), Belt(54), Belt(55)]),
                amount: 123,
                base_batch_end: 199,
                as_of: Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]),
            }],
        }
    }

    fn sample_base_event_id_ending_in_zero(start: u8) -> BaseEventId {
        let mut bytes: Vec<u8> = (0..32).map(|offset| start.wrapping_add(offset)).collect();
        bytes[31] = 0;
        BaseEventId(bytes)
    }

    #[test]
    fn atom_bytes_to_belt_digits_matches_known_vector() {
        let atom = AtomBytes((0..32).map(|offset| 1_u8.wrapping_add(offset)).collect());

        assert_eq!(
            atom.to_belt_digits(),
            vec![
                Belt(578_437_696_156_539_417),
                Belt(10_923_933_468_832_943_055),
                Belt(14_755_409_445_788_166_057),
                Belt(2_314_601_845_482_878_064),
            ]
        );
    }

    #[test]
    fn atom_bytes_to_belt_digits_keeps_zero_as_single_digit() {
        assert_eq!(AtomBytes(Vec::new()).to_belt_digits(), vec![Belt(0)]);
        assert_eq!(AtomBytes(vec![0, 0, 0]).to_belt_digits(), vec![Belt(0)]);
    }

    #[test]
    fn base_event_id_roundtrip_preserves_trailing_zero() {
        let original = sample_base_event_id_ending_in_zero(0x61);
        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let encoded = original.to_noun(&mut allocator);
        let decoded = BaseEventId::from_noun(&encoded, &allocator.noun_space())
            .expect("base_event_id should decode with padded trailing zero");

        assert_eq!(decoded.0.len(), 32);
        assert_eq!(decoded, original);
    }

    #[test]
    fn withdrawal_id_roundtrip_preserves_trailing_zero_base_event_id() {
        let id = WithdrawalId {
            as_of: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            base_event_id: sample_base_event_id_ending_in_zero(0x71),
        };
        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let encoded = id.to_noun(&mut allocator);
        let decoded = WithdrawalId::from_noun(&encoded, &allocator.noun_space())
            .expect("withdrawal id should decode fixed-width base_event_id");

        assert_eq!(decoded.base_event_id.0.len(), 32);
        assert_eq!(decoded, id);
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

    fn sample_withdrawal_id() -> WithdrawalId {
        WithdrawalId {
            as_of: Tip5Hash([Belt(11), Belt(22), Belt(33), Belt(44), Belt(55)]),
            base_event_id: BaseEventId(
                (0..32).map(|offset| 0xfa_u8.wrapping_add(offset)).collect(),
            ),
        }
    }

    fn sample_selected_inputs() -> Vec<Name> {
        vec![
            Name::new(
                Tip5Hash([Belt(101), Belt(102), Belt(103), Belt(104), Belt(105)]),
                Tip5Hash([Belt(201), Belt(202), Belt(203), Belt(204), Belt(205)]),
            ),
            Name::new(
                Tip5Hash([Belt(111), Belt(112), Belt(113), Belt(114), Belt(115)]),
                Tip5Hash([Belt(211), Belt(212), Belt(213), Belt(214), Belt(215)]),
            ),
        ]
    }

    fn sample_selected_notes() -> Vec<SelectedWithdrawalNoteData> {
        sample_selected_inputs()
            .into_iter()
            .enumerate()
            .map(|(idx, name)| {
                let note_data = nockchain_types::v1::NoteData::new(vec![
                    nockchain_types::v1::NoteDataEntry::lock(
                        nockchain_types::v1::Lock::SpendCondition(
                            nockchain_types::v1::SpendCondition::simple_pkh(Tip5Hash([
                                Belt(501 + idx as u64),
                                Belt(502 + idx as u64),
                                Belt(503 + idx as u64),
                                Belt(504 + idx as u64),
                                Belt(505 + idx as u64),
                            ])),
                        ),
                    ),
                ]);
                SelectedWithdrawalNoteData {
                    name: name.clone(),
                    note: nockchain_types::v1::Note::V1(nockchain_types::v1::NoteV1::new(
                        nockchain_types::tx_engine::common::BlockHeight(Belt(700 + idx as u64)),
                        name,
                        note_data,
                        nockchain_types::tx_engine::common::Nicks(42 + idx),
                    )),
                }
            })
            .collect()
    }

    //  NOTE: The withdrawal transaction is a simple fan-in transaction, not a withdrawal transaction.
    fn sample_withdrawal_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("Failed to cue transaction fixture");

        let space = slab.noun_space();
        nockchain_types::v1::Transaction::from_noun(&noun, &space)
            .expect("Failed to decode transaction fixture")
    }

    fn sample_create_withdrawal_tx_data() -> CreateWithdrawalTxData {
        CreateWithdrawalTxData {
            id: sample_withdrawal_id(),
            recipient: Tip5Hash([Belt(301), Belt(302), Belt(303), Belt(304), Belt(305)]),
            amount: 123_456,
            burned_amount: 124_443,
            base_batch_end: 77,
            epoch: 3,
            snapshot: WithdrawalSnapshot {
                height: 99,
                block_id: Tip5Hash([Belt(401), Belt(402), Belt(403), Belt(404), Belt(405)]),
            },
            fee: 987,
            selected_notes: sample_selected_notes(),
        }
    }

    fn sample_sign_tx_data() -> WithdrawalProposalData {
        sample_withdrawal_proposal()
    }
    //  NOTE: The withdrawal transaction is a simple fan-in transaction, not a withdrawal transaction.
    fn sample_withdrawal_proposal() -> WithdrawalProposalData {
        let transaction = sample_withdrawal_transaction();
        WithdrawalProposalData {
            id: sample_withdrawal_id(),
            recipient: Tip5Hash([Belt(301), Belt(302), Belt(303), Belt(304), Belt(305)]),
            amount: 123_456,
            burned_amount: 124_443,
            base_batch_end: 77,
            epoch: 3,
            snapshot: WithdrawalSnapshot {
                height: 99,
                block_id: Tip5Hash([Belt(401), Belt(402), Belt(403), Belt(404), Belt(405)]),
            },
            selected_inputs: transaction.normalized_input_names(),
            transaction,
        }
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

        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
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

        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
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
    fn test_cause_base_block_withdrawals_committed_roundtrip() {
        init_test_logging();
        info!("Starting base-block-withdrawals-committed cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let ack = sample_base_block_commit_ack();
        let original_cause = BridgeCause::base_block_withdrawals_committed(ack.clone());

        let encoded_noun = original_cause.to_noun(&mut allocator);
        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode base-block-withdrawals-committed cause from noun");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");
        match decoded_cause.1 {
            BridgeCauseVariant::BaseBlockWithdrawalsCommitted(decoded) => {
                assert_eq!(decoded, ack);
            }
            _ => panic!("Expected BaseBlockWithdrawalsCommitted variant"),
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

        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
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
    fn test_cause_create_withdrawal_tx_roundtrip() {
        init_test_logging();
        info!("Starting create-withdrawal-tx cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let request = sample_create_withdrawal_tx_data();
        let original_cause =
            BridgeCause(0, BridgeCauseVariant::CreateWithdrawalTx(request.clone()));

        let encoded_noun = original_cause.to_noun(&mut allocator);
        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode create-withdrawal-tx cause from noun");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::CreateWithdrawalTx(decoded) => {
                assert_eq!(
                    decoded, request,
                    "create-withdrawal-tx payload should roundtrip"
                );
                info!("create-withdrawal-tx cause validated successfully");
            }
            _ => panic!("Expected CreateWithdrawalTx variant"),
        }
    }

    #[test]
    fn test_cause_sign_tx_roundtrip() {
        init_test_logging();
        info!("Starting sign-tx cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let proposal = sample_sign_tx_data();
        let original_cause = BridgeCause(0, BridgeCauseVariant::SignTx(proposal.clone()));

        let encoded_noun = original_cause.to_noun(&mut allocator);
        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode sign-tx cause from noun");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::SignTx(decoded) => {
                assert_eq!(decoded, proposal, "sign-tx payload should roundtrip");
                info!("sign-tx cause validated successfully");
            }
            _ => panic!("Expected SignTx variant"),
        }
    }

    #[test]
    fn test_cause_proposed_nock_tx_roundtrip() {
        init_test_logging();
        info!("Starting proposed-nock-tx cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let proposal = sample_withdrawal_proposal();
        let original_cause = BridgeCause(0, BridgeCauseVariant::ProposedNockTx(proposal.clone()));

        let encoded_noun = original_cause.to_noun(&mut allocator);
        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode proposed-nock-tx cause from noun");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::ProposedNockTx(decoded) => {
                assert_eq!(
                    decoded, proposal,
                    "withdrawal proposal envelope should roundtrip"
                );
                info!("proposed-nock-tx cause validated successfully");
            }
            _ => panic!("Expected ProposedNockTx variant"),
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

        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
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
        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode stop cause from noun");

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
        let decoded_cause = BridgeCause::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode start cause from noun");

        assert_eq!(decoded_cause.0, 0, "Version should be 0");

        match decoded_cause.1 {
            BridgeCauseVariant::Start(_tag) => {
                info!("start cause validated successfully");
            }
            _ => panic!("Expected Start variant"),
        }
    }

    #[test]
    fn test_cause_set_blockchain_constants_roundtrip() {
        init_test_logging();
        info!("Starting set-blockchain-constants cause roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let constants = nockchain_types::default_fakenet_blockchain_constants();
        let original = BridgeCause::set_blockchain_constants(constants.clone());
        let encoded = original.to_noun(&mut allocator);
        let decoded = BridgeCause::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode set-blockchain-constants cause");

        assert_eq!(decoded.0, 0);
        match decoded.1 {
            BridgeCauseVariant::SetBlockchainConstants(value) => {
                assert_eq!(value, constants);
                info!("set-blockchain-constants cause roundtrip successful");
            }
            _ => panic!("Expected SetBlockchainConstants variant"),
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
                "base-block-withdrawals-committed",
                BridgeCause::base_block_withdrawals_committed(sample_base_block_commit_ack()),
            ),
            (
                "proposed-base-call",
                BridgeCause(
                    0,
                    BridgeCauseVariant::ProposedBaseCall(ProposedBaseCallData { requests: vec![] }),
                ),
            ),
            (
                "create-withdrawal-tx",
                BridgeCause(
                    0,
                    BridgeCauseVariant::CreateWithdrawalTx(sample_create_withdrawal_tx_data()),
                ),
            ),
            (
                "set-blockchain-constants",
                BridgeCause(
                    0,
                    BridgeCauseVariant::SetBlockchainConstants(
                        nockchain_types::default_fakenet_blockchain_constants(),
                    ),
                ),
            ),
            (
                "sign-tx",
                BridgeCause(0, BridgeCauseVariant::SignTx(sample_sign_tx_data())),
            ),
            (
                "proposed-nock-tx",
                BridgeCause(
                    0,
                    BridgeCauseVariant::ProposedNockTx(sample_withdrawal_proposal()),
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
            let decoded = BridgeCause::from_noun(&encoded, &allocator.noun_space())
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
        let decoded_empty = BridgeCause::from_noun(&encoded_empty, &allocator.noun_space())
            .expect("Failed to decode empty blocks");

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
        let decoded_nonempty = BridgeCause::from_noun(&encoded_nonempty, &allocator.noun_space())
            .expect("Failed to decode non-empty blocks");

        match decoded_nonempty.1 {
            BridgeCauseVariant::BaseBlocks(batch) => {
                assert_eq!(batch.len(), 1, "Blocks list should contain an entry");
                info!("Non-empty blocks list encoded/decoded correctly");
            }
            _ => panic!("Expected BaseBlocks variant"),
        }
    }

    #[test]
    fn test_effect_create_withdrawal_txs_roundtrip() {
        init_test_logging();
        info!("Starting create-withdrawal-txs effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let req1 = NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id_ending_in_zero(0xde),
            recipient: Tip5Hash([Belt(1); 5]),
            amount: 1000,
            base_batch_end: 42,
            as_of: Tip5Hash([Belt(2); 5]),
        };
        let req2 = NockWithdrawalRequestKernelData {
            base_event_id: crate::shared::types::BaseEventId(
                (0..32).map(|offset| 0xbe_u8.wrapping_add(offset)).collect(),
            ),
            recipient: Tip5Hash([Belt(3); 5]),
            amount: 2000,
            base_batch_end: 43,
            as_of: Tip5Hash([Belt(4); 5]),
        };
        let requests = vec![req1.clone(), req2.clone()];

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::CreateWithdrawalTxs(requests),
        };

        let encoded_noun = original_effect.to_noun(&mut allocator);
        let decoded_effect = BridgeEffect::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode create-withdrawal-txs effect from noun");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::CreateWithdrawalTxs(data) => {
                assert_eq!(data.len(), 2, "Should have 2 requests");
                assert_eq!(data[0].base_event_id.0, req1.base_event_id.0);
                assert_eq!(data[0].recipient, req1.recipient);
                assert_eq!(data[0].amount, req1.amount);
                assert_eq!(data[0].base_batch_end, req1.base_batch_end);
                assert_eq!(data[0].as_of, req1.as_of);
                assert_eq!(data[1].base_event_id.0, req2.base_event_id.0);
                assert_eq!(data[1].recipient, req2.recipient);
                assert_eq!(data[1].amount, req2.amount);
                assert_eq!(data[1].base_batch_end, req2.base_batch_end);
                assert_eq!(data[1].as_of, req2.as_of);
                info!("create-withdrawal-txs effect validated successfully");
            }
            _ => panic!("Expected CreateWithdrawalTxs variant"),
        }
    }

    #[test]
    fn test_effect_base_block_withdrawals_pending_roundtrip() {
        init_test_logging();
        info!("Starting base-block-withdrawals-pending effect roundtrip test");

        let pending = sample_pending_base_block_commit();
        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::BaseBlockWithdrawalsPending(pending.clone()),
        };

        let encoded_noun = original_effect.to_noun(&mut allocator);
        let decoded_effect = BridgeEffect::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode base-block-withdrawals-pending effect from noun");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");
        match decoded_effect.variant {
            BridgeEffectVariant::BaseBlockWithdrawalsPending(decoded) => {
                assert_eq!(decoded, pending);
            }
            _ => panic!("Expected BaseBlockWithdrawalsPending variant"),
        }
    }

    #[test]
    fn test_effect_withdrawal_proposal_built_roundtrip() {
        init_test_logging();
        info!("Starting withdrawal-proposal-built effect roundtrip test");

        let proposal = sample_withdrawal_proposal();
        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::WithdrawalProposalBuilt(proposal.clone()),
        };

        let encoded_noun = original_effect.to_noun(&mut allocator);
        let decoded_effect = BridgeEffect::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode withdrawal-proposal-built effect from noun");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::WithdrawalProposalBuilt(decoded) => {
                assert_eq!(
                    decoded, proposal,
                    "withdrawal-proposal-built effect should roundtrip"
                );
                info!("withdrawal-proposal-built effect validated successfully");
            }
            _ => panic!("Expected WithdrawalProposalBuilt variant"),
        }
    }

    #[test]
    fn test_effect_withdrawal_tx_signed_roundtrip() {
        init_test_logging();
        info!("Starting withdrawal-tx-signed effect roundtrip test");

        let proposal = sample_withdrawal_proposal();
        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::WithdrawalTxSigned(proposal.clone()),
        };

        let encoded_noun = original_effect.to_noun(&mut allocator);
        let decoded_effect = BridgeEffect::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode withdrawal-tx-signed effect from noun");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::WithdrawalTxSigned(decoded) => {
                assert_eq!(
                    decoded, proposal,
                    "withdrawal-tx-signed effect should roundtrip"
                );
                info!("withdrawal-tx-signed effect validated successfully");
            }
            _ => panic!("Expected WithdrawalTxSigned variant"),
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
        let requests = vec![req1.clone(), req2.clone()];

        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::CommitNockDeposits(requests),
        };

        let encoded_noun = original_effect.to_noun(&mut allocator);
        let decoded_effect = BridgeEffect::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode commit-nock-deposits effect from noun");

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
                info!("commit-nock-deposits effect validated successfully");
            }
            _ => panic!("Expected CommitNockDeposits variant"),
        }
    }

    #[test]
    fn test_effect_grpc_roundtrip() {
        init_test_logging();
        info!("Starting grpc effect roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();

        let grpc = GrpcEffect::Peek(GrpcPeekData {
            pid: 42,
            typ: AtomBytes(b"height".to_vec()),
            path: vec![AtomBytes(b"base-hashchain-next-height".to_vec())],
        });
        let original_effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::Grpc(grpc),
        };

        let encoded_noun = original_effect.to_noun(&mut allocator);
        let decoded_effect = BridgeEffect::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode grpc effect from noun");

        assert_eq!(decoded_effect.version, 0, "Version should be 0");

        match decoded_effect.variant {
            BridgeEffectVariant::Grpc(GrpcEffect::Peek(data)) => {
                assert_eq!(data.pid, 42);
                assert_eq!(data.typ.0, b"height".to_vec());
                assert_eq!(data.path.len(), 1);
                assert_eq!(data.path[0].0, b"base-hashchain-next-height".to_vec());
                info!("grpc effect validated successfully");
            }
            _ => panic!("Expected Grpc::Peek variant"),
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
        let decoded_effect = BridgeEffect::from_noun(&encoded_noun, &allocator.noun_space())
            .expect("Failed to decode stop effect from noun");

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
        let decoded = BridgeCause::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode large AtomBytes");

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
    fn test_edge_case_many_commit_requests() {
        init_test_logging();
        info!("Testing commit-nock-deposits effect with many requests");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let mut requests = Vec::new();
        for i in 0..10u64 {
            requests.push(NockDepositRequestKernelData {
                tx_id: Tip5Hash([Belt(i + 1); 5]),
                name: Name::new(Tip5Hash([Belt(i + 2); 5]), Tip5Hash([Belt(i + 3); 5])),
                recipient: EthAddress([i as u8; 20]),
                amount: 1_000 + i,
                block_height: 100 + i,
                as_of: Tip5Hash([Belt(i + 4); 5]),
            });
        }

        let effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::CommitNockDeposits(requests.clone()),
        };

        let encoded = effect.to_noun(&mut allocator);
        let decoded = BridgeEffect::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode commit-nock-deposits effect with many requests");

        match decoded.variant {
            BridgeEffectVariant::CommitNockDeposits(data) => {
                assert_eq!(data.len(), requests.len(), "Request count should match");
                assert_eq!(data[0].tx_id, requests[0].tx_id);
                assert_eq!(data[9].tx_id, requests[9].tx_id);
                info!("commit-nock-deposits list encoded/decoded correctly");
            }
            _ => panic!("Expected CommitNockDeposits variant"),
        }
    }

    #[test]
    fn test_edge_case_empty_withdrawal_request_list() {
        init_test_logging();
        info!("Testing create-withdrawal-txs effect with empty request list");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let effect = BridgeEffect {
            version: 0,
            variant: BridgeEffectVariant::CreateWithdrawalTxs(Vec::new()),
        };

        let encoded = effect.to_noun(&mut allocator);
        let decoded = BridgeEffect::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode create-withdrawal-txs effect with empty list");

        match decoded.variant {
            BridgeEffectVariant::CreateWithdrawalTxs(data) => {
                assert!(data.is_empty(), "Withdrawal request list should be empty");
                info!("empty create-withdrawal-txs list encoded/decoded correctly");
            }
            _ => panic!("Expected CreateWithdrawalTxs variant"),
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
            bridge_lock_root: nockchain_types::tx_engine::common::Hash([
                Belt(10),
                Belt(11),
                Belt(12),
                Belt(13),
                Belt(14),
            ]),
            my_eth_key: AtomBytes(vec![0xab, 0xcd]),
            my_nock_key: SchnorrSecretKey([Belt(42); 8]),
        };

        let cause = BridgeCause(0, BridgeCauseVariant::ConfigLoad(Some(config.clone())));

        let encoded = cause.to_noun(&mut allocator);
        let decoded = BridgeCause::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode cfg-load with Some");

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
    fn node_config_debug_redacts_private_keys() {
        let config = NodeConfig {
            node_id: 7,
            nodes: vec![NodeInfo {
                ip: "127.0.0.1".to_string(),
                eth_pubkey: AtomBytes(vec![0x01, 0x02]),
                nock_pkh: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            }],
            bridge_lock_root: Tip5Hash([Belt(10), Belt(11), Belt(12), Belt(13), Belt(14)]),
            my_eth_key: AtomBytes(vec![201, 202, 203, 204]),
            my_nock_key: SchnorrSecretKey([Belt(9_999_991); 8]),
        };

        let rendered = format!("{config:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("201"));
        assert!(!rendered.contains("9_999_991"));
        assert!(!rendered.contains("9999991"));
    }

    #[test]
    fn test_bridge_constants_roundtrip() {
        init_test_logging();
        info!("Starting bridge-constants roundtrip test");

        let mut allocator: NounSlab<NockJammer> = NounSlab::new();
        let original = BridgeConstants::default();

        let encoded = original.to_noun(&mut allocator);
        let decoded = BridgeConstants::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode BridgeConstants");

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
        let decoded = BridgeCause::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode set-constants cause");

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
            base_event_id: crate::shared::types::BaseEventId(
                (0..32).map(|offset| 0x12_u8.wrapping_add(offset)).collect(),
            ),
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

        let decoded = BaseEvent::from_noun(&encoded, &allocator.noun_space())
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
            base_event_id: sample_base_event_id_ending_in_zero(0xab),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xbb; 20]),
                amount: 131072, // 2 NOCK in internal units
                lock_root: Tip5Hash([Belt(111), Belt(222), Belt(333), Belt(444), Belt(555)]),
            },
        };

        let encoded = event.to_noun(&mut allocator);
        info!("Encoded BaseEvent with BurnForWithdrawal to noun");

        let decoded = BaseEvent::from_noun(&encoded, &allocator.noun_space())
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
            base_event_id: crate::shared::types::BaseEventId(
                (0..32).map(|offset| 0x01_u8.wrapping_add(offset)).collect(),
            ),
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
            base_event_id: crate::shared::types::BaseEventId(
                (0..32).map(|offset| 0x03_u8.wrapping_add(offset)).collect(),
            ),
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

        let decoded = BridgeCause::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode BridgeCause with BaseBlocks");
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
        let decoded = CountPeek::from_noun(&encoded, &allocator.noun_space())
            .expect("Failed to decode CountPeek from noun");
        assert_eq!(
            decoded.inner,
            Some(Some(42)),
            "CountPeek value should match"
        );

        // Test with None (absent)
        let mut allocator2: NounSlab<NockJammer> = NounSlab::new();
        let original_none = CountPeek { inner: Some(None) };
        let encoded_none = original_none.to_noun(&mut allocator2);
        let decoded_none = CountPeek::from_noun(&encoded_none, &allocator2.noun_space())
            .expect("Failed to decode CountPeek None from noun");
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
        let decoded_true = BoolPeek::from_noun(&encoded_true, &allocator.noun_space())
            .expect("Failed to decode BoolPeek true from noun");
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
        let decoded_false = BoolPeek::from_noun(&encoded_false, &allocator2.noun_space())
            .expect("Failed to decode BoolPeek false from noun");
        assert_eq!(
            decoded_false.inner,
            Some(Some(false)),
            "BoolPeek false should match"
        );

        info!("BoolPeek roundtrip validated successfully");
    }
}
