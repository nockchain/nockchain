use std::fmt;

use bytes::Bytes;
use either::Either;
use habit::{BitReader, BitWriter};
use intmap::IntMap;
use nockapp::noun::slab::{CueError as SlabCueError, Jammer as SlabJammer, NounSlab};
use nockvm::ext::{noun_equality, AtomExt};
use nockvm::mug::{calc_atom_mug_u32, calc_cell_mug_u32, get_mug, set_mug};
use nockvm::noun::{Atom, Cell, CellMemory, DirectAtom, Noun, NounAllocator, NounSpace, D};
use nockvm::serialization::{met0_u64_to_usize, met0_usize};

pub struct Chaff;

const MAX_USIZE_BITS: usize = usize::BITS as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueError {
    BadBackref,
    BackrefTooBig,
    TruncatedBuffer,
}

impl fmt::Display for CueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CueError::BadBackref => f.write_str("cue: Bad backref"),
            CueError::BackrefTooBig => f.write_str("cue: backref too big"),
            CueError::TruncatedBuffer => f.write_str("cue: truncated buffer"),
        }
    }
}

impl std::error::Error for CueError {}

struct NounMap<V>(IntMap<u64, Vec<(Noun, V)>>);

impl<V> NounMap<V> {
    fn new() -> Self {
        NounMap(IntMap::new())
    }

    fn insert(&mut self, key: Noun, value: V, space: &NounSpace) {
        let key_mug = noun_mug(key, space) as u64;
        if let Some(vec) = self.0.get_mut(key_mug) {
            let mut chain_iter = vec[..].iter_mut();
            if let Some(entry) =
                chain_iter.find(|entry| noun_equality(key.in_space(space), entry.0.in_space(space)))
            {
                entry.1 = value;
            } else {
                vec.push((key, value));
            }
        } else {
            self.0.insert(key_mug, vec![(key, value)]);
        }
    }

    fn get(&self, key: Noun, space: &NounSpace) -> Option<&V> {
        let key_mug = noun_mug(key, space) as u64;
        self.0.get(key_mug).and_then(|vec| {
            vec.iter()
                .find(|entry| noun_equality(key.in_space(space), entry.0.in_space(space)))
                .map(|entry| &entry.1)
        })
    }
}

fn noun_mug(a: Noun, space: &NounSpace) -> u32 {
    let mut stack = vec![a];
    while let Some(noun) = stack.pop() {
        if let Ok(mut allocated) = noun.as_allocated() {
            if get_mug(noun, space).is_none() {
                match allocated.as_either() {
                    Either::Left(indirect) => unsafe {
                        set_mug(
                            &mut allocated,
                            calc_atom_mug_u32(indirect.as_atom(), space),
                            space,
                        );
                    },
                    Either::Right(cell) => match (
                        get_mug(cell.in_space(space).head().noun(), space),
                        get_mug(cell.in_space(space).tail().noun(), space),
                    ) {
                        (Some(head_mug), Some(tail_mug)) => unsafe {
                            set_mug(
                                &mut allocated,
                                calc_cell_mug_u32(head_mug, tail_mug, space),
                                space,
                            );
                        },
                        _ => {
                            stack.push(noun);
                            stack.push(cell.in_space(space).tail().noun());
                            stack.push(cell.in_space(space).head().noun());
                        }
                    },
                }
            }
        }
    }
    get_mug(a, space).expect("Noun should have a mug once mugged.")
}

impl Chaff {
    pub fn cue_into<A: NounAllocator>(allocator: &mut A, bytes: Bytes) -> Result<Noun, CueError> {
        fn rub_backref(reader: &mut BitReader) -> Result<usize, CueError> {
            let zeros = reader.read_unary().ok_or(CueError::TruncatedBuffer)?;
            if zeros == 0 {
                return Ok(0);
            }
            if zeros > MAX_USIZE_BITS {
                return Err(CueError::BackrefTooBig);
            }
            let size_low = if zeros > 1 {
                reader
                    .read_bits_to_usize(zeros - 1)
                    .ok_or(CueError::TruncatedBuffer)?
            } else {
                0
            };
            let bit_count = (1usize << (zeros - 1)) | size_low;
            if bit_count > MAX_USIZE_BITS {
                return Err(CueError::BackrefTooBig);
            }
            reader
                .read_bits_to_usize(bit_count)
                .ok_or(CueError::TruncatedBuffer)
        }

        fn rub_atom<A: NounAllocator>(
            allocator: &mut A,
            reader: &mut BitReader,
        ) -> Result<Atom, CueError> {
            let zeros = reader.read_unary().ok_or(CueError::TruncatedBuffer)?;
            if zeros == 0 {
                return unsafe { Ok(DirectAtom::new_unchecked(0).as_atom()) };
            }
            if zeros > MAX_USIZE_BITS {
                return Err(CueError::TruncatedBuffer);
            }
            let size_low = if zeros > 1 {
                reader
                    .read_bits_to_usize(zeros - 1)
                    .ok_or(CueError::TruncatedBuffer)?
            } else {
                0
            };
            let bit_count = (1usize << (zeros - 1)) | size_low;
            if bit_count < 64 {
                let value = reader
                    .read_bits_to_usize(bit_count)
                    .ok_or(CueError::TruncatedBuffer)? as u64;
                unsafe { Ok(DirectAtom::new_unchecked(value).as_atom()) }
            } else {
                let byte_len = (bit_count + 7) >> 3;
                let mut buffer = vec![0u8; byte_len];
                reader
                    .read_bits_to_bytes(&mut buffer, bit_count)
                    .ok_or(CueError::TruncatedBuffer)?;
                Ok(<Atom as AtomExt>::from_bytes(allocator, &buffer))
            }
        }

        enum CueStackEntry {
            DestinationPointer(*mut Noun),
            BackRef(u64, *const Noun),
        }

        let mut reader = BitReader::new(bytes);
        let mut result = D(0);
        let mut stack = vec![CueStackEntry::DestinationPointer(&mut result)];
        let mut backrefs: IntMap<u64, Noun> = IntMap::new();

        while let Some(entry) = stack.pop() {
            match entry {
                CueStackEntry::DestinationPointer(dest) => {
                    let backref_pos = reader.position() as u64;
                    let tag = reader.read_bit().ok_or(CueError::TruncatedBuffer)?;
                    if !tag {
                        let atom = rub_atom(allocator, &mut reader)?;
                        unsafe {
                            *dest = atom.as_noun();
                        }
                        backrefs.insert(backref_pos, unsafe { *dest });
                    } else {
                        let second = reader.read_bit().ok_or(CueError::TruncatedBuffer)?;
                        if second {
                            let backref = rub_backref(&mut reader)? as u64;
                            let noun =
                                backrefs.get(backref).copied().ok_or(CueError::BadBackref)?;
                            unsafe {
                                *dest = noun;
                            }
                        } else {
                            let (cell, cell_mem): (Cell, *mut CellMemory) =
                                unsafe { Cell::new_raw_mut(allocator) };
                            unsafe {
                                *dest = cell.as_noun();
                            }
                            stack.push(CueStackEntry::BackRef(backref_pos, dest as *const Noun));
                            unsafe {
                                stack
                                    .push(CueStackEntry::DestinationPointer(&mut (*cell_mem).tail));
                                stack
                                    .push(CueStackEntry::DestinationPointer(&mut (*cell_mem).head));
                            }
                        }
                    }
                }
                CueStackEntry::BackRef(pos, noun_ptr) => {
                    backrefs.insert(pos, unsafe { *noun_ptr });
                }
            }
        }

        Ok(result)
    }

    pub fn jam(noun: Noun, space: &NounSpace) -> Bytes {
        fn mat_backref_fast(writer: &mut BitWriter, backref: usize) {
            if backref == 0 {
                writer.write_bits_from_value(0b111, 3);
                return;
            }
            let backref_sz = met0_u64_to_usize(backref as u64);
            let backref_sz_sz = met0_u64_to_usize(backref_sz as u64);
            writer.write_bit(true);
            writer.write_bit(true);
            writer.write_zeros(backref_sz_sz);
            writer.write_bit(true);
            writer.write_bits_from_value(backref_sz, backref_sz_sz - 1);
            writer.write_bits_from_value(backref, backref_sz);
        }

        fn mat_atom_fast(writer: &mut BitWriter, atom: Atom, space: &NounSpace) {
            unsafe {
                if atom.as_noun().raw_equals(&D(0)) {
                    writer.write_bits_from_value(0b10, 2);
                    return;
                }
            }
            let atom_sz = met0_usize(atom, space);
            let atom_sz_sz = met0_u64_to_usize(atom_sz as u64);
            writer.write_bit(false);
            writer.write_zeros(atom_sz_sz);
            writer.write_bit(true);
            writer.write_bits_from_value(atom_sz, atom_sz_sz - 1);
            writer.write_bits_from_le_bytes(atom.in_space(space).as_ne_bytes(), atom_sz);
        }

        let mut writer = BitWriter::new();
        let mut backref_map = NounMap::<usize>::new();
        let mut stack = vec![noun];
        while let Some(noun) = stack.pop() {
            if let Some(backref) = backref_map.get(noun, space) {
                if let Ok(atom) = noun.in_space(space).as_atom() {
                    let atom = atom.atom();
                    if met0_u64_to_usize(*backref as u64) < met0_usize(atom, space) {
                        mat_backref_fast(&mut writer, *backref);
                    } else {
                        mat_atom_fast(&mut writer, atom, space);
                    }
                } else {
                    mat_backref_fast(&mut writer, *backref);
                }
            } else {
                backref_map.insert(noun, writer.bit_len(), space);
                match noun.in_space(space).as_either_atom_cell() {
                    Either::Left(atom) => {
                        mat_atom_fast(&mut writer, atom.atom(), space);
                    }
                    Either::Right(cell) => {
                        writer.write_bit(true);
                        writer.write_bit(false);
                        stack.push(cell.tail().noun());
                        stack.push(cell.head().noun());
                    }
                }
            }
        }

        writer.into_bytes()
    }
}

impl SlabJammer for Chaff {
    fn jam(noun: Noun, space: &NounSpace) -> Bytes {
        Chaff::jam(noun, space)
    }

    fn cue(slab: &mut NounSlab<Self>, bytes: Bytes) -> Result<Noun, SlabCueError> {
        let noun = Chaff::cue_into(slab, bytes).map_err(|err| match err {
            CueError::BadBackref => SlabCueError::BadBackref,
            CueError::BackrefTooBig => SlabCueError::BackrefTooBig,
            CueError::TruncatedBuffer => SlabCueError::TruncatedBuffer,
        })?;
        slab.set_root(noun);
        Ok(noun)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use habit::BitWriter;
    use nockvm::ext::AtomExt;
    use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use nockvm::noun::{Atom, Cell, Noun, D, T};
    use quickcheck::{Arbitrary, Gen, TestResult};

    use crate::{Chaff, CueError};

    const TOP_SLOTS: usize = 1 << 10;

    fn fresh_stack() -> NockStack {
        NockStack::new(NOCK_STACK_SIZE_TINY, TOP_SLOTS)
    }

    fn atom_from_bytes(stack: &mut NockStack, bytes: &[u8]) -> Atom {
        <Atom as AtomExt>::from_bytes(stack, bytes)
    }

    fn build_shared_noun(stack: &mut NockStack) -> Noun {
        let shared = D(42);
        let cell = Cell::new(stack, shared, shared).as_noun();
        T(stack, &[shared, cell, shared])
    }

    fn build_list(stack: &mut NockStack, leaves: &[u16]) -> Noun {
        let mut list = D(0);
        for value in leaves.iter().rev() {
            list = Cell::new(stack, D(*value as u64), list).as_noun();
        }
        list
    }

    fn roundtrip(noun: Noun, stack: &mut NockStack) {
        let jammed = Chaff::jam(noun, &stack.noun_space());
        let mut cue_stack = fresh_stack();
        let cued = Chaff::cue_into(&mut cue_stack, jammed.clone()).expect("cue should succeed");
        let rejam = Chaff::jam(cued, &cue_stack.noun_space());
        assert_eq!(jammed, rejam);
    }

    #[test]
    fn chaff_roundtrip_direct_atom() {
        let mut stack = fresh_stack();
        roundtrip(D(0), &mut stack);
    }

    #[test]
    fn chaff_roundtrip_indirect_atom() {
        let mut stack = fresh_stack();
        let bytes = vec![0xFFu8; 32];
        let atom = atom_from_bytes(&mut stack, &bytes);
        roundtrip(atom.as_noun(), &mut stack);
    }

    #[test]
    fn chaff_roundtrip_nested_cells() {
        let mut stack = fresh_stack();
        let noun = T(&mut stack, &[D(1), D(2), D(3), D(4)]);
        roundtrip(noun, &mut stack);
    }

    #[test]
    fn chaff_handles_shared_backrefs() {
        let mut stack = fresh_stack();
        let noun = build_shared_noun(&mut stack);
        roundtrip(noun, &mut stack);
    }

    #[test]
    fn chaff_rejects_truncated_input() {
        let mut stack = fresh_stack();
        let jammed = Bytes::from_static(&[0b1]);
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[test]
    fn chaff_rejects_bad_backref() {
        let mut stack = fresh_stack();
        let jammed = Bytes::from_static(&[0b0000_0111]);
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_backref_before_definition() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(true);
        writer.write_bit(true);
        writer.write_zeros(1);
        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);
        let jammed = writer.into_bytes();
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_backref_far_ahead() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(true);
        writer.write_bit(true);
        writer.write_zeros(3);
        writer.write_bit(true);
        writer.write_bits_from_value(0b11, 2);
        writer.write_bits_from_value(0b1010, 4);
        let jammed = writer.into_bytes();
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_self_referential_backref() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(true);
        writer.write_bit(true);
        writer.write_zeros(1);
        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(false);
        let jammed = writer.into_bytes();
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_backref_too_big() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(true);
        writer.write_bit(true);
        writer.write_zeros(usize::BITS as usize + 1);
        writer.write_bit(true);
        let jammed = writer.into_bytes();
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::BackrefTooBig)));
    }

    #[test]
    fn chaff_rejects_truncated_backref_size() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(true);
        writer.write_bit(true);
        writer.write_zeros(2);
        writer.write_bit(true);
        writer.write_bits_from_value(0b01, 2);
        let jammed = writer.into_bytes();
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[test]
    fn chaff_rejects_atom_missing_size_bits() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(false);
        writer.write_bit(false);
        writer.write_bit(false);
        let jammed = writer.into_bytes();
        assert!(matches!(
            Chaff::cue_into(&mut stack, jammed),
            Err(CueError::TruncatedBuffer)
        ));
    }

    #[test]
    fn chaff_rejects_truncated_indirect_atom() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(false);
        writer.write_zeros(6);
        writer.write_bit(true);
        writer.write_bits_from_value(0b100000, 6);
        writer.write_bits_from_value(0xFFFF, 16);
        let jammed = writer.into_bytes();
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[test]
    fn chaff_rejects_zero_atom_bad_encoding() {
        let mut stack = fresh_stack();
        let mut writer = BitWriter::new();
        writer.write_bit(false);
        writer.write_bit(false);
        writer.write_bit(false);
        let jammed = writer.into_bytes();
        let result = Chaff::cue_into(&mut stack, jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[derive(Clone, Debug)]
    struct SmallNoun {
        leaves: Vec<u16>,
    }

    impl Arbitrary for SmallNoun {
        fn arbitrary(g: &mut Gen) -> Self {
            let len = 1 + (usize::arbitrary(g) % 8);
            let mut leaves = Vec::with_capacity(len);
            for _ in 0..len {
                leaves.push(u16::arbitrary(g));
            }
            Self { leaves }
        }
    }

    #[test]
    fn chaff_roundtrip_list_fixture() {
        let mut stack = fresh_stack();
        let noun = build_list(&mut stack, &[1, 2, 3, 4, 5]);
        roundtrip(noun, &mut stack);
    }

    #[test]
    fn chaff_roundtrip_larger_atom_fixture() {
        let mut stack = fresh_stack();
        let bytes = (0u8..64).collect::<Vec<_>>();
        let atom = atom_from_bytes(&mut stack, &bytes);
        let noun = T(&mut stack, &[D(7), atom.as_noun(), D(9)]);
        roundtrip(noun, &mut stack);
    }

    quickcheck::quickcheck! {
        fn prop_chaff_roundtrip(small: SmallNoun) -> TestResult {
            let mut stack = fresh_stack();
            let noun = build_list(&mut stack, &small.leaves);
            let jammed = Chaff::jam(noun, &stack.noun_space());
            let mut cue_stack = fresh_stack();
            let cued = match Chaff::cue_into(&mut cue_stack, jammed.clone()) {
                Ok(noun) => noun,
                Err(_) => return TestResult::failed(),
            };
            let rejam = Chaff::jam(cued, &cue_stack.noun_space());
            TestResult::from_bool(jammed == rejam)
        }
    }

    quickcheck::quickcheck! {
        fn prop_chaff_handles_small_atoms(values: Vec<u64>) -> TestResult {
            let mut values = values;
            values.truncate(8);
            let mut stack = fresh_stack();
            let mut list = D(0);
            for value in values.iter().rev() {
                let bounded = value & (nockvm::noun::DIRECT_MAX >> 1);
                list = Cell::new(&mut stack, D(bounded), list).as_noun();
            }
            let jammed = Chaff::jam(list, &stack.noun_space());
            let mut cue_stack = fresh_stack();
            let cued = match Chaff::cue_into(&mut cue_stack, jammed.clone()) {
                Ok(noun) => noun,
                Err(_) => return TestResult::failed(),
            };
            let rejam = Chaff::jam(cued, &cue_stack.noun_space());
            TestResult::from_bool(jammed == rejam)
        }
    }
}
