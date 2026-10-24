//! Direct conversion of space-resident nockvm nouns into hash-consed nockasm
//! nouns.
//!
//! The cache write path used to bridge representations by jamming the slab
//! noun and cueing the bytes back through nockasm — two full serializations
//! of every product just to change Rust types. This walks the slab noun once,
//! interning every distinct subtree so the produced nockasm nouns carry
//! maximal `Rc` sharing: equal subtrees are pointer-equal, which keeps
//! `nockasm::lift_bundle`'s structural memo on its O(1) `ptr_eq` fast path.
//!
//! The intern tables key parents by their children's intern ids, so structural
//! equality of parents reduces to id-pair equality (children are canonical
//! before any parent is built). No structural comparisons or hashes of whole
//! subtrees ever run; every node costs O(1) map work.

use nockvm::noun::{Noun, NounSpace};

use crate::errors::{CompilerError, Result};
use crate::native::ut::types::FastHashMap;

#[derive(Default)]
pub struct SlabToNockasm {
    /// Canonical noun per intern id. The maps below store bare ids, so every
    /// canonical noun is held exactly once here — the maps stay compact and
    /// carry no `Rc` refcount traffic.
    canon: Vec<nockasm::Noun>,
    /// Raw slab noun bits (allocation offset + tag, or direct-atom value) to
    /// intern id. Sound because slab nouns are immutable and a given raw
    /// value always denotes the same noun within one space.
    slab_memo: FastHashMap<u64, u32>,
    small_atoms: FastHashMap<u64, u32>,
    big_atoms: std::collections::HashMap<
        Box<[u8]>,
        u32,
        std::hash::BuildHasherDefault<crate::native::ut::types::FastHasher>,
    >,
    cells: FastHashMap<(u32, u32), u32>,
}

enum Task {
    Visit(Noun),
    Build { raw: u64 },
}

impl SlabToNockasm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert one root. Interning state persists across calls, so converting
    /// several roots through one instance preserves sharing between them —
    /// matching the sharing the old single-list jam gave `lift_bundle`.
    pub fn convert(&mut self, root: Noun, space: &NounSpace) -> Result<nockasm::Noun> {
        let mut tasks = vec![Task::Visit(root)];
        let mut values: Vec<u32> = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(noun) => {
                    let raw = unsafe { noun.as_raw() };
                    if let Some(&hit) = self.slab_memo.get(&raw) {
                        values.push(hit);
                        continue;
                    }
                    let handle = noun.in_space(space);
                    if let Ok(atom) = handle.as_atom() {
                        let id = match atom.as_u64() {
                            Ok(value) => self.small_atom(value)?,
                            Err(_) => self.wide_atom(atom.as_ne_bytes())?,
                        };
                        self.slab_memo.insert(raw, id);
                        values.push(id);
                        continue;
                    }
                    let cell = handle.as_cell().map_err(|err| {
                        CompilerError::Decode(format!("nasm bridge noun not cell: {err:?}"))
                    })?;
                    let (head, tail) = cell.head_tail();
                    tasks.push(Task::Build { raw });
                    tasks.push(Task::Visit(tail.noun()));
                    tasks.push(Task::Visit(head.noun()));
                }
                Task::Build { raw } => {
                    let tail_id = values
                        .pop()
                        .expect("nasm bridge build frame missing tail value");
                    let head_id = values
                        .pop()
                        .expect("nasm bridge build frame missing head value");
                    let id = match self.cells.get(&(head_id, tail_id)) {
                        Some(&id) => id,
                        None => {
                            let cell = nockasm::Noun::cell(
                                self.canon[head_id as usize].clone(),
                                self.canon[tail_id as usize].clone(),
                            );
                            let id = self.intern(cell)?;
                            self.cells.insert((head_id, tail_id), id);
                            id
                        }
                    };
                    self.slab_memo.insert(raw, id);
                    values.push(id);
                }
            }
        }
        let id = values
            .pop()
            .expect("nasm bridge traversal must produce a root value");
        debug_assert!(values.is_empty());
        Ok(self.canon[id as usize].clone())
    }

    fn intern(&mut self, noun: nockasm::Noun) -> Result<u32> {
        let id = u32::try_from(self.canon.len()).map_err(|_| {
            CompilerError::Decode("nasm bridge exceeded u32 distinct nouns".to_string())
        })?;
        self.canon.push(noun);
        Ok(id)
    }

    fn small_atom(&mut self, value: u64) -> Result<u32> {
        if let Some(&id) = self.small_atoms.get(&value) {
            return Ok(id);
        }
        let id = self.intern(nockasm::Noun::from(value))?;
        self.small_atoms.insert(value, id);
        Ok(id)
    }

    /// Intern an atom given as (possibly zero-padded) little-endian bytes.
    fn wide_atom(&mut self, bytes: &[u8]) -> Result<u32> {
        let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        let significant = &bytes[..end];
        if significant.len() <= 8 {
            let mut buf = [0u8; 8];
            buf[..significant.len()].copy_from_slice(significant);
            return self.small_atom(u64::from_le_bytes(buf));
        }
        if let Some(&id) = self.big_atoms.get(significant) {
            return Ok(id);
        }
        let id = self.intern(nockasm::Noun::from(nockasm::Atom::from_le_bytes(
            significant,
        )))?;
        self.big_atoms.insert(significant.into(), id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::NounSlab;
    use nockvm::ext::AtomExt;
    use nockvm::noun::{Atom, NounAllocator, D, T};

    use super::*;

    /// The old write path proved representation equivalence by construction
    /// (jam → cue). The direct converter must produce nouns that jam to the
    /// same bytes and that lift into the same bundle.
    #[test]
    fn direct_conversion_matches_jam_cue_bridge() {
        let mut slab: NounSlab = NounSlab::new();
        let big = Atom::from_bytes(&mut slab, &[0xab; 19]).as_noun();
        let shared = T(&mut slab, &[D(42), big, D(7)]);
        // Duplicate copies of an equal subtree (distinct allocations) plus
        // genuine pointer sharing, atoms both small and wide.
        let shared_copy = T(&mut slab, &[D(42), big, D(7)]);
        let root = T(&mut slab, &[shared, shared_copy, shared, D(0)]);
        slab.set_root(root);
        let space = slab.noun_space();

        let jammed = slab.jam();
        let via_jam = nockasm::cue(&jammed).expect("cue slab jam");

        let direct = SlabToNockasm::new()
            .convert(root, &space)
            .expect("direct conversion");

        assert_eq!(direct, via_jam);
        assert_eq!(nockasm::jam(&direct), nockasm::jam(&via_jam));

        let lift = |noun: &nockasm::Noun| {
            nockasm::lift_bundle(&[nockasm::DagInput {
                name: "root",
                noun,
                mode: nockasm::DagMode::Noun,
            }])
            .expect("lift")
            .to_bytes()
        };
        assert_eq!(lift(&direct), lift(&via_jam));
    }

    #[test]
    fn cross_root_sharing_is_preserved() {
        let mut slab: NounSlab = NounSlab::new();
        let shared = T(&mut slab, &[D(1), D(2)]);
        let left = T(&mut slab, &[shared, D(3)]);
        let right = T(&mut slab, &[D(4), shared]);
        let root = T(&mut slab, &[left, right]);
        slab.set_root(root);
        let space = slab.noun_space();

        let mut bridge = SlabToNockasm::new();
        let left = bridge.convert(left, &space).expect("left");
        let right = bridge.convert(right, &space).expect("right");
        let bundle = nockasm::lift_bundle(&[
            nockasm::DagInput {
                name: "left",
                noun: &left,
                mode: nockasm::DagMode::Noun,
            },
            nockasm::DagInput {
                name: "right",
                noun: &right,
                mode: nockasm::DagMode::Noun,
            },
        ])
        .expect("lift");
        // [1 2] must appear once: 1, 2, [1 2], 3, [[1 2] 3], 4, [4 [1 2]].
        assert_eq!(bundle.nodes().len(), 7);
    }
}
