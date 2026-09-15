//! Native, compile-local representation of Honk's seminoun lattice.
//!
//! The Hoon definition represents every abstract value as a noun pair whose
//! mask is itself a recursive noun tree. Musk used to rebuild and immediately
//! decode those trees at every Nock step. This arena preserves the same four
//! semantic states behind compact IDs and hash-conses them for exact memo keys.

use num_bigint::BigUint;

use crate::native::ir::value_dag::ValueId;
use crate::native::ut::types::FastHashMap;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct SemiId(pub(crate) u32);

#[derive(Clone, Debug)]
pub(crate) enum SemiNode {
    Complete(ValueId),
    Blocked,
    Half { head: SemiId, tail: SemiId },
    Lazy { fragment: BigUint, resolver_id: u64 },
}

#[derive(Default)]
pub(crate) struct SemiArena {
    entries: Vec<SemiNode>,
    complete: FastHashMap<ValueId, SemiId>,
    blocked: Option<SemiId>,
    halves: FastHashMap<(SemiId, SemiId), SemiId>,
    lazy: FastHashMap<(BigUint, u64), SemiId>,
    by_raw: FastHashMap<u64, SemiId>,
}

impl SemiArena {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, node: SemiNode) -> SemiId {
        let id = SemiId(
            u32::try_from(self.entries.len())
                .expect("one Honk compile cannot contain more than u32::MAX seminoun nodes"),
        );
        self.entries.push(node);
        id
    }

    pub(crate) fn node(&self, id: SemiId) -> &SemiNode {
        &self.entries[id.0 as usize]
    }

    pub(crate) fn complete(&mut self, value: ValueId) -> SemiId {
        if let Some(id) = self.complete.get(&value).copied() {
            return id;
        }
        let id = self.push(SemiNode::Complete(value));
        self.complete.insert(value, id);
        id
    }

    pub(crate) fn blocked(&mut self) -> SemiId {
        if let Some(id) = self.blocked {
            return id;
        }
        let id = self.push(SemiNode::Blocked);
        self.blocked = Some(id);
        id
    }

    pub(crate) fn half(&mut self, head: SemiId, tail: SemiId) -> SemiId {
        let key = (head, tail);
        if let Some(id) = self.halves.get(&key).copied() {
            return id;
        }
        let id = self.push(SemiNode::Half { head, tail });
        self.halves.insert(key, id);
        id
    }

    pub(crate) fn lazy(&mut self, fragment: BigUint, resolver_id: u64) -> SemiId {
        let key = (fragment, resolver_id);
        if let Some(id) = self.lazy.get(&key).copied() {
            return id;
        }
        let id = self.push(SemiNode::Lazy {
            fragment: key.0.clone(),
            resolver_id,
        });
        self.lazy.insert(key, id);
        id
    }

    pub(crate) fn raw_lookup(&self, raw: u64) -> Option<SemiId> {
        self.by_raw.get(&raw).copied()
    }

    pub(crate) fn raw_register(&mut self, raw: u64, id: SemiId) {
        self.by_raw.insert(raw, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantically_equal_nodes_share_identity() {
        let mut arena = SemiArena::new();
        let value = ValueId(7);
        let complete = arena.complete(value);
        assert_eq!(complete, arena.complete(value));
        let blocked = arena.blocked();
        assert_eq!(blocked, arena.blocked());
        assert_eq!(arena.half(complete, blocked), arena.half(complete, blocked));
        assert_eq!(
            arena.lazy(BigUint::from(42u32), 9),
            arena.lazy(BigUint::from(42u32), 9)
        );
    }
}
