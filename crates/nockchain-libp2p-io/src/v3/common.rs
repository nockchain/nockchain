//! Checked wire primitives. No constructor accepts a runtime pointer or JAM.
use std::collections::HashMap;
use std::io;

use nockapp::noun::slab::NounSlab;
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, Noun, NounHandle, D, T};

use super::pb;

pub(crate) const MAX_NODES: usize = 1_000_000;
pub(crate) const PRIME: u64 = nockchain_math::belt::PRIME;

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(crate) fn required<T>(value: Option<T>, field: &str) -> io::Result<T> {
    value.ok_or_else(|| invalid(format!("missing {field}")))
}

pub(crate) fn tuple<'a>(mut noun: NounHandle<'a>, len: usize) -> io::Result<Vec<NounHandle<'a>>> {
    if len == 0 {
        return Err(invalid("empty tuple specification"));
    }
    let mut fields = Vec::with_capacity(len);
    for _ in 1..len {
        let cell = noun.as_cell().map_err(|_| invalid("expected tuple cell"))?;
        fields.push(cell.head());
        noun = cell.tail();
    }
    fields.push(noun);
    Ok(fields)
}

pub(crate) fn list(mut noun: NounHandle<'_>) -> io::Result<Vec<NounHandle<'_>>> {
    let mut items = Vec::new();
    loop {
        if let Ok(atom) = noun.as_atom() {
            if atom.as_u64() == Ok(0) {
                return Ok(items);
            }
            return Err(invalid("list must end with zero"));
        }
        if items.len() == MAX_NODES {
            return Err(invalid("list exceeds node limit"));
        }
        let cell = noun.as_cell().map_err(|_| invalid("expected list cell"))?;
        items.push(cell.head());
        noun = cell.tail();
    }
}

/// Iterate a Hoon treap without accepting nonzero leaf terminators.
pub(crate) fn tree_entries(noun: NounHandle<'_>) -> io::Result<Vec<NounHandle<'_>>> {
    let mut work = vec![(noun, false)];
    let mut entries = Vec::new();
    let mut visited = 0;
    while let Some((node, emit)) = work.pop() {
        if emit {
            entries.push(node);
            continue;
        }
        if let Ok(atom) = node.as_atom() {
            if atom.as_u64() != Ok(0) {
                return Err(invalid("tree must end with zero"));
            }
            continue;
        }
        visited += 1;
        if visited > MAX_NODES {
            return Err(invalid("tree exceeds node limit"));
        }
        let parts = tuple(node, 3)?;
        work.push((parts[2], false));
        work.push((parts[0], true));
        work.push((parts[1], false));
    }
    Ok(entries)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PeerAtom(Vec<u8>);

impl PeerAtom {
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        if bytes.last() == Some(&0) {
            return Err(invalid("atom is not minimally encoded"));
        }
        Ok(Self(bytes))
    }

    pub(crate) fn from_proto(value: pb::Atom) -> io::Result<Self> {
        Self::from_bytes(required(value.little_endian, "atom.little_endian")?)
    }

    pub(crate) fn to_proto(&self) -> pb::Atom {
        pb::Atom {
            little_endian: Some(self.0.clone()),
        }
    }

    pub(crate) fn u64_checked(&self) -> io::Result<u64> {
        if self.0.len() > 8 {
            return Err(invalid("atom exceeds u64"));
        }
        let mut bytes = [0; 8];
        bytes[..self.0.len()].copy_from_slice(&self.0);
        Ok(u64::from_le_bytes(bytes))
    }

    pub(crate) fn from_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let atom = noun.as_atom().map_err(|_| invalid("expected atom"))?;
        // NockVM stores atoms as native-endian u64 limbs, least significant
        // limb first. Convert each limb explicitly to the wire byte order.
        let raw = atom.as_ne_bytes();
        let mut bytes = Vec::with_capacity(raw.len());
        for chunk in raw.chunks_exact(8) {
            let mut limb = [0; 8];
            limb.copy_from_slice(chunk);
            bytes.extend_from_slice(&u64::from_ne_bytes(limb).to_le_bytes());
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        Ok(Self(bytes))
    }

    pub(crate) fn to_noun(&self, slab: &mut NounSlab) -> Noun {
        if self.0.is_empty() {
            return D(0);
        }
        // Atom::from_bytes copies native limb storage. The wire atom uses
        // little-endian limbs, so translate each padded limb explicitly.
        let mut native = Vec::with_capacity(self.0.len());
        for chunk in self.0.chunks(8) {
            let mut limb = [0; 8];
            limb[..chunk.len()].copy_from_slice(chunk);
            native.extend_from_slice(&u64::from_le_bytes(limb).to_ne_bytes());
        }
        Atom::from_bytes(slab, &native).as_noun()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PeerHash([u64; 5]);

impl PeerHash {
    pub(crate) fn new(limbs: [u64; 5]) -> io::Result<Self> {
        if limbs.iter().any(|limb| *limb >= PRIME) {
            return Err(invalid("hash limb is not a Goldilocks field element"));
        }
        Ok(Self(limbs))
    }

    pub(crate) fn from_proto(hash: pb::Hash) -> io::Result<Self> {
        Self::new([
            required(hash.limb0, "hash.limb0")?,
            required(hash.limb1, "hash.limb1")?,
            required(hash.limb2, "hash.limb2")?,
            required(hash.limb3, "hash.limb3")?,
            required(hash.limb4, "hash.limb4")?,
        ])
    }

    pub(crate) fn to_proto(&self) -> pb::Hash {
        pb::Hash {
            limb0: Some(self.0[0]),
            limb1: Some(self.0[1]),
            limb2: Some(self.0[2]),
            limb3: Some(self.0[3]),
            limb4: Some(self.0[4]),
        }
    }

    pub(crate) fn from_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let parts = tuple(noun, 5)?;
        let mut limbs = [0; 5];
        for (limb, part) in limbs.iter_mut().zip(parts) {
            *limb = part
                .as_atom()
                .map_err(|_| invalid("hash limb must be atom"))?
                .as_u64()
                .map_err(|_| invalid("hash limb exceeds u64"))?;
        }
        Self::new(limbs)
    }

    pub(crate) fn to_noun(&self, slab: &mut NounSlab) -> Noun {
        let limbs = self.0.map(|limb| Atom::new(slab, limb).as_noun());
        T(slab, &limbs)
    }

    pub(crate) fn from_base58(value: &str) -> io::Result<Self> {
        let integer = crate::tip5_util::base58_to_ubig(value.to_owned())
            .map_err(|_| invalid("invalid hash base58"))?;
        Self::new(
            crate::tip5_util::decimal_to_base_p(integer)
                .map_err(|_| invalid("hash outside five-limb domain"))?,
        )
    }

    pub(crate) fn to_base58(&self) -> io::Result<String> {
        Ok(crate::tip5_util::ubig_to_base58(
            crate::tip5_util::base_p_to_decimal(self.0).map_err(|_| invalid("invalid hash"))?,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Atom(PeerAtom),
    Cell(usize, usize),
}

/// An immutable, acyclic, fully validated noun representation. The only node
/// references are indices into this owned vector, never runtime addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerNoun(Vec<Node>);

impl PeerNoun {
    pub(crate) fn from_proto(value: pb::NounValue) -> io::Result<Self> {
        if value.nodes.is_empty() || value.nodes.len() > MAX_NODES {
            return Err(invalid("noun node count outside supported range"));
        }
        let mut nodes = Vec::with_capacity(value.nodes.len());
        let mut referenced = vec![false; value.nodes.len()];
        for (index, node) in value.nodes.into_iter().enumerate() {
            match (node.atom, node.head, node.tail) {
                (Some(atom), None, None) => nodes.push(Node::Atom(PeerAtom::from_bytes(atom)?)),
                (None, Some(head), Some(tail)) => {
                    let head = head as usize;
                    let tail = tail as usize;
                    if head >= index || tail >= index {
                        return Err(invalid(
                            "noun children must reference earlier completed nodes",
                        ));
                    }
                    referenced[head] = true;
                    referenced[tail] = true;
                    nodes.push(Node::Cell(head, tail));
                }
                _ => return Err(invalid("noun node must be exactly an atom or cell")),
            }
        }
        if referenced[..referenced.len() - 1].contains(&false) {
            return Err(invalid("noun contains unreachable nodes"));
        }
        Ok(Self(nodes))
    }

    pub(crate) fn to_proto(&self) -> pb::NounValue {
        pb::NounValue {
            nodes: self
                .0
                .iter()
                .map(|node| match node {
                    Node::Atom(atom) => pb::NounNode {
                        atom: Some(atom.0.clone()),
                        head: None,
                        tail: None,
                    },
                    Node::Cell(head, tail) => pb::NounNode {
                        atom: None,
                        head: Some(*head as u32),
                        tail: Some(*tail as u32),
                    },
                })
                .collect(),
        }
    }

    pub(crate) fn validate_based(&self) -> io::Result<()> {
        for node in &self.0 {
            if let Node::Atom(atom) = node {
                if atom.u64_checked()? >= PRIME {
                    return Err(invalid("noun atom is not based"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn from_noun(noun: NounHandle<'_>) -> io::Result<Self> {
        let mut work = vec![(noun, false)];
        let mut complete = HashMap::new();
        let mut nodes = Vec::new();
        while let Some((noun, finish)) = work.pop() {
            // SAFETY: this is identity-only memoization of an already rooted
            // local noun. Raw values are never dereferenced or reconstructed.
            let key = unsafe { noun.noun().as_raw() };
            if complete.contains_key(&key) {
                continue;
            }
            if nodes.len() >= MAX_NODES || work.len() > MAX_NODES * 2 {
                return Err(invalid("noun exceeds node limit"));
            }
            if noun.is_atom() {
                complete.insert(key, nodes.len());
                nodes.push(Node::Atom(PeerAtom::from_noun(noun)?));
            } else {
                let cell = noun.as_cell().map_err(|_| invalid("expected noun cell"))?;
                if finish {
                    // SAFETY: as above, these keys only identify local nodes.
                    let head_key = unsafe { cell.head().noun().as_raw() };
                    let tail_key = unsafe { cell.tail().noun().as_raw() };
                    let head = *complete
                        .get(&head_key)
                        .ok_or_else(|| invalid("unfinished noun head"))?;
                    let tail = *complete
                        .get(&tail_key)
                        .ok_or_else(|| invalid("unfinished noun tail"))?;
                    complete.insert(key, nodes.len());
                    nodes.push(Node::Cell(head, tail));
                } else {
                    work.push((noun, true));
                    work.push((cell.tail(), false));
                    work.push((cell.head(), false));
                }
            }
        }
        Ok(Self(nodes))
    }

    pub(crate) fn to_noun(&self, slab: &mut NounSlab) -> io::Result<Noun> {
        let mut nouns = Vec::with_capacity(self.0.len());
        for node in &self.0 {
            let noun = match node {
                Node::Atom(atom) => atom.to_noun(slab),
                Node::Cell(head, tail) => T(slab, &[nouns[*head], nouns[*tail]]),
            };
            nouns.push(noun);
        }
        nouns.last().copied().ok_or_else(|| invalid("empty noun"))
    }
}

#[cfg(test)]
mod tests {
    use nockvm::noun::{NounAllocator, D};

    use super::*;

    #[test]
    fn arbitrary_atom_preserves_high_bits() {
        let mut slab: NounSlab = NounSlab::new();
        for bytes in [vec![], vec![255; 8], vec![1, 2, 3, 4, 5, 6, 7, 8, 9]] {
            let atom = PeerAtom::from_bytes(bytes).unwrap();
            let noun = atom.to_noun(&mut slab);
            assert_eq!(
                PeerAtom::from_noun(noun.in_space(&slab.noun_space())).unwrap(),
                atom
            );
        }
    }

    #[test]
    fn wire_atom_limbs_have_the_specified_numeric_value() {
        let mut slab: NounSlab = NounSlab::new();
        let atom = PeerAtom::from_bytes(vec![1, 2, 3, 4, 5, 6, 7, 8, 9]).unwrap();
        let noun = atom.to_noun(&mut slab);
        let space = slab.noun_space();
        let value = noun.in_space(&space).as_atom().unwrap();
        let limbs = value
            .as_ne_bytes()
            .chunks_exact(8)
            .map(|chunk| {
                let mut limb = [0; 8];
                limb.copy_from_slice(chunk);
                u64::from_ne_bytes(limb)
            })
            .collect::<Vec<_>>();
        assert_eq!(limbs, vec![0x0807_0605_0403_0201, 9]);
    }

    #[test]
    fn noun_dag_roundtrips_shared_structure() {
        let mut slab: NounSlab = NounSlab::new();
        let pair = T(&mut slab, &[D(42), D(7)]);
        let root = T(&mut slab, &[pair, pair]);
        slab.set_root(root);
        let value = PeerNoun::from_noun(root.in_space(&slab.noun_space())).unwrap();
        let decoded = PeerNoun::from_proto(value.to_proto()).unwrap();
        let mut other: NounSlab = NounSlab::new();
        let root = decoded.to_noun(&mut other).unwrap();
        other.set_root(root);
        assert_eq!(slab.jam(), other.jam());
    }

    #[test]
    fn noun_requires_complete_reachable_nodes_and_canonical_atoms() {
        let atom = pb::NounNode {
            atom: Some(vec![]),
            head: None,
            tail: None,
        };
        assert!(PeerNoun::from_proto(pb::NounValue {
            nodes: vec![atom.clone(), atom.clone()]
        })
        .is_err());
        assert!(PeerNoun::from_proto(pb::NounValue {
            nodes: vec![pb::NounNode {
                atom: None,
                head: Some(0),
                tail: Some(0)
            }]
        })
        .is_err());
        assert!(PeerNoun::from_proto(pb::NounValue {
            nodes: vec![pb::NounNode {
                atom: Some(vec![0]),
                ..atom
            }]
        })
        .is_err());
        assert!(PeerHash::new([PRIME; 5]).is_err());
        assert!(PeerHash::from_proto(pb::Hash::default()).is_err());
    }
}
