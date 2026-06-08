use std::collections::BTreeSet;
use std::mem::size_of;

use bytes::Bytes;
use libp2p::PeerId;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::NockAppError;
use nockvm::noun::{Atom, Noun, NounAllocator, NounHandle, NounSpace, D, T};
use nockvm_macros::tas;
use rand::{rng, Rng};
use serde_bytes::ByteBuf;

use crate::p2p_util::PeerIdExt;
use crate::tip5_util::{
    base58_to_ubig, decimal_to_base_p, tip5_hash_to_base58, tip5_hash_to_base58_stack,
};

pub(crate) const FACT_POKE_VERSION: u64 = 0;
const GEN2_BATCH_POW_DOMAIN_SEPARATOR: &[u8] = b"nockchain:req-res:gen2:pow:v1";
const GOSSIP_POW_DOMAIN_SEPARATOR: &[u8] = b"nockchain:req-res:gossip:pow:v1";

#[derive(Debug, Clone)]
pub enum NockchainFact {
    // [%heard-block p=page:dt] with poke slab
    HeardBlock(String, NounSlab),
    // [%heard-elders p=[oldest=page-number:dt ids=(list block-id:dt)]] with poke slab
    HeardElders(u64, Vec<String>, NounSlab),
    // [%heard-tx p=raw-tx:dt] with poke slab
    HeardTx(String, NounSlab),
}

impl NockchainFact {
    pub(crate) fn from_rooted_message_slab(mut slab: NounSlab) -> Result<Self, NockAppError> {
        let noun = unsafe { *slab.root() };
        let space = slab.noun_space();
        let root = noun.in_space(&space);
        let head = root.as_cell()?.head();

        if head.eq_bytes(b"heard-block") {
            let page = root.as_cell()?.tail();
            let block_id = block_id_from_page(page)?;
            let block_id_str = tip5_hash_to_base58_stack(&mut slab, block_id.noun(), &space)?;
            slab.modify(|response_noun| {
                vec![D(tas!(b"fact")), D(FACT_POKE_VERSION), response_noun]
            });
            Ok(NockchainFact::HeardBlock(block_id_str, slab))
        } else if head.eq_bytes(b"heard-tx") {
            let raw_tx = root.as_cell()?.tail();
            let tx_id = tx_id_from_raw_tx(raw_tx)?;
            let tx_id_str = tip5_hash_to_base58_stack(&mut slab, tx_id.noun(), &space)?;
            slab.modify(|response_noun| {
                vec![D(tas!(b"fact")), D(FACT_POKE_VERSION), response_noun]
            });
            Ok(NockchainFact::HeardTx(tx_id_str, slab))
        } else if head.eq_bytes(b"heard-elders") {
            let elders_dat = root.as_cell()?.tail();
            let oldest = elders_dat.as_cell()?.head().as_atom()?.as_u64()?;
            let elder_ids = elders_dat.as_cell()?.tail();
            let mut elder_id_strings = Vec::new();
            for id_noun in elder_ids.list_iter() {
                elder_id_strings.push(tip5_hash_to_base58_stack(
                    &mut slab,
                    id_noun.noun(),
                    &space,
                )?);
            }
            slab.modify(|response_noun| {
                vec![D(tas!(b"fact")), D(FACT_POKE_VERSION), response_noun]
            });
            Ok(NockchainFact::HeardElders(oldest, elder_id_strings, slab))
        } else {
            Err(NockAppError::OtherError(String::from(
                "Invalid fact head tag",
            )))
        }
    }

    pub fn from_message_bytes(message: &[u8]) -> Result<Self, NockAppError> {
        let mut slab = NounSlab::new();
        let noun = slab.cue_into(Bytes::copy_from_slice(message))?;
        slab.set_root(noun);
        Self::from_rooted_message_slab(slab)
    }

    pub(crate) fn from_owned_noun_slab(slab: NounSlab) -> Result<Self, NockAppError> {
        Self::from_rooted_message_slab(slab)
    }

    #[cfg(test)]
    pub fn from_noun_slab(slab: &mut NounSlab) -> Result<Self, NockAppError> {
        let mut message_slab = NounSlab::new();
        message_slab.copy_from_slab(slab);
        Self::from_rooted_message_slab(message_slab)
    }

    pub fn fact_poke(&self) -> &NounSlab {
        match self {
            Self::HeardBlock(_, slab) => slab,
            Self::HeardTx(_, slab) => slab,
            Self::HeardElders(_, _, slab) => slab,
        }
    }
}

pub(crate) fn block_id_from_page<'a>(page: NounHandle<'a>) -> Result<NounHandle<'a>, NockAppError> {
    let page_cell = page.as_cell()?;
    // page v0: [block-id ...]
    // page v1: [%1 block-id ...]
    match page_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version == 1 {
                Ok(page_cell.tail().as_cell()?.head())
            } else {
                Err(NockAppError::OtherError(format!(
                    "Unsupported page version {}",
                    version
                )))
            }
        }
        Err(_) => Ok(page_cell.head()),
    }
}

fn tx_id_from_raw_tx<'a>(raw_tx: NounHandle<'a>) -> Result<NounHandle<'a>, NockAppError> {
    let raw_tx_cell = raw_tx.as_cell()?;
    // raw-tx v0: [tx-id ...]
    // raw-tx v1: [%1 tx-id ...]
    match raw_tx_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version == 1 {
                Ok(raw_tx_cell.tail().as_cell()?.head())
            } else {
                Err(NockAppError::OtherError(format!(
                    "Unsupported raw-tx version {}",
                    version
                )))
            }
        }
        Err(_) => Ok(raw_tx_cell.head()),
    }
}

#[derive(Debug, Clone)]
pub enum NockchainDataRequest {
    BlockByHeight(u64), // Height requested
    #[allow(dead_code)]
    EldersById(String, PeerId, NounSlab), // Block ID as string, peer id, block id as noun,
    #[allow(dead_code)]
    RawTransactionById(String, NounSlab), // transaction id as string, transaction id as noun,
    /// Request a block at a given height bundled with its raw transactions
    /// in one response. Wire shape: `[%request [%block-with-txs [%by-height h]]]`.
    BlockWithTxsByHeight(u64),
    /// Request a contiguous range of `len` blocks starting at `start_height`,
    /// each bundled with its raw transactions, in one response. Wire shape:
    /// `[%request [%block-with-txs [%by-range start len]]]`. `len` is
    /// constrained to fit in u8 (capped at 255) to keep range requests
    /// bounded. Phase 3 of catch-up prefetch.
    #[allow(dead_code)]
    BlockRangeWithTxs {
        start_height: u64,
        len: u8,
    },
}

/// Maximum number of blocks a single range request may carry. Capped at u8
/// to keep responder cost and decoder allocation bounded. Responders can
/// return fewer contiguous blocks when the byte budget fills first.
pub const BLOCK_RANGE_REQUEST_MAX_LEN: u8 = u8::MAX;
const RESPONSE_ENVELOPE_ID_MAX_BYTES: usize = 128;

impl NockchainDataRequest {
    /// Takes noun of type [%request p=request]
    pub fn from_noun(noun: Noun, space: &NounSpace) -> Result<Self, NockAppError> {
        let res = (|| {
            let request_cell = noun.in_space(space).as_cell()?;
            if !request_cell.head().eq_bytes(b"request") {
                return Err(NockAppError::OtherError(String::from(
                    "Missing %request tag",
                )));
            }
            // kind cell type
            // $%  [%block request-block]
            //     [%block-with-txs [%by-height page-number]]
            //     [%raw-tx request-tx]
            // ==
            let kind_cell = request_cell.tail().as_cell()?;
            if kind_cell.head().eq_bytes(b"block") {
                // block_cell type
                // $%  [%by-height p=page-number:dt]
                //     [%elders p=block-id:dt q=peer-id]
                // ==
                let block_cell = kind_cell.tail().as_cell()?;
                if block_cell.head().eq_bytes(b"by-height") {
                    let height = block_cell.tail().as_atom()?.as_u64()?;
                    Ok(Self::BlockByHeight(height))
                } else if block_cell.head().eq_bytes(b"elders") {
                    let elders_cell = block_cell.tail().as_cell()?;
                    let block_id = tip5_hash_to_base58(elders_cell.head().noun(), space)?;
                    let peer_id = PeerId::from_noun(elders_cell.tail().noun(), space)?;
                    let slab = {
                        let mut slab = NounSlab::new();
                        slab.copy_into(elders_cell.head().noun(), space);
                        slab
                    };
                    Ok(Self::EldersById(block_id, peer_id, slab))
                } else {
                    Err(NockAppError::OtherError(String::from(
                        "Failed to parse EldersById message",
                    )))
                }
            } else if kind_cell.head().eq_bytes(b"block-with-txs") {
                // inner cell type:
                //   [%by-height page-number]
                //   [%by-range start=page-number len=@]
                let inner_cell = kind_cell.tail().as_cell()?;
                if inner_cell.head().eq_bytes(b"by-height") {
                    let height = inner_cell.tail().as_atom()?.as_u64()?;
                    Ok(Self::BlockWithTxsByHeight(height))
                } else if inner_cell.head().eq_bytes(b"by-range") {
                    let range_cell = inner_cell.tail().as_cell()?;
                    let start_height = range_cell.head().as_atom()?.as_u64()?;
                    let len_atom = range_cell.tail().as_atom()?.as_u64()?;
                    if len_atom == 0 {
                        return Err(NockAppError::OtherError(String::from(
                            "BlockRangeWithTxs len must be > 0",
                        )));
                    }
                    if len_atom > u64::from(BLOCK_RANGE_REQUEST_MAX_LEN) {
                        return Err(NockAppError::OtherError(format!(
                            "BlockRangeWithTxs len {len_atom} exceeds cap {}",
                            BLOCK_RANGE_REQUEST_MAX_LEN
                        )));
                    }
                    let len = u8::try_from(len_atom).map_err(|_| {
                        NockAppError::OtherError(String::from(
                            "BlockRangeWithTxs len did not fit in u8",
                        ))
                    })?;
                    Ok(Self::BlockRangeWithTxs { start_height, len })
                } else {
                    Err(NockAppError::OtherError(String::from(
                        "Failed to parse block-with-txs message: missing %by-height or %by-range tag",
                    )))
                }
            } else if kind_cell.head().eq_bytes(b"raw-tx") {
                // has type [%by-id p=tx-id:dt]
                let raw_tx_cell = kind_cell.tail().as_cell()?;
                let raw_tx_id = tip5_hash_to_base58(raw_tx_cell.tail().noun(), space)?;
                let slab = {
                    let mut slab = NounSlab::new();
                    slab.copy_into(raw_tx_cell.tail().noun(), space);
                    slab
                };
                Ok(Self::RawTransactionById(raw_tx_id, slab))
            } else {
                Err(NockAppError::OtherError(String::from(
                    "Failed to parse RawTransaction message",
                )))
            }
        })();
        res.map_err(|_| NockAppError::IoError(std::io::Error::other("bad request")))
    }
}

/// Build a jammed request message for the existing unbundled block request.
/// Wire noun is `[%request [%block [%by-height h]]]`.
pub fn block_by_height_message(height: u64) -> ByteBuf {
    let mut slab: NounSlab = NounSlab::new();
    let height_atom = Atom::new(&mut slab, height).as_noun();
    let by_height_tag = make_tas(&mut slab, "by-height").as_noun();
    let by_height = T(&mut slab, &[by_height_tag, height_atom]);
    let block = T(&mut slab, &[D(tas!(b"block")), by_height]);
    let request = T(&mut slab, &[D(tas!(b"request")), block]);
    slab.set_root(request);
    ByteBuf::from(slab.jam().as_ref())
}

/// Build a jammed request message for a block-with-txs request at a given
/// height. Wire noun is `[%request [%block-with-txs [%by-height h]]]` so the
/// shape parallels the existing `[%request [%block [%by-height h]]]` used for
/// unbundled block requests and can round-trip through
/// `NockchainDataRequest::from_noun`.
#[allow(dead_code)]
pub fn block_with_txs_by_height_request_message(height: u64) -> Result<ByteBuf, NockAppError> {
    let mut slab: NounSlab = NounSlab::new();
    let height_atom = Atom::new(&mut slab, height).as_noun();
    // `by-height` (9 bytes) and `block-with-txs` (14 bytes) exceed the 8-byte
    // limit of the `tas!` macro, so construct them via `make_tas`.
    let by_height_tag = make_tas(&mut slab, "by-height").as_noun();
    let block_with_txs_tag = make_tas(&mut slab, "block-with-txs").as_noun();
    let by_height = T(&mut slab, &[by_height_tag, height_atom]);
    let block_with_txs = T(&mut slab, &[block_with_txs_tag, by_height]);
    let request = T(&mut slab, &[D(tas!(b"request")), block_with_txs]);
    slab.set_root(request);
    Ok(ByteBuf::from(slab.jam().as_ref()))
}

/// Build a jammed request message for a contiguous block-range bundle.
/// Wire noun is `[%request [%block-with-txs [%by-range start len]]]`.
/// `len` must be in `1..=BLOCK_RANGE_REQUEST_MAX_LEN`.
#[allow(dead_code)]
pub fn block_range_with_txs_request_message(
    start_height: u64,
    len: u8,
) -> Result<ByteBuf, NockAppError> {
    if len == 0 {
        return Err(NockAppError::OtherError(String::from(
            "block-range request len must be > 0",
        )));
    }
    let mut slab: NounSlab = NounSlab::new();
    let start_atom = Atom::new(&mut slab, start_height).as_noun();
    let len_atom = Atom::new(&mut slab, u64::from(len)).as_noun();
    let by_range_tag = make_tas(&mut slab, "by-range").as_noun();
    let block_with_txs_tag = make_tas(&mut slab, "block-with-txs").as_noun();
    let by_range = T(&mut slab, &[by_range_tag, start_atom, len_atom]);
    let block_with_txs = T(&mut slab, &[block_with_txs_tag, by_range]);
    let request = T(&mut slab, &[D(tas!(b"request")), block_with_txs]);
    slab.set_root(request);
    Ok(ByteBuf::from(slab.jam().as_ref()))
}

pub(crate) fn decode_request_item_message(
    message: &[u8],
) -> Result<NockchainDataRequest, NockAppError> {
    let mut request_slab: NounSlab = NounSlab::new();
    let request_noun = request_slab.cue_into(Bytes::copy_from_slice(message))?;
    let space = request_slab.noun_space();
    NockchainDataRequest::from_noun(request_noun, &space)
}

pub(crate) fn request_slab_from_message(message: &[u8]) -> Result<NounSlab, NockAppError> {
    let mut request_slab = NounSlab::new();
    let request_noun = request_slab.cue_into(Bytes::copy_from_slice(message))?;
    request_slab.set_root(request_noun);
    Ok(request_slab)
}

fn tip5_hash_noun_from_base58(slab: &mut NounSlab, base58: &str) -> Result<Noun, NockAppError> {
    let ubig = base58_to_ubig(base58.to_owned())?;
    let words = decimal_to_base_p(ubig)?;
    let word0 = Atom::new(slab, words[0]).as_noun();
    let word1 = Atom::new(slab, words[1]).as_noun();
    let word2 = Atom::new(slab, words[2]).as_noun();
    let word3 = Atom::new(slab, words[3]).as_noun();
    let word4 = Atom::new(slab, words[4]).as_noun();
    Ok(T(slab, &[word0, word1, word2, word3, word4]))
}

pub(crate) fn raw_tx_request_message(tx_id: &str) -> Result<ByteBuf, NockAppError> {
    let mut slab = NounSlab::new();
    let tx_id_noun = tip5_hash_noun_from_base58(&mut slab, tx_id)?;
    let by_id = T(&mut slab, &[D(tas!(b"by-id")), tx_id_noun]);
    let raw_tx = T(&mut slab, &[D(tas!(b"raw-tx")), by_id]);
    let request = T(&mut slab, &[D(tas!(b"request")), raw_tx]);
    slab.set_root(request);
    Ok(ByteBuf::from(slab.jam().as_ref()))
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BatchRequestItem {
    pub item_id: u32,
    pub message: ByteBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BatchResultStatus {
    Result,
    Ack,
    NotFound,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BatchErrorClass {
    Decode,
    Backpressure,
    TooLarge,
    InvalidPow,
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EnvelopeKind {
    HeardBlock,
    HeardTx,
    HeardElders,
    /// A bundle of `[block + its raw txs, in order]`, with an optional
    /// remainder list of tx-ids the responder couldn't fit under the
    /// block-batch response cap. The envelope's `message` field carries
    /// the block fact (same payload shape as `HeardBlock`), `tx_envelopes`
    /// carries the bundled raw-tx facts in block-declared tx-id order, and
    /// `unincluded_tx_ids` lists any tx-ids the requester must fetch
    /// separately via the classic `RawTransactionById` path.
    HeardBlockWithTxs,
    /// A contiguous range of `HeardBlockWithTxs` bundles produced by a
    /// `BlockRangeWithTxs` request. Heights are strictly contiguous and
    /// the count is `<= len`; if the chain runs out, the envelope returns
    /// fewer bundles and the requester reissues for the missing tail.
    /// Carried in `range_blocks`; the envelope's `message`, `block_id`,
    /// `tx_envelopes`, and `unincluded_tx_ids` fields are all unused for
    /// this kind. Phase 3 of catch-up prefetch.
    HeardBlockRangeWithTxs,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledBlockWithTxs {
    pub block_id: String,
    pub block_message: ByteBuf,
    pub tx_envelopes: Vec<BundledTxEnvelope>,
    pub unincluded_tx_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundledTxEnvelope {
    pub tx_id: String,
    pub message: ByteBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseEnvelope {
    pub kind: EnvelopeKind,
    pub block_id: Option<String>,
    pub tx_id: Option<String>,
    pub message: ByteBuf,
    /// Only populated when `kind == HeardBlockWithTxs`. Skipped on the
    /// wire for every other envelope kind so peers running older code
    /// with `deny_unknown_fields` never see a field they don't know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_envelopes: Option<Vec<BundledTxEnvelope>>,
    /// Tx-ids named in the block that did not fit in this bundle response
    /// under the block-batch response cap. Requester must chase each via
    /// `RawTransactionById`. Only populated for `HeardBlockWithTxs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unincluded_tx_ids: Option<Vec<String>>,
    /// Contiguous range of bundled blocks. Only populated for
    /// `HeardBlockRangeWithTxs`. Skipped on the wire for every other
    /// envelope kind so peers running older code with `deny_unknown_fields`
    /// never see a field they don't know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_blocks: Option<Vec<BundledBlockWithTxs>>,
}

impl ResponseEnvelope {
    fn validate_id(label: &str, id: &str) -> Result<(), NockAppError> {
        if id.is_empty() {
            return Err(NockAppError::OtherError(format!(
                "{label} must not be empty"
            )));
        }
        if id.len() > RESPONSE_ENVELOPE_ID_MAX_BYTES {
            return Err(NockAppError::OtherError(format!(
                "{label} exceeded {RESPONSE_ENVELOPE_ID_MAX_BYTES} bytes",
            )));
        }
        Ok(())
    }

    pub fn heard_block(block_id: String, message: impl AsRef<[u8]>) -> Self {
        Self {
            kind: EnvelopeKind::HeardBlock,
            block_id: Some(block_id),
            tx_id: None,
            message: ByteBuf::from(message.as_ref().to_vec()),
            tx_envelopes: None,
            unincluded_tx_ids: None,
            range_blocks: None,
        }
    }

    pub fn heard_tx(tx_id: String, message: impl AsRef<[u8]>) -> Self {
        Self {
            kind: EnvelopeKind::HeardTx,
            block_id: None,
            tx_id: Some(tx_id),
            message: ByteBuf::from(message.as_ref().to_vec()),
            tx_envelopes: None,
            unincluded_tx_ids: None,
            range_blocks: None,
        }
    }

    pub fn heard_block_with_txs(
        block_id: String,
        block_message: impl AsRef<[u8]>,
        tx_envelopes: Vec<BundledTxEnvelope>,
        unincluded_tx_ids: Vec<String>,
    ) -> Self {
        Self {
            kind: EnvelopeKind::HeardBlockWithTxs,
            block_id: Some(block_id),
            tx_id: None,
            message: ByteBuf::from(block_message.as_ref().to_vec()),
            tx_envelopes: Some(tx_envelopes),
            unincluded_tx_ids: Some(unincluded_tx_ids),
            range_blocks: None,
        }
    }

    pub fn heard_block_range_with_txs(blocks: Vec<BundledBlockWithTxs>) -> Self {
        Self {
            kind: EnvelopeKind::HeardBlockRangeWithTxs,
            block_id: None,
            tx_id: None,
            message: ByteBuf::new(),
            tx_envelopes: None,
            unincluded_tx_ids: None,
            range_blocks: Some(blocks),
        }
    }

    pub fn heard_elders(message: impl AsRef<[u8]>) -> Self {
        Self {
            kind: EnvelopeKind::HeardElders,
            block_id: None,
            tx_id: None,
            message: ByteBuf::from(message.as_ref().to_vec()),
            tx_envelopes: None,
            unincluded_tx_ids: None,
            range_blocks: None,
        }
    }

    pub fn validate(&self) -> Result<(), NockAppError> {
        match self.kind {
            EnvelopeKind::HeardBlock => {
                let Some(block_id) = self.block_id.as_ref() else {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block envelope requires block_id",
                    )));
                };
                Self::validate_id("heard-block block_id", block_id)?;
                if self.tx_id.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block envelope must not include tx_id",
                    )));
                }
                if self.tx_envelopes.is_some()
                    || self.unincluded_tx_ids.is_some()
                    || self.range_blocks.is_some()
                {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block envelope must not include bundle or range fields",
                    )));
                }
            }
            EnvelopeKind::HeardTx => {
                let Some(tx_id) = self.tx_id.as_ref() else {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-tx envelope requires tx_id",
                    )));
                };
                Self::validate_id("heard-tx tx_id", tx_id)?;
                if self.block_id.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-tx envelope must not include block_id",
                    )));
                }
                if self.tx_envelopes.is_some()
                    || self.unincluded_tx_ids.is_some()
                    || self.range_blocks.is_some()
                {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-tx envelope must not include bundle or range fields",
                    )));
                }
            }
            EnvelopeKind::HeardElders => {
                if self.block_id.is_some() || self.tx_id.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-elders envelope must not include block_id or tx_id",
                    )));
                }
                if self.tx_envelopes.is_some()
                    || self.unincluded_tx_ids.is_some()
                    || self.range_blocks.is_some()
                {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-elders envelope must not include bundle or range fields",
                    )));
                }
            }
            EnvelopeKind::HeardBlockWithTxs => {
                let Some(block_id) = self.block_id.as_ref() else {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-with-txs envelope requires block_id",
                    )));
                };
                Self::validate_id("heard-block-with-txs block_id", block_id)?;
                if self.tx_id.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-with-txs envelope must not include tx_id",
                    )));
                }
                let Some(tx_envelopes) = self.tx_envelopes.as_ref() else {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-with-txs envelope requires tx_envelopes",
                    )));
                };
                if self.unincluded_tx_ids.is_none() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-with-txs envelope requires unincluded_tx_ids",
                    )));
                }
                let mut seen_tx_ids = BTreeSet::new();
                for bundled in tx_envelopes {
                    Self::validate_id("heard-block-with-txs bundled tx_id", &bundled.tx_id)?;
                    if !seen_tx_ids.insert(bundled.tx_id.clone()) {
                        return Err(NockAppError::OtherError(format!(
                            "heard-block-with-txs duplicate bundled tx_id {}",
                            bundled.tx_id
                        )));
                    }
                }
                if let Some(remainder) = self.unincluded_tx_ids.as_ref() {
                    for tx_id in remainder {
                        Self::validate_id("heard-block-with-txs unincluded tx_id", tx_id)?;
                        if seen_tx_ids.contains(tx_id) {
                            return Err(NockAppError::OtherError(format!(
                                "heard-block-with-txs tx_id {} is both bundled and unincluded",
                                tx_id
                            )));
                        }
                    }
                }
                if self.range_blocks.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-with-txs envelope must not include range_blocks",
                    )));
                }
            }
            EnvelopeKind::HeardBlockRangeWithTxs => {
                if self.block_id.is_some() || self.tx_id.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-range-with-txs envelope must not include block_id or tx_id",
                    )));
                }
                if self.tx_envelopes.is_some() || self.unincluded_tx_ids.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-range-with-txs envelope must not include single-bundle fields",
                    )));
                }
                if !self.message.is_empty() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-range-with-txs envelope must not carry a top-level message",
                    )));
                }
                let Some(blocks) = self.range_blocks.as_ref() else {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-range-with-txs envelope requires range_blocks",
                    )));
                };
                if blocks.is_empty() {
                    return Err(NockAppError::OtherError(String::from(
                        "heard-block-range-with-txs envelope must not be empty",
                    )));
                }
                let mut seen_block_ids = BTreeSet::new();
                for block in blocks {
                    Self::validate_id(
                        "heard-block-range-with-txs bundled block_id", &block.block_id,
                    )?;
                    if !seen_block_ids.insert(block.block_id.clone()) {
                        return Err(NockAppError::OtherError(format!(
                            "heard-block-range-with-txs duplicate block_id {}",
                            block.block_id
                        )));
                    }
                    if block.block_message.is_empty() {
                        return Err(NockAppError::OtherError(String::from(
                            "heard-block-range-with-txs block_message must not be empty",
                        )));
                    }
                    let mut seen_tx_ids = BTreeSet::new();
                    for bundled in &block.tx_envelopes {
                        Self::validate_id(
                            "heard-block-range-with-txs bundled tx_id", &bundled.tx_id,
                        )?;
                        if !seen_tx_ids.insert(bundled.tx_id.clone()) {
                            return Err(NockAppError::OtherError(format!(
                                "heard-block-range-with-txs duplicate bundled tx_id {} in block {}",
                                bundled.tx_id, block.block_id
                            )));
                        }
                    }
                    for tx_id in &block.unincluded_tx_ids {
                        Self::validate_id("heard-block-range-with-txs unincluded tx_id", tx_id)?;
                        if seen_tx_ids.contains(tx_id) {
                            return Err(NockAppError::OtherError(format!(
                                "heard-block-range-with-txs tx_id {} is both bundled and unincluded in block {}",
                                tx_id, block.block_id
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchResultItem {
    pub item_id: u32,
    pub status: BatchResultStatus,
    pub error: Option<BatchErrorClass>,
    pub envelope: Option<ResponseEnvelope>,
}

impl BatchResultItem {
    pub fn validate(&self) -> Result<(), NockAppError> {
        match self.status {
            BatchResultStatus::Result => {
                if self.error.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "result batch status must not include error classification",
                    )));
                }
                if self.envelope.is_none() {
                    return Err(NockAppError::OtherError(String::from(
                        "result batch status requires response envelope",
                    )));
                }
            }
            BatchResultStatus::Error => {
                if self.error.is_none() {
                    return Err(NockAppError::OtherError(String::from(
                        "batch error status requires error classification",
                    )));
                }
                if self.envelope.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "batch error status must not include response envelope",
                    )));
                }
            }
            BatchResultStatus::Ack | BatchResultStatus::NotFound => {
                if self.error.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "non-error batch status must not include error classification",
                    )));
                }
                if self.envelope.is_some() {
                    return Err(NockAppError::OtherError(String::from(
                        "non-result batch status must not include response envelope",
                    )));
                }
            }
        }
        if let Some(envelope) = &self.envelope {
            envelope.validate()?;
        }
        Ok(())
    }
}

fn validate_batch_item_ids(items: &[BatchRequestItem]) -> Result<(), NockAppError> {
    let mut seen_ids = BTreeSet::new();
    for item in items {
        if !seen_ids.insert(item.item_id) {
            return Err(NockAppError::OtherError(format!(
                "duplicate batch item_id {}",
                item.item_id
            )));
        }
    }
    Ok(())
}

fn canonical_batch_item_bytes(items: &[BatchRequestItem]) -> Result<Vec<u8>, NockAppError> {
    validate_batch_item_ids(items)?;
    let item_count = u32::try_from(items.len()).map_err(|_| {
        NockAppError::OtherError(String::from("batch item count exceeds u32 wire limit"))
    })?;

    let payload_capacity = items.iter().try_fold(size_of::<u32>(), |acc, item| {
        let message_len = u32::try_from(item.message.len()).map_err(|_| {
            NockAppError::OtherError(format!(
                "batch item {} exceeds u32 wire length limit",
                item.item_id
            ))
        })?;
        Ok::<usize, NockAppError>(acc + size_of::<u32>() + size_of::<u32>() + message_len as usize)
    })?;

    let mut bytes = Vec::with_capacity(payload_capacity);
    bytes.extend_from_slice(&item_count.to_le_bytes());
    for item in items {
        let message_len = u32::try_from(item.message.len()).map_err(|_| {
            NockAppError::OtherError(format!(
                "batch item {} exceeds u32 wire length limit",
                item.item_id
            ))
        })?;
        bytes.extend_from_slice(&item.item_id.to_le_bytes());
        bytes.extend_from_slice(&message_len.to_le_bytes());
        bytes.extend_from_slice(&item.message);
    }
    Ok(bytes)
}

fn gen1_pow_preimage(
    nonce: u64,
    sender_peer_id: &libp2p::PeerId,
    receiver_peer_id: &libp2p::PeerId,
    message: &[u8],
) -> Vec<u8> {
    let sender_peer_bytes = (*sender_peer_id).to_bytes();
    let receiver_peer_bytes = (*receiver_peer_id).to_bytes();
    let mut pow_buf = Vec::with_capacity(
        size_of::<u64>() + sender_peer_bytes.len() + receiver_peer_bytes.len() + message.len(),
    );
    pow_buf.extend_from_slice(&nonce.to_le_bytes());
    pow_buf.extend_from_slice(&sender_peer_bytes);
    pow_buf.extend_from_slice(&receiver_peer_bytes);
    pow_buf.extend_from_slice(message);
    pow_buf
}

fn gen2_pow_preimage(
    nonce: u64,
    sender_peer_id: &libp2p::PeerId,
    receiver_peer_id: &libp2p::PeerId,
    items: &[BatchRequestItem],
) -> Result<Vec<u8>, NockAppError> {
    let sender_peer_bytes = (*sender_peer_id).to_bytes();
    let receiver_peer_bytes = (*receiver_peer_id).to_bytes();
    let canonical_items = canonical_batch_item_bytes(items)?;
    let mut pow_buf = Vec::with_capacity(
        GEN2_BATCH_POW_DOMAIN_SEPARATOR.len()
            + size_of::<u64>()
            + sender_peer_bytes.len()
            + receiver_peer_bytes.len()
            + canonical_items.len(),
    );
    pow_buf.extend_from_slice(GEN2_BATCH_POW_DOMAIN_SEPARATOR);
    pow_buf.extend_from_slice(&nonce.to_le_bytes());
    pow_buf.extend_from_slice(&sender_peer_bytes);
    pow_buf.extend_from_slice(&receiver_peer_bytes);
    pow_buf.extend_from_slice(&canonical_items);
    Ok(pow_buf)
}

fn gossip_pow_preimage(
    nonce: u64,
    sender_peer_id: &libp2p::PeerId,
    receiver_peer_id: &libp2p::PeerId,
    message: &[u8],
) -> Vec<u8> {
    let sender_peer_bytes = (*sender_peer_id).to_bytes();
    let receiver_peer_bytes = (*receiver_peer_id).to_bytes();
    let mut pow_buf = Vec::with_capacity(
        GOSSIP_POW_DOMAIN_SEPARATOR.len()
            + size_of::<u64>()
            + sender_peer_bytes.len()
            + receiver_peer_bytes.len()
            + message.len(),
    );
    pow_buf.extend_from_slice(GOSSIP_POW_DOMAIN_SEPARATOR);
    pow_buf.extend_from_slice(&nonce.to_le_bytes());
    pow_buf.extend_from_slice(&sender_peer_bytes);
    pow_buf.extend_from_slice(&receiver_peer_bytes);
    pow_buf.extend_from_slice(message);
    pow_buf
}

fn solve_pow(
    builder: &mut equix::EquiXBuilder,
    build_preimage: impl Fn(u64) -> Result<Vec<u8>, NockAppError>,
) -> Result<(equix::SolutionByteArray, u64), NockAppError> {
    let start_nonce = rng().random::<u64>();
    let mut nonce = start_nonce;
    loop {
        let pow_buf = build_preimage(nonce)?;
        if let Ok(sols) = builder.solve(&pow_buf) {
            if !sols.is_empty() {
                return Ok((sols[0].to_bytes(), nonce));
            }
        }
        nonce = nonce.wrapping_add(1);
        if nonce == start_nonce {
            return Err(NockAppError::OtherError(String::from(
                "pow nonce space exhausted",
            )));
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
/// Network struct (in serde/CBOR) for requests
pub enum NockchainRequest {
    /// Request a block or TX from another node, carry PoW
    Request {
        pow: equix::SolutionByteArray,
        nonce: u64,
        message: ByteBuf,
    },
    /// Gossip a block or TX to another node
    Gossip { message: ByteBuf },
    /// Request a batch of transport items from another node, carry PoW
    BatchRequest {
        pow: equix::SolutionByteArray,
        nonce: u64,
        items: Vec<BatchRequestItem>,
    },
    /// Gossip a block or TX to another node with sender-bound PoW
    AuthenticatedGossip {
        pow: equix::SolutionByteArray,
        nonce: u64,
        message: ByteBuf,
    },
}

const REPLAY_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const REPLAY_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

fn replay_hash_update(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(REPLAY_HASH_PRIME);
    }
}

fn replay_hash_u32(hash: &mut u64, value: u32) {
    replay_hash_update(hash, &value.to_le_bytes());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RequestReplayKind {
    BatchRequest,
    AuthenticatedGossip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RequestReplayKey {
    pub(crate) kind: RequestReplayKind,
    pub(crate) nonce: u64,
    pub(crate) payload_hash: u64,
    pub(crate) payload_bytes: usize,
}

fn batch_items_replay_hash(items: &[BatchRequestItem]) -> Result<(u64, usize), NockAppError> {
    let item_count = u32::try_from(items.len()).map_err(|_| {
        NockAppError::OtherError(format!(
            "batch item count {} exceeds u32 wire limit",
            items.len()
        ))
    })?;
    let mut hash = REPLAY_HASH_OFFSET;
    replay_hash_u32(&mut hash, item_count);
    let mut payload_bytes = size_of::<u32>();
    for item in items {
        let message_len = u32::try_from(item.message.len()).map_err(|_| {
            NockAppError::OtherError(format!(
                "batch item {} exceeds u32 wire length limit",
                item.item_id
            ))
        })?;
        replay_hash_u32(&mut hash, item.item_id);
        replay_hash_u32(&mut hash, message_len);
        replay_hash_update(&mut hash, &item.message);
        payload_bytes += size_of::<u32>() + size_of::<u32>() + item.message.len();
    }
    Ok((hash, payload_bytes))
}

impl NockchainRequest {
    /// Make a new "request" which gossips a block or a TX
    pub(crate) fn new_gossip(message: &NounSlab) -> NockchainRequest {
        let message_bytes = ByteBuf::from(message.jam().as_ref());
        NockchainRequest::Gossip {
            message: message_bytes,
        }
    }

    pub fn authenticated_gossip_from_message(
        builder: &mut equix::EquiXBuilder,
        local_peer_id: &libp2p::PeerId,
        remote_peer_id: &libp2p::PeerId,
        message: ByteBuf,
    ) -> Result<NockchainRequest, NockAppError> {
        let (pow, nonce) = solve_pow(builder, |nonce| {
            Ok(gossip_pow_preimage(
                nonce, local_peer_id, remote_peer_id, &message,
            ))
        })?;

        Ok(NockchainRequest::AuthenticatedGossip {
            pow,
            nonce,
            message,
        })
    }

    pub(crate) fn authenticate_gossip(
        self,
        builder: &mut equix::EquiXBuilder,
        local_peer_id: &libp2p::PeerId,
        remote_peer_id: &libp2p::PeerId,
    ) -> Result<NockchainRequest, NockAppError> {
        match self {
            Self::Gossip { message } => Self::authenticated_gossip_from_message(
                builder, local_peer_id, remote_peer_id, message,
            ),
            other => Ok(other),
        }
    }

    /// Make a new request for a block or a TX
    pub(crate) fn new_request(
        builder: &mut equix::EquiXBuilder,
        local_peer_id: &libp2p::PeerId,
        remote_peer_id: &libp2p::PeerId,
        message: &NounSlab,
    ) -> NockchainRequest {
        let message_bytes = ByteBuf::from(message.jam().as_ref());

        let mut nonce = 0u64;
        let sol_bytes = loop {
            let pow_buf = gen1_pow_preimage(nonce, local_peer_id, remote_peer_id, &message_bytes);
            if let Ok(sols) = builder.solve(&pow_buf) {
                if !sols.is_empty() {
                    break sols[0].to_bytes();
                }
            }
            nonce += 1;
        };

        NockchainRequest::Request {
            pow: sol_bytes,
            nonce,
            message: message_bytes,
        }
    }

    pub fn new_batch_request(
        builder: &mut equix::EquiXBuilder,
        local_peer_id: &libp2p::PeerId,
        remote_peer_id: &libp2p::PeerId,
        items: Vec<BatchRequestItem>,
    ) -> Result<NockchainRequest, NockAppError> {
        validate_batch_item_ids(&items)?;

        let (sol_bytes, nonce) = solve_pow(builder, |nonce| {
            gen2_pow_preimage(nonce, local_peer_id, remote_peer_id, &items)
        })?;

        Ok(NockchainRequest::BatchRequest {
            pow: sol_bytes,
            nonce,
            items,
        })
    }

    pub fn validate(&self) -> Result<(), NockAppError> {
        match self {
            Self::Request { .. } | Self::Gossip { .. } | Self::AuthenticatedGossip { .. } => Ok(()),
            Self::BatchRequest { items, .. } => validate_batch_item_ids(items),
        }
    }

    pub(crate) fn replay_key(&self) -> Result<Option<RequestReplayKey>, NockAppError> {
        match self {
            Self::Request { .. } => Ok(None),
            Self::BatchRequest { nonce, items, .. } => {
                let (payload_hash, payload_bytes) = batch_items_replay_hash(items)?;
                Ok(Some(RequestReplayKey {
                    kind: RequestReplayKind::BatchRequest,
                    nonce: *nonce,
                    payload_hash,
                    payload_bytes,
                }))
            }
            Self::AuthenticatedGossip { nonce, message, .. } => {
                let mut hash = REPLAY_HASH_OFFSET;
                replay_hash_update(&mut hash, message);
                Ok(Some(RequestReplayKey {
                    kind: RequestReplayKind::AuthenticatedGossip,
                    nonce: *nonce,
                    payload_hash: hash,
                    payload_bytes: message.len(),
                }))
            }
            Self::Gossip { .. } => Ok(None),
        }
    }

    /// Verify the EquiX PoW attached to a request
    pub(crate) fn verify_pow(
        &self,
        builder: &mut equix::EquiXBuilder,
        local_peer_id: &libp2p::PeerId,
        remote_peer_id: &libp2p::PeerId,
    ) -> Result<(), NockAppError> {
        match self {
            NockchainRequest::Request {
                pow,
                nonce,
                message,
            } => {
                // This looks backwards, but it's because local/remote swap between
                // sender-side generation and receiver-side verification.
                let pow_buf = gen1_pow_preimage(*nonce, remote_peer_id, local_peer_id, message);
                builder.verify_bytes(&pow_buf, pow).map_err(|err| {
                    NockAppError::OtherError(format!("pow verification failed: {err}"))
                })
            }
            NockchainRequest::Gossip { message: _ } => Ok(()),
            NockchainRequest::BatchRequest { pow, nonce, items } => {
                let pow_buf = gen2_pow_preimage(*nonce, remote_peer_id, local_peer_id, items)?;
                builder.verify_bytes(&pow_buf, pow).map_err(|err| {
                    NockAppError::OtherError(format!("pow verification failed: {err}"))
                })
            }
            NockchainRequest::AuthenticatedGossip {
                pow,
                nonce,
                message,
            } => {
                let pow_buf = gossip_pow_preimage(*nonce, remote_peer_id, local_peer_id, message);
                builder.verify_bytes(&pow_buf, pow).map_err(|err| {
                    NockAppError::OtherError(format!("pow verification failed: {err}"))
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
/// Responses to Nockchain requests
pub enum NockchainResponse {
    /// The requested block or raw-tx
    Result { message: ByteBuf },
    /// If the request was a gossip, no actual response is needed
    Ack { acked: bool },
    /// Per-item outcomes for a batched request
    BatchResult { results: Vec<BatchResultItem> },
}

impl NockchainResponse {
    pub(crate) fn new_response_result(message: impl AsRef<[u8]>) -> NockchainResponse {
        let message_bytes: &[u8] = message.as_ref();
        let message_bytebuf = ByteBuf::from(message_bytes.to_vec());
        NockchainResponse::Result {
            message: message_bytebuf,
        }
    }

    pub fn validate(&self) -> Result<(), NockAppError> {
        match self {
            NockchainResponse::Result { .. } | NockchainResponse::Ack { .. } => Ok(()),
            NockchainResponse::BatchResult { results } => {
                let mut seen_ids = BTreeSet::new();
                for result in results {
                    if !seen_ids.insert(result.item_id) {
                        return Err(NockAppError::OtherError(format!(
                            "duplicate batch result item_id {}",
                            result.item_id
                        )));
                    }
                    result.validate()?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nockvm::noun::{Atom, NounAllocator, D, T};
    use nockvm_macros::tas;
    use serde_bytes::ByteBuf;

    use super::{
        block_range_with_txs_request_message, decode_request_item_message, BatchRequestItem,
        BatchResultItem, BatchResultStatus, BundledBlockWithTxs, BundledTxEnvelope, EnvelopeKind,
        NockchainDataRequest, NockchainFact, NockchainRequest, NockchainResponse, NounSlab,
        ResponseEnvelope,
    };

    #[test]
    fn test_nockchain_fact_rejects_invalid_jam_without_panicking() {
        let result = std::panic::catch_unwind(|| NockchainFact::from_message_bytes(&[0; 13]));
        assert!(result.is_ok(), "invalid jam payload must not panic");
        assert!(result
            .expect("catch_unwind result should be present")
            .is_err());
    }

    #[test]
    fn test_response_envelope_validation_matrix() {
        let valid_block = ResponseEnvelope::heard_block(String::from("block-id"), [1, 2, 3]);
        assert!(valid_block.validate().is_ok());

        let valid_tx = ResponseEnvelope::heard_tx(String::from("tx-id"), [4, 5, 6]);
        assert!(valid_tx.validate().is_ok());

        let valid_elders = ResponseEnvelope::heard_elders([7, 8, 9]);
        assert!(valid_elders.validate().is_ok());

        let invalid_block = ResponseEnvelope::heard_block(String::from("block-id"), [1]);
        let invalid_block = ResponseEnvelope {
            tx_id: Some(String::from("tx-id")),
            ..invalid_block
        };
        assert!(invalid_block.validate().is_err());
    }

    #[test]
    fn test_batch_result_validation_requires_consistent_error_fields() {
        let invalid = NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Error,
                error: None,
                envelope: None,
            }],
        };
        assert!(invalid.validate().is_err());

        let valid = NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Error,
                error: Some(super::BatchErrorClass::Internal),
                envelope: None,
            }],
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn test_batch_result_validation_requires_envelopes_for_result_items() {
        let invalid = NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Result,
                error: None,
                envelope: None,
            }],
        };
        assert!(invalid.validate().is_err());

        let valid = NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(String::from("tx-id"), [1, 2, 3])),
            }],
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn replay_key_ignores_singleton_requests() {
        let replay_key = NockchainRequest::Request {
            pow: [0; 16],
            nonce: 9,
            message: ByteBuf::from(b"first".to_vec()),
        }
        .replay_key()
        .expect("singleton request replay decision should build");

        assert!(replay_key.is_none());
    }

    #[test]
    fn replay_key_matches_identical_batch_payloads() {
        let items = vec![
            BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"b".to_vec()),
            },
            BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"a".to_vec()),
            },
        ];
        let first = NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 99,
            items: items.clone(),
        }
        .replay_key()
        .expect("batch replay key should build")
        .expect("batch has a replay key");
        let second = NockchainRequest::BatchRequest {
            pow: [1; 16],
            nonce: 99,
            items,
        }
        .replay_key()
        .expect("batch replay key should build")
        .expect("batch has a replay key");

        assert_eq!(first.kind, crate::messages::RequestReplayKind::BatchRequest);
        assert_eq!(first, second);
    }

    #[test]
    fn replay_key_distinguishes_reissued_batch_nonce() {
        let items = vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(b"a".to_vec()),
        }];
        let first = NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 99,
            items: items.clone(),
        }
        .replay_key()
        .expect("batch replay key should build")
        .expect("batch has a replay key");
        let second = NockchainRequest::BatchRequest {
            pow: [1; 16],
            nonce: 100,
            items,
        }
        .replay_key()
        .expect("batch replay key should build")
        .expect("batch has a replay key");

        assert_ne!(first, second);
    }

    #[test]
    fn replay_key_ignores_legacy_gossip() {
        let key = NockchainRequest::Gossip {
            message: ByteBuf::from(b"gossip".to_vec()),
        }
        .replay_key()
        .expect("legacy gossip replay key should be absent");

        assert!(key.is_none());
    }

    #[test]
    fn test_new_batch_request_rejects_duplicate_item_ids() {
        let mut builder = equix::EquiXBuilder::new();
        let local_peer_id = libp2p::PeerId::random();
        let remote_peer_id = libp2p::PeerId::random();
        let items = vec![
            BatchRequestItem {
                item_id: 7,
                message: serde_bytes::ByteBuf::from(vec![1, 2, 3]),
            },
            BatchRequestItem {
                item_id: 7,
                message: serde_bytes::ByteBuf::from(vec![4, 5, 6]),
            },
        ];

        let result = NockchainRequest::new_batch_request(
            &mut builder, &local_peer_id, &remote_peer_id, items,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_request_validation_rejects_duplicate_item_ids() {
        let request = NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: vec![
                BatchRequestItem {
                    item_id: 5,
                    message: ByteBuf::from(vec![1, 2, 3]),
                },
                BatchRequestItem {
                    item_id: 5,
                    message: ByteBuf::from(vec![4, 5, 6]),
                },
            ],
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn test_gen2_pow_preimage_matches_spec_layout() {
        let sender_peer_id = libp2p::PeerId::random();
        let receiver_peer_id = libp2p::PeerId::random();
        let nonce = 0x0807_0605_0403_0201u64;
        let items = vec![
            BatchRequestItem {
                item_id: 7,
                message: ByteBuf::from(vec![0xAA, 0xBB, 0xCC]),
            },
            BatchRequestItem {
                item_id: 11,
                message: ByteBuf::from(vec![0x10, 0x20]),
            },
        ];

        let actual = super::gen2_pow_preimage(nonce, &sender_peer_id, &receiver_peer_id, &items)
            .expect("gen2 PoW preimage should build");

        let mut expected = Vec::new();
        expected.extend_from_slice(super::GEN2_BATCH_POW_DOMAIN_SEPARATOR);
        expected.extend_from_slice(&nonce.to_le_bytes());
        expected.extend_from_slice(&sender_peer_id.to_bytes());
        expected.extend_from_slice(&receiver_peer_id.to_bytes());
        expected.extend_from_slice(&(items.len() as u32).to_le_bytes());
        expected.extend_from_slice(&7u32.to_le_bytes());
        expected.extend_from_slice(&3u32.to_le_bytes());
        expected.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        expected.extend_from_slice(&11u32.to_le_bytes());
        expected.extend_from_slice(&2u32.to_le_bytes());
        expected.extend_from_slice(&[0x10, 0x20]);

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_envelope_decode_rejects_unknown_fields() {
        #[derive(serde::Serialize)]
        struct EnvelopeWithHeight {
            kind: EnvelopeKind,
            block_id: Option<String>,
            tx_id: Option<String>,
            message: ByteBuf,
            height: u64,
        }

        let encoded = serde_cbor::to_vec(&EnvelopeWithHeight {
            kind: EnvelopeKind::HeardBlock,
            block_id: Some(String::from("block-id")),
            tx_id: None,
            message: ByteBuf::from(vec![1, 2, 3]),
            height: 42,
        })
        .expect("test envelope should serialize");

        let decoded: Result<ResponseEnvelope, _> = serde_cbor::from_slice(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn test_block_with_txs_by_height_request_round_trips_through_from_noun() {
        use bytes::Bytes;
        use nockapp::noun::slab::NounSlab;

        use super::{block_with_txs_by_height_request_message, NockchainDataRequest};

        let height = 44_192u64;
        let message = block_with_txs_by_height_request_message(height)
            .expect("bundle request message should jam");

        let mut slab: NounSlab = NounSlab::new();
        let noun = slab
            .cue_into(Bytes::copy_from_slice(&message))
            .expect("bundle request message should cue back");
        let space = slab.noun_space();
        let decoded = NockchainDataRequest::from_noun(noun, &space)
            .expect("bundle request noun should decode");
        match decoded {
            NockchainDataRequest::BlockWithTxsByHeight(decoded_height) => {
                assert_eq!(decoded_height, height)
            }
            other => panic!("expected BlockWithTxsByHeight, got {other:?}"),
        }
    }

    #[test]
    fn test_heard_block_with_txs_envelope_round_trips_through_cbor() {
        use super::BundledTxEnvelope;

        let tx_envelopes = vec![
            BundledTxEnvelope {
                tx_id: String::from("tx-a"),
                message: ByteBuf::from(vec![0xAA, 0xAB]),
            },
            BundledTxEnvelope {
                tx_id: String::from("tx-b"),
                message: ByteBuf::from(vec![0xBB, 0xBC, 0xBD]),
            },
        ];
        let envelope = ResponseEnvelope::heard_block_with_txs(
            String::from("bundle-block"),
            vec![0xAA, 0xBB, 0xCC],
            tx_envelopes,
            vec![String::from("tx-c-unincluded")],
        );
        envelope
            .validate()
            .expect("freshly constructed bundle envelope should validate");

        let encoded = serde_cbor::to_vec(&envelope).expect("bundle envelope should serialize");
        let decoded: ResponseEnvelope =
            serde_cbor::from_slice(&encoded).expect("bundle envelope should deserialize");
        assert_eq!(decoded.kind, EnvelopeKind::HeardBlockWithTxs);
        assert_eq!(decoded.block_id.as_deref(), Some("bundle-block"));
        assert!(decoded.tx_id.is_none());
        assert_eq!(
            decoded.tx_envelopes.as_ref().map(|v| v.len()),
            Some(2),
            "bundled tx envelopes must survive the round-trip"
        );
        assert_eq!(
            decoded.unincluded_tx_ids.as_deref(),
            Some(&[String::from("tx-c-unincluded")][..]),
            "unincluded tx ids must survive the round-trip"
        );
        decoded
            .validate()
            .expect("decoded bundle should revalidate");
    }

    #[test]
    fn test_heard_block_with_txs_envelope_rejects_duplicate_and_overlapping_tx_ids() {
        use super::BundledTxEnvelope;

        let duplicate_bundled = ResponseEnvelope::heard_block_with_txs(
            String::from("block-id"),
            vec![1, 2],
            vec![
                BundledTxEnvelope {
                    tx_id: String::from("tx-dup"),
                    message: ByteBuf::from(vec![0]),
                },
                BundledTxEnvelope {
                    tx_id: String::from("tx-dup"),
                    message: ByteBuf::from(vec![1]),
                },
            ],
            vec![],
        );
        assert!(duplicate_bundled.validate().is_err());

        let overlapping = ResponseEnvelope::heard_block_with_txs(
            String::from("block-id"),
            vec![1, 2],
            vec![BundledTxEnvelope {
                tx_id: String::from("shared-tx"),
                message: ByteBuf::from(vec![0]),
            }],
            vec![String::from("shared-tx")],
        );
        assert!(overlapping.validate().is_err());
    }

    #[test]
    fn test_heard_block_with_txs_envelope_requires_bundle_fields() {
        let missing_envelopes = ResponseEnvelope {
            kind: EnvelopeKind::HeardBlockWithTxs,
            block_id: Some(String::from("block-id")),
            tx_id: None,
            message: ByteBuf::from(vec![1]),
            tx_envelopes: None,
            unincluded_tx_ids: Some(vec![]),
            range_blocks: None,
        };
        assert!(missing_envelopes.validate().is_err());

        let missing_remainder = ResponseEnvelope {
            kind: EnvelopeKind::HeardBlockWithTxs,
            block_id: Some(String::from("block-id")),
            tx_id: None,
            message: ByteBuf::from(vec![1]),
            tx_envelopes: Some(vec![]),
            unincluded_tx_ids: None,
            range_blocks: None,
        };
        assert!(missing_remainder.validate().is_err());

        let non_bundle_with_bundle_fields = ResponseEnvelope {
            kind: EnvelopeKind::HeardBlock,
            block_id: Some(String::from("block-id")),
            tx_id: None,
            message: ByteBuf::from(vec![1]),
            tx_envelopes: Some(vec![]),
            unincluded_tx_ids: None,
            range_blocks: None,
        };
        assert!(non_bundle_with_bundle_fields.validate().is_err());
    }

    #[test]
    fn test_block_range_request_round_trips_through_noun() {
        use nockapp::utils::make_tas;

        let mut slab: NounSlab = NounSlab::new();
        let start_atom = Atom::new(&mut slab, 100u64).as_noun();
        let len_atom = Atom::new(&mut slab, 8u64).as_noun();
        let by_range_tag = make_tas(&mut slab, "by-range").as_noun();
        let block_with_txs_tag = make_tas(&mut slab, "block-with-txs").as_noun();
        let by_range = T(&mut slab, &[by_range_tag, start_atom, len_atom]);
        let block_with_txs = T(&mut slab, &[block_with_txs_tag, by_range]);
        let request = T(&mut slab, &[D(tas!(b"request")), block_with_txs]);
        slab.set_root(request);

        let space = slab.noun_space();
        let parsed = NockchainDataRequest::from_noun(request, &space)
            .expect("by-range request should parse");
        match parsed {
            NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
                assert_eq!(start_height, 100);
                assert_eq!(len, 8);
            }
            other => panic!("expected BlockRangeWithTxs, got {other:?}"),
        }
    }

    #[test]
    fn test_block_range_request_message_decodes_back() {
        let bytes = block_range_with_txs_request_message(42, 16)
            .expect("range request message should encode");
        let parsed = decode_request_item_message(&bytes).expect("range message should decode");
        assert!(matches!(
            parsed,
            NockchainDataRequest::BlockRangeWithTxs {
                start_height: 42,
                len: 16
            }
        ));
    }

    #[test]
    fn test_block_range_request_rejects_zero_len_and_overlong() {
        // zero-len
        {
            let mut slab: NounSlab = NounSlab::new();
            let by_range_tag = nockapp::utils::make_tas(&mut slab, "by-range").as_noun();
            let block_with_txs_tag =
                nockapp::utils::make_tas(&mut slab, "block-with-txs").as_noun();
            let one = Atom::new(&mut slab, 1u64).as_noun();
            let zero = Atom::new(&mut slab, 0u64).as_noun();
            let zero_len = T(&mut slab, &[by_range_tag, one, zero]);
            let body = T(&mut slab, &[block_with_txs_tag, zero_len]);
            let req = T(&mut slab, &[D(tas!(b"request")), body]);
            slab.set_root(req);
            let space = slab.noun_space();
            assert!(NockchainDataRequest::from_noun(req, &space).is_err());
        }

        // over-cap (1024 > BLOCK_RANGE_REQUEST_MAX_LEN)
        {
            let mut slab: NounSlab = NounSlab::new();
            let by_range_tag = nockapp::utils::make_tas(&mut slab, "by-range").as_noun();
            let block_with_txs_tag =
                nockapp::utils::make_tas(&mut slab, "block-with-txs").as_noun();
            let one = Atom::new(&mut slab, 1u64).as_noun();
            let too_big_atom = Atom::new(&mut slab, 1024u64).as_noun();
            let too_big = T(&mut slab, &[by_range_tag, one, too_big_atom]);
            let body = T(&mut slab, &[block_with_txs_tag, too_big]);
            let req = T(&mut slab, &[D(tas!(b"request")), body]);
            slab.set_root(req);
            let space = slab.noun_space();
            assert!(NockchainDataRequest::from_noun(req, &space).is_err());
        }
    }

    #[test]
    fn test_heard_block_range_envelope_round_trips_through_cbor() {
        let blocks = vec![
            BundledBlockWithTxs {
                block_id: String::from("block-1"),
                block_message: ByteBuf::from(vec![0xAA]),
                tx_envelopes: vec![BundledTxEnvelope {
                    tx_id: String::from("tx-a"),
                    message: ByteBuf::from(vec![0x01]),
                }],
                unincluded_tx_ids: vec![],
            },
            BundledBlockWithTxs {
                block_id: String::from("block-2"),
                block_message: ByteBuf::from(vec![0xBB]),
                tx_envelopes: vec![],
                unincluded_tx_ids: vec![String::from("tx-b-unincluded")],
            },
        ];
        let envelope = ResponseEnvelope::heard_block_range_with_txs(blocks);
        envelope
            .validate()
            .expect("freshly constructed range envelope should validate");

        let encoded = serde_cbor::to_vec(&envelope).expect("range envelope should serialize");
        let decoded: ResponseEnvelope =
            serde_cbor::from_slice(&encoded).expect("range envelope should deserialize");
        assert_eq!(decoded.kind, EnvelopeKind::HeardBlockRangeWithTxs);
        let decoded_blocks = decoded
            .range_blocks
            .as_ref()
            .expect("range envelope must carry range_blocks");
        assert_eq!(decoded_blocks.len(), 2);
        assert_eq!(decoded_blocks[0].block_id, "block-1");
        assert_eq!(
            decoded_blocks[0].tx_envelopes[0].tx_id, "tx-a",
            "bundled txs survive the round trip"
        );
        assert_eq!(decoded_blocks[1].unincluded_tx_ids, vec!["tx-b-unincluded"]);
        decoded
            .validate()
            .expect("decoded range envelope should revalidate");
    }

    #[test]
    fn test_heard_block_range_envelope_rejects_duplicate_block_ids() {
        let dup = ResponseEnvelope::heard_block_range_with_txs(vec![
            BundledBlockWithTxs {
                block_id: String::from("dup"),
                block_message: ByteBuf::from(vec![1]),
                tx_envelopes: vec![],
                unincluded_tx_ids: vec![],
            },
            BundledBlockWithTxs {
                block_id: String::from("dup"),
                block_message: ByteBuf::from(vec![1]),
                tx_envelopes: vec![],
                unincluded_tx_ids: vec![],
            },
        ]);
        assert!(dup.validate().is_err());
    }

    #[test]
    fn test_heard_block_range_envelope_rejects_empty_blocks() {
        let empty = ResponseEnvelope::heard_block_range_with_txs(Vec::new());
        assert!(empty.validate().is_err());
    }
}
