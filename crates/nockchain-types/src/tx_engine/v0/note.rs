use std::collections::HashSet;

use anyhow::Result;
use nockapp::Noun;
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::structs::HoonMapIter;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::{zmap, zset};
use nockvm::noun::{NounAllocator, D, SIG};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

#[cfg(test)]
use crate::tx_engine::common::BlockHeightDelta;
use crate::tx_engine::common::{
    BlockHeight, Hash, Name, Nicks, SchnorrPubkey, Source, TimelockRangeAbsolute,
    TimelockRangeRelative, Version,
};

// Nockchain Note
//
//   +$  form
//  $:  $:  version=%0  ::  utxo version number
//        ::    the page number in which the note was added to the balance.
//        ::NOTE while for dumbnet this could be block-id instead, and that
//        ::would simplify some code, for airwalk this would lead to a hashloop
//        origin-page=page-number
//        ::    a note with a null timelock has no page-number restrictions
//        ::    on when it may be spent
//        =timelock
//    ==
//  ::
//    name=Name
//    =lock
//    =source
//    assets=coins
//  ==
//

/// A transaction lock type
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    /// Number of required signatures to spend the note
    pub keys_required: u64,
    /// Set of public keys that can sign
    pub pubkeys: Vec<SchnorrPubkey>,
}

impl NounEncode for Lock {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        let m = u64::to_noun(&self.keys_required, stack);
        let keys_noun_map = self
            .pubkeys
            .iter()
            .fold(SIG, |map, pubkey: &SchnorrPubkey| {
                let mut val = pubkey.to_noun(stack);
                zset::z_set_put(stack, &map, &mut val, &DefaultTipHasher)
                    .expect("Failed to put public key into set")
            });
        let lock_noun = nockvm::noun::T(stack, &[m, keys_noun_map]);
        lock_noun
    }
}

impl NounDecode for Lock {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let keys_required = cell.head().as_atom()?.as_u64()? as u64;

        // It is called HoonMapIter, but it can be used for sets as well
        let pubkeys_iter = HoonMapIter::from(cell.tail());

        let mut pubkeys = Vec::new();
        for pubkey in pubkeys_iter {
            let schnorr = SchnorrPubkey::from_noun(&pubkey)?;
            pubkeys.push(schnorr);
        }

        let unique = pubkeys.iter().collect::<HashSet<_>>();

        if pubkeys.len() != unique.len() {
            return Err(NounDecodeError::Custom(
                "Expected unique public keys".to_string(),
            ));
        }

        if keys_required == 0 {
            tracing::warn!("NounDecode Lock: expected m > 0");
        }

        if keys_required > unique.len() as u64 {
            tracing::warn!("NounDecode Lock: expected m <= number of public keys");
        }

        Ok(Lock {
            keys_required,
            pubkeys,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct NoteV0 {
    pub head: NoteHead,
    pub tail: NoteTail,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct NoteHead {
    pub version: Version,
    pub origin_page: BlockHeight,
    pub timelock: Timelock,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct NoteTail {
    pub name: Name,
    pub lock: Lock,
    pub source: Source,
    pub assets: Nicks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Balance(pub Vec<(Name, NoteV0)>);

impl NounEncode for Balance {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        let keys_noun_map = self.0.iter().fold(D(0), |map, (name, note)| {
            let mut key = name.to_noun(stack);
            let mut value = note.to_noun(stack);
            zmap::z_map_put(stack, &map, &mut key, &mut value, &DefaultTipHasher).unwrap()
        });
        keys_noun_map
    }
}

impl NounDecode for Balance {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let notes = HoonMapIter::from(*noun)
            .filter(|kv| kv.is_cell())
            .map(|kv| {
                let [k, v] = kv.uncell()?;
                let name = Name::from_noun(&k)?;
                let note = NoteV0::from_noun(&v)?;
                Ok((name, note))
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;
        Ok(Balance(notes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounDecode, NounEncode)]
pub struct BalanceUpdate {
    pub height: BlockHeight,
    pub block_id: Hash,
    pub notes: Balance,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct Timelock(pub Option<TimelockIntent>);

/// Represents a timelock intent - optional constraint for output notes
#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct TimelockIntent {
    pub absolute: TimelockRangeAbsolute,
    pub relative: TimelockRangeRelative,
}

impl TimelockIntent {
    pub fn none() -> Self {
        Self {
            absolute: TimelockRangeAbsolute::none(),
            relative: TimelockRangeRelative::none(),
        }
    }
}
#[cfg(test)]
mod test {
    use bytes::Bytes;
    use nockapp::noun::slab::NounSlab;
    use nockchain_math::belt::Belt;
    use nockvm::noun::NounAllocator;
    use noun_serde::{NounDecode, NounEncode};
    use quickcheck::{quickcheck, Arbitrary, Gen};

    use super::{
        Balance, BalanceUpdate, BlockHeight, Hash, Lock, Name, Nicks, NoteHead, NoteTail, NoteV0,
        Source, Timelock, TimelockIntent, TimelockRangeAbsolute, TimelockRangeRelative, Version,
    };
    use crate::common;

    // TODO: implement a more elegant way of switching between cargo and bazel builds
    fn try_path(jam: &str) -> Result<Bytes, Box<dyn std::error::Error>> {
        let possible_paths = [
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("jams")
                .join(jam),
            std::path::Path::new("open/crates/nockchain-types/jams").join(jam),
        ];

        let jam_path = possible_paths
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .expect("Failed to read jam file from any known location");
        Ok(Bytes::from(jam_path))
    }

    #[test]
    fn test_balance() -> Result<(), Box<dyn std::error::Error>> {
        let balance_jam = try_path("balance.jam")?;
        let mut slab: NounSlab = NounSlab::new();
        let mut balance_noun = slab.cue_into(balance_jam)?;
        let balance = Balance::from_noun(&balance_noun)?;
        let mut balance_noun_from_struct = Balance::to_noun(&balance, &mut slab);
        unsafe { slab.equals(&mut balance_noun, &mut balance_noun_from_struct) };
        Ok(())
    }

    #[test]
    fn test_note() -> Result<(), Box<dyn std::error::Error>> {
        let note_jam = try_path("note.jam")?;
        let mut slab: NounSlab = NounSlab::new();
        let mut note_noun = slab.cue_into(note_jam)?;
        let note = NoteV0::from_noun(&note_noun)?;
        let mut note_noun_from_struct = NoteV0::to_noun(&note, &mut slab);
        assert!(unsafe { slab.equals(&mut note_noun, &mut note_noun_from_struct) });
        //eprintln!("{:?}", utxo);
        Ok(())
    }

    #[test]
    fn test_timelock() -> Result<(), Box<dyn std::error::Error>> {
        let timelock_jam = try_path("timelock.jam")?;
        let mut slab: NounSlab = NounSlab::new();
        let timelock_noun = slab.cue_into(timelock_jam)?;
        let _ = <Option<TimelockIntent>>::from_noun(&timelock_noun);
        Ok(())
    }

    fn bh(v: u64) -> BlockHeight {
        BlockHeight(Belt(v))
    }
    fn dh(v: u64) -> common::BlockHeightDelta {
        common::BlockHeightDelta(Belt(v))
    }

    #[test]
    fn test_timelock_roundtrip_none() {
        let mut slab: NounSlab = NounSlab::new();
        let tl = Timelock(None);
        let mut n1 = Timelock::to_noun(&tl, &mut slab);
        let tl2 = Timelock::from_noun(&n1).expect("decode");
        let mut n2 = Timelock::to_noun(&tl2, &mut slab);
        assert!(unsafe { slab.equals(&mut n1, &mut n2) });
    }

    #[test]
    fn test_timelock_roundtrip_absolute_only() {
        let mut slab: NounSlab = NounSlab::new();
        let tl = Timelock(Some(TimelockIntent {
            absolute: TimelockRangeAbsolute::new(Some(bh(100)), None),
            relative: TimelockRangeRelative::none(),
        }));
        let mut n1 = Timelock::to_noun(&tl, &mut slab);
        let tl2 = Timelock::from_noun(&n1).expect("decode");
        let mut n2 = Timelock::to_noun(&tl2, &mut slab);
        assert!(unsafe { slab.equals(&mut n1, &mut n2) });
    }

    #[test]
    fn hash_from_base58_accepts_valid_id() {
        let tx_id = "3giXkwW4zbFhoyJu27RbP6VNiYgR6yaTfk2AYnEHvxtVaGbmcVD6jb9";
        Hash::from_base58(tx_id).expect("expected valid base58 hash");
    }

    #[test]
    fn hash_from_base58_rejects_invalid_id() {
        let invalid_tx_id = "not-a-valid-hash";
        assert!(Hash::from_base58(invalid_tx_id).is_err());
    }

    #[test]
    fn hash_roundtrip_from_base58() {
        let tx_id = "3giXkwW4zbFhoyJu27RbP6VNiYgR6yaTfk2AYnEHvxtVaGbmcVD6jb9";
        let hash = Hash::from_base58(tx_id).expect("expected valid base58 hash");
        let hash_str = hash.to_base58();
        assert_eq!(hash_str, tx_id);
    }

    #[test]
    fn test_timelock_roundtrip_relative_only() {
        let mut slab: NounSlab = NounSlab::new();
        let tl = Timelock(Some(TimelockIntent {
            absolute: TimelockRangeAbsolute::none(),
            relative: TimelockRangeRelative::new(Some(dh(5)), Some(dh(50))),
        }));
        let mut n1 = Timelock::to_noun(&tl, &mut slab);
        let tl2 = Timelock::from_noun(&n1).expect("decode");
        let mut n2 = Timelock::to_noun(&tl2, &mut slab);
        assert!(unsafe { slab.equals(&mut n1, &mut n2) });
    }

    #[test]
    fn test_timelock_roundtrip_both() {
        let mut slab: NounSlab = NounSlab::new();
        let tl = Timelock(Some(TimelockIntent {
            absolute: TimelockRangeAbsolute::new(Some(bh(10)), Some(bh(20))),
            relative: TimelockRangeRelative::new(Some(dh(1)), Some(dh(2))),
        }));
        let mut n1 = Timelock::to_noun(&tl, &mut slab);
        let tl2 = Timelock::from_noun(&n1).expect("decode");
        let mut n2 = Timelock::to_noun(&tl2, &mut slab);
        assert!(unsafe { slab.equals(&mut n1, &mut n2) });
    }

    // ----------------------
    // QuickCheck generators
    // ----------------------
    fn arb_belt(g: &mut Gen) -> Belt {
        // Avoid extreme values that can trigger non-determinism in TIP hashing paths.
        let mut v = u64::arbitrary(g) & 0x7FFF_FFFF_FFFF_FFFF;
        if v == 0 {
            v = 1;
        }
        Belt(v)
    }
    fn arb_hash(g: &mut Gen) -> Hash {
        Hash([arb_belt(g), arb_belt(g), arb_belt(g), arb_belt(g), arb_belt(g)])
    }

    impl Arbitrary for Version {
        fn arbitrary(g: &mut Gen) -> Self {
            match u8::arbitrary(g) % 3 {
                0 => Version::V0,
                1 => Version::V1,
                _ => Version::V2,
            }
        }
    }

    impl Arbitrary for BlockHeight {
        fn arbitrary(g: &mut Gen) -> Self {
            BlockHeight(arb_belt(g))
        }
    }

    impl Arbitrary for common::BlockHeightDelta {
        fn arbitrary(g: &mut Gen) -> Self {
            common::BlockHeightDelta(arb_belt(g))
        }
    }

    impl Arbitrary for TimelockRangeAbsolute {
        fn arbitrary(g: &mut Gen) -> Self {
            let min = if bool::arbitrary(g) {
                Some(BlockHeight::arbitrary(g))
            } else {
                None
            };
            let max = if bool::arbitrary(g) {
                Some(BlockHeight::arbitrary(g))
            } else {
                None
            };
            TimelockRangeAbsolute { min, max }
        }
    }

    impl Arbitrary for TimelockRangeRelative {
        fn arbitrary(g: &mut Gen) -> Self {
            let min = if bool::arbitrary(g) {
                Some(common::BlockHeightDelta::arbitrary(g))
            } else {
                None
            };
            let max = if bool::arbitrary(g) {
                Some(common::BlockHeightDelta::arbitrary(g))
            } else {
                None
            };
            TimelockRangeRelative { min, max }
        }
    }

    impl Arbitrary for TimelockIntent {
        fn arbitrary(g: &mut Gen) -> Self {
            TimelockIntent {
                absolute: TimelockRangeAbsolute::arbitrary(g),
                relative: TimelockRangeRelative::arbitrary(g),
            }
        }
    }

    impl Arbitrary for Timelock {
        fn arbitrary(g: &mut Gen) -> Self {
            if bool::arbitrary(g) {
                Timelock(Some(TimelockIntent::arbitrary(g)))
            } else {
                Timelock(None)
            }
        }
    }

    impl Arbitrary for Name {
        fn arbitrary(g: &mut Gen) -> Self {
            return Name::new(arb_hash(g), arb_hash(g));
        }
    }

    impl Arbitrary for Source {
        fn arbitrary(g: &mut Gen) -> Self {
            Source {
                hash: arb_hash(g),
                is_coinbase: bool::arbitrary(g),
            }
        }
    }

    impl Arbitrary for Lock {
        fn arbitrary(_g: &mut Gen) -> Self {
            Lock {
                keys_required: 0,
                pubkeys: Vec::new(),
            }
        }
    }

    impl Arbitrary for Nicks {
        fn arbitrary(g: &mut Gen) -> Self {
            Nicks((u16::arbitrary(g)) as usize)
        }
    }

    impl Arbitrary for NoteHead {
        fn arbitrary(g: &mut Gen) -> Self {
            NoteHead {
                version: Version::arbitrary(g),
                origin_page: BlockHeight::arbitrary(g),
                timelock: Timelock::arbitrary(g),
            }
        }
    }

    impl Arbitrary for NoteTail {
        fn arbitrary(g: &mut Gen) -> Self {
            NoteTail {
                name: Name::arbitrary(g),
                lock: Lock::arbitrary(g),
                source: Source::arbitrary(g),
                assets: Nicks::arbitrary(g),
            }
        }
    }

    impl Arbitrary for NoteV0 {
        fn arbitrary(g: &mut Gen) -> Self {
            NoteV0 {
                head: NoteHead::arbitrary(g),
                tail: NoteTail::arbitrary(g),
            }
        }
    }

    impl Arbitrary for Balance {
        fn arbitrary(g: &mut Gen) -> Self {
            use std::collections::HashSet;
            let mut set: HashSet<(Hash, Hash)> = HashSet::new();
            let mut items: Vec<(Name, NoteV0)> = Vec::new();
            let len = 1 + (usize::arbitrary(g) % 5) as usize;
            for _ in 0..len {
                let name = Name::arbitrary(g);
                if set.insert((name.first.clone(), name.last.clone())) {
                    items.push((name, NoteV0::arbitrary(g)));
                }
            }
            Balance(items)
        }
    }

    impl Arbitrary for BalanceUpdate {
        fn arbitrary(g: &mut Gen) -> Self {
            BalanceUpdate {
                height: BlockHeight::arbitrary(g),
                block_id: arb_hash(g),
                notes: Balance::arbitrary(g),
            }
        }
    }

    #[test]
    fn quickcheck_balance_update_noun_roundtrip() {
        fn prop(update: BalanceUpdate) -> bool {
            let mut slab: NounSlab = NounSlab::new();
            let mut n1 = BalanceUpdate::to_noun(&update, &mut slab);
            let decoded = match BalanceUpdate::from_noun(&n1) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let mut n2 = BalanceUpdate::to_noun(&decoded, &mut slab);
            unsafe { slab.equals(&mut n1, &mut n2) }
        }
        quickcheck(prop as fn(BalanceUpdate) -> bool);
    }
}
