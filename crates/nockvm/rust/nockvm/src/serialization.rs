use bitvec::prelude::{BitSlice, Lsb0};
use either::Either::{Left, Right};

use crate::hamt::MutHamt;
use crate::interpreter::Error::{self, *};
use crate::interpreter::Mote::*;
use crate::mem::NockStack;
use crate::noun::{Atom, Cell, DirectAtom, IndirectAtom, Noun, D};

crate::gdb!();

/// Calculate the number of bits needed to represent an atom
pub fn met0_usize(atom: Atom) -> usize {
    let atom_bitslice = atom.as_bitslice();
    atom_bitslice.last_one().map_or(0, |last_one| last_one + 1)
}

/// Calculate the number of bits needed to represent a u64 as a usize
pub fn met0_u64_to_usize(x: u64) -> usize {
    let usize_bitslice = BitSlice::<u64, Lsb0>::from_element(&x);
    usize_bitslice.last_one().map_or(0, |last_one| last_one + 1)
}

/// Read the next bit from the bitslice and advance the cursor
pub fn next_bit(cursor: &mut usize, slice: &BitSlice<u64, Lsb0>) -> bool {
    if (*slice).len() > *cursor {
        let res = slice[*cursor];
        *cursor += 1;
        res
    } else {
        false
    }
}

/// Reads the next up to n bits from the bitslice and advance the cursor
pub fn next_up_to_n_bits<'a>(
    cursor: &mut usize,
    slice: &'a BitSlice<u64, Lsb0>,
    n: usize,
) -> &'a BitSlice<u64, Lsb0> {
    let res = if (slice).len() >= *cursor + n {
        &slice[*cursor..*cursor + n]
    } else if slice.len() > *cursor {
        &slice[*cursor..]
    } else {
        BitSlice::<u64, Lsb0>::empty()
    };
    *cursor += n;
    res
}

/// Get the remaining bits from the cursor position
pub fn rest_bits(cursor: usize, slice: &BitSlice<u64, Lsb0>) -> &BitSlice<u64, Lsb0> {
    if slice.len() > cursor {
        &slice[cursor..]
    } else {
        BitSlice::<u64, Lsb0>::empty()
    }
}

#[derive(Copy, Clone)]
enum CueStackEntry {
    DestinationPointer(*mut Noun),
    BackRef(u64, *const Noun),
}

/// Deserialize a noun from a BitSlice
///
/// This function implements the inverse of jam, unpacking a serialized noun.
///
/// Corresponds to `++cue` in the hoon stdlib, but uses a stack-based approach instead of recursion:
///
/// ```hoon
/// ++  cue                                                 ::  unpack
///   ~/  %cue
///   |=  a=@
///   ^-  *
///   =+  b=0
///   =+  m=`(map @ *)`~
///   =<  q
///   |-  ^-  [p=@ q=* r=(map @ *)]
///   ?:  =(0 (cut 0 [b 1] a))
///     =+  c=(rub +(b) a)
///     [+(p.c) q.c (~(put by m) b q.c)]
///   =+  c=(add 2 b)
///   ?:  =(0 (cut 0 [+(b) 1] a))
///     =+  u=$(b c)
///     =+  v=$(b (add p.u c), m r.u)
///     =+  w=[q.u q.v]
///     [(add 2 (add p.u p.v)) w (~(put by r.v) b w)]
///   =+  d=(rub c a)
///   [(add 2 p.d) (need (~(get by m) q.d)) m]
/// ```
///
/// The deserialization process works as follows:
/// - 0 bit: Indicates an atom follows
/// - 10 bits: Indicates a cell follows
/// - 11 bits: Indicates a backreference follows
///
/// # Arguments
/// * `stack` - A mutable reference to the NockStack
/// * `buffer` - A reference to a BitSlice containing the serialized noun
///
/// # Returns
/// A Result containing either the deserialized Noun or an Error
pub fn cue_bitslice(stack: &mut NockStack, buffer: &BitSlice<u64, Lsb0>) -> Result<Noun, Error> {
    let backref_map = MutHamt::<Noun>::new(stack);
    let mut result = D(0);
    let mut cursor = 0;

    unsafe {
        stack.with_frame(0, |stack: &mut NockStack| {
            *(stack.push::<CueStackEntry>()) =
                CueStackEntry::DestinationPointer(&mut result as *mut Noun);
            loop {
                if stack.stack_is_empty() {
                    break Ok(result);
                }
                let stack_entry = *stack.top::<CueStackEntry>();
                stack.pop::<CueStackEntry>();
                // Capture the destination pointer and pop it off the stack
                match stack_entry {
                    CueStackEntry::DestinationPointer(dest_ptr) => {
                        // 1 bit
                        if next_bit(&mut cursor, buffer) {
                            // 11 tag: backref
                            if next_bit(&mut cursor, buffer) {
                                let mut backref_noun =
                                    Atom::new(stack, rub_backref(&mut cursor, buffer)?).as_noun();
                                *dest_ptr = backref_map
                                    .lookup(stack, &mut backref_noun)
                                    .ok_or(Deterministic(Exit, D(0)))?;
                            } else {
                                // 10 tag: cell
                                let (cell, cell_mem_ptr) = Cell::new_raw_mut(stack);
                                *dest_ptr = cell.as_noun();
                                let mut backref_atom =
                                    Atom::new(stack, (cursor - 2) as u64).as_noun();
                                backref_map.insert(stack, &mut backref_atom, *dest_ptr);
                                *(stack.push()) = CueStackEntry::BackRef(
                                    cursor as u64 - 2,
                                    dest_ptr as *const Noun,
                                );
                                *(stack.push()) =
                                    CueStackEntry::DestinationPointer(&mut (*cell_mem_ptr).tail);
                                *(stack.push()) =
                                    CueStackEntry::DestinationPointer(&mut (*cell_mem_ptr).head);
                            }
                        } else {
                            // 0 tag: atom
                            let backref: u64 = (cursor - 1) as u64;
                            *dest_ptr = rub_atom(stack, &mut cursor, buffer)?.as_noun();
                            let mut backref_atom = Atom::new(stack, backref).as_noun();
                            backref_map.insert(stack, &mut backref_atom, *dest_ptr);
                        }
                    }
                    CueStackEntry::BackRef(backref, noun_ptr) => {
                        let mut backref_atom = Atom::new(stack, backref).as_noun();
                        backref_map.insert(stack, &mut backref_atom, *noun_ptr)
                    }
                }
            }
        })
    }
}

/// Deserialize a noun from an Atom
///
/// This function is a wrapper around cue_bitslice that takes an Atom as input.
///
/// # Arguments
/// * `stack` - A mutable reference to the NockStack
/// * `buffer` - An Atom containing the serialized noun
///
/// # Returns
/// A Result containing either the deserialized Noun or an Error
pub fn cue(stack: &mut NockStack, buffer: Atom) -> Result<Noun, Error> {
    let buffer_bitslice = buffer.as_bitslice();
    cue_bitslice(stack, buffer_bitslice)
}

/// Get the size in bits of an encoded atom or backref
/// TODO: use first_zero() on a slice of the buffer
fn get_size(cursor: &mut usize, buffer: &BitSlice<u64, Lsb0>) -> Result<usize, Error> {
    let buff_at_cursor = rest_bits(*cursor, buffer);
    let bitsize = buff_at_cursor
        .first_one()
        .ok_or(Deterministic(Exit, D(0)))?;
    if bitsize == 0 {
        *cursor += 1;
        Ok(0)
    } else {
        let mut size: u64 = 0;
        *cursor += bitsize + 1;
        let size_bits = next_up_to_n_bits(cursor, buffer, bitsize - 1);
        BitSlice::from_element_mut(&mut size)[0..bitsize - 1].copy_from_bitslice(size_bits);
        Ok((size as usize) + (1 << (bitsize - 1)))
    }
}

/// Length-decode an atom from the buffer
///
/// Corresponds to `++rub` in the hoon stdlib.
///
/// ```hoon
/// ++  rub                                                 ::  length-decode
///   ~/  %rub
///   |=  [a=@ b=@]
///   ^-  [p=@ q=@]
///   =+  ^=  c
///       =+  [c=0 m=(met 0 b)]
///       |-  ?<  (gth c m)
///       ?.  =(0 (cut 0 [(add a c) 1] b))
///         c
///       $(c +(c))
///   ?:  =(0 c)
///     [1 0]
///   =+  d=(add a +(c))
///   =+  e=(add (bex (dec c)) (cut 0 [d (dec c)] b))
///   [(add (add c c) e) (cut 0 [(add d (dec c)) e] b)]
/// ```
fn rub_atom(
    stack: &mut NockStack,
    cursor: &mut usize,
    buffer: &BitSlice<u64, Lsb0>,
) -> Result<Atom, Error> {
    let size = get_size(cursor, buffer)?;
    let bits = next_up_to_n_bits(cursor, buffer, size);
    if size == 0 {
        unsafe { Ok(DirectAtom::new_unchecked(0).as_atom()) }
    } else if size < 64 {
        // Fits in a direct atom
        let mut direct_raw = 0;
        BitSlice::from_element_mut(&mut direct_raw)[0..bits.len()].copy_from_bitslice(bits);
        unsafe { Ok(DirectAtom::new_unchecked(direct_raw).as_atom()) }
    } else {
        // Need an indirect atom
        let wordsize = (size + 63) >> 6;
        let (mut atom, slice) = unsafe { IndirectAtom::new_raw_mut_bitslice(stack, wordsize) };
        slice[0..bits.len()].copy_from_bitslice(bits);
        debug_assert!(atom.size() > 0);
        unsafe { Ok(atom.normalize_as_atom()) }
    }
}

/// Deserialize a backreference from the buffer
fn rub_backref(cursor: &mut usize, buffer: &BitSlice<u64, Lsb0>) -> Result<u64, Error> {
    // TODO: What's size here usually?
    let size = get_size(cursor, buffer)?;
    if size == 0 {
        Ok(0)
    } else if size <= 64 {
        // TODO: Size <= 64, so we can fit the backref in a direct atom?
        let mut backref: u64 = 0;
        BitSlice::from_element_mut(&mut backref)[0..size]
            .copy_from_bitslice(&buffer[*cursor..*cursor + size]);
        *cursor += size;
        Ok(backref)
    } else {
        Err(NonDeterministic(Fail, D(0)))
    }
}

struct JamState<'a> {
    cursor: usize,
    size: usize,
    atom: IndirectAtom,
    slice: &'a mut BitSlice<u64, Lsb0>,
}

/// Serialize a noun into an atom
///
/// Corresponds to ++jam in the hoon stdlib.
///
/// Implements a compact encoding scheme for nouns, with backreferences for shared structures.
pub fn jam(stack: &mut NockStack, noun: Noun) -> Atom {
    let backref_map = MutHamt::new(stack);
    let size = 8;
    let (atom, slice) = unsafe { IndirectAtom::new_raw_mut_bitslice(stack, size) };
    let mut state = JamState {
        cursor: 0,
        size,
        atom,
        slice,
    };
    stack.frame_push(0);
    unsafe {
        *(stack.push::<Noun>()) = noun;
    };
    'jam: loop {
        if stack.stack_is_empty() {
            break;
        } else {
            let mut noun = unsafe { *(stack.top::<Noun>()) };
            if let Some(backref) = backref_map.lookup(stack, &mut noun) {
                match noun.as_either_atom_cell() {
                    Left(atom) => {
                        let atom_size = met0_usize(atom);
                        let backref_size = met0_u64_to_usize(backref);
                        if atom_size <= backref_size {
                            jam_atom(stack, &mut state, atom);
                        } else {
                            jam_backref(stack, &mut state, backref);
                        }
                    }
                    Right(_cell) => {
                        jam_backref(stack, &mut state, backref);
                    }
                }
                unsafe {
                    stack.pop::<Noun>();
                };
                continue 'jam;
            };
            backref_map.insert(stack, &mut noun, state.cursor as u64);
            match noun.as_either_atom_cell() {
                Left(atom) => {
                    jam_atom(stack, &mut state, atom);
                    unsafe {
                        stack.pop::<Noun>();
                    };
                    continue;
                }
                Right(cell) => {
                    jam_cell(stack, &mut state);
                    unsafe {
                        stack.pop::<Noun>();
                        *(stack.push::<Noun>()) = cell.tail();
                        *(stack.push::<Noun>()) = cell.head();
                    };
                    continue;
                }
            }
        }
    }
    unsafe {
        let mut result = state.atom.normalize_as_atom();
        stack.preserve(&mut result);
        stack.frame_pop();
        result
    }
}

/// Serialize an atom into the jam state
fn jam_atom(traversal: &mut NockStack, state: &mut JamState, atom: Atom) {
    loop {
        if state.cursor + 1 > state.slice.len() {
            double_atom_size(traversal, state);
        } else {
            break;
        }
    }
    state.slice.set(state.cursor, false); // 0 tag for atom
    state.cursor += 1;
    loop {
        if let Ok(()) = mat(traversal, state, atom) {
            break;
        } else {
            double_atom_size(traversal, state);
        }
    }
}

/// Serialize a cell into the jam state
fn jam_cell(traversal: &mut NockStack, state: &mut JamState) {
    loop {
        if state.cursor + 2 > state.slice.len() {
            double_atom_size(traversal, state);
        } else {
            break;
        }
    }
    state.slice.set(state.cursor, true); // 1 bit
    state.slice.set(state.cursor + 1, false); // 0 bit, forming 10 tag for cell
    state.cursor += 2;
}

/// Serialize a backreference into the jam state
fn jam_backref(traversal: &mut NockStack, state: &mut JamState, backref: u64) {
    loop {
        if state.cursor + 2 > state.slice.len() {
            double_atom_size(traversal, state);
        } else {
            break;
        }
    }
    state.slice.set(state.cursor, true); // 1 bit
    state.slice.set(state.cursor + 1, true); // 1 bit, forming 11 tag for backref
    state.cursor += 2;
    let backref_atom = Atom::new(traversal, backref);
    loop {
        if let Ok(()) = mat(traversal, state, backref_atom) {
            break;
        } else {
            double_atom_size(traversal, state);
        }
    }
}

/// Double the size of the atom in the jam state
fn double_atom_size(traversal: &mut NockStack, state: &mut JamState) {
    let new_size = state.size + state.size;
    let (new_atom, new_slice) = unsafe { IndirectAtom::new_raw_mut_bitslice(traversal, new_size) };
    new_slice[0..state.cursor].copy_from_bitslice(&state.slice[0..state.cursor]);
    state.size = new_size;
    state.atom = new_atom;
    state.slice = new_slice;
}

/// Encode an atom's size and value into the jam state
///
/// INVARIANT: mat must not modify state.cursor unless it will also return `Ok(())`
fn mat(traversal: &mut NockStack, state: &mut JamState, atom: Atom) -> Result<(), ()> {
    let b_atom_size = met0_usize(atom);
    let b_atom_size_atom = Atom::new(traversal, b_atom_size as u64);
    if b_atom_size == 0 {
        if state.cursor + 1 > state.slice.len() {
            Err(())
        } else {
            state.slice.set(state.cursor, true);
            state.cursor += 1;
            Ok(())
        }
    } else {
        let c_b_size = met0_usize(b_atom_size_atom);
        if state.cursor + c_b_size + c_b_size + b_atom_size > state.slice.len() {
            Err(())
        } else {
            state.slice[state.cursor..state.cursor + c_b_size].fill(false); // a 0 bit for each bit in the atom size
            state.slice.set(state.cursor + c_b_size, true); // a terminating 1 bit
            state.slice[state.cursor + c_b_size + 1..state.cursor + c_b_size + c_b_size]
                .copy_from_bitslice(&b_atom_size_atom.as_bitslice()[0..c_b_size - 1]); // the atom size excepting the most significant 1 (since we know where that is from the size-of-the-size)
            state.slice[state.cursor + c_b_size + c_b_size
                ..state.cursor + c_b_size + c_b_size + b_atom_size]
                .copy_from_bitslice(&atom.as_bitslice()[0..b_atom_size]);
            state.cursor += c_b_size + c_b_size + b_atom_size;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {

    use std::mem::size_of;

    use rand::prelude::*;

    use super::*;
    use crate::jets::util::test::assert_noun_eq;
    use crate::mem::NockStack;
    use crate::noun::{Atom, Cell, CellMemory, Noun};
    fn setup_stack() -> NockStack {
        NockStack::new(1 << 30, 0)
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_atom() {
        let mut stack = setup_stack();
        let atom = Atom::new(&mut stack, 42);
        let jammed = jam(&mut stack, atom.as_noun());
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_noun_eq(&mut stack, cued, atom.as_noun());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_cell() {
        let mut stack = setup_stack();
        let n1 = Atom::new(&mut stack, 1).as_noun();
        let n2 = Atom::new(&mut stack, 2).as_noun();
        let cell = Cell::new(&mut stack, n1, n2).as_noun();
        let jammed = jam(&mut stack, cell);
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_noun_eq(&mut stack, cued, cell);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_nested_cell() {
        let mut stack = setup_stack();
        let n3 = Atom::new(&mut stack, 3).as_noun();
        let n4 = Atom::new(&mut stack, 4).as_noun();
        let inner_cell = Cell::new(&mut stack, n3, n4);
        let n1 = Atom::new(&mut stack, 1).as_noun();
        let outer_cell = Cell::new(&mut stack, n1, inner_cell.as_noun());
        let jammed = jam(&mut stack, outer_cell.as_noun());
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_noun_eq(&mut stack, cued, outer_cell.as_noun());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_shared_structure() {
        let mut stack = setup_stack();
        let shared_atom = Atom::new(&mut stack, 42);
        let cell = Cell::new(&mut stack, shared_atom.as_noun(), shared_atom.as_noun());
        let jammed = jam(&mut stack, cell.as_noun());
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_noun_eq(&mut stack, cued, cell.as_noun());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_large_atom() {
        let mut stack = setup_stack();
        let large_atom = Atom::new(&mut stack, u64::MAX);
        let jammed = jam(&mut stack, large_atom.as_noun());
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_noun_eq(&mut stack, cued, large_atom.as_noun());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_empty_atom() {
        let mut stack = setup_stack();
        let empty_atom = Atom::new(&mut stack, 0);
        let jammed = jam(&mut stack, empty_atom.as_noun());
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_noun_eq(&mut stack, cued, empty_atom.as_noun());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_complex_structure() {
        let mut stack = setup_stack();
        let atom1 = Atom::new(&mut stack, 1);
        let atom2 = Atom::new(&mut stack, 2);
        let cell1 = Cell::new(&mut stack, atom1.as_noun(), atom2.as_noun());
        let cell2 = Cell::new(&mut stack, cell1.as_noun(), atom2.as_noun());
        let cell3 = Cell::new(&mut stack, cell2.as_noun(), cell1.as_noun());
        let jammed = jam(&mut stack, cell3.as_noun());
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_noun_eq(&mut stack, cued, cell3.as_noun());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_cue_invalid_input() {
        let mut stack = setup_stack();
        let invalid_atom = Atom::new(&mut stack, 0b11); // Invalid tag
        let result = cue(&mut stack, invalid_atom);
        assert!(result.is_err());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_cue_roundtrip_property() {
        let rng = StdRng::seed_from_u64(1);
        let depth = 9;
        println!("Testing noun with depth: {}", depth);

        let mut stack = setup_stack();
        let mut rng_clone = rng.clone();
        let (original, total_size) = generate_deeply_nested_noun(&mut stack, depth, &mut rng_clone);

        println!(
            "Total size of all generated nouns: {:.2} KB",
            total_size as f64 / 1024.0
        );
        println!("Original size: {:.2} KB", original.mass() as f64 / 1024.0);
        let jammed = jam(&mut stack, original);
        println!(
            "Jammed size: {:.2} KB",
            jammed.as_noun().mass() as f64 / 1024.0
        );
        let cued = cue(&mut stack, jammed).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        println!("Cued size: {:.2} KB", cued.mass() as f64 / 1024.0);

        assert_noun_eq(&mut stack, cued, original);
    }

    // FIXME: why is bits unused?
    #[allow(clippy::only_used_in_recursion)]
    fn generate_random_noun(stack: &mut NockStack, bits: usize, rng: &mut StdRng) -> (Noun, usize) {
        const MAX_DEPTH: usize = 100; // Adjust this value as needed
        fn inner(
            stack: &mut NockStack,
            bits: usize,
            rng: &mut StdRng,
            depth: usize,
            accumulated_size: usize,
        ) -> (Noun, usize) {
            let mut done = false;
            if depth >= MAX_DEPTH || stack.size() < 1024 || accumulated_size > stack.size() - 1024 {
                // println!("Done at depth and size: {} {:.2} KB", depth, accumulated_size as f64 / 1024.0);
                done = true;
            }

            let result = if rng.random_bool(0.5) || done {
                let value = rng.random::<u64>();
                let atom = Atom::new(stack, value);
                let noun = atom.as_noun();
                (noun, accumulated_size + noun.mass())
            } else {
                let (left, left_size) = inner(stack, bits / 2, rng, depth + 1, accumulated_size);
                let (right, _) = inner(stack, bits / 2, rng, depth + 1, left_size);

                let cell = Cell::new(stack, left, right);
                let noun = cell.as_noun();
                (noun, noun.mass())
            };

            if space_needed_noun(result.0, stack) > stack.size() {
                eprintln!(
                    "Stack size exceeded with noun size {:.2} KB",
                    result.0.mass() as f64 / 1024.0
                );
                unsafe {
                    let top_noun = *stack.top::<Noun>();
                    (top_noun, result.1)
                }
            } else {
                result
            }
        }

        inner(stack, bits, rng, 0, 0)
    }

    fn space_needed_noun(noun: Noun, stack: &mut NockStack) -> usize {
        unsafe {
            stack.with_frame(0, |stack| {
                *(stack.push::<Noun>()) = noun;
                let mut size = 0;
                while !stack.stack_is_empty() {
                    let noun = *(stack.top::<Noun>());
                    stack.pop::<Noun>();
                    match noun.as_either_atom_cell() {
                        Left(atom) => match atom.as_either() {
                            Left(_) => {}
                            Right(indirect) => {
                                size += indirect.raw_size();
                            }
                        },
                        Right(cell) => {
                            size += size_of::<CellMemory>();
                            *(stack.push::<Noun>()) = cell.tail();
                            *(stack.push::<Noun>()) = cell.head();
                        }
                    }
                }
                size
            })
        }
    }

    fn generate_deeply_nested_noun(
        stack: &mut NockStack,
        depth: usize,
        rng: &mut StdRng,
    ) -> (Noun, usize) {
        if depth == 0 {
            let (noun, size) = generate_random_noun(stack, 100, rng);
            (noun, size)
        } else {
            let (left, left_size) = generate_deeply_nested_noun(stack, depth - 1, rng);
            let (right, right_size) = generate_deeply_nested_noun(stack, depth - 1, rng);
            let cell = Cell::new(stack, left, right);
            let noun = cell.as_noun();
            let total_size = left_size + right_size + noun.mass();

            if { space_needed_noun(noun, stack) } > stack.size() {
                eprintln!(
                    "Stack size exceeded at depth {} with noun size {:.2} KB",
                    depth,
                    noun.mass() as f64 / 1024.0
                );
                unsafe {
                    let top_noun = *stack.top::<Noun>();
                    (top_noun, total_size)
                }
            } else {
                // println!("Size: {:.2} KB, depth: {}", noun.mass() as f64 / 1024.0, depth);
                (noun, total_size)
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_cue_invalid_backreference() {
        std::env::set_var("RUST_BACKTRACE", "full");

        let mut stack = setup_stack();
        let invalid_atom = Atom::new(&mut stack, 0b11); // Invalid atom representation
        let result = cue(&mut stack, invalid_atom);

        assert!(result.is_err());
        if let Err(e) = result {
            println!("Error: {:?}", e);
            assert!(matches!(e, Error::Deterministic(_, _)));
        }
    }

    #[ignore] // We will put this back when we have proper error catching
    #[test]
    fn test_cue_nondeterministic_error() {
        let mut big_stack = NockStack::new(1 << 30, 0);

        let mut rng = StdRng::seed_from_u64(1);

        // Create an atom with a very large value to potentially cause overflow
        let (large_atom, _) = generate_deeply_nested_noun(&mut big_stack, 5, &mut rng);

        // Attempt to jam and then cue the large atom in the big stack
        let jammed = jam(&mut big_stack, large_atom);

        // make a smaller stack to try to cause a nondeterministic error
        // NOTE: if the stack is big enough to fit the jammed atom, cue panics
        let mut stack = NockStack::new(jammed.as_noun().mass() / 2_usize, 0);

        // Attempt to cue the jammed noun with limited stack space
        let result: Result<(), Error> = match cue(&mut stack, jammed) {
            Ok(_res) => {
                panic!("Unexpected success: cue operation did not fail");
            }
            Err(e) => Err(e),
        };

        // Check if we got a nondeterministic error
        println!("Result: {:?}", result);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e, Error::NonDeterministic(_, _)));
            println!("got expected error: {:?}", e);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_cell_construction() {
        let mut stack = setup_stack();
        let (cell, cell_mem_ptr) = unsafe { Cell::new_raw_mut(&mut stack) };
        unsafe { assert!(cell_mem_ptr as *const CellMemory == cell.to_raw_pointer()) };
    }

    #[test]
    fn test_cyclic_structure() {
        use bitvec::prelude::*;

        let mut stack = NockStack::new(1000, 0);

        // Create a jammed representation of a cyclic structure
        // [0 *] where * refers back to the entire cell, i.e. 0b11110001
        let mut jammed = BitVec::<u64, Lsb0>::new();
        jammed.extend_from_bitslice(bits![u64, Lsb0; 1, 1, 1]); // Backref to the entire structure
        jammed.extend_from_bitslice(bits![u64, Lsb0; 1, 0, 0]); // Atom 0
        jammed.extend_from_bitslice(bits![u64, Lsb0; 0, 1]); // Cell

        let bitslice = jammed.as_bitslice();

        let result = cue_bitslice(&mut stack, bitslice);
        assert!(
            result.is_err(),
            "Expected error due to cyclic structure, but cue_bitslice completed successfully"
        );
    }
}
