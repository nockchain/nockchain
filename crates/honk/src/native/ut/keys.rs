//! Named memo keys. Each key hashes like the tuple of its fields in declaration order.

use num_bigint::BigUint;

use super::types::Poly;
use crate::native::identity::*;
use crate::native::ir::ty::TypeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemanticContextKey {
    pub vet_key: VetMode,
    pub fan_context_key: FanContextId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemoContextKey {
    pub arm_epoch_key: ArmEpoch,
    pub placeholder_context_key: PlaceholderSignature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CacheContextKey {
    pub semantic: SemanticContextKey,
    pub memo: MemoContextKey,
}

/// Cache encoding of core polymorphism, preserving dry=0 and wet=1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct PolyKey(u8);

impl From<Poly> for PolyKey {
    fn from(poly: Poly) -> Self {
        Self(match poly {
            Poly::Dry => 0,
            Poly::Wet => 1,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CoreMintKey {
    pub subject: TypeId,
    pub goal: TypeId,
    pub tomes: TomesSignature,
    pub vet: VetMode,
    pub poly: PolyKey,
    pub fan: FanContextId,
    pub arm_epoch: ArmEpoch,
    pub placeholder: PlaceholderSignature,
}

/// Native caches use exact TypeIds; noun-boundary buckets use NounMugs and
/// check structural equality before accepting a hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MintKey<T> {
    pub subject: T,
    pub goal: T,
    pub vet: VetMode,
    pub gene: HoonSignature,
    pub fan: FanContextId,
    pub arm_epoch: ArmEpoch,
    pub placeholder: PlaceholderSignature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MullKey {
    pub subject: TypeId,
    pub goal: TypeId,
    pub secondary_subject: TypeId,
    pub vet: VetMode,
    pub gene: HoonSignature,
    pub fan: FanContextId,
    pub arm_epoch: ArmEpoch,
    pub placeholder: PlaceholderSignature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeBinaryKey<T> {
    pub subject: T,
    pub reference: T,
    pub vet: VetMode,
    pub fan: FanContextId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RestKey {
    pub subject: NounMug,
    pub legs: NounMug,
    pub vet: VetMode,
    pub fan: FanContextId,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FishKey {
    pub subject: TypeId,
    pub axis: BigUint,
    pub vet: VetMode,
    pub fan: FanContextId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SetSignature {
    pub sum: u64,
    pub xor: u64,
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BranSemiKey {
    pub subject: TypeId,
    pub vet: VetMode,
    pub fan: FanContextId,
    pub seen: SetSignature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HoldKey<T> {
    pub subject: T,
    pub gene: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LazyCoreKey {
    pub subject: TypeId,
    pub tomes: TomesSignature,
    pub poly: PolyKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MissKey {
    pub subject: TypeId,
    pub reference: TypeId,
    pub vet: VetMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WetRibKey {
    pub subject: TypeId,
    pub secondary_subject: TypeId,
    pub gene: NounIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArawKey {
    pub subject: super::SemiId,
    pub formula: NounIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MackKey {
    pub core: super::ValueId,
    pub axis: SmallAxis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FoldKey {
    pub subject: super::SemiId,
    pub formula: super::FormulaId,
}

/// Bucket key for a (hold, axis) pair: the xor of their mugs. Hits are compared exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct HoldAxisHash(pub(super) u32);

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::collections::HashSet;
    use std::hash::{Hash, Hasher};
    use std::mem::{align_of, size_of};

    use super::*;
    use crate::native::ut::types::FastHasher;

    fn hash<T: Hash, H: Hasher + Default>(value: &T) -> u64 {
        let mut state = H::default();
        value.hash(&mut state);
        state.finish()
    }

    fn same_representation<T: Hash, U: Hash>(key: T, tuple: U) {
        assert_eq!(size_of::<T>(), size_of::<U>());
        assert_eq!(align_of::<T>(), align_of::<U>());
        assert_eq!(
            hash::<_, DefaultHasher>(&key),
            hash::<_, DefaultHasher>(&tuple)
        );
        assert_eq!(hash::<_, FastHasher>(&key), hash::<_, FastHasher>(&tuple));
    }

    #[test]
    fn native_keys_preserve_tuple_layout_and_hashing() {
        for vet in [false, true] {
            for (poly, raw_poly) in [(Poly::Dry, 0u8), (Poly::Wet, 1u8)] {
                same_representation(
                    CoreMintKey {
                        subject: TypeId(1),
                        goal: TypeId(2),
                        tomes: TomesSignature(3),
                        vet: VetMode(vet),
                        poly: poly.into(),
                        fan: FanContextId(4),
                        arm_epoch: ArmEpoch(5),
                        placeholder: PlaceholderSignature(6),
                    },
                    (1u32, 2u32, 3u64, u8::from(vet), raw_poly, 4u64, 5u64, 6u64),
                );
            }
            same_representation(
                MintKey {
                    subject: TypeId(1),
                    goal: TypeId(2),
                    vet: VetMode(vet),
                    gene: HoonSignature(3),
                    fan: FanContextId(4),
                    arm_epoch: ArmEpoch(5),
                    placeholder: PlaceholderSignature(6),
                },
                (1u32, 2u32, u8::from(vet), 3u64, 4u64, 5u64, 6u64),
            );
            same_representation(
                MullKey {
                    subject: TypeId(1),
                    goal: TypeId(2),
                    secondary_subject: TypeId(3),
                    vet: VetMode(vet),
                    gene: HoonSignature(4),
                    fan: FanContextId(5),
                    arm_epoch: ArmEpoch(6),
                    placeholder: PlaceholderSignature(7),
                },
                (1u32, 2u32, 3u32, u8::from(vet), 4u64, 5u64, 6u64, 7u64),
            );
            same_representation(
                TypeBinaryKey {
                    subject: TypeId(1),
                    reference: TypeId(2),
                    vet: VetMode(vet),
                    fan: FanContextId(3),
                },
                (1u32, 2u32, u8::from(vet), 3u64),
            );
            same_representation(
                RestKey {
                    subject: NounMug(1),
                    legs: NounMug(2),
                    vet: VetMode(vet),
                    fan: FanContextId(3),
                },
                (1u32, 2u32, u8::from(vet), 3u64),
            );
            same_representation(
                FishKey {
                    subject: TypeId(1),
                    axis: BigUint::from(2u32),
                    vet: VetMode(vet),
                    fan: FanContextId(3),
                },
                (1u32, BigUint::from(2u32), u8::from(vet), 3u64),
            );
        }
        same_representation(
            HoldKey {
                subject: NounIdentity(1),
                gene: NounIdentity(2),
            },
            (1u64, 2u64),
        );
        same_representation(
            HoldKey {
                subject: NounMug(1),
                gene: NounMug(2),
            },
            (1u32, 2u32),
        );
    }

    #[test]
    fn every_mint_input_partitions_the_cache() {
        let key = MintKey {
            subject: TypeId(1),
            goal: TypeId(2),
            vet: VetMode(false),
            gene: HoonSignature(3),
            fan: FanContextId(4),
            arm_epoch: ArmEpoch(5),
            placeholder: PlaceholderSignature(6),
        };
        let keys = [
            key,
            MintKey {
                subject: TypeId(2),
                ..key
            },
            MintKey {
                goal: TypeId(1),
                ..key
            },
            MintKey {
                vet: VetMode(true),
                ..key
            },
            MintKey {
                gene: HoonSignature(4),
                ..key
            },
            MintKey {
                fan: FanContextId(5),
                ..key
            },
            MintKey {
                arm_epoch: ArmEpoch(6),
                ..key
            },
            MintKey {
                placeholder: PlaceholderSignature(7),
                ..key
            },
        ];
        assert_eq!(keys.into_iter().collect::<HashSet<_>>().len(), keys.len());
    }

    #[test]
    fn compact_ids_do_not_grow_keys() {
        assert!(size_of::<BranSemiKey>() <= size_of::<(u64, u8, u64, u64, u64, usize)>());
        assert!(size_of::<MissKey>() <= size_of::<(u64, u64, u8)>());
        assert!(size_of::<WetRibKey>() <= size_of::<(usize, usize, u64)>());
        assert!(size_of::<LazyCoreKey>() <= size_of::<(usize, u64, u8)>());
    }
}
