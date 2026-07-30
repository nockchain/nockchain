//! Compile-local canonical identity for Nock values used by seminoun analysis.
//!
//! Seminouns repeatedly rebuild structurally equal cores at fresh slab
//! addresses. A raw-pointer cache therefore misses and falls back to a full
//! structural noun comparison. `ValueArena` assigns the same compact ID to
//! equal value trees as they enter or are constructed by the abstract
//! interpreter, so downstream caches can key on exact identity.

use nockvm::noun::{Noun, NounSpace};

use crate::errors::{CompilerError, Result};
use crate::native::noun::slab_mug;
use crate::native::ut::types::FastHashMap;

/// Compile-local structural identity of one Nock value.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ValueId(pub(crate) u32);

#[derive(Clone, Copy, Debug)]
enum ValueKind {
    Atom,
    Cell { head: ValueId, tail: ValueId },
}

#[derive(Clone, Copy, Debug)]
struct ValueEntry {
    noun: Noun,
    kind: ValueKind,
}

/// Hash-consed value nodes plus a source-address import memo.
#[derive(Default)]
pub struct ValueArena {
    entries: Vec<ValueEntry>,
    by_raw: FastHashMap<u64, ValueId>,
    direct_atoms: FastHashMap<u64, ValueId>,
    indirect_atom_buckets: FastHashMap<u32, Vec<ValueId>>,
    cells: FastHashMap<(ValueId, ValueId), ValueId>,
}

impl ValueArena {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn entry(&self, id: ValueId) -> &ValueEntry {
        &self.entries[id.0 as usize]
    }

    #[inline]
    fn raw(noun: Noun) -> u64 {
        unsafe { noun.as_raw() }
    }

    fn push(&mut self, noun: Noun, kind: ValueKind) -> ValueId {
        let id = ValueId(
            u32::try_from(self.entries.len())
                .expect("one Honk compile cannot contain more than u32::MAX value nodes"),
        );
        self.entries.push(ValueEntry { noun, kind });
        self.by_raw.insert(Self::raw(noun), id);
        id
    }

    fn intern_atom(&mut self, noun: Noun, space: &NounSpace) -> Result<ValueId> {
        let raw = Self::raw(noun);
        if let Some(id) = self.by_raw.get(&raw).copied() {
            return Ok(id);
        }
        let atom = noun
            .in_space(space)
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("value atom: {err}")))?;
        if let Ok(value) = atom.as_u64() {
            if let Some(id) = self.direct_atoms.get(&value).copied() {
                self.by_raw.insert(raw, id);
                return Ok(id);
            }
            let id = self.push(noun, ValueKind::Atom);
            self.direct_atoms.insert(value, id);
            return Ok(id);
        }

        let mug = slab_mug(noun, space);
        if let Some(bucket) = self.indirect_atom_buckets.get(&mug) {
            for id in bucket.iter().copied() {
                let prior = self
                    .entry(id)
                    .noun
                    .in_space(space)
                    .as_atom()
                    .map_err(|err| CompilerError::Decode(format!("interned value atom: {err}")))?;
                if atom.eq_bytes(prior.as_ne_bytes()) {
                    self.by_raw.insert(raw, id);
                    return Ok(id);
                }
            }
        }
        let id = self.push(noun, ValueKind::Atom);
        self.indirect_atom_buckets.entry(mug).or_default().push(id);
        Ok(id)
    }

    /// Register a cell whose canonical children are already known.
    pub fn intern_cell_with_noun(&mut self, noun: Noun, head: ValueId, tail: ValueId) -> ValueId {
        let raw = Self::raw(noun);
        if let Some(id) = self.by_raw.get(&raw).copied() {
            return id;
        }
        let key = (head, tail);
        if let Some(id) = self.cells.get(&key).copied() {
            self.by_raw.insert(raw, id);
            return id;
        }
        let id = self.push(noun, ValueKind::Cell { head, tail });
        self.cells.insert(key, id);
        id
    }

    /// Import an arbitrary slab-resident noun iteratively. Every imported
    /// descendant receives a raw-address memo, while structurally equal cells
    /// converge through their already-canonical child IDs.
    pub fn import(&mut self, noun: Noun, space: &NounSpace) -> Result<ValueId> {
        let root_raw = Self::raw(noun);
        if let Some(id) = self.by_raw.get(&root_raw).copied() {
            return Ok(id);
        }

        let mut work = vec![(noun, false)];
        while let Some((current, expanded)) = work.pop() {
            let raw = Self::raw(current);
            if self.by_raw.contains_key(&raw) {
                continue;
            }
            if current.in_space(space).as_atom().is_ok() {
                self.intern_atom(current, space)?;
                continue;
            }
            let cell = current
                .in_space(space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("value cell: {err}")))?;
            let (head, tail) = cell.head_tail();
            let head = head.noun();
            let tail = tail.noun();
            if !expanded {
                work.push((current, true));
                work.push((tail, false));
                work.push((head, false));
                continue;
            }
            let head_id = self
                .by_raw
                .get(&Self::raw(head))
                .copied()
                .expect("expanded value head must be interned");
            let tail_id = self
                .by_raw
                .get(&Self::raw(tail))
                .copied()
                .expect("expanded value tail must be interned");
            self.intern_cell_with_noun(current, head_id, tail_id);
        }
        self.by_raw
            .get(&root_raw)
            .copied()
            .ok_or_else(|| CompilerError::Noun("value import did not register root".into()))
    }

    #[inline]
    pub fn id_for_noun(&self, noun: Noun) -> Option<ValueId> {
        self.by_raw.get(&Self::raw(noun)).copied()
    }

    #[inline]
    pub fn noun(&self, id: ValueId) -> Noun {
        self.entry(id).noun
    }

    #[cfg(test)]
    pub fn children(&self, id: ValueId) -> Option<(ValueId, ValueId)> {
        match self.entry(id).kind {
            ValueKind::Atom => None,
            ValueKind::Cell { head, tail } => Some((head, tail)),
        }
    }
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::{NounAllocator, D, T};

    use super::*;

    #[test]
    fn structurally_equal_cells_share_value_id() {
        let mut slab: NounSlab = NounSlab::new();
        let left = T(&mut slab, &[D(1), D(2), D(3)]);
        let right = T(&mut slab, &[D(1), D(2), D(3)]);
        assert!(!unsafe { left.raw_equals(&right) });

        let mut arena = ValueArena::new();
        let left_id = arena.import(left, &slab.noun_space()).unwrap();
        let right_id = arena.import(right, &slab.noun_space()).unwrap();
        assert_eq!(left_id, right_id);
        assert!(arena.children(left_id).is_some());
    }
}
