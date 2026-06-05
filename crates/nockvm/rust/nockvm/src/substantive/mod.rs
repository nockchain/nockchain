pub mod convert;

use std::collections::HashSet;

use ibig::UBig;
use slotmap::{new_key_type, SlotMap};
// use std::fmt;

/// The maximum value of a “direct” atom (63 bits).
pub const DIRECT_MAX: u64 = u64::MAX >> 1;

/// An enum storing the various possible node contents:
///  - Direct(u64): small atoms up to DIRECT_MAX.
///  - Indirect(Vec<u64>): larger atoms represented as one or more u64 words (little-endian).
///  - Cell(NounKey, NounKey): cons cell linking two Nouns (by keys).
#[derive(Debug, PartialEq, Eq)]
pub enum Node {
    Direct(u64),
    Indirect(Vec<u64>),
    Cell(NounKey, NounKey),
}

// A key that references one of the above `Node`s in the `SlotMap`.
new_key_type! { pub struct NounKey; }

/// Errors we might run into when manipulating Nouns.
#[derive(Debug)]
pub enum NounError {
    AxisDescendsThroughAtom,
    NotRepresentable,
    NotCell,
}

impl std::fmt::Display for NounError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NounError::AxisDescendsThroughAtom => {
                write!(f, "Axis tried to descend through an atom")
            }
            NounError::NotRepresentable => write!(f, "Not representable"),
            NounError::NotCell => write!(f, "Not a cell"),
        }
    }
}

impl std::error::Error for NounError {}

pub type Result<T> = std::result::Result<T, NounError>;

/// The arena that owns all Nouns (Node entries) in a SlotMap.
pub struct NounArena {
    nodes: SlotMap<NounKey, Node>,
}

impl Default for NounArena {
    fn default() -> Self {
        Self::new()
    }
}

impl NounArena {
    /// Create a new, empty `NounArena`.
    pub fn new() -> Self {
        NounArena {
            nodes: SlotMap::with_key(), // or just SlotMap::new()
        }
    }

    /// Add a direct (small) atom to the arena, returning its key.
    pub fn add_direct(&mut self, value: u64) -> NounKey {
        self.nodes.insert(Node::Direct(value))
    }

    /// Add an indirect (large) atom to the arena, returning its key.
    pub fn add_indirect(&mut self, data: Vec<u64>) -> NounKey {
        self.nodes.insert(Node::Indirect(data))
    }

    /// Add a cell (head, tail) to the arena, returning its key.
    pub fn add_cell(&mut self, head: NounKey, tail: NounKey) -> NounKey {
        self.nodes.insert(Node::Cell(head, tail))
    }

    /// Look up the underlying `Node` for a given key.
    pub fn get_node(&self, nk: NounKey) -> &Node {
        self.nodes.get(nk).expect("Invalid NounKey")
    }

    /// Determine if the given key refers to an atom (direct or indirect).
    pub fn is_atom(&self, nk: NounKey) -> bool {
        match self.get_node(nk) {
            Node::Direct(_) | Node::Indirect(_) => true,
            Node::Cell(..) => false,
        }
    }

    /// Determine if the given key refers to a cell.
    pub fn is_cell(&self, nk: NounKey) -> bool {
        matches!(self.get_node(nk), Node::Cell(..))
    }

    /// Create a noun from a u64, deciding direct vs. indirect automatically.
    pub fn from_u64(&mut self, value: u64) -> NounKey {
        if value <= DIRECT_MAX {
            self.add_direct(value)
        } else {
            // store as a one-element vector
            self.add_indirect(vec![value])
        }
    }

    // to_le_bytes and new_raw are copies.  We should be able to do this completely without copies
    // if we integrate with ibig properly.
    // pub fn from_ubig<A: NounAllocator>(allocator: &mut A, big: &UBig) -> Atom {
    //     let bit_size = big.bit_len();
    //     let buffer = big.to_le_bytes_stack();
    //     if bit_size < 64 {
    //         let mut value = 0u64;
    //         for i in (0..bit_size).step_by(8) {
    //             value |= (buffer[i / 8] as u64) << i;
    //         }
    //         unsafe { DirectAtom::new_unchecked(value).as_atom() }
    //     } else {
    //         let byte_size = (big.bit_len() + 7) >> 3;
    //         unsafe { IndirectAtom::new_raw_bytes(allocator, byte_size, buffer.as_ptr()).as_atom() }
    //     }
    // }

    /// Create a noun from an `ibig::UBig`.
    /// If it fits into DIRECT_MAX, store as direct; otherwise store in a Vec<u64>.
    pub fn from_ubig(&mut self, big: &UBig) -> NounKey {
        let bit_size = big.bit_len();
        let buffer = big.to_le_bytes_stack();
        if bit_size <= 63 {
            // safely fits in a u64
            let mut value = 0u64;
            for i in (0..bit_size).step_by(8) {
                value |= (buffer[i / 8] as u64) << i;
            }
            self.add_direct(value)
        } else {
            // store as little-endian words
            let bytes_le = big.to_le_bytes();
            let mut words = Vec::with_capacity(bytes_le.len().div_ceil(8));
            for chunk in bytes_le.chunks(8) {
                let mut arr = [0u8; 8];
                arr[..chunk.len()].copy_from_slice(chunk);
                words.push(u64::from_le_bytes(arr));
            }
            // remove trailing zeros if any (but leave at least one word)
            while words.len() > 1
                && *words.last().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                }) == 0
            {
                words.pop();
            }
            self.add_indirect(words)
        }
    }

    /// Retrieve a sub-noun by axis navigation (like `slot(axis)` in Nock).
    ///  - axis 1 -> the noun itself
    ///  - axis 2 -> head of noun
    ///  - axis 3 -> tail of noun
    ///  - etc., interpreting the axis bits from LSB to MSB
    pub fn slot(&self, root: NounKey, axis: u64) -> Result<NounKey> {
        if axis == 0 {
            return Err(NounError::AxisDescendsThroughAtom);
        }
        if axis == 1 {
            // "root" axis
            return Ok(root);
        }

        let mut path = axis;
        let mut current = root;
        // descend
        while path > 1 {
            let last_bit = path & 1;
            path >>= 1;

            match self.get_node(current) {
                Node::Cell(head, tail) => {
                    current = if last_bit == 0 { *head } else { *tail };
                }
                // tried to descend through an atom
                Node::Direct(_) | Node::Indirect(_) => {
                    return Err(NounError::AxisDescendsThroughAtom);
                }
            }
        }
        Ok(current)
    }

    /// Retrieve (head, tail) if this is a cell, else error.
    pub fn as_cell(&self, noun: NounKey) -> Result<(NounKey, NounKey)> {
        match self.get_node(noun) {
            Node::Cell(h, t) => Ok((*h, *t)),
            _ => Err(NounError::NotCell),
        }
    }

    /// Simple check for cycles by pointer equality in the slotmap.
    /// Usually, `SlotMap` cannot form cycles in typical usage unless you store references
    /// back into the same map, but we can do a DFS to confirm for demonstration.
    pub fn is_acyclic(&self, root: NounKey) -> bool {
        let mut visited = HashSet::new();
        self.acyclic_dfs(root, &mut visited)
    }

    fn acyclic_dfs(&self, current: NounKey, visited: &mut HashSet<NounKey>) -> bool {
        if !visited.insert(current) {
            // we've seen this node, so there's a cycle
            return false;
        }
        match self.get_node(current) {
            Node::Cell(h, t) => {
                // check sub-nodes
                if self.acyclic_dfs(*h, visited) && self.acyclic_dfs(*t, visited) {
                    // remove from visited for safe backtracking, or we can keep for global cycle detection
                    visited.remove(&current);
                    true
                } else {
                    false
                }
            }
            Node::Direct(_) | Node::Indirect(_) => {
                // atoms can't form cycles
                visited.remove(&current);
                true
            }
        }
    }
}

/// Convenience function to create a cell in the arena.
pub fn new_cell(arena: &mut NounArena, head: NounKey, tail: NounKey) -> NounKey {
    arena.add_cell(head, tail)
}

/// Convenience function to create a “tape” from a string: a linked list of bytes.
pub fn tape(arena: &mut NounArena, text: &str) -> NounKey {
    // Build from the end
    let mut list = arena.add_direct(0); // empty list
    for &byte in text.as_bytes().iter().rev() {
        let atom = arena.add_direct(byte as u64);
        list = arena.add_cell(atom, list);
    }
    list
}

/// Implement a custom Debug that prints out the entire structure of a noun,
/// for demonstration (not a cycle-safe impl).
pub fn debug_print(arena: &NounArena, key: NounKey) -> String {
    match arena.get_node(key) {
        Node::Direct(x) => format!("{x}"),
        Node::Indirect(words) => {
            if words.is_empty() {
                "0x".to_string()
            } else {
                let mut s = String::from("0x");
                for w in words.iter().rev() {
                    s.push_str(&format!("_{w:016x}"));
                }
                s
            }
        }
        Node::Cell(h, t) => {
            let hd = debug_print(arena, *h);
            let tl = debug_print(arena, *t);
            format!("[{hd} {tl}]")
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // TODO: I ripped ibig::UBig out of this test because it was tripping a memory leak under Miri
    #[test]
    fn test_noun_arena() {
        let mut arena = NounArena::new();

        // Make a small direct atom
        let d1 = arena.from_u64(123);

        // Make a large indirect atom explicitly (simulating what we did with ibig)
        // Using a large number that would be stored indirectly: [0xFFFFFFFFFFFFFFFF, 0x1]
        // This represents 2^128 - 1
        let a2 = arena.add_indirect(vec![0xFFFFFFFFFFFFFFFF, 0x1]);

        // Make a cell
        let cell = arena.add_cell(d1, a2);

        // Print it
        println!("Cell is {:?}", debug_print(&arena, cell));

        // Access sub-part by axis.  Axis=2 => the head
        println!(
            "slot(cell,2) => {:?}",
            debug_print(&arena, arena.slot(cell, 2).expect("Failed to get slot"))
        );
        // Axis=3 => the tail
        println!(
            "slot(cell,3) => {:?}",
            debug_print(
                &arena,
                arena.slot(cell, 3).unwrap_or_else(|err| panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                ))
            )
        );

        // Test that we can actually get the expected values
        let head = arena.slot(cell, 2).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        match arena.get_node(head) {
            Node::Direct(n) => assert_eq!(*n, 123),
            _ => panic!("Expected Direct(123)"),
        }

        let tail = arena.slot(cell, 3).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        match arena.get_node(tail) {
            Node::Indirect(words) => {
                assert_eq!(words.len(), 2);
                assert_eq!(words[0], 0xFFFFFFFFFFFFFFFF);
                assert_eq!(words[1], 0x1);
            }
            _ => panic!("Expected Indirect atom"),
        }

        // Tape example
        let txt = "hello";
        let tape_key = tape(&mut arena, txt);
        println!("Tape for '{}' => {:?}", txt, debug_print(&arena, tape_key));

        // Verify the tape contents
        let mut curr = tape_key;
        let chars: Vec<_> = txt.bytes().collect(); // Get chars in original order
        for &expected_char in chars.iter() {
            match arena.as_cell(curr) {
                Ok((head, tail)) => {
                    // Check the character
                    match arena.get_node(head) {
                        Node::Direct(n) => {
                            assert_eq!(*n as u8, expected_char);
                        }
                        _ => panic!("Expected Direct atom in tape"),
                    }
                    curr = tail;
                }
                Err(_) => panic!("Expected Cell in tape"),
            }
        }
        // Should be at the terminating 0
        match arena.get_node(curr) {
            Node::Direct(n) => assert_eq!(*n, 0),
            _ => panic!("Expected Direct(0) at end of tape"),
        }

        // Cycle check
        println!("Is cell acyclic? {}", arena.is_acyclic(cell));
        assert!(arena.is_acyclic(cell));
    }
}
