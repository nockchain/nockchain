//! Complete, checked consensus pages at the peer boundary.

use std::collections::BTreeSet;
use std::io;

use nockapp::noun::slab::NounSlab;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::{zmap, zset};
use nockvm::noun::{Atom, Noun, NounHandle, D, T};

use super::common::{
    invalid, list, required, tree_entries, tuple, PeerAtom, PeerHash, PeerNoun, MAX_NODES, PRIME,
};
use super::pb;
use super::transaction::{pubkey_from_noun, pubkey_to_noun};

#[derive(Clone, Debug)]
enum Coinbase {
    Legacy(Vec<(pb::TxPubkey, u64)>),
    V1(Vec<(PeerHash, u64)>),
}

/// Fields stay private after validation. Proof and consensus admission remain
/// the kernel's responsibility; this type establishes representation validity.
#[derive(Clone, Debug)]
pub(crate) struct PeerPage {
    digest: PeerHash,
    pow: Option<PeerNoun>,
    parent: PeerHash,
    tx_ids: Vec<PeerHash>,
    coinbase: Coinbase,
    timestamp: PeerAtom,
    epoch_counter: PeerAtom,
    target: Vec<u32>,
    accumulated_work: Vec<u32>,
    height: u64,
    message: Vec<u64>,
}

fn scalar(noun: NounHandle<'_>) -> io::Result<u64> {
    noun.as_atom()
        .map_err(|_| invalid("page scalar must be an atom"))?
        .as_u64()
        .map_err(|_| invalid("page scalar exceeds u64"))
}

fn belt(value: u64) -> io::Result<u64> {
    if value >= PRIME {
        Err(invalid("page value is outside the Goldilocks field"))
    } else {
        Ok(value)
    }
}

fn count<T>(items: &[T]) -> io::Result<()> {
    if items.len() > MAX_NODES {
        Err(invalid("page collection exceeds noun node limit"))
    } else {
        Ok(())
    }
}

fn big_num_from_noun(noun: NounHandle<'_>) -> io::Result<Vec<u32>> {
    let parts = tuple(noun, 2)?;
    if scalar(parts[0])? != nockvm_macros::tas!(b"bn") {
        return Err(invalid("page bignum must have the bn tag"));
    }
    list(parts[1])?
        .into_iter()
        .map(|chunk| {
            u32::try_from(scalar(chunk)?).map_err(|_| invalid("page bignum limb exceeds u32"))
        })
        .collect()
}

fn big_num_to_noun(limbs: &[u32], slab: &mut NounSlab) -> Noun {
    let mut list = D(0);
    for limb in limbs.iter().rev() {
        list = T(slab, &[D(u64::from(*limb)), list]);
    }
    T(slab, &[D(nockvm_macros::tas!(b"bn")), list])
}

impl Coinbase {
    fn from_proto(value: pb::PageCoinbase, version: u32) -> io::Result<Self> {
        use pb::page_coinbase::Kind;
        match (version, required(value.kind, "page.coinbase.kind")?) {
            (0, Kind::Legacy(value)) => {
                count(&value.entries)?;
                let mut entries = Vec::with_capacity(value.entries.len());
                let mut validation = NounSlab::new();
                for entry in value.entries {
                    let key = required(entry.key, "page.coinbase.key")?;
                    pubkey_to_noun(&key, &mut validation)?;
                    if entries.iter().any(|(existing, _)| existing == &key) {
                        return Err(invalid("duplicate page coinbase key"));
                    }
                    let coins = belt(required(entry.coins, "page.coinbase.coins")?)?;
                    entries.push((key, coins));
                }
                Ok(Self::Legacy(entries))
            }
            (1, Kind::V1(value)) => {
                count(&value.entries)?;
                let mut entries = Vec::with_capacity(value.entries.len());
                let mut seen = BTreeSet::new();
                for entry in value.entries {
                    let key = PeerHash::from_proto(required(entry.key, "page.coinbase.key")?)?;
                    if !seen.insert(key.clone()) {
                        return Err(invalid("duplicate page coinbase key"));
                    }
                    let coins = belt(required(entry.coins, "page.coinbase.coins")?)?;
                    entries.push((key, coins));
                }
                Ok(Self::V1(entries))
            }
            _ => Err(invalid("page coinbase kind does not match page version")),
        }
    }

    fn to_proto(&self) -> pb::PageCoinbase {
        use pb::page_coinbase::Kind;
        let kind = match self {
            Self::Legacy(entries) => Kind::Legacy(pb::PageLegacyCoinbase {
                entries: entries
                    .iter()
                    .map(|(key, coins)| pb::PageLegacyCoinbaseEntry {
                        key: Some(key.clone()),
                        coins: Some(*coins),
                    })
                    .collect(),
            }),
            Self::V1(entries) => Kind::V1(pb::PageV1Coinbase {
                entries: entries
                    .iter()
                    .map(|(key, coins)| pb::PageV1CoinbaseEntry {
                        key: Some(key.to_proto()),
                        coins: Some(*coins),
                    })
                    .collect(),
            }),
        };
        pb::PageCoinbase { kind: Some(kind) }
    }

    fn from_noun(noun: NounHandle<'_>, version: u32) -> io::Result<Self> {
        use pb::page_coinbase::Kind;
        let entries = tree_entries(noun)?;
        let kind = match version {
            0 => Kind::Legacy(pb::PageLegacyCoinbase {
                entries: entries
                    .into_iter()
                    .map(|entry| {
                        let parts = tuple(entry, 2)?;
                        Ok(pb::PageLegacyCoinbaseEntry {
                            key: Some(pubkey_from_noun(parts[0])?),
                            coins: Some(scalar(parts[1])?),
                        })
                    })
                    .collect::<io::Result<_>>()?,
            }),
            1 => Kind::V1(pb::PageV1Coinbase {
                entries: entries
                    .into_iter()
                    .map(|entry| {
                        let parts = tuple(entry, 2)?;
                        Ok(pb::PageV1CoinbaseEntry {
                            key: Some(PeerHash::from_noun(parts[0])?.to_proto()),
                            coins: Some(scalar(parts[1])?),
                        })
                    })
                    .collect::<io::Result<_>>()?,
            }),
            _ => return Err(invalid("unsupported page version")),
        };
        Self::from_proto(pb::PageCoinbase { kind: Some(kind) }, version)
    }

    fn to_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let mut map = D(0);
        let mut insert = |slab: &mut NounSlab, mut key, coins| {
            let mut value = Atom::new(slab, coins).as_noun();
            map = zmap::z_map_put(slab, &map, &mut key, &mut value, &DefaultTipHasher).map_err(
                |error| invalid(format!("page coinbase construction failed: {error:?}")),
            )?;
            Ok::<(), io::Error>(())
        };
        match self {
            Self::Legacy(entries) => {
                for (key, coins) in entries {
                    let key = pubkey_to_noun(key, slab)?;
                    insert(slab, key, *coins)?;
                }
            }
            Self::V1(entries) => {
                for (key, coins) in entries {
                    let key = key.to_noun(slab);
                    insert(slab, key, *coins)?;
                }
            }
        }
        Ok(map)
    }

    fn version(&self) -> u32 {
        match self {
            Self::Legacy(_) => 0,
            Self::V1(_) => 1,
        }
    }
}

impl PeerPage {
    pub(crate) fn from_proto(value: pb::Page) -> io::Result<Self> {
        let version = required(value.version, "page.version")?;
        if version > 1 {
            return Err(invalid("unsupported page version"));
        }
        count(&value.tx_ids)?;
        count(&value.message)?;
        let mut seen = BTreeSet::new();
        let mut tx_ids = Vec::with_capacity(value.tx_ids.len());
        for hash in value.tx_ids {
            let hash = PeerHash::from_proto(hash)?;
            if !seen.insert(hash.clone()) {
                return Err(invalid("duplicate page transaction id"));
            }
            tx_ids.push(hash);
        }
        let target = required(value.target, "page.target")?.limbs;
        let accumulated_work = required(value.accumulated_work, "page.accumulated_work")?.limbs;
        count(&target)?;
        count(&accumulated_work)?;
        Ok(Self {
            digest: PeerHash::from_proto(required(value.digest, "page.digest")?)?,
            pow: value.pow.map(PeerNoun::from_proto).transpose()?,
            parent: PeerHash::from_proto(required(value.parent, "page.parent")?)?,
            tx_ids,
            coinbase: Coinbase::from_proto(required(value.coinbase, "page.coinbase")?, version)?,
            timestamp: PeerAtom::from_proto(required(value.timestamp, "page.timestamp")?)?,
            epoch_counter: PeerAtom::from_proto(required(
                value.epoch_counter, "page.epoch_counter",
            )?)?,
            target,
            accumulated_work,
            height: required(value.height, "page.height")?,
            message: value
                .message
                .into_iter()
                .map(belt)
                .collect::<io::Result<_>>()?,
        })
    }

    pub(crate) fn to_proto(&self) -> pb::Page {
        pb::Page {
            version: Some(self.coinbase.version()),
            digest: Some(self.digest.to_proto()),
            pow: self.pow.as_ref().map(PeerNoun::to_proto),
            parent: Some(self.parent.to_proto()),
            tx_ids: self.tx_ids.iter().map(PeerHash::to_proto).collect(),
            coinbase: Some(self.coinbase.to_proto()),
            timestamp: Some(self.timestamp.to_proto()),
            epoch_counter: Some(self.epoch_counter.to_proto()),
            target: Some(pb::PageBigNum {
                limbs: self.target.clone(),
            }),
            accumulated_work: Some(pb::PageBigNum {
                limbs: self.accumulated_work.clone(),
            }),
            height: Some(self.height),
            message: self.message.clone(),
        }
    }

    pub(crate) fn from_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let root = noun.as_cell().map_err(|_| invalid("page must be a cell"))?;
        let (version, body) = if root.head().is_atom() {
            if scalar(root.head())? != 1 {
                return Err(invalid("unsupported page version"));
            }
            (1, root.tail())
        } else {
            (0, noun)
        };
        let fields = tuple(body, 11)?;
        let pow = if fields[1].is_atom() {
            if scalar(fields[1])? != 0 {
                return Err(invalid("absent page proof must be zero"));
            }
            None
        } else {
            let unit = tuple(fields[1], 2)?;
            if scalar(unit[0])? != 0 {
                return Err(invalid("present page proof must have a zero unit tag"));
            }
            Some(PeerNoun::from_noun(unit[1])?.to_proto())
        };
        let tx_ids = tree_entries(fields[3])?
            .into_iter()
            .map(|hash| Ok(PeerHash::from_noun(hash)?.to_proto()))
            .collect::<io::Result<_>>()?;
        let coinbase = Coinbase::from_noun(fields[4], version)?;
        Self::from_proto(pb::Page {
            version: Some(version),
            digest: Some(PeerHash::from_noun(fields[0])?.to_proto()),
            pow,
            parent: Some(PeerHash::from_noun(fields[2])?.to_proto()),
            tx_ids,
            coinbase: Some(coinbase.to_proto()),
            timestamp: Some(PeerAtom::from_noun(fields[5])?.to_proto()),
            epoch_counter: Some(PeerAtom::from_noun(fields[6])?.to_proto()),
            target: Some(pb::PageBigNum {
                limbs: big_num_from_noun(fields[7])?,
            }),
            accumulated_work: Some(pb::PageBigNum {
                limbs: big_num_from_noun(fields[8])?,
            }),
            height: Some(scalar(fields[9])?),
            message: list(fields[10])?
                .into_iter()
                .map(scalar)
                .collect::<io::Result<_>>()?,
        })
    }

    pub(crate) fn to_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let digest = self.digest.to_noun(slab);
        let pow = match &self.pow {
            Some(proof) => {
                let proof = proof.to_noun(slab)?;
                T(slab, &[D(0), proof])
            }
            None => D(0),
        };
        let parent = self.parent.to_noun(slab);
        let mut tx_ids = D(0);
        for hash in &self.tx_ids {
            let mut noun = hash.to_noun(slab);
            tx_ids =
                zset::z_set_put(slab, &tx_ids, &mut noun, &DefaultTipHasher).map_err(|error| {
                    invalid(format!(
                        "page transaction set construction failed: {error:?}"
                    ))
                })?;
        }
        let coinbase = self.coinbase.to_noun(slab)?;
        let timestamp = self.timestamp.to_noun(slab);
        let epoch_counter = self.epoch_counter.to_noun(slab);
        let target = big_num_to_noun(&self.target, slab);
        let accumulated_work = big_num_to_noun(&self.accumulated_work, slab);
        let height = Atom::new(slab, self.height).as_noun();
        let mut message = D(0);
        for value in self.message.iter().rev() {
            let value = Atom::new(slab, *value).as_noun();
            message = T(slab, &[value, message]);
        }
        let page = T(
            slab,
            &[
                digest, pow, parent, tx_ids, coinbase, timestamp, epoch_counter, target,
                accumulated_work, height, message,
            ],
        );
        match self.coinbase.version() {
            0 => Ok(page),
            _ => Ok(T(slab, &[D(1), page])),
        }
    }
}

#[cfg(test)]
mod tests {
    use nockvm::ext::AtomExt;
    use nockvm::noun::NounAllocator;
    use prost::Message;

    use super::*;

    fn hash(seed: u64) -> PeerHash {
        PeerHash::new([seed, seed + 1, seed + 2, seed + 3, seed + 4]).unwrap()
    }

    fn pubkey(seed: u64) -> pb::TxPubkey {
        pb::TxPubkey {
            x: (seed..seed + 6).collect(),
            y: (seed + 6..seed + 12).collect(),
            infinity: Some(false),
        }
    }

    // Construct the consensus tuple independently of the page conversion.
    // This includes nonempty coinbase maps and sets, high atoms, a message
    // element larger than u32, and bignums with trailing zero limbs.
    fn fixture(version: u32, with_pow: bool) -> (NounSlab, Noun) {
        let mut slab = NounSlab::new();
        let digest = hash(10).to_noun(&mut slab);
        let parent = hash(20).to_noun(&mut slab);
        let mut tx_ids = D(0);
        for seed in [30, 40, 50] {
            let mut id = hash(seed).to_noun(&mut slab);
            tx_ids = zset::z_set_put(&mut slab, &tx_ids, &mut id, &DefaultTipHasher).unwrap();
        }
        let mut coinbase = D(0);
        for (seed, coins) in [(60, 500), (80, 250)] {
            let mut key = match version {
                0 => pubkey_to_noun(&pubkey(seed), &mut slab).unwrap(),
                _ => hash(seed).to_noun(&mut slab),
            };
            let mut amount = D(coins);
            coinbase = zmap::z_map_put(
                &mut slab, &coinbase, &mut key, &mut amount, &DefaultTipHasher,
            )
            .unwrap();
        }
        let pow = if with_pow {
            let proof = T(&mut slab, &[D(5), D(17), D(0)]);
            T(&mut slab, &[D(0), proof])
        } else {
            D(0)
        };
        let timestamp = Atom::from_bytes(&mut slab, &[1, 2, 3, 4, 5, 6, 7, 8, 9]).as_noun();
        let epoch = Atom::from_bytes(&mut slab, &[9, 8, 7, 6, 5, 4, 3, 2, 1]).as_noun();
        let target_chunks = T(&mut slab, &[D(123), D(0), D(0)]);
        let target = T(&mut slab, &[D(nockvm_macros::tas!(b"bn")), target_chunks]);
        let work_chunks = T(&mut slab, &[D(0), D(u64::from(u32::MAX)), D(0), D(0)]);
        let work = T(&mut slab, &[D(nockvm_macros::tas!(b"bn")), work_chunks]);
        let large_message = Atom::new(&mut slab, PRIME - 1).as_noun();
        let message = T(&mut slab, &[D(1), large_message, D(0)]);
        let body = T(
            &mut slab,
            &[
                digest,
                pow,
                parent,
                tx_ids,
                coinbase,
                timestamp,
                epoch,
                target,
                work,
                D(1234),
                message,
            ],
        );
        let root = if version == 0 {
            body
        } else {
            T(&mut slab, &[D(1), body])
        };
        slab.set_root(root);
        (slab, root)
    }

    fn wire(version: u32) -> pb::Page {
        let (slab, root) = fixture(version, true);
        PeerPage::from_noun(root.in_space(&slab.noun_space()))
            .unwrap()
            .to_proto()
    }

    fn jam(page: pb::Page) -> Vec<u8> {
        let page = PeerPage::from_proto(page).unwrap();
        let mut slab = NounSlab::new();
        let noun = page.to_noun(&mut slab).unwrap();
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    #[test]
    fn both_page_versions_preserve_every_consensus_field() {
        for version in [0, 1] {
            for with_pow in [false, true] {
                let (slab, root) = fixture(version, with_pow);
                let original = slab.jam();
                let page = PeerPage::from_noun(root.in_space(&slab.noun_space())).unwrap();
                let wire = page.to_proto();
                assert_eq!(wire.version, Some(version));
                assert_eq!(wire.pow.is_some(), with_pow);
                assert_eq!(wire.target.as_ref().unwrap().limbs, [123, 0]);
                assert_eq!(
                    wire.accumulated_work.as_ref().unwrap().limbs,
                    [0, u32::MAX, 0]
                );
                assert_eq!(wire.message, [1, PRIME - 1]);
                assert_eq!(
                    wire.timestamp
                        .as_ref()
                        .unwrap()
                        .little_endian
                        .as_ref()
                        .unwrap()
                        .len(),
                    9
                );
                let bytes = wire.encode_to_vec();
                let decoded = pb::Page::decode(bytes.as_slice()).unwrap();
                assert_eq!(jam(decoded), original.as_ref());
            }
        }
    }

    #[test]
    fn missing_fields_and_coinbase_version_mismatch_are_rejected() {
        let original = wire(0);
        let mut missing = original.clone();
        missing.version = None;
        assert!(PeerPage::from_proto(missing).is_err());
        let mut missing = original.clone();
        missing.timestamp = None;
        assert!(PeerPage::from_proto(missing).is_err());
        let mut missing = original.clone();
        missing.height = None;
        assert!(PeerPage::from_proto(missing).is_err());
        let mut missing = original.clone();
        missing.target = None;
        assert!(PeerPage::from_proto(missing).is_err());
        let mut mismatched = original.clone();
        mismatched.version = Some(1);
        assert!(PeerPage::from_proto(mismatched).is_err());
        let mut unsupported = original;
        unsupported.version = Some(2);
        assert!(PeerPage::from_proto(unsupported).is_err());
    }

    #[test]
    fn duplicate_transaction_and_coinbase_keys_are_rejected() {
        for version in [0, 1] {
            let mut page = wire(version);
            page.tx_ids.push(page.tx_ids[0].clone());
            assert!(PeerPage::from_proto(page).is_err());
            let mut page = wire(version);
            match page.coinbase.as_mut().unwrap().kind.as_mut().unwrap() {
                pb::page_coinbase::Kind::Legacy(map) => map.entries.push(map.entries[0].clone()),
                pb::page_coinbase::Kind::V1(map) => map.entries.push(map.entries[0].clone()),
            }
            assert!(PeerPage::from_proto(page).is_err());
        }
    }

    #[test]
    fn set_and_map_wire_order_do_not_change_the_consensus_noun() {
        for version in [0, 1] {
            let page = wire(version);
            let expected = jam(page.clone());
            let mut reordered = page;
            reordered.tx_ids.reverse();
            match reordered.coinbase.as_mut().unwrap().kind.as_mut().unwrap() {
                pb::page_coinbase::Kind::Legacy(map) => map.entries.reverse(),
                pb::page_coinbase::Kind::V1(map) => map.entries.reverse(),
            }
            assert_eq!(jam(reordered), expected);
        }
    }

    #[test]
    fn page_field_elements_and_pubkey_shapes_are_checked() {
        let mut page = wire(1);
        page.message.push(PRIME);
        assert!(PeerPage::from_proto(page).is_err());
        let mut page = wire(1);
        let pb::page_coinbase::Kind::V1(map) =
            page.coinbase.as_mut().unwrap().kind.as_mut().unwrap()
        else {
            panic!("v1 fixture");
        };
        map.entries[0].coins = Some(PRIME);
        assert!(PeerPage::from_proto(page).is_err());
        let mut page = wire(0);
        let pb::page_coinbase::Kind::Legacy(map) =
            page.coinbase.as_mut().unwrap().kind.as_mut().unwrap()
        else {
            panic!("legacy fixture");
        };
        map.entries[0].key.as_mut().unwrap().x.pop();
        assert!(PeerPage::from_proto(page).is_err());
    }

    #[test]
    fn bignum_noun_requires_tag_u32_limbs_and_zero_termination() {
        let mut slab: NounSlab = NounSlab::new();
        let oversized = Atom::new(&mut slab, u64::from(u32::MAX) + 1).as_noun();
        let chunks = T(&mut slab, &[oversized, D(0)]);
        let too_large = T(&mut slab, &[D(nockvm_macros::tas!(b"bn")), chunks]);
        assert!(big_num_from_noun(too_large.in_space(&slab.noun_space())).is_err());
        let wrong_tag = T(&mut slab, &[D(0), D(0)]);
        assert!(big_num_from_noun(wrong_tag.in_space(&slab.noun_space())).is_err());
        let wrong_end = T(&mut slab, &[D(nockvm_macros::tas!(b"bn")), D(7)]);
        assert!(big_num_from_noun(wrong_end.in_space(&slab.noun_space())).is_err());
    }
}
