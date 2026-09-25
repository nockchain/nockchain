//! Scalar identities used by compiler arenas and memo tables.
//!
//! These wrappers have the representation of their underlying scalar. IDs are
//! local to one compiler context. Mug and node-hash hits are compared exactly,
//! as are `SpecSignature` buckets. `HoonSignature` and `TomesSignature` are
//! digests trusted as exact keys by the mint, mull, core-mint, and lazy
//! resolver caches and by the `open_cache` address guard.
//!
//! Equal integer representations do not make different ID domains compatible:
//! ```compile_fail,E0308
//! use honk::native::identity::{FanContextId, LazyResolverId};
//! let fan: FanContextId = LazyResolverId::default();
//! ```

use hatch::ast::hoon::Hoon;
use nockvm::noun::Noun;

macro_rules! identity {
    ($(#[$meta:meta])* $name:ident, $scalar:ty) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(pub(crate) $scalar);
    };
}

identity!(/// Raw noun word, used only for identity within its owning noun space.
    NounIdentity, u64);
identity!(/// A noun's 31-bit structural mug; collisions require exact comparison.
    NounMug, u32);
identity!(/// Hash bucket for shallow native IR structure.
    NodeHash, u64);
identity!(/// Cached hash of owned jam bytes.
    JamHash, u64);
identity!(/// Address of owned jam bytes retained by the compiler context.
    JamIdentity, usize);
identity!(/// Hoon cache signature, from its AST (with spots) or its boundary noun mug.
    HoonSignature, u64);
identity!(/// Structural signature of an optional core-name prefix.
    PrefixSignature, u32);
identity!(/// Signature of an arm map and its optional name prefix.
    TomesSignature, u64);
identity!(/// Interned set of active or scope-reachable hold legs; zero is empty.
    FanContextId, u64);
identity!(/// Interned structural [subject gene] hold leg.
    FanLegId, u64);
identity!(/// Recursion-sensitive arm generation; zero denotes a reusable steady state.
    ArmEpoch, u64);
identity!(/// Signature of the active placeholder recursion guards.
    PlaceholderSignature, u64);
identity!(/// Compile-local lazy core resolver, serialized at noun boundaries.
    LazyResolverId, u64);
identity!(/// A small atom's numeric value (which need not fit a direct noun).
    AtomValue, u64);
identity!(/// Identity assigned by the structural noun type interner.
    NestNounId, u64);
identity!(/// Address of a borrowed Hoon node; reusable addresses require a signature guard.
    HoonIdentity, usize);
identity!(/// Structural signature of a mold specification.
    SpecSignature, u64);
identity!(/// Identity of the interpreter stack frame owning copied nouns.
    EvalFrameId, u64);
identity!(/// Small Nock axis used to select a core arm.
    SmallAxis, u64);
identity!(/// Numeric Nock opcode at a formula boundary.
    NockOpcode, u8);
identity!(/// Whether type verification is active for a memoized operation.
    VetMode, bool);

impl NockOpcode {
    pub const CELL: Self = Self(3);
    pub const INCREMENT: Self = Self(4);
    pub const EQUAL: Self = Self(5);
    pub const COMPOSE: Self = Self(7);
    pub const PUSH: Self = Self(8);
    pub const SCRY: Self = Self(12);
}

impl HoonIdentity {
    #[inline]
    pub(crate) fn of(hoon: &Hoon) -> Self {
        Self(hoon as *const Hoon as usize)
    }
}

impl NounIdentity {
    #[inline]
    pub(crate) fn of(noun: impl std::borrow::Borrow<Noun>) -> Self {
        // Reading a noun's identity does not dereference it. Callers retain its
        // owning space for the lifetime of the memo containing this word.
        Self(unsafe { noun.borrow().as_raw() })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::mem::{align_of, size_of};

    use super::*;

    #[test]
    fn scalar_wrappers_preserve_layout_and_hashing() {
        fn check<T: Hash, U: Hash>(wrapped: T, raw: U) {
            assert_eq!(size_of::<T>(), size_of::<U>());
            assert_eq!(align_of::<T>(), align_of::<U>());
            let mut a = DefaultHasher::new();
            let mut b = DefaultHasher::new();
            wrapped.hash(&mut a);
            raw.hash(&mut b);
            assert_eq!(a.finish(), b.finish());
        }
        check(NounIdentity(17), 17u64);
        check(NounMug(17), 17u32);
        check(NodeHash(17), 17u64);
        check(JamHash(17), 17u64);
        check(JamIdentity(17), 17usize);
        check(HoonSignature(17), 17u64);
        check(TomesSignature(17), 17u64);
        check(PrefixSignature(17), 17u32);
        check(FanContextId(17), 17u64);
        check(FanLegId(17), 17u64);
        check(ArmEpoch(17), 17u64);
        check(PlaceholderSignature(17), 17u64);
        check(LazyResolverId(17), 17u64);
        check(AtomValue(17), 17u64);
        check(NestNounId(17), 17u64);
        check(HoonIdentity(17), 17usize);
        check(SpecSignature(17), 17u64);
        check(EvalFrameId(17), 17u64);
        check(SmallAxis(17), 17u64);
        check(VetMode(true), true);
        check(NockOpcode::COMPOSE, 7u8);
    }
}

/// Ordered children of an interned cell. The ID domain belongs to its arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CellKey<I> {
    pub head: I,
    pub tail: I,
}
