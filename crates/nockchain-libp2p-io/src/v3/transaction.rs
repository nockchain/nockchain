//! Lossless transaction boundary for peer protocol v3.
//!
//! Protobuf messages remain untrusted until `Transaction::from_proto` succeeds.
//! The checked wrapper keeps its fields private. The bridge constructs the exact
//! consensus noun shape using named message types, and never interprets jam.
use std::io;

use nockapp::noun::slab::NounSlab;
use nockchain_math::belt::PRIME;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::{zmap, zset};
use nockvm::noun::{Noun, NounAllocator, NounHandle, D, T};
use noun_serde::NounEncode;

use super::common::{list, tree_entries, tuple, PeerHash, PeerNoun, MAX_NODES};
use super::pb;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// A complete transaction whose wire values passed the peer-domain checks.
/// Consensus authorization, signatures, and transaction identity are still
/// checked by the kernel; transport decoding does not establish their validity.
#[derive(Clone, Debug)]
pub struct Transaction {
    wire: pb::Transaction,
}

impl Transaction {
    pub fn from_proto(wire: pb::Transaction) -> io::Result<Self> {
        let mut slab: NounSlab = NounSlab::new();
        wire.encode_noun(&mut slab)?;
        Ok(Self { wire })
    }

    pub fn to_proto(&self) -> pb::Transaction {
        self.wire.clone()
    }

    pub fn from_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        Self::from_proto(pb::Transaction::decode_noun(noun)?)
    }

    pub fn to_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        self.wire.encode_noun(slab)
    }
}

trait NounCodec: Sized {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun>;
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self>;
}

impl NounCodec for u64 {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        if *self >= PRIME {
            return Err(invalid("transaction scalar is outside the base field"));
        }
        Ok(self.to_noun(slab))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let value = noun
            .as_atom()
            .map_err(|_| invalid("expected transaction atom"))?
            .as_u64()
            .map_err(|_| invalid("transaction atom exceeds u64"))?;
        if value >= PRIME {
            return Err(invalid("transaction atom is outside the base field"));
        }
        Ok(value)
    }
}

impl NounCodec for bool {
    fn encode_noun(&self, _slab: &mut NounSlab) -> io::Result<Noun> {
        Ok(D(u64::from(!*self)))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        match u64::decode_noun(noun)? {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(invalid("expected Hoon boolean")),
        }
    }
}

impl NounCodec for pb::Hash {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        Ok(PeerHash::from_proto(self.clone())?.to_noun(slab))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        Ok(PeerHash::from_noun(noun)?.to_proto())
    }
}

impl NounCodec for pb::NounValue {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let noun = PeerNoun::from_proto(self.clone())?;
        noun.validate_based()?;
        noun.to_noun(slab)
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let noun = PeerNoun::from_noun(noun)?;
        noun.validate_based()?;
        Ok(noun.to_proto())
    }
}

fn required<T: NounCodec>(field: &Option<T>, slab: &mut NounSlab) -> io::Result<Noun> {
    field
        .as_ref()
        .ok_or_else(|| invalid("missing required transaction field"))?
        .encode_noun(slab)
}

fn optional<T: NounCodec>(field: &Option<T>, slab: &mut NounSlab) -> io::Result<Noun> {
    match field {
        None => Ok(D(0)),
        Some(value) => {
            let noun = value.encode_noun(slab)?;
            Ok(T(slab, &[D(0), noun]))
        }
    }
}

fn decode_optional<T: NounCodec>(noun: NounHandle<'_>) -> io::Result<Option<T>> {
    if noun.is_atom() {
        zero(noun)?;
        return Ok(None);
    }
    let parts = tuple(noun, 2)?;
    zero(parts[0])?;
    Ok(Some(T::decode_noun(parts[1])?))
}

fn zero(noun: NounHandle<'_>) -> io::Result<()> {
    if u64::decode_noun(noun)? != 0 {
        return Err(invalid("expected null transaction field"));
    }
    Ok(())
}

macro_rules! decode_field {
    (required, $noun:expr) => {
        Some(NounCodec::decode_noun($noun)?)
    };
    (optional, $noun:expr) => {
        decode_optional($noun)?
    };
}

macro_rules! tuple_codec {
    ($name:ident, $count:expr; $( $field:ident : $kind:ident ),+ $(,)?) => {
        impl NounCodec for pb::$name {
            fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
                let fields = [$( $kind(&self.$field, slab)? ),+];
                Ok(T(slab, &fields))
            }
            fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
                let parts = tuple(noun, $count)?;
                let mut parts = parts.into_iter();
                Ok(Self { $( $field: decode_field!($kind, parts.next().expect("tuple arity is checked")) ),+ })
            }
        }
    }
}

tuple_codec!(TxSource, 2; hash: required, is_coinbase: required);
tuple_codec!(TxTimeRange, 2; min: optional, max: optional);
tuple_codec!(TxTimeIntent, 2; absolute: required, relative: required);
tuple_codec!(TxLegacy, 4; id: required, inputs: required, timelock_range: required, total_fees: required);
tuple_codec!(TxLegacyInput, 2; note: required, spend: required);
tuple_codec!(TxLegacySpend, 3; signature: optional, seeds: required, fee: required);
tuple_codec!(TxLegacySeed, 5; output_source: optional, recipient: required, timelock_intent: optional, gift: required, parent_hash: required);
tuple_codec!(TxSpend0, 3; signature: required, seeds: required, fee: required);
tuple_codec!(TxSpend1, 3; witness: required, seeds: required, fee: required);
tuple_codec!(TxSeed, 5; output_source: optional, lock_root: required, note_data: required, gift: required, parent_hash: required);
tuple_codec!(TxPkhSignature, 2; pubkey: required, signature: required);
tuple_codec!(TxLockMerkleBody, 3; spend_condition: required, axis: required, proof: required);
tuple_codec!(TxTim, 2; relative: required, absolute: required);

fn encode_array(values: &[u64], count: usize, slab: &mut NounSlab) -> io::Result<Noun> {
    if values.len() != count {
        return Err(invalid(format!("expected {count} field limbs")));
    }
    let values = values
        .iter()
        .map(|v| v.encode_noun(slab))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(T(slab, &values))
}

fn decode_array(noun: NounHandle<'_>, count: usize) -> io::Result<Vec<u64>> {
    tuple(noun, count)?
        .into_iter()
        .map(u64::decode_noun)
        .collect()
}

impl NounCodec for pb::TxPubkey {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let x = encode_array(&self.x, 6, slab)?;
        let y = encode_array(&self.y, 6, slab)?;
        let infinity = required(&self.infinity, slab)?;
        Ok(T(slab, &[x, y, infinity]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 3)?;
        Ok(Self {
            x: decode_array(parts[0], 6)?,
            y: decode_array(parts[1], 6)?,
            infinity: Some(bool::decode_noun(parts[2])?),
        })
    }
}

impl NounCodec for pb::TxSchnorrSignature {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let challenge = encode_array(&self.challenge, 8, slab)?;
        let signature = encode_array(&self.signature, 8, slab)?;
        Ok(T(slab, &[challenge, signature]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 2)?;
        Ok(Self {
            challenge: decode_array(parts[0], 8)?,
            signature: decode_array(parts[1], 8)?,
        })
    }
}

impl NounCodec for pb::TxName {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let first = required(&self.first, slab)?;
        let last = required(&self.last, slab)?;
        Ok(T(slab, &[first, last, D(0)]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 3)?;
        zero(parts[2])?;
        Ok(Self {
            first: Some(pb::Hash::decode_noun(parts[0])?),
            last: Some(pb::Hash::decode_noun(parts[1])?),
        })
    }
}

// All treap keys and set entries are based nouns. A flat preorder key
// preserves complete structure, independently of noun sharing, so insertion
// cannot silently deduplicate a repeated key or entry. Flat keys also keep
// comparisons and destruction independent of the depth of opaque note data.
fn unique(value: Noun, slab: &NounSlab, seen: &mut Vec<Vec<Option<u64>>>) -> io::Result<()> {
    let space = slab.noun_space();
    let mut pending = vec![value.in_space(&space)];
    let mut key = Vec::new();
    while let Some(noun) = pending.pop() {
        if key.len() >= MAX_NODES {
            return Err(invalid("transaction key exceeds node limit"));
        }
        if noun.is_atom() {
            key.push(Some(u64::decode_noun(noun)?));
        } else {
            let cell = noun.as_cell().map_err(|_| invalid("expected key cell"))?;
            key.push(None);
            pending.push(cell.tail());
            pending.push(cell.head());
        }
    }
    if seen.contains(&key) {
        return Err(invalid("duplicate transaction map key or set entry"));
    }
    seen.push(key);
    Ok(())
}

macro_rules! map_codec {
    ($name:ident, $entry:ident) => {
        impl NounCodec for pb::$name {
            fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
                let mut map = D(0);
                let mut seen = Vec::new();
                for entry in &self.entries {
                    let mut key = required(&entry.key, slab)?;
                    let mut value = required(&entry.value, slab)?;
                    unique(key, slab, &mut seen)?;
                    map = zmap::z_map_put(slab, &map, &mut key, &mut value, &DefaultTipHasher)
                        .map_err(|e| {
                            invalid(format!("transaction map construction failed: {e:?}"))
                        })?;
                }
                Ok(map)
            }
            fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
                let entries = tree_entries(noun)?
                    .into_iter()
                    .map(|entry| {
                        let parts = tuple(entry, 2)?;
                        Ok(pb::$entry {
                            key: Some(NounCodec::decode_noun(parts[0])?),
                            value: Some(NounCodec::decode_noun(parts[1])?),
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                Ok(Self { entries })
            }
        }
    };
}
map_codec!(TxSignatures, TxSignatureEntry);
map_codec!(TxInputs, TxInputEntry);
map_codec!(TxSpends, TxSpendEntry);
map_codec!(TxNoteData, TxNoteDataEntry);
map_codec!(TxPkhSignatures, TxPkhSignatureEntry);
map_codec!(TxPreimages, TxPreimage);

fn encode_set<V: NounCodec>(values: &[V], slab: &mut NounSlab) -> io::Result<Noun> {
    let mut set = D(0);
    let mut seen = Vec::new();
    for value in values {
        let mut noun = value.encode_noun(slab)?;
        unique(noun, slab, &mut seen)?;
        set = zset::z_set_put(slab, &set, &mut noun, &DefaultTipHasher)
            .map_err(|e| invalid(format!("transaction set construction failed: {e:?}")))?;
    }
    Ok(set)
}

fn decode_set<V: NounCodec>(noun: NounHandle<'_>) -> io::Result<Vec<V>> {
    tree_entries(noun)?
        .into_iter()
        .map(V::decode_noun)
        .collect()
}

macro_rules! set_codec {
    ($name:ident, $field:ident) => {
        impl NounCodec for pb::$name {
            fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
                encode_set(&self.$field, slab)
            }
            fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
                Ok(Self {
                    $field: decode_set(noun)?,
                })
            }
        }
    };
}
set_codec!(TxLegacySeeds, entries);
set_codec!(TxSeeds, entries);
set_codec!(TxHax, hashes);

impl NounCodec for pb::TxLegacyLock {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let keys_required = required(&self.keys_required, slab)?;
        let pubkeys = encode_set(&self.pubkeys, slab)?;
        Ok(T(slab, &[keys_required, pubkeys]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 2)?;
        Ok(Self {
            keys_required: Some(u64::decode_noun(parts[0])?),
            pubkeys: decode_set(parts[1])?,
        })
    }
}

impl NounCodec for pb::TxPkh {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let threshold = required(&self.threshold, slab)?;
        let hashes = encode_set(&self.hashes, slab)?;
        Ok(T(slab, &[threshold, hashes]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 2)?;
        Ok(Self {
            threshold: Some(u64::decode_noun(parts[0])?),
            hashes: decode_set(parts[1])?,
        })
    }
}

fn encode_list<V: NounCodec>(values: &[V], slab: &mut NounSlab) -> io::Result<Noun> {
    let mut tail = D(0);
    for value in values.iter().rev() {
        let head = value.encode_noun(slab)?;
        tail = T(slab, &[head, tail]);
    }
    Ok(tail)
}

impl NounCodec for pb::TxSpendCondition {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        encode_list(&self.primitives, slab)
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        Ok(Self {
            primitives: list(noun)?
                .into_iter()
                .map(pb::TxLockPrimitive::decode_noun)
                .collect::<io::Result<_>>()?,
        })
    }
}

impl NounCodec for pb::TxMerkleProof {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let root = required(&self.root, slab)?;
        let path = encode_list(&self.path, slab)?;
        Ok(T(slab, &[root, path]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 2)?;
        Ok(Self {
            root: Some(pb::Hash::decode_noun(parts[0])?),
            path: list(parts[1])?
                .into_iter()
                .map(pb::Hash::decode_noun)
                .collect::<io::Result<_>>()?,
        })
    }
}

impl NounCodec for pb::TxLegacyNote {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let origin = required(&self.origin_page, slab)?;
        let timelock = optional(&self.timelock, slab)?;
        let head = T(slab, &[D(0), origin, timelock]);
        let name = required(&self.name, slab)?;
        let lock = required(&self.lock, slab)?;
        let source = required(&self.source, slab)?;
        let assets = required(&self.assets, slab)?;
        Ok(T(slab, &[head, name, lock, source, assets]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 5)?;
        let head = tuple(parts[0], 3)?;
        zero(head[0])?;
        Ok(Self {
            origin_page: Some(u64::decode_noun(head[1])?),
            timelock: decode_optional(head[2])?,
            name: Some(pb::TxName::decode_noun(parts[1])?),
            lock: Some(pb::TxLegacyLock::decode_noun(parts[2])?),
            source: Some(pb::TxSource::decode_noun(parts[3])?),
            assets: Some(u64::decode_noun(parts[4])?),
        })
    }
}

impl NounCodec for pb::TxWitness {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let lmp = required(&self.lock_merkle_proof, slab)?;
        let pkh = required(&self.pkh, slab)?;
        let hax = required(&self.hax, slab)?;
        Ok(T(slab, &[lmp, pkh, hax, D(0)]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 4)?;
        zero(parts[3])?;
        Ok(Self {
            lock_merkle_proof: Some(pb::TxLockMerkleProof::decode_noun(parts[0])?),
            pkh: Some(pb::TxPkhSignatures::decode_noun(parts[1])?),
            hax: Some(pb::TxPreimages::decode_noun(parts[2])?),
        })
    }
}

impl NounCodec for pb::TxSpend {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        use pb::tx_spend::Kind;
        let (tag, value) = match self
            .kind
            .as_ref()
            .ok_or_else(|| invalid("missing spend kind"))?
        {
            Kind::Legacy(value) => (0, value.encode_noun(slab)?),
            Kind::Witness(value) => (1, value.encode_noun(slab)?),
        };
        Ok(T(slab, &[D(tag), value]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        use pb::tx_spend::Kind;
        let parts = tuple(noun, 2)?;
        let kind = match u64::decode_noun(parts[0])? {
            0 => Kind::Legacy(pb::TxSpend0::decode_noun(parts[1])?),
            1 => Kind::Witness(pb::TxSpend1::decode_noun(parts[1])?),
            _ => return Err(invalid("unsupported spend kind")),
        };
        Ok(Self { kind: Some(kind) })
    }
}

impl NounCodec for pb::TxV1 {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let id = required(&self.id, slab)?;
        let spends = required(&self.spends, slab)?;
        Ok(T(slab, &[D(1), id, spends]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 3)?;
        if u64::decode_noun(parts[0])? != 1 {
            return Err(invalid("unsupported transaction version"));
        }
        Ok(Self {
            id: Some(pb::Hash::decode_noun(parts[1])?),
            spends: Some(pb::TxSpends::decode_noun(parts[2])?),
        })
    }
}

impl NounCodec for pb::Transaction {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        use pb::transaction::Version;
        match self
            .version
            .as_ref()
            .ok_or_else(|| invalid("missing transaction version"))?
        {
            Version::Legacy(value) => value.encode_noun(slab),
            Version::V1(value) => value.encode_noun(slab),
        }
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        use pb::transaction::Version;
        let cell = noun
            .as_cell()
            .map_err(|_| invalid("transaction must be a cell"))?;
        let version = if cell.head().is_atom() {
            Version::V1(pb::TxV1::decode_noun(noun)?)
        } else {
            Version::Legacy(pb::TxLegacy::decode_noun(noun)?)
        };
        Ok(Self {
            version: Some(version),
        })
    }
}

impl NounCodec for pb::TxLockMerkleProof {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        use pb::tx_lock_merkle_proof::Kind;
        match self
            .kind
            .as_ref()
            .ok_or_else(|| invalid("missing lock-merkle-proof kind"))?
        {
            Kind::Stub(value) => value.encode_noun(slab),
            Kind::Full(value) => {
                let body = value.encode_noun(slab)?;
                Ok(T(slab, &[D(nockvm_macros::tas!(b"full")), body]))
            }
        }
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        use pb::tx_lock_merkle_proof::Kind;
        let cell = noun
            .as_cell()
            .map_err(|_| invalid("lock-merkle-proof must be a cell"))?;
        let full = cell.head().as_atom().ok().and_then(|a| a.as_u64().ok())
            == Some(nockvm_macros::tas!(b"full"));
        let kind = if full {
            Kind::Full(pb::TxLockMerkleBody::decode_noun(cell.tail())?)
        } else {
            Kind::Stub(pb::TxLockMerkleBody::decode_noun(noun)?)
        };
        Ok(Self { kind: Some(kind) })
    }
}

impl NounCodec for pb::TxLockPrimitive {
    fn encode_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        use pb::tx_lock_primitive::Kind;
        let (tag, value) = match self
            .kind
            .as_ref()
            .ok_or_else(|| invalid("missing lock primitive kind"))?
        {
            Kind::Pkh(value) => (nockvm_macros::tas!(b"pkh"), value.encode_noun(slab)?),
            Kind::Tim(value) => (nockvm_macros::tas!(b"tim"), value.encode_noun(slab)?),
            Kind::Hax(value) => (nockvm_macros::tas!(b"hax"), value.encode_noun(slab)?),
            Kind::Burn(_) => (nockvm_macros::tas!(b"brn"), D(0)),
        };
        Ok(T(slab, &[D(tag), value]))
    }
    fn decode_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        use pb::tx_lock_primitive::Kind;
        let parts = tuple(noun, 2)?;
        let kind = match u64::decode_noun(parts[0])? {
            nockvm_macros::tas!(b"pkh") => Kind::Pkh(pb::TxPkh::decode_noun(parts[1])?),
            nockvm_macros::tas!(b"tim") => Kind::Tim(pb::TxTim::decode_noun(parts[1])?),
            nockvm_macros::tas!(b"hax") => Kind::Hax(pb::TxHax::decode_noun(parts[1])?),
            nockvm_macros::tas!(b"brn") => {
                zero(parts[1])?;
                Kind::Burn(pb::TxBurn {})
            }
            _ => return Err(invalid("unsupported lock primitive kind")),
        };
        Ok(Self { kind: Some(kind) })
    }
}

pub(crate) fn pubkey_from_noun(noun: NounHandle<'_>) -> io::Result<pb::TxPubkey> {
    pb::TxPubkey::decode_noun(noun)
}

pub(crate) fn pubkey_to_noun(value: &pb::TxPubkey, slab: &mut NounSlab) -> io::Result<Noun> {
    value.encode_noun(slab)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use nockchain_math::owned_based_noun::OwnedBasedNoun;
    use prost::Message;

    use super::*;

    fn fixture(jam: &'static [u8]) -> Transaction {
        let mut slab: NounSlab = NounSlab::new();
        let noun = slab
            .cue_into(Bytes::from_static(jam))
            .expect("public fixture decodes");
        Transaction::from_noun(noun.in_space(&slab.noun_space()))
            .expect("public fixture is a transaction")
    }

    fn assert_fixture_roundtrip(jam: &'static [u8]) {
        let mut original: NounSlab = NounSlab::new();
        let noun = original
            .cue_into(Bytes::from_static(jam))
            .expect("public fixture decodes");
        original.set_root(noun);
        let expected = original.jam();
        let domain = Transaction::from_noun(noun.in_space(&original.noun_space()))
            .expect("typed transaction");
        let encoded = domain.to_proto().encode_to_vec();
        let wire = pb::Transaction::decode(encoded.as_slice()).expect("protobuf roundtrip");
        let domain = Transaction::from_proto(wire).expect("checked protobuf transaction");
        let mut target: NounSlab = NounSlab::new();
        let actual = domain.to_noun(&mut target).expect("fallible noun bridge");
        target.set_root(actual);
        assert_eq!(
            target.jam(),
            expected,
            "consensus noun shape and values must be unchanged"
        );
    }

    #[test]
    fn public_consensus_v0_fixture_preserves_exact_noun() {
        assert_fixture_roundtrip(include_bytes!(
            "../../../nockchain-types/jams/v0/raw-tx.jam"
        ));
    }

    #[test]
    fn public_consensus_v1_fixture_preserves_exact_noun() {
        assert_fixture_roundtrip(include_bytes!(
            "../../../nockchain-types/jams/v1/raw-tx.jam"
        ));
    }

    #[test]
    fn missing_and_out_of_field_values_are_rejected() {
        let mut wire = fixture(include_bytes!(
            "../../../nockchain-types/jams/v1/raw-tx.jam"
        ))
        .to_proto();
        let Some(pb::transaction::Version::V1(tx)) = wire.version.as_mut() else {
            panic!("v1 fixture")
        };
        tx.id.as_mut().unwrap().limb0 = Some(PRIME);
        assert!(Transaction::from_proto(wire).is_err());
        assert!(Transaction::from_proto(pb::Transaction::default()).is_err());
        let mut wire = fixture(include_bytes!(
            "../../../nockchain-types/jams/v0/raw-tx.jam"
        ))
        .to_proto();
        let Some(pb::transaction::Version::Legacy(tx)) = wire.version.as_mut() else {
            panic!("legacy fixture")
        };
        tx.total_fees = None;
        assert!(Transaction::from_proto(wire).is_err());
    }

    #[test]
    fn duplicate_map_keys_and_set_entries_are_rejected() {
        let mut wire = fixture(include_bytes!(
            "../../../nockchain-types/jams/v1/raw-tx.jam"
        ))
        .to_proto();
        let Some(pb::transaction::Version::V1(tx)) = wire.version.as_mut() else {
            panic!("v1 fixture")
        };
        let spends = tx.spends.as_mut().unwrap();
        spends.entries.push(spends.entries[0].clone());
        assert!(Transaction::from_proto(wire).is_err());

        let mut wire = fixture(include_bytes!(
            "../../../nockchain-types/jams/v0/raw-tx.jam"
        ))
        .to_proto();
        let Some(pb::transaction::Version::Legacy(tx)) = wire.version.as_mut() else {
            panic!("legacy fixture")
        };
        let seeds = tx.inputs.as_mut().unwrap().entries[0]
            .value
            .as_mut()
            .unwrap()
            .spend
            .as_mut()
            .unwrap()
            .seeds
            .as_mut()
            .unwrap();
        seeds.entries.push(seeds.entries[0].clone());
        assert!(Transaction::from_proto(wire).is_err());
    }

    #[test]
    fn duplicate_set_values_are_detected_across_distinct_noun_dag_encodings() {
        let mut wire = fixture(include_bytes!(
            "../../../nockchain-types/jams/v1/raw-tx.jam"
        ))
        .to_proto();
        let Some(pb::transaction::Version::V1(tx)) = wire.version.as_mut() else {
            panic!("v1 fixture")
        };
        let spend = tx.spends.as_mut().unwrap().entries[0]
            .value
            .as_mut()
            .unwrap();
        let seeds = match spend.kind.as_mut().unwrap() {
            pb::tx_spend::Kind::Legacy(s) => s.seeds.as_mut().unwrap(),
            pb::tx_spend::Kind::Witness(s) => s.seeds.as_mut().unwrap(),
        };
        let atom = pb::NounNode {
            atom: Some(vec![1]),
            head: None,
            tail: None,
        };
        let mut a = seeds.entries[0].clone();
        a.note_data = Some(pb::TxNoteData {
            entries: vec![pb::TxNoteDataEntry {
                key: Some(0),
                value: Some(pb::NounValue {
                    nodes: vec![
                        atom.clone(),
                        pb::NounNode {
                            atom: None,
                            head: Some(0),
                            tail: Some(0),
                        },
                    ],
                }),
            }],
        });
        let mut b = a.clone();
        b.note_data.as_mut().unwrap().entries[0].value = Some(pb::NounValue {
            nodes: vec![
                atom.clone(),
                atom,
                pb::NounNode {
                    atom: None,
                    head: Some(0),
                    tail: Some(1),
                },
            ],
        });
        seeds.entries = vec![a, b];
        assert!(Transaction::from_proto(wire).is_err());
    }

    #[test]
    fn scalar_presence_and_null_sentinels_are_preserved() {
        let range = pb::TxTimeRange {
            min: Some(0),
            max: None,
        };
        let mut slab: NounSlab = NounSlab::new();
        let noun = range.encode_noun(&mut slab).unwrap();
        assert_eq!(
            pb::TxTimeRange::decode_noun(noun.in_space(&slab.noun_space())).unwrap(),
            range
        );

        let mut wire = fixture(include_bytes!(
            "../../../nockchain-types/jams/v0/raw-tx.jam"
        ))
        .to_proto();
        let Some(pb::transaction::Version::Legacy(tx)) = wire.version.as_mut() else {
            panic!("legacy fixture")
        };
        let spend = tx.inputs.as_mut().unwrap().entries[0]
            .value
            .as_mut()
            .unwrap()
            .spend
            .as_mut()
            .unwrap();
        spend.signature = Some(pb::TxSignatures { entries: vec![] });
        let mut slab: NounSlab = NounSlab::new();
        let noun = wire.encode_noun(&mut slab).unwrap();
        let decoded = pb::Transaction::decode_noun(noun.in_space(&slab.noun_space())).unwrap();
        assert_eq!(
            decoded, wire,
            "present empty signature is distinct from absence"
        );

        let invalid_name = T(&mut slab, &[D(0), D(0), D(1)]);
        assert!(pb::TxName::decode_noun(invalid_name.in_space(&slab.noun_space())).is_err());
    }

    #[test]
    fn pubkey_and_signature_limb_counts_are_checked() {
        let mut slab: NounSlab = NounSlab::new();
        let mut key = pb::TxPubkey {
            x: vec![0; 6],
            y: vec![0; 6],
            infinity: Some(false),
        };
        key.x.pop();
        assert!(key.encode_noun(&mut slab).is_err());
        key.x.push(PRIME);
        assert!(key.encode_noun(&mut slab).is_err());
        let signature = pb::TxSchnorrSignature {
            challenge: vec![0; 8],
            signature: vec![0; 7],
        };
        assert!(signature.encode_noun(&mut slab).is_err());
    }

    #[test]
    fn mixed_v1_spends_all_lock_primitives_and_opaque_data_preserve_consensus_shape() {
        use nockchain_math::belt::Belt;
        use nockchain_math::crypto::cheetah::A_GEN;
        use nockchain_types::{common as c, v1};

        fn hash(value: u64) -> c::Hash {
            c::Hash::from_limbs(&[value, 0, 0, 0, 0])
        }
        let key = c::SchnorrPubkey(A_GEN);
        let sig = c::SchnorrSignature {
            chal: [Belt(7); 8],
            sig: [Belt(9); 8],
        };
        let condition = v1::SpendCondition::new(vec![
            v1::LockPrimitive::Pkh(v1::Pkh::new(1, [hash(11)])),
            v1::LockPrimitive::Tim(v1::LockTim {
                rel: c::TimelockRangeRelative {
                    min: Some(c::BlockHeightDelta(Belt(0))),
                    max: Some(c::BlockHeightDelta(Belt(50))),
                },
                abs: c::TimelockRangeAbsolute {
                    min: Some(c::BlockHeight(Belt(20))),
                    max: None,
                },
            }),
            v1::LockPrimitive::Hax(v1::Hax::new([hash(12)])),
            v1::LockPrimitive::Burn,
        ]);
        let opaque = OwnedBasedNoun::cell(
            OwnedBasedNoun::try_atom(5).unwrap(),
            OwnedBasedNoun::try_atom(6).unwrap(),
        );
        let seeds = v1::Seeds(vec![v1::Seed {
            output_source: Some(c::Source {
                hash: hash(13),
                is_coinbase: false,
            }),
            lock_root: hash(14),
            // Reserved application keys remain arbitrary consensus data here.
            note_data: v1::NoteData::new(vec![v1::NoteDataEntry::new(
                "lock".into(),
                opaque.clone(),
            )]),
            gift: c::Nicks(15),
            parent_hash: hash(16),
        }]);
        for full in [false, true] {
            let proof = v1::MerkleProof {
                root: hash(17),
                path: vec![hash(18), hash(19)],
            };
            let lmp = if full {
                v1::LockMerkleProof::new_full(condition.clone(), 2, proof)
            } else {
                v1::LockMerkleProof::new_stub(condition.clone(), 2, proof)
            };
            let witness = v1::Witness::new(
                lmp,
                v1::PkhSignature::new(vec![v1::PkhSignatureEntry {
                    pkh: hash(11),
                    pubkey: key.clone(),
                    signature: sig.clone(),
                }]),
                vec![v1::HaxPreimage {
                    hash: hash(12),
                    value: opaque.clone(),
                }],
            );
            let raw = v1::RawTx {
                version: c::Version::V1,
                id: hash(20),
                spends: v1::Spends(vec![
                    (
                        c::Name::new(hash(21), hash(22)),
                        v1::Spend::Legacy(v1::Spend0 {
                            signature: c::Signature(vec![(key.clone(), sig.clone())]),
                            seeds: seeds.clone(),
                            fee: c::Nicks(23),
                        }),
                    ),
                    (
                        c::Name::new(hash(24), hash(25)),
                        v1::Spend::Witness(v1::Spend1 {
                            witness,
                            seeds: seeds.clone(),
                            fee: c::Nicks(26),
                        }),
                    ),
                ]),
            };
            let mut source: NounSlab = NounSlab::new();
            let noun = raw.to_noun(&mut source);
            source.set_root(noun);
            let expected = source.jam();
            let checked = Transaction::from_noun(noun.in_space(&source.noun_space())).unwrap();
            let encoded = checked.to_proto().encode_to_vec();
            let checked =
                Transaction::from_proto(pb::Transaction::decode(encoded.as_slice()).unwrap())
                    .unwrap();
            let mut target: NounSlab = NounSlab::new();
            let noun = checked.to_noun(&mut target).unwrap();
            target.set_root(noun);
            assert_eq!(target.jam(), expected);
        }
    }
}
