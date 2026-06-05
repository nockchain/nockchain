use super::*;
use crate::noun::{
    Atom, Cell, IndirectAtom, Noun, NounAllocator, DIRECT_MASK, DIRECT_TAG, INDIRECT_TAG,
};

pub trait IntoArena {
    fn into_arena(self, arena: &mut NounArena) -> NounKey;
}

pub trait FromArena {
    fn from_arena(arena: &NounArena, key: NounKey, alloc: &mut impl NounAllocator) -> Self;
}

impl IntoArena for Noun {
    fn into_arena(self, arena: &mut NounArena) -> NounKey {
        // Use raw value to check type since we need to preserve tag bits
        unsafe {
            if self.raw & DIRECT_MASK == DIRECT_TAG {
                // Direct atom
                arena.add_direct(self.raw) // Keep raw value with tag bits
            } else if self.raw & !(u64::MAX >> 2) == INDIRECT_TAG {
                // Indirect atom
                let indirect = self.indirect().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                let size = indirect.size();
                let mut words = Vec::with_capacity(size);
                let src = indirect.data_pointer();
                for i in 0..size {
                    words.push(*src.add(i));
                }
                arena.add_indirect(words)
            } else {
                // Cell
                let cell = self.cell().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                let head = cell.head().into_arena(arena);
                let tail = cell.tail().into_arena(arena);
                arena.add_cell(head, tail)
            }
        }
    }
}

impl IntoArena for Atom {
    fn into_arena(self, arena: &mut NounArena) -> NounKey {
        unsafe {
            if self.raw & DIRECT_MASK == DIRECT_TAG {
                // Preserve raw value including tag bits
                arena.add_direct(self.raw)
            } else {
                // Indirect atom
                let indirect = self.indirect().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                let size = indirect.size();
                let mut words = Vec::with_capacity(size);
                let src = indirect.data_pointer();
                for i in 0..size {
                    words.push(*src.add(i));
                }
                arena.add_indirect(words)
            }
        }
    }
}

impl FromArena for Noun {
    fn from_arena(arena: &NounArena, key: NounKey, alloc: &mut impl NounAllocator) -> Self {
        match arena.get_node(key) {
            Node::Direct(value) => {
                // Direct atoms come with their tag bits preserved
                Noun { raw: *value }
            }
            Node::Indirect(words) => {
                // Need to create a proper indirect atom with correct tag
                unsafe { IndirectAtom::new_raw(alloc, words.len(), words.as_ptr()).as_noun() }
            }
            Node::Cell(head, tail) => {
                // Create cell with proper tag bits
                let head_noun = Noun::from_arena(arena, *head, alloc);
                let tail_noun = Noun::from_arena(arena, *tail, alloc);
                Cell::new(alloc, head_noun, tail_noun).as_noun()
            }
        }
    }
}

impl FromArena for Atom {
    fn from_arena(arena: &NounArena, key: NounKey, alloc: &mut impl NounAllocator) -> Self {
        match arena.get_node(key) {
            Node::Direct(value) => {
                // Direct atoms preserve their tag bits
                Atom { raw: *value }
            }
            Node::Indirect(words) => unsafe {
                IndirectAtom::new_raw(alloc, words.len(), words.as_ptr()).as_atom()
            },
            Node::Cell(..) => panic!("Cannot convert Cell to Atom"),
        }
    }
}

impl NounArena {
    pub fn import_noun(&mut self, noun: Noun) -> NounKey {
        noun.into_arena(self)
    }

    pub fn export_noun(&self, key: NounKey, alloc: &mut impl NounAllocator) -> Noun {
        Noun::from_arena(self, key, alloc)
    }

    pub fn import_atom(&mut self, atom: Atom) -> NounKey {
        atom.into_arena(self)
    }

    pub fn export_atom(&self, key: NounKey, alloc: &mut impl NounAllocator) -> Result<Atom> {
        match self.get_node(key) {
            Node::Direct(_) | Node::Indirect(_) => Ok(Atom::from_arena(self, key, alloc)),
            Node::Cell(..) => Err(NounError::NotCell),
        }
    }
}

unsafe fn noun_semantically_equal(a: &Noun, b: &Noun) -> bool {
    // If they're exactly equal (including pointer bits), great
    if a.raw_equals(b) {
        return true;
    }

    // For direct atoms, only value matters
    if a.is_direct() && b.is_direct() {
        return a
            .direct()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .data()
            == b.direct()
                .unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                })
                .data();
    }

    // For indirect atoms, compare content
    if a.is_indirect() && b.is_indirect() {
        let a_ind = a.indirect().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let b_ind = b.indirect().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        if a_ind.size() != b_ind.size() {
            return false;
        }
        let a_slice = a_ind.as_slice();
        let b_slice = b_ind.as_slice();
        return a_slice == b_slice;
    }

    // For cells, recursively compare head and tail
    if a.is_cell() && b.is_cell() {
        let a_cell = a.cell().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let b_cell = b.cell().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        return noun_semantically_equal(&a_cell.head(), &b_cell.head())
            && noun_semantically_equal(&a_cell.tail(), &b_cell.tail());
    }

    false
}

#[cfg(test)]
mod tests {
    use either::Either;

    use super::*;
    use crate::mem::NockStack;
    use crate::noun::DirectAtom;

    fn print_noun_debug(noun: &Noun) -> String {
        match noun.as_either_atom_cell() {
            Either::Left(atom) => match atom.as_either() {
                Either::Left(direct) => format!("Direct({:#x})", direct.data()),
                Either::Right(indirect) => {
                    let mut s = String::from("Indirect[");
                    unsafe {
                        for i in 0..indirect.size() {
                            if i > 0 {
                                s.push_str(", ");
                            }
                            s.push_str(&format!("{:#x}", *indirect.data_pointer().add(i)));
                        }
                    }
                    s.push(']');
                    s
                }
            },
            Either::Right(cell) => {
                format!(
                    "[{} {}]",
                    print_noun_debug(&cell.head()),
                    print_noun_debug(&cell.tail())
                )
            }
        }
    }

    fn print_raw_debug(noun: &Noun) -> String {
        format!("raw: {:#x}", unsafe { noun.raw })
    }

    #[test]
    fn test_direct_atom_conversion() {
        let mut arena = NounArena::new();
        let mut stack = NockStack::new(1024, 100);

        let small = unsafe { DirectAtom::new_unchecked(42).as_noun() };
        println!(
            "Original: {} ({})",
            print_noun_debug(&small),
            print_raw_debug(&small)
        );

        let key = small.into_arena(&mut arena);
        println!("Arena node: {:?}", arena.get_node(key));

        let roundtrip = Noun::from_arena(&arena, key, &mut stack);
        println!(
            "Roundtrip: {} ({})",
            print_noun_debug(&roundtrip),
            print_raw_debug(&roundtrip)
        );

        assert!(unsafe { noun_semantically_equal(&small, &roundtrip) });
    }

    #[test]
    fn test_indirect_atom_conversion() {
        let mut arena = NounArena::new();
        let mut stack = NockStack::new(1024, 100);

        let large_value = DIRECT_MAX + 1;
        let large = unsafe { IndirectAtom::new_raw(&mut stack, 1, &large_value).as_noun() };
        println!(
            "Original: {} ({})",
            print_noun_debug(&large),
            print_raw_debug(&large)
        );

        let key = large.into_arena(&mut arena);
        println!("Arena node: {:?}", arena.get_node(key));

        let roundtrip = Noun::from_arena(&arena, key, &mut stack);
        println!(
            "Roundtrip: {} ({})",
            print_noun_debug(&roundtrip),
            print_raw_debug(&roundtrip)
        );

        assert!(unsafe { noun_semantically_equal(&large, &roundtrip) });
    }

    #[test]
    fn test_cell_conversion() {
        let mut arena = NounArena::new();
        let mut stack = NockStack::new(1024, 100);

        let small = unsafe { DirectAtom::new_unchecked(42).as_noun() };
        let large_value = DIRECT_MAX + 1;
        let large = unsafe { IndirectAtom::new_raw(&mut stack, 1, &large_value).as_noun() };
        let cell = Cell::new(&mut stack, small, large).as_noun();
        println!(
            "Original: {} ({})",
            print_noun_debug(&cell),
            print_raw_debug(&cell)
        );

        let key = cell.into_arena(&mut arena);
        println!("Arena node: {:?}", arena.get_node(key));

        let roundtrip = Noun::from_arena(&arena, key, &mut stack);
        println!(
            "Roundtrip: {} ({})",
            print_noun_debug(&roundtrip),
            print_raw_debug(&roundtrip)
        );

        assert!(unsafe { noun_semantically_equal(&cell, &roundtrip) });
    }

    #[test]
    fn test_complex_structure() {
        let mut arena = NounArena::new();
        let mut stack = NockStack::new(1024, 100);

        // Create [1 [2 3]]
        let n1 = unsafe { DirectAtom::new_unchecked(1).as_noun() };
        let n2 = unsafe { DirectAtom::new_unchecked(2).as_noun() };
        let n3 = unsafe { DirectAtom::new_unchecked(3).as_noun() };
        let inner = Cell::new(&mut stack, n2, n3).as_noun();
        let outer = Cell::new(&mut stack, n1, inner).as_noun();
        println!(
            "Original: {} ({})",
            print_noun_debug(&outer),
            print_raw_debug(&outer)
        );

        let key = outer.into_arena(&mut arena);
        println!("Arena structure:");
        println!("Root: {:?}", arena.get_node(key));

        let (head, tail) = arena.as_cell(key).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        println!("Head: {:?}", arena.get_node(head));
        println!("Tail: {:?}", arena.get_node(tail));

        if let Node::Cell(inner_head, inner_tail) = arena.get_node(tail) {
            println!("Inner head: {:?}", arena.get_node(*inner_head));
            println!("Inner tail: {:?}", arena.get_node(*inner_tail));
        }

        let roundtrip = Noun::from_arena(&arena, key, &mut stack);
        println!(
            "Roundtrip: {} ({})",
            print_noun_debug(&roundtrip),
            print_raw_debug(&roundtrip)
        );

        assert!(unsafe { noun_semantically_equal(&outer, &roundtrip) });
    }
}
