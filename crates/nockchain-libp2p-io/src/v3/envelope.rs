//! Convert complete protobuf DTOs into checked values before driver dispatch.
use std::collections::BTreeSet;
use std::io;

use bytes::Bytes;
use libp2p::PeerId;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockvm::noun::{Atom, Noun, NounAllocator, NounHandle, D, T};
use prost::Message;
use serde_bytes::ByteBuf;

use super::common::{invalid, list, required, tuple, PeerHash};
use super::page::PeerPage;
use super::pb;
use super::transaction::Transaction;
use crate::messages::{
    decode_request_item_message, BatchErrorClass, BatchRequestItem, BatchResultItem,
    BatchResultStatus, BundledBlockWithTxs, BundledTxEnvelope, EnvelopeKind, NockchainDataRequest,
    NockchainRequest, NockchainResponse, ResponseEnvelope, NETWORK_CUE_MAX_NODES,
};

const MAX_BATCH_ITEMS: usize = 4096;
const MAX_BUNDLE_ITEMS: usize = 262_144;
const MAX_RANGE_BLOCKS: usize = 255;
const MAX_ELDERS: usize = 24;

fn count(len: usize, max: usize, label: &str) -> io::Result<()> {
    if len > max {
        return Err(invalid(format!("{label} exceeds item limit")));
    }
    Ok(())
}

fn external_error(error: impl std::fmt::Display) -> io::Error {
    invalid(error.to_string())
}

/// Only the outbound adapter calls cue: these bytes originate in this process's
/// driver/kernel, never in a peer protobuf field. Inbound calls construct nouns.
fn local_slab(message: &[u8]) -> io::Result<NounSlab> {
    let mut slab = NounSlab::new();
    let noun = slab
        .cue_into_with_max_nodes(Bytes::copy_from_slice(message), NETWORK_CUE_MAX_NODES)
        .map_err(external_error)?;
    slab.set_root(noun);
    Ok(slab)
}

fn jam(slab: &NounSlab) -> ByteBuf {
    ByteBuf::from(slab.jam().as_ref())
}

fn preserve_local_message(original: &[u8], reconstructed: &[u8]) -> io::Result<()> {
    if original != reconstructed {
        return Err(invalid(
            "local message is not the canonical v3 consensus representation",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Elders {
    oldest: u64,
    ids: Vec<PeerHash>,
}

impl Elders {
    fn from_proto(wire: pb::Elders) -> io::Result<Self> {
        count(wire.block_ids.len(), MAX_ELDERS, "elders")?;
        Ok(Self {
            oldest: required(wire.oldest_height, "elders.oldest_height")?,
            ids: wire
                .block_ids
                .into_iter()
                .map(PeerHash::from_proto)
                .collect::<io::Result<_>>()?,
        })
    }

    fn to_proto(&self) -> pb::Elders {
        pb::Elders {
            oldest_height: Some(self.oldest),
            block_ids: self.ids.iter().map(PeerHash::to_proto).collect(),
        }
    }

    fn from_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let fields = tuple(noun, 2)?;
        let oldest = fields[0]
            .as_atom()
            .map_err(external_error)?
            .as_u64()
            .map_err(external_error)?;
        let ids = list(fields[1])?;
        count(ids.len(), MAX_ELDERS, "elders")?;
        Ok(Self {
            oldest,
            ids: ids
                .into_iter()
                .map(PeerHash::from_noun)
                .collect::<io::Result<_>>()?,
        })
    }

    fn to_noun(&self, slab: &mut NounSlab) -> Noun {
        let oldest = Atom::new(slab, self.oldest).as_noun();
        let mut ids = D(0);
        for id in self.ids.iter().rev() {
            let id = id.to_noun(slab);
            ids = T(slab, &[id, ids]);
        }
        T(slab, &[oldest, ids])
    }
}

#[derive(Clone, Debug)]
enum Fact {
    Block(PeerPage),
    Transaction(Transaction),
    Elders(Elders),
}

impl Fact {
    fn from_proto(wire: pb::Fact) -> io::Result<Self> {
        match required(wire.kind, "fact.kind")? {
            pb::fact::Kind::Block(value) => Ok(Self::Block(PeerPage::from_proto(value)?)),
            pb::fact::Kind::Transaction(value) => {
                Ok(Self::Transaction(Transaction::from_proto(value)?))
            }
            pb::fact::Kind::Elders(value) => Ok(Self::Elders(Elders::from_proto(value)?)),
        }
    }

    fn to_proto(&self) -> pb::Fact {
        use pb::fact::Kind;
        pb::Fact {
            kind: Some(match self {
                Self::Block(value) => Kind::Block(value.to_proto()),
                Self::Transaction(value) => Kind::Transaction(value.to_proto()),
                Self::Elders(value) => Kind::Elders(value.to_proto()),
            }),
        }
    }

    fn from_local_message(message: &[u8]) -> io::Result<Self> {
        let slab = local_slab(message)?;
        let space = slab.noun_space();
        let noun = unsafe { slab.root() }.in_space(&space);
        let cell = noun.as_cell().map_err(external_error)?;
        let fact = if cell.head().eq_bytes(b"heard-block") {
            Self::Block(PeerPage::from_noun(cell.tail())?)
        } else if cell.head().eq_bytes(b"heard-tx") {
            Self::Transaction(Transaction::from_noun(cell.tail())?)
        } else if cell.head().eq_bytes(b"heard-elders") {
            Self::Elders(Elders::from_noun(cell.tail())?)
        } else {
            return Err(invalid("unknown local fact kind"));
        };
        // Authentication commits to this canonical consensus encoding. Never
        // silently change the sender's preimage by normalizing a local payload.
        preserve_local_message(message, &fact.to_message()?)?;
        Ok(fact)
    }

    fn to_message(&self) -> io::Result<ByteBuf> {
        let mut slab = NounSlab::new();
        let (tag, body) = match self {
            Self::Block(value) => ("heard-block", value.to_noun(&mut slab)?),
            Self::Transaction(value) => ("heard-tx", value.to_noun(&mut slab)?),
            Self::Elders(value) => ("heard-elders", value.to_noun(&mut slab)),
        };
        let tag = make_tas(&mut slab, tag).as_noun();
        let noun = T(&mut slab, &[tag, body]);
        slab.set_root(noun);
        Ok(jam(&slab))
    }
}

fn block_id(page: &PeerPage) -> io::Result<String> {
    PeerHash::from_proto(required(page.to_proto().digest, "page.digest")?)?.to_base58()
}

fn transaction_id(tx: &Transaction) -> io::Result<String> {
    use pb::transaction::Version;
    let id = match required(tx.to_proto().version, "transaction.version")? {
        Version::Legacy(tx) => tx.id,
        Version::V1(tx) => tx.id,
    };
    PeerHash::from_proto(required(id, "transaction.id")?)?.to_base58()
}

#[derive(Debug)]
enum DataRequest {
    Block(u64),
    Elders(PeerHash, PeerId),
    Transaction(PeerHash),
    Bundle(u64),
    Range { start: u64, length: u8 },
}

impl DataRequest {
    fn from_proto(wire: pb::DataRequest) -> io::Result<Self> {
        use pb::data_request::Kind;
        Ok(match required(wire.kind, "request.kind")? {
            Kind::BlockByHeight(height) => Self::Block(height),
            Kind::Elders(elders) => Self::Elders(
                PeerHash::from_proto(required(elders.block_id, "elders.block_id")?)?,
                PeerId::from_bytes(&required(elders.peer_id, "elders.peer_id")?)
                    .map_err(external_error)?,
            ),
            Kind::TransactionById(id) => Self::Transaction(PeerHash::from_proto(id)?),
            Kind::BlockWithTransactionsByHeight(height) => Self::Bundle(height),
            Kind::BlockRange(range) => {
                let start = required(range.start_height, "range.start_height")?;
                let length = required(range.length, "range.length")?;
                if length == 0 || length > MAX_RANGE_BLOCKS as u32 {
                    return Err(invalid("range length must be in 1..255"));
                }
                start
                    .checked_add(u64::from(length) - 1)
                    .ok_or_else(|| invalid("range height overflow"))?;
                Self::Range {
                    start,
                    length: length as u8,
                }
            }
        })
    }

    fn to_proto(&self) -> pb::DataRequest {
        use pb::data_request::Kind;
        pb::DataRequest {
            kind: Some(match self {
                Self::Block(height) => Kind::BlockByHeight(*height),
                Self::Elders(id, peer) => Kind::Elders(pb::EldersRequest {
                    block_id: Some(id.to_proto()),
                    peer_id: Some(peer.to_bytes()),
                }),
                Self::Transaction(id) => Kind::TransactionById(id.to_proto()),
                Self::Bundle(height) => Kind::BlockWithTransactionsByHeight(*height),
                Self::Range { start, length } => Kind::BlockRange(pb::BlockRangeRequest {
                    start_height: Some(*start),
                    length: Some(u32::from(*length)),
                }),
            }),
        }
    }

    fn from_local_message(message: &[u8]) -> io::Result<Self> {
        let request = match decode_request_item_message(message).map_err(external_error)? {
            NockchainDataRequest::BlockByHeight(height) => Self::Block(height),
            NockchainDataRequest::EldersById(id, peer, _) => {
                Self::Elders(PeerHash::from_base58(&id)?, peer)
            }
            NockchainDataRequest::RawTransactionById(id, _) => {
                Self::Transaction(PeerHash::from_base58(&id)?)
            }
            NockchainDataRequest::BlockWithTxsByHeight(height) => Self::Bundle(height),
            NockchainDataRequest::BlockRangeWithTxs { start_height, len } => Self::Range {
                start: start_height,
                length: len,
            },
        };
        let request = Self::from_proto(request.to_proto())?;
        preserve_local_message(message, &request.to_message())?;
        Ok(request)
    }

    fn to_message(&self) -> ByteBuf {
        let mut slab = NounSlab::new();
        let request_tag = make_tas(&mut slab, "request").as_noun();
        let fields = match self {
            Self::Block(height) | Self::Bundle(height) => {
                let kind = if matches!(self, Self::Block(_)) {
                    "block"
                } else {
                    "block-with-txs"
                };
                let kind = make_tas(&mut slab, kind).as_noun();
                let by_height = make_tas(&mut slab, "by-height").as_noun();
                let height = Atom::new(&mut slab, *height).as_noun();
                vec![request_tag, kind, by_height, height]
            }
            Self::Elders(id, peer) => {
                let block = make_tas(&mut slab, "block").as_noun();
                let elders = make_tas(&mut slab, "elders").as_noun();
                let id = id.to_noun(&mut slab);
                let peer = make_tas(&mut slab, &peer.to_base58()).as_noun();
                vec![request_tag, block, elders, id, peer]
            }
            Self::Transaction(id) => {
                let tx = make_tas(&mut slab, "raw-tx").as_noun();
                let by_id = make_tas(&mut slab, "by-id").as_noun();
                let id = id.to_noun(&mut slab);
                vec![request_tag, tx, by_id, id]
            }
            Self::Range { start, length } => {
                let bundle = make_tas(&mut slab, "block-with-txs").as_noun();
                let by_range = make_tas(&mut slab, "by-range").as_noun();
                let start = Atom::new(&mut slab, *start).as_noun();
                vec![request_tag, bundle, by_range, start, D(u64::from(*length))]
            }
        };
        let noun = T(&mut slab, &fields);
        slab.set_root(noun);
        jam(&slab)
    }
}

pub(crate) fn encode_request(request: &NockchainRequest) -> io::Result<Vec<u8>> {
    request.validate().map_err(external_error)?;
    let kind = match request {
        NockchainRequest::BatchRequest { pow, nonce, items } => {
            count(items.len(), MAX_BATCH_ITEMS, "batch request")?;
            let items = items
                .iter()
                .map(|item| {
                    Ok(pb::RequestItem {
                        item_id: Some(item.item_id),
                        request: Some(DataRequest::from_local_message(&item.message)?.to_proto()),
                    })
                })
                .collect::<io::Result<_>>()?;
            pb::peer_request::Kind::Batch(pb::BatchRequest {
                proof: Some(pow.to_vec()),
                nonce: Some(*nonce),
                items,
            })
        }
        NockchainRequest::AuthenticatedGossip {
            pow,
            nonce,
            message,
        } => {
            let fact = Fact::from_local_message(message)?;
            if matches!(fact, Fact::Elders(_)) {
                return Err(invalid("elders are response-only"));
            }
            pb::peer_request::Kind::Gossip(pb::AuthenticatedGossip {
                proof: Some(pow.to_vec()),
                nonce: Some(*nonce),
                fact: Some(fact.to_proto()),
            })
        }
    };
    Ok(pb::PeerRequest { kind: Some(kind) }.encode_to_vec())
}

fn proof(value: Option<Vec<u8>>) -> io::Result<equix::SolutionByteArray> {
    required(value, "proof")?
        .try_into()
        .map_err(|_| invalid("invalid proof length"))
}

pub(crate) fn decode_request(bytes: &[u8]) -> io::Result<NockchainRequest> {
    let wire = pb::PeerRequest::decode(bytes).map_err(external_error)?;
    let request = match required(wire.kind, "peer_request.kind")? {
        pb::peer_request::Kind::Batch(batch) => {
            let pow = proof(batch.proof)?;
            let nonce = required(batch.nonce, "batch.nonce")?;
            count(batch.items.len(), MAX_BATCH_ITEMS, "batch request")?;
            let mut ids = BTreeSet::new();
            let items = batch
                .items
                .into_iter()
                .map(|item| {
                    let item_id = required(item.item_id, "request_item.item_id")?;
                    if !ids.insert(item_id) {
                        return Err(invalid("duplicate request item ID"));
                    }
                    let request =
                        DataRequest::from_proto(required(item.request, "request_item.request")?)?;
                    Ok(BatchRequestItem {
                        item_id,
                        message: request.to_message(),
                    })
                })
                .collect::<io::Result<_>>()?;
            NockchainRequest::BatchRequest { pow, nonce, items }
        }
        pb::peer_request::Kind::Gossip(gossip) => {
            let pow = proof(gossip.proof)?;
            let nonce = required(gossip.nonce, "gossip.nonce")?;
            let fact = Fact::from_proto(required(gossip.fact, "gossip.fact")?)?;
            if matches!(fact, Fact::Elders(_)) {
                return Err(invalid("elders are response-only"));
            }
            NockchainRequest::AuthenticatedGossip {
                pow,
                nonce,
                message: fact.to_message()?,
            }
        }
    };
    request.validate().map_err(external_error)?;
    Ok(request)
}

#[derive(Clone, Debug)]
struct Bundle {
    block: PeerPage,
    transactions: Vec<Transaction>,
    unincluded: Vec<PeerHash>,
}

impl Bundle {
    fn from_proto(wire: pb::BlockBundle) -> io::Result<Self> {
        count(
            wire.transactions
                .len()
                .saturating_add(wire.unincluded_transaction_ids.len()),
            MAX_BUNDLE_ITEMS,
            "bundle transactions",
        )?;
        let bundle = Self {
            block: PeerPage::from_proto(required(wire.block, "bundle.block")?)?,
            transactions: wire
                .transactions
                .into_iter()
                .map(Transaction::from_proto)
                .collect::<io::Result<_>>()?,
            unincluded: wire
                .unincluded_transaction_ids
                .into_iter()
                .map(PeerHash::from_proto)
                .collect::<io::Result<_>>()?,
        };
        let expected = bundle
            .block
            .to_proto()
            .tx_ids
            .into_iter()
            .map(|id| PeerHash::from_proto(id)?.to_base58())
            .collect::<io::Result<BTreeSet<_>>>()?;
        let mut actual = BTreeSet::new();
        for tx in &bundle.transactions {
            if !actual.insert(transaction_id(tx)?) {
                return Err(invalid("duplicate bundled transaction"));
            }
        }
        for id in &bundle.unincluded {
            if !actual.insert(id.to_base58()?) {
                return Err(invalid("duplicate or overlapping unincluded transaction"));
            }
        }
        if actual != expected {
            return Err(invalid(
                "bundle transactions do not cover block transaction IDs",
            ));
        }
        Ok(bundle)
    }

    fn to_proto(&self) -> pb::BlockBundle {
        pb::BlockBundle {
            block: Some(self.block.to_proto()),
            transactions: self
                .transactions
                .iter()
                .map(Transaction::to_proto)
                .collect(),
            unincluded_transaction_ids: self.unincluded.iter().map(PeerHash::to_proto).collect(),
        }
    }

    fn from_internal(block: &BundledBlockWithTxs) -> io::Result<Self> {
        let Fact::Block(page) = Fact::from_local_message(&block.block_message)? else {
            return Err(invalid("bundle requires block fact"));
        };
        if block_id(&page)? != block.block_id {
            return Err(invalid("bundle block ID mismatch"));
        }
        let transactions = block
            .tx_envelopes
            .iter()
            .map(|tx| {
                let Fact::Transaction(value) = Fact::from_local_message(&tx.message)? else {
                    return Err(invalid("bundle requires transaction fact"));
                };
                if transaction_id(&value)? != tx.tx_id {
                    return Err(invalid("bundled transaction ID mismatch"));
                }
                Ok(value.to_proto())
            })
            .collect::<io::Result<_>>()?;
        Self::from_proto(pb::BlockBundle {
            block: Some(page.to_proto()),
            transactions,
            unincluded_transaction_ids: block
                .unincluded_tx_ids
                .iter()
                .map(|id| Ok(PeerHash::from_base58(id)?.to_proto()))
                .collect::<io::Result<_>>()?,
        })
    }

    fn into_internal(self) -> io::Result<BundledBlockWithTxs> {
        let id = block_id(&self.block)?;
        let block_message = Fact::Block(self.block).to_message()?;
        let tx_envelopes = self
            .transactions
            .into_iter()
            .map(|tx| {
                Ok(BundledTxEnvelope {
                    tx_id: transaction_id(&tx)?,
                    message: Fact::Transaction(tx).to_message()?,
                })
            })
            .collect::<io::Result<_>>()?;
        let unincluded_tx_ids = self
            .unincluded
            .iter()
            .map(PeerHash::to_base58)
            .collect::<io::Result<_>>()?;
        Ok(BundledBlockWithTxs {
            block_id: id,
            block_message,
            tx_envelopes,
            unincluded_tx_ids,
        })
    }
}

fn envelope_to_proto(envelope: &ResponseEnvelope) -> io::Result<pb::ResponseEnvelope> {
    use pb::response_envelope::Kind;
    envelope.validate().map_err(external_error)?;
    let kind = match envelope.kind {
        EnvelopeKind::HeardBlock | EnvelopeKind::HeardTx | EnvelopeKind::HeardElders => {
            match (envelope.kind, Fact::from_local_message(&envelope.message)?) {
                (EnvelopeKind::HeardBlock, Fact::Block(page)) => {
                    if Some(block_id(&page)?) != envelope.block_id {
                        return Err(invalid("envelope block ID mismatch"));
                    }
                    Kind::Block(page.to_proto())
                }
                (EnvelopeKind::HeardTx, Fact::Transaction(tx)) => {
                    if Some(transaction_id(&tx)?) != envelope.tx_id {
                        return Err(invalid("envelope transaction ID mismatch"));
                    }
                    Kind::Transaction(tx.to_proto())
                }
                (EnvelopeKind::HeardElders, Fact::Elders(elders)) => {
                    Kind::Elders(elders.to_proto())
                }
                _ => return Err(invalid("envelope kind does not match payload")),
            }
        }
        EnvelopeKind::HeardBlockWithTxs => {
            let block = BundledBlockWithTxs {
                block_id: required(envelope.block_id.clone(), "bundle.block_id")?,
                block_message: envelope.message.clone(),
                tx_envelopes: required(envelope.tx_envelopes.clone(), "bundle.transactions")?,
                unincluded_tx_ids: required(
                    envelope.unincluded_tx_ids.clone(),
                    "bundle.unincluded",
                )?,
            };
            Kind::BlockBundle(Bundle::from_internal(&block)?.to_proto())
        }
        EnvelopeKind::HeardBlockRangeWithTxs => {
            let blocks = required(envelope.range_blocks.as_ref(), "range.blocks")?;
            count(blocks.len(), MAX_RANGE_BLOCKS, "range blocks")?;
            Kind::BlockRange(pb::BlockRange {
                blocks: blocks
                    .iter()
                    .map(|block| Ok(Bundle::from_internal(block)?.to_proto()))
                    .collect::<io::Result<_>>()?,
            })
        }
    };
    Ok(pb::ResponseEnvelope { kind: Some(kind) })
}

fn envelope_from_proto(wire: pb::ResponseEnvelope) -> io::Result<ResponseEnvelope> {
    use pb::response_envelope::Kind;
    let envelope = match required(wire.kind, "response_envelope.kind")? {
        Kind::Block(page) => {
            let page = PeerPage::from_proto(page)?;
            ResponseEnvelope::heard_block(block_id(&page)?, Fact::Block(page).to_message()?)
        }
        Kind::Transaction(tx) => {
            let tx = Transaction::from_proto(tx)?;
            ResponseEnvelope::heard_tx(transaction_id(&tx)?, Fact::Transaction(tx).to_message()?)
        }
        Kind::Elders(elders) => {
            ResponseEnvelope::heard_elders(Fact::Elders(Elders::from_proto(elders)?).to_message()?)
        }
        Kind::BlockBundle(bundle) => {
            let bundle = Bundle::from_proto(bundle)?.into_internal()?;
            ResponseEnvelope::heard_block_with_txs(
                bundle.block_id, bundle.block_message, bundle.tx_envelopes,
                bundle.unincluded_tx_ids,
            )
        }
        Kind::BlockRange(range) => {
            count(range.blocks.len(), MAX_RANGE_BLOCKS, "range blocks")?;
            let blocks = range
                .blocks
                .into_iter()
                .map(|block| Bundle::from_proto(block)?.into_internal())
                .collect::<io::Result<_>>()?;
            ResponseEnvelope::heard_block_range_with_txs(blocks)
        }
    };
    envelope.validate().map_err(external_error)?;
    Ok(envelope)
}

pub(crate) fn response_envelope_encoded_len(envelope: &ResponseEnvelope) -> io::Result<usize> {
    Ok(envelope_to_proto(envelope)?.encoded_len())
}

pub(crate) fn encode_response(response: &NockchainResponse) -> io::Result<Vec<u8>> {
    response.validate().map_err(external_error)?;
    let kind = match response {
        NockchainResponse::Result { .. } => {
            return Err(invalid("internal execution results are not wire responses"))
        }
        NockchainResponse::Ack { acked } => pb::peer_response::Kind::Acknowledged(*acked),
        NockchainResponse::BatchResult { results } => {
            count(results.len(), MAX_BATCH_ITEMS, "batch result")?;
            let items = results
                .iter()
                .map(|result| {
                    use pb::result_item::Outcome;
                    let outcome = match result.status {
                        BatchResultStatus::Result => Outcome::Result(envelope_to_proto(required(
                            result.envelope.as_ref(),
                            "result.envelope",
                        )?)?),
                        BatchResultStatus::Ack => Outcome::Acknowledged(pb::Empty {}),
                        BatchResultStatus::NotFound => Outcome::NotFound(pb::Empty {}),
                        BatchResultStatus::Error => {
                            let error = match required(result.error, "result.error")? {
                                BatchErrorClass::Decode => pb::ErrorClass::Decode,
                                BatchErrorClass::Backpressure => pb::ErrorClass::Backpressure,
                                BatchErrorClass::TooLarge => pb::ErrorClass::TooLarge,
                                BatchErrorClass::InvalidPow => pb::ErrorClass::InvalidPow,
                                BatchErrorClass::Internal => pb::ErrorClass::Internal,
                            };
                            Outcome::Error(pb::ErrorResult {
                                classification: Some(error as i32),
                            })
                        }
                    };
                    Ok(pb::ResultItem {
                        item_id: Some(result.item_id),
                        outcome: Some(outcome),
                    })
                })
                .collect::<io::Result<_>>()?;
            pb::peer_response::Kind::Batch(pb::BatchResult { items })
        }
    };
    Ok(pb::PeerResponse { kind: Some(kind) }.encode_to_vec())
}

pub(crate) fn decode_response(bytes: &[u8]) -> io::Result<NockchainResponse> {
    let wire = pb::PeerResponse::decode(bytes).map_err(external_error)?;
    let response = match required(wire.kind, "peer_response.kind")? {
        pb::peer_response::Kind::Acknowledged(acked) => NockchainResponse::Ack { acked },
        pb::peer_response::Kind::Batch(batch) => {
            count(batch.items.len(), MAX_BATCH_ITEMS, "batch result")?;
            let mut ids = BTreeSet::new();
            let results = batch
                .items
                .into_iter()
                .map(|item| {
                    use pb::result_item::Outcome;
                    let item_id = required(item.item_id, "result.item_id")?;
                    if !ids.insert(item_id) {
                        return Err(invalid("duplicate response item ID"));
                    }
                    let (status, error, envelope) = match required(item.outcome, "result.outcome")?
                    {
                        Outcome::Result(envelope) => (
                            BatchResultStatus::Result,
                            None,
                            Some(envelope_from_proto(envelope)?),
                        ),
                        Outcome::Acknowledged(_) => (BatchResultStatus::Ack, None, None),
                        Outcome::NotFound(_) => (BatchResultStatus::NotFound, None, None),
                        Outcome::Error(error) => {
                            let error = match pb::ErrorClass::try_from(required(
                                error.classification, "error.classification",
                            )?)
                            .map_err(external_error)?
                            {
                                pb::ErrorClass::Decode => BatchErrorClass::Decode,
                                pb::ErrorClass::Backpressure => BatchErrorClass::Backpressure,
                                pb::ErrorClass::TooLarge => BatchErrorClass::TooLarge,
                                pb::ErrorClass::InvalidPow => BatchErrorClass::InvalidPow,
                                pb::ErrorClass::Internal => BatchErrorClass::Internal,
                                pb::ErrorClass::Unspecified => {
                                    return Err(invalid("unspecified error classification"))
                                }
                            };
                            (BatchResultStatus::Error, Some(error), None)
                        }
                    };
                    Ok(BatchResultItem {
                        item_id,
                        status,
                        error,
                        envelope,
                    })
                })
                .collect::<io::Result<_>>()?;
            NockchainResponse::BatchResult { results }
        }
    };
    response.validate().map_err(external_error)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(value: u64) -> pb::Hash {
        PeerHash::new([value, 0, 0, 0, 0]).unwrap().to_proto()
    }

    fn page(height: u64, tx_ids: Vec<pb::Hash>) -> PeerPage {
        PeerPage::from_proto(pb::Page {
            version: Some(1),
            digest: Some(hash(height)),
            pow: None,
            parent: Some(hash(height.saturating_sub(1))),
            tx_ids,
            coinbase: Some(pb::PageCoinbase {
                kind: Some(pb::page_coinbase::Kind::V1(pb::PageV1Coinbase {
                    entries: vec![],
                })),
            }),
            timestamp: Some(pb::Atom {
                little_endian: Some(vec![]),
            }),
            epoch_counter: Some(pb::Atom {
                little_endian: Some(vec![]),
            }),
            target: Some(pb::PageBigNum { limbs: vec![1, 0] }),
            accumulated_work: Some(pb::PageBigNum { limbs: vec![2, 0] }),
            height: Some(height),
            message: vec![0, (1u64 << 40) + 1],
        })
        .unwrap()
    }

    fn transaction() -> Transaction {
        let slab = local_slab(include_bytes!(
            "../../../nockchain-types/jams/v1/raw-tx.jam"
        ))
        .unwrap();
        Transaction::from_noun(unsafe { slab.root() }.in_space(&slab.noun_space())).unwrap()
    }

    #[test]
    fn every_request_variant_preserves_its_internal_message() {
        let id = PeerHash::new([1, 2, 3, 4, 5]).unwrap();
        let requests = [
            DataRequest::Block(0),
            DataRequest::Block(u64::MAX),
            DataRequest::Elders(id.clone(), PeerId::random()),
            DataRequest::Transaction(id),
            DataRequest::Bundle(100),
            DataRequest::Range {
                start: 0,
                length: 255,
            },
            DataRequest::Range {
                start: u64::MAX,
                length: 1,
            },
        ];
        let request = NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: requests
                .into_iter()
                .enumerate()
                .map(|(index, request)| BatchRequestItem {
                    item_id: index as u32,
                    message: request.to_message(),
                })
                .collect(),
        };
        assert_eq!(
            decode_request(&encode_request(&request).unwrap()).unwrap(),
            request
        );
    }

    #[test]
    fn typed_request_roundtrip_preserves_sender_bound_pow() {
        let sender = PeerId::random();
        let receiver = PeerId::random();
        let mut builder = equix::EquiXBuilder::new();
        let request = NockchainRequest::new_batch_request(
            &mut builder,
            &sender,
            &receiver,
            vec![
                BatchRequestItem {
                    item_id: 0,
                    message: DataRequest::Block(0).to_message(),
                },
                BatchRequestItem {
                    item_id: u32::MAX,
                    message: DataRequest::Range {
                        start: 10,
                        length: 3,
                    }
                    .to_message(),
                },
            ],
        )
        .unwrap();
        let mut wire = encode_request(&request).unwrap();
        // An unknown optional protobuf field does not become part of the PoW
        // commitment. The completed checked request has the same meaning.
        wire.extend_from_slice(&[0xf8, 0x07, 1]);
        let decoded = decode_request(&wire).unwrap();
        assert_eq!(decoded, request);
        decoded
            .verify_pow(&mut builder, &receiver, &sender)
            .unwrap();
        assert!(decoded
            .verify_pow(&mut builder, &sender, &receiver)
            .is_err());

        let gossip = NockchainRequest::authenticated_gossip_from_message(
            &mut builder,
            &sender,
            &receiver,
            Fact::Transaction(transaction()).to_message().unwrap(),
        )
        .unwrap();
        let decoded = decode_request(&encode_request(&gossip).unwrap()).unwrap();
        assert_eq!(decoded, gossip);
        decoded
            .verify_pow(&mut builder, &receiver, &sender)
            .unwrap();
    }

    #[test]
    fn response_variants_and_bundles_roundtrip() {
        let tx = transaction();
        let tx_id = PeerHash::from_base58(&transaction_id(&tx).unwrap()).unwrap();
        let bundle = Bundle::from_proto(pb::BlockBundle {
            block: Some(page(1, vec![tx_id.to_proto()]).to_proto()),
            transactions: vec![tx.to_proto()],
            unincluded_transaction_ids: vec![],
        })
        .unwrap()
        .into_internal()
        .unwrap();
        let block = page(5, vec![]);
        let envelopes = [
            ResponseEnvelope::heard_block(
                block_id(&block).unwrap(),
                Fact::Block(block).to_message().unwrap(),
            ),
            ResponseEnvelope::heard_tx(
                transaction_id(&tx).unwrap(),
                Fact::Transaction(tx).to_message().unwrap(),
            ),
            ResponseEnvelope::heard_elders(
                Fact::Elders(Elders {
                    oldest: 0,
                    ids: vec![],
                })
                .to_message()
                .unwrap(),
            ),
            ResponseEnvelope::heard_block_with_txs(
                bundle.block_id.clone(),
                &bundle.block_message,
                bundle.tx_envelopes.clone(),
                vec![],
            ),
            ResponseEnvelope::heard_block_range_with_txs(vec![bundle]),
        ];
        let mut results: Vec<_> = envelopes
            .into_iter()
            .enumerate()
            .map(|(index, envelope)| BatchResultItem {
                item_id: index as u32,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(envelope),
            })
            .collect();
        results.extend([
            BatchResultItem {
                item_id: 5,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            },
            BatchResultItem {
                item_id: 6,
                status: BatchResultStatus::NotFound,
                error: None,
                envelope: None,
            },
            BatchResultItem {
                item_id: 7,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
        ]);
        let response = NockchainResponse::BatchResult { results };
        assert_eq!(
            decode_response(&encode_response(&response).unwrap()).unwrap(),
            response
        );
        for acked in [true, false] {
            let response = NockchainResponse::Ack { acked };
            assert_eq!(
                decode_response(&encode_response(&response).unwrap()).unwrap(),
                response
            );
        }
    }

    #[test]
    fn missing_presence_unknown_operations_and_duplicate_ids_fail() {
        assert!(decode_request(&[]).is_err());
        assert!(decode_response(&[]).is_err());
        let item = pb::RequestItem {
            item_id: Some(0),
            request: Some(DataRequest::Block(0).to_proto()),
        };
        let batch = pb::BatchRequest {
            proof: Some(vec![0; 16]),
            nonce: Some(0),
            items: vec![item.clone()],
        };
        let decode = |batch| {
            decode_request(
                &pb::PeerRequest {
                    kind: Some(pb::peer_request::Kind::Batch(batch)),
                }
                .encode_to_vec(),
            )
        };
        assert!(decode(batch.clone()).is_ok());
        let mut changed = batch.clone();
        changed.nonce = None;
        assert!(decode(changed).is_err());
        let mut changed = batch.clone();
        changed.items[0].item_id = None;
        assert!(decode(changed).is_err());
        let mut changed = batch.clone();
        changed.items.push(item);
        assert!(decode(changed).is_err());
        let mut changed = batch;
        changed.items[0].request = Some(pb::DataRequest::default());
        assert!(decode(changed).is_err());
        for classification in [None, Some(0), Some(99)] {
            let response = pb::PeerResponse {
                kind: Some(pb::peer_response::Kind::Batch(pb::BatchResult {
                    items: vec![pb::ResultItem {
                        item_id: Some(0),
                        outcome: Some(pb::result_item::Outcome::Error(pb::ErrorResult {
                            classification,
                        })),
                    }],
                })),
            };
            assert!(decode_response(&response.encode_to_vec()).is_err());
        }
    }

    #[test]
    fn bundle_coverage_and_local_identity_are_checked() {
        let block = page(1, vec![hash(10)]);
        let mut bundle = pb::BlockBundle {
            block: Some(block.to_proto()),
            transactions: vec![],
            unincluded_transaction_ids: vec![hash(10)],
        };
        assert!(Bundle::from_proto(bundle.clone()).is_ok());
        bundle.unincluded_transaction_ids.push(hash(10));
        assert!(Bundle::from_proto(bundle.clone()).is_err());
        bundle.unincluded_transaction_ids.clear();
        assert!(Bundle::from_proto(bundle).is_err());
        let mismatched = ResponseEnvelope::heard_block(
            PeerHash::new([2, 0, 0, 0, 0]).unwrap().to_base58().unwrap(),
            Fact::Block(block).to_message().unwrap(),
        );
        assert!(envelope_to_proto(&mismatched).is_err());
    }

    #[test]
    fn invalid_ranges_and_noncanonical_local_messages_are_rejected() {
        for (start, length) in [(0, 0), (0, 256), (u64::MAX, 2)] {
            assert!(DataRequest::from_proto(pb::DataRequest {
                kind: Some(pb::data_request::Kind::BlockRange(pb::BlockRangeRequest {
                    start_height: Some(start),
                    length: Some(length)
                }))
            })
            .is_err());
        }
        let mut local = DataRequest::Block(1).to_message().into_vec();
        local.push(0);
        assert!(DataRequest::from_local_message(&local).is_err());
        let internal = NockchainResponse::Result {
            message: ByteBuf::new(),
        };
        assert!(encode_response(&internal).is_err());
    }
}
