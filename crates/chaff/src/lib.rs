use bytes::Bytes;
use either::Either;
use habit::{BitReader, BitWriter};
use intmap::IntMap;
use nockapp::noun::slab::{CueError, Jammer, NounMap, NounSlab};
use nockvm::noun::{Atom, Cell, CellMemory, DirectAtom, IndirectAtom, Noun, D};
use nockvm::serialization::{met0_u64_to_usize, met0_usize};

pub struct Chaff;

const MAX_USIZE_BITS: usize = usize::BITS as usize;

impl Jammer for Chaff {
    fn cue(slab: &mut NounSlab<Self>, bytes: Bytes) -> Result<Noun, CueError> {
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

        fn rub_atom(slab: &mut NounSlab<Chaff>, reader: &mut BitReader) -> Result<Atom, CueError> {
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
                if reader.bits_remaining() < bit_count {
                    return Err(CueError::TruncatedBuffer);
                }
                let byte_len = bit_count.checked_add(7).ok_or(CueError::TruncatedBuffer)? >> 3;
                let (mut atom, buffer) = unsafe { IndirectAtom::new_raw_mut_bytes(slab, byte_len) };
                reader
                    .read_bits_to_bytes(buffer, bit_count)
                    .ok_or(CueError::TruncatedBuffer)?;
                unsafe { Ok(atom.normalize_as_atom()) }
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
                        let atom = rub_atom(slab, &mut reader)?;
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
                                unsafe { Cell::new_raw_mut(slab) };
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

        slab.set_root(result);
        Ok(result)
    }

    fn jam(noun: Noun) -> Bytes {
        fn mat_backref_fast(writer: &mut BitWriter, backref: usize) {
            if backref == 0 {
                writer.write_bits_from_value(0b111, 3); // 1 1 1
                return;
            }
            let backref_sz = met0_u64_to_usize(backref as u64);
            let backref_sz_sz = met0_u64_to_usize(backref_sz as u64);
            // backref tag 1 1
            writer.write_bit(true);
            writer.write_bit(true);
            // write backref_sz_sz zeros
            writer.write_zeros(backref_sz_sz);
            // delimiter 1
            writer.write_bit(true);
            // write backref_sz_sz-1 bits of backref_sz (LSB first)
            writer.write_bits_from_value(backref_sz, backref_sz_sz - 1);
            // write backref bits (backref_sz bits)
            writer.write_bits_from_value(backref, backref_sz);
        }

        fn mat_atom_fast(writer: &mut BitWriter, atom: Atom) {
            unsafe {
                if atom.as_noun().raw_equals(&D(0)) {
                    writer.write_bits_from_value(0b10, 2); // 0 1
                    return;
                }
            }
            let atom_sz = met0_usize(atom);
            let atom_sz_sz = met0_u64_to_usize(atom_sz as u64);
            writer.write_bit(false); // atom tag 0
            writer.write_zeros(atom_sz_sz); // size zeros
            writer.write_bit(true); // delimiter
                                    // write size bits (atom_sz_sz - 1)
            writer.write_bits_from_value(atom_sz, atom_sz_sz - 1);
            // write atom bits (little-endian order)
            writer.write_bits_from_le_bytes(atom.as_ne_bytes(), atom_sz);
        }

        // Main jam implementation ----------------------------------------
        let mut writer = BitWriter::new();
        let mut backref_map = NounMap::<usize>::new();
        let mut stack = vec![noun];
        while let Some(noun) = stack.pop() {
            if let Some(backref) = backref_map.get(noun) {
                // already seen this noun
                if let Ok(atom) = noun.as_atom() {
                    if met0_u64_to_usize(*backref as u64) < met0_usize(atom) {
                        mat_backref_fast(&mut writer, *backref);
                    } else {
                        mat_atom_fast(&mut writer, atom);
                    }
                } else {
                    mat_backref_fast(&mut writer, *backref);
                }
            } else {
                backref_map.insert(noun, writer.bit_len());
                match noun.as_either_atom_cell() {
                    Either::Left(atom) => {
                        mat_atom_fast(&mut writer, atom);
                    }
                    Either::Right(cell) => {
                        // cell tag 1 0
                        writer.write_bit(true);
                        writer.write_bit(false);
                        // push tail then head (LIFO stack)
                        stack.push(cell.tail());
                        stack.push(cell.head());
                    }
                }
            }
        }

        writer.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use bincode::config::{self, Configuration};
    use bincode::Decode;
    use blake3::hash;
    use bytes::Bytes;
    use habit::BitWriter;
    use nockapp::nockapp::save::{
        JammedCheckpointV0, JammedCheckpointV1, JammedCheckpointV2, SaveableCheckpoint, Saver,
        JAM_MAGIC_BYTES,
    };
    use nockapp::noun::slab::{slab_noun_equality, CueError, Jammer, NockJammer, NounSlab};
    use nockapp::noun::NounAllocatorExt;
    use nockapp::utils::NOCK_STACK_SIZE_TINY;
    use nockapp::JammedNoun;
    use nockvm::ext::{AtomExt, NounExt};
    use nockvm::mem::NockStack;
    use nockvm::noun::{Atom, Cell, Noun, D, T};
    use quickcheck::{Arbitrary, Gen, TestResult};
    use tempfile::TempDir;

    use super::Chaff;

    const NOCKVM_STACK_WORDS_MIN: usize = 1 << 20;
    const NOCKVM_STACK_WORDS_JAM_SCALE: usize = 8;
    const NOCKVM_STACK_WORDS_MASS_SCALE: usize = 4;

    fn atom_from_bytes(slab: &mut NounSlab<Chaff>, bytes: &[u8]) -> Atom {
        <Atom as AtomExt>::from_bytes(slab, bytes)
    }

    fn atom_with_exact_bit_size(slab: &mut NounSlab<Chaff>, bits: usize) -> Atom {
        assert!(bits > 0, "atom bit size must be positive");
        let byte_len = (bits + 7) >> 3;
        let mut bytes = vec![0u8; byte_len];
        for (idx, byte) in bytes
            .iter_mut()
            .enumerate()
            .take(byte_len.saturating_sub(1))
        {
            *byte = ((idx as u8).wrapping_mul(37)).wrapping_add(0x5A);
        }
        let high_bit = (bits - 1) & 7;
        bytes[byte_len - 1] |= 1u8 << high_bit;
        atom_from_bytes(slab, &bytes)
    }

    fn build_backref_threshold_fixture(
        slab: &mut NounSlab<Chaff>,
        prefix_len: usize,
        repeated: Atom,
    ) -> Noun {
        let mut list = D(0);
        list = Cell::new(slab, repeated.as_noun(), list).as_noun();
        list = Cell::new(slab, repeated.as_noun(), list).as_noun();
        for idx in (0..prefix_len).rev() {
            list = Cell::new(slab, D(10_000 + idx as u64), list).as_noun();
        }
        list
    }

    fn build_shared_noun(slab: &mut NounSlab<Chaff>) -> Noun {
        let shared = D(42);
        let cell = Cell::new(slab, shared, shared).as_noun();
        T(slab, &[shared, cell, shared])
    }

    #[test]
    fn chaff_roundtrip_direct_atom() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = D(0);
        slab.set_root(noun);
        let jammed = slab.jam();
        let mut cued_slab: NounSlab<Chaff> = NounSlab::new();
        let cued = cued_slab.cue_into(jammed).expect("cue should succeed");
        assert!(slab_noun_equality(unsafe { slab.root() }, &cued));
    }

    #[test]
    fn chaff_roundtrip_indirect_atom() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let bytes = vec![0xFFu8; 32];
        let atom = atom_from_bytes(&mut slab, &bytes);
        slab.set_root(atom.as_noun());
        let jammed = slab.jam();
        let mut cued_slab: NounSlab<Chaff> = NounSlab::new();
        let cued = cued_slab.cue_into(jammed).expect("cue should succeed");
        assert!(slab_noun_equality(unsafe { slab.root() }, &cued));
    }

    #[test]
    fn chaff_roundtrip_nested_cells() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = T(&mut slab, &[D(1), D(2), D(3), D(4)]);
        slab.set_root(noun);
        let jammed = slab.jam();
        let mut cued_slab: NounSlab<Chaff> = NounSlab::new();
        let cued = cued_slab.cue_into(jammed).expect("cue should succeed");
        assert!(slab_noun_equality(unsafe { slab.root() }, &cued));
    }

    #[test]
    fn chaff_handles_shared_backrefs() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = build_shared_noun(&mut slab);
        slab.set_root(noun);
        let jammed = slab.jam();
        let mut cued_slab: NounSlab<Chaff> = NounSlab::new();
        let cued = cued_slab.cue_into(jammed).expect("cue should succeed");
        assert!(slab_noun_equality(unsafe { slab.root() }, &cued));
    }

    #[test]
    fn chaff_jam_matches_nock_jam() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = T(&mut slab, &[D(5), D(23), D(7)]);
        slab.set_root(noun);
        let chaff_jam = slab.jam();
        let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();
        let copied = nock_slab.copy_into(noun);
        let nock_jam = NockJammer::jam(copied);
        assert_eq!(chaff_jam, nock_jam);
    }

    #[test]
    fn chaff_rejects_truncated_input() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let jammed = Bytes::from_static(&[0b1]);
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[test]
    fn chaff_rejects_bad_backref() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let jammed = Bytes::from_static(&[0b0000_0111]);
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_backref_before_definition() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(true); // tag
        writer.write_bit(true); // backref
        writer.write_zeros(1); // size-of-size zeros
        writer.write_bit(true); // delimiter
        writer.write_bit(false); // backref size = 1
        writer.write_bit(true); // backref value = 1 (missing)
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_backref_far_ahead() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(true); // tag
        writer.write_bit(true); // backref
        writer.write_zeros(3); // size-of-size zeros
        writer.write_bit(true); // delimiter
        writer.write_bits_from_value(0b11, 2); // backref size = 4
        writer.write_bits_from_value(0b1010, 4); // backref = 10
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_self_referential_backref() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(true); // tag
        writer.write_bit(true); // backref
        writer.write_zeros(1); // size-of-size zeros
        writer.write_bit(true); // delimiter
        writer.write_bit(false); // backref size = 1
        writer.write_bit(false); // backref value = 0 (self)
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::BadBackref)));
    }

    #[test]
    fn chaff_rejects_backref_too_big() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(true); // tag
        writer.write_bit(true); // backref
        writer.write_zeros(usize::BITS as usize + 1);
        writer.write_bit(true); // delimiter
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::BackrefTooBig)));
    }

    #[test]
    fn chaff_rejects_truncated_backref_size() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(true); // tag
        writer.write_bit(true); // backref
        writer.write_zeros(2); // size-of-size zeros (expect 3 bits for size)
        writer.write_bit(true); // delimiter
        writer.write_bits_from_value(0b01, 2); // only two size bits
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[test]
    fn chaff_rejects_atom_missing_size_bits() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(false); // atom tag
        writer.write_bit(false); // first zero
        writer.write_bit(false); // second zero
                                 // zeros should be 3 (two zeros then delimiter), need 2 bits for size_low
                                 // but we end here - truncated
        let jammed = writer.into_bytes();
        assert!(slab.cue_into(jammed).is_err());
    }

    #[test]
    fn chaff_rejects_truncated_indirect_atom() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(false); // atom tag
        writer.write_zeros(6); // zeros = 7, expect 6 bits for size_low
        writer.write_bit(true); // delimiter
        writer.write_bits_from_value(0b100000, 6); // size_low = 33 (bit_count = 65)
        writer.write_bits_from_value(0xFFFF, 16); // partial value (needs 65 bits)
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[test]
    fn chaff_rejects_atom_size_prefix_too_big() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(false); // atom tag
        writer.write_zeros(usize::BITS as usize + 1); // zeros > MAX_USIZE_BITS
        writer.write_bit(true); // unary delimiter
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    #[test]
    fn chaff_rejects_huge_atom_size_without_panicking() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(false); // atom tag
        writer.write_zeros(usize::BITS as usize); // zeros == MAX_USIZE_BITS
        writer.write_bit(true); // unary delimiter
        if usize::BITS > 1 {
            writer.write_bits_from_value(usize::MAX >> 1, usize::BITS as usize - 1);
        }
        let jammed = writer.into_bytes();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| slab.cue_into(jammed)));
        assert!(
            result.is_ok(),
            "cue should not panic on huge atom size prefix"
        );
        assert!(matches!(
            result.expect("catch_unwind should succeed"),
            Err(CueError::TruncatedBuffer)
        ));
    }

    #[test]
    fn chaff_rejects_zero_atom_bad_encoding() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let mut writer = BitWriter::new();
        writer.write_bit(false); // atom tag
        writer.write_bit(false); // zeros = 0 (should be "0 1" encoding)
        writer.write_bit(false);
        let jammed = writer.into_bytes();
        let result = slab.cue_into(jammed);
        assert!(matches!(result, Err(CueError::TruncatedBuffer)));
    }

    const SHARED_PREFIX_BOUNDARIES: [usize; 15] =
        [7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129];

    #[derive(Clone, Copy, Debug)]
    struct NounTreeProfile {
        max_depth: usize,
        max_indirect_bytes: usize,
    }

    const TIER1_PROFILE: NounTreeProfile = NounTreeProfile {
        max_depth: 6,
        max_indirect_bytes: 256,
    };

    const TIER2_PROFILE: NounTreeProfile = NounTreeProfile {
        max_depth: 10,
        max_indirect_bytes: 4 * 1024,
    };

    const TIER3_PROFILE: NounTreeProfile = NounTreeProfile {
        max_depth: 14,
        max_indirect_bytes: 64 * 1024,
    };

    #[derive(Clone, Debug)]
    enum NounTree {
        Atom(AtomKind),
        Cell(Box<NounTree>, Box<NounTree>),
    }

    #[derive(Clone, Debug)]
    enum AtomKind {
        Zero,
        Direct(u64),
        Indirect(Vec<u8>),
    }

    #[derive(Clone, Debug)]
    struct Tier1Tree(NounTree);

    #[derive(Clone, Debug)]
    struct Tier2Tree(NounTree);

    #[derive(Clone, Debug)]
    struct Tier3Tree(NounTree);

    #[derive(Clone, Copy, Debug)]
    enum JamMutationKind {
        TruncateAtBit,
        InflateUnaryPrefix,
        DeflateUnaryPrefix,
        RewriteBackrefFuture,
        RewriteBackrefSelf,
        FlipTagBit,
        DropPayloadBytes,
        AppendNonZeroTrailingByte,
    }

    #[derive(Clone, Debug)]
    struct JamMutationCase {
        base_tree: NounTree,
        seed_selector: u8,
        mutation: JamMutationKind,
        salt: u16,
    }

    impl JamMutationKind {
        fn is_differential_safe(self) -> bool {
            matches!(
                self,
                JamMutationKind::AppendNonZeroTrailingByte
                    | JamMutationKind::RewriteBackrefFuture
                    | JamMutationKind::RewriteBackrefSelf
                    | JamMutationKind::FlipTagBit
            )
        }
    }

    impl Arbitrary for JamMutationKind {
        fn arbitrary(g: &mut Gen) -> Self {
            match usize::arbitrary(g) % 8 {
                0 => Self::TruncateAtBit,
                1 => Self::InflateUnaryPrefix,
                2 => Self::DeflateUnaryPrefix,
                3 => Self::RewriteBackrefFuture,
                4 => Self::RewriteBackrefSelf,
                5 => Self::FlipTagBit,
                6 => Self::DropPayloadBytes,
                _ => Self::AppendNonZeroTrailingByte,
            }
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(std::iter::empty())
        }
    }

    impl Arbitrary for JamMutationCase {
        fn arbitrary(g: &mut Gen) -> Self {
            Self {
                base_tree: arbitrary_noun_tree(g, TIER1_PROFILE),
                seed_selector: u8::arbitrary(g),
                mutation: JamMutationKind::arbitrary(g),
                salt: u16::arbitrary(g),
            }
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            let mut out = Vec::new();
            for shrunk in shrink_noun_tree(&self.base_tree) {
                out.push(Self {
                    base_tree: shrunk,
                    seed_selector: self.seed_selector,
                    mutation: self.mutation,
                    salt: self.salt,
                });
            }
            Box::new(out.into_iter())
        }
    }

    impl Arbitrary for Tier1Tree {
        fn arbitrary(g: &mut Gen) -> Self {
            Self(arbitrary_noun_tree(g, TIER1_PROFILE))
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(shrink_noun_tree(&self.0).into_iter().map(Self))
        }
    }

    impl Arbitrary for Tier2Tree {
        fn arbitrary(g: &mut Gen) -> Self {
            Self(arbitrary_noun_tree(g, TIER2_PROFILE))
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(shrink_noun_tree(&self.0).into_iter().map(Self))
        }
    }

    impl Arbitrary for Tier3Tree {
        fn arbitrary(g: &mut Gen) -> Self {
            Self(arbitrary_noun_tree(g, TIER3_PROFILE))
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(shrink_noun_tree(&self.0).into_iter().map(Self))
        }
    }

    fn arbitrary_noun_tree(g: &mut Gen, profile: NounTreeProfile) -> NounTree {
        gen_noun_tree(g, profile, profile.max_depth)
    }

    fn gen_noun_tree(g: &mut Gen, profile: NounTreeProfile, fuel: usize) -> NounTree {
        if fuel == 0 {
            return NounTree::Atom(gen_atom_kind(g, profile));
        }

        let choose_cell = (usize::arbitrary(g) % 10) < 6;
        if !choose_cell {
            return NounTree::Atom(gen_atom_kind(g, profile));
        }

        let remaining = fuel - 1;
        let head_fuel = if remaining == 0 {
            0
        } else {
            usize::arbitrary(g) % (remaining + 1)
        };
        let tail_fuel = remaining.saturating_sub(head_fuel);
        NounTree::Cell(
            Box::new(gen_noun_tree(g, profile, head_fuel)),
            Box::new(gen_noun_tree(g, profile, tail_fuel)),
        )
    }

    fn gen_atom_kind(g: &mut Gen, profile: NounTreeProfile) -> AtomKind {
        let choice = usize::arbitrary(g) % 100;
        match choice {
            0..=2 => AtomKind::Zero,
            3..=34 => {
                let val = u64::arbitrary(g) & nockvm::noun::DIRECT_MAX;
                AtomKind::Direct(val)
            }
            35..=54 => {
                let bits = *g.choose(&[63usize, 64, 65]).expect("bits boundary choice");
                let bytes = gen_atom_bytes_exact_bits(g, bits);
                if bits < 64 {
                    AtomKind::Direct(bytes_to_u64(&bytes) & nockvm::noun::DIRECT_MAX)
                } else {
                    AtomKind::Indirect(bytes)
                }
            }
            _ => {
                let byte_len = choose_indirect_byte_len(g, profile.max_indirect_bytes);
                let mut bytes: Vec<u8> = (0..byte_len).map(|_| u8::arbitrary(g)).collect();
                if let Some(last) = bytes.last_mut() {
                    if *last == 0 {
                        *last = 1 + (u8::arbitrary(g) % 0x7F);
                    }
                }
                AtomKind::Indirect(bytes)
            }
        }
    }

    fn choose_indirect_byte_len(g: &mut Gen, max_indirect_bytes: usize) -> usize {
        let min_len = 9usize;
        let max_len = max_indirect_bytes.max(min_len);
        const BOUNDARY_BYTES: [usize; 12] = [9, 10, 15, 16, 17, 31, 32, 33, 63, 64, 65, 129];
        let use_boundary = (usize::arbitrary(g) % 10) < 6;
        if use_boundary {
            let candidate = BOUNDARY_BYTES[usize::arbitrary(g) % BOUNDARY_BYTES.len()];
            candidate.min(max_len)
        } else {
            min_len + (usize::arbitrary(g) % (max_len - min_len + 1))
        }
    }

    fn gen_atom_bytes_exact_bits(g: &mut Gen, bits: usize) -> Vec<u8> {
        let byte_len = bits.div_ceil(8);
        let mut bytes: Vec<u8> = (0..byte_len).map(|_| u8::arbitrary(g)).collect();
        let high_byte_idx = byte_len - 1;
        let high_bit = (bits - 1) & 7;
        bytes[high_byte_idx] |= 1u8 << high_bit;
        if high_bit < 7 {
            bytes[high_byte_idx] &= (1u8 << (high_bit + 1)) - 1;
        }
        bytes
    }

    fn bytes_to_u64(bytes: &[u8]) -> u64 {
        let mut value = 0u64;
        for (idx, byte) in bytes.iter().copied().enumerate() {
            if idx >= 8 {
                break;
            }
            value |= (byte as u64) << (idx * 8);
        }
        value
    }

    fn shrink_noun_tree(tree: &NounTree) -> Vec<NounTree> {
        match tree {
            NounTree::Atom(kind) => shrink_atom_kind(kind)
                .into_iter()
                .map(NounTree::Atom)
                .collect(),
            NounTree::Cell(head, tail) => {
                let mut out = vec![(**head).clone(), (**tail).clone()];
                out.extend(
                    shrink_noun_tree(head)
                        .into_iter()
                        .map(|shrunk| NounTree::Cell(Box::new(shrunk), Box::new((**tail).clone()))),
                );
                out.extend(
                    shrink_noun_tree(tail)
                        .into_iter()
                        .map(|shrunk| NounTree::Cell(Box::new((**head).clone()), Box::new(shrunk))),
                );
                out
            }
        }
    }

    fn shrink_atom_kind(kind: &AtomKind) -> Vec<AtomKind> {
        match kind {
            AtomKind::Zero => Vec::new(),
            AtomKind::Direct(value) => {
                let mut out = vec![AtomKind::Zero];
                for shrunk in value.shrink() {
                    out.push(AtomKind::Direct(shrunk & nockvm::noun::DIRECT_MAX));
                }
                out
            }
            AtomKind::Indirect(bytes) => {
                let mut out = vec![AtomKind::Zero, AtomKind::Direct(1), AtomKind::Direct(63)];
                for bits in [63usize, 64, 65] {
                    let mut seed = vec![0u8; bits.div_ceil(8)];
                    let high_byte_idx = seed.len() - 1;
                    let high_bit = (bits - 1) & 7;
                    seed[high_byte_idx] |= 1u8 << high_bit;
                    if bits < 64 {
                        out.push(AtomKind::Direct(bytes_to_u64(&seed)));
                    } else {
                        out.push(AtomKind::Indirect(seed));
                    }
                }

                if bytes.len() > 9 {
                    let mut shorter = bytes[..bytes.len() / 2].to_vec();
                    if shorter.len() < 9 {
                        shorter.resize(9, 0);
                    }
                    if let Some(last) = shorter.last_mut() {
                        if *last == 0 {
                            *last = 1;
                        }
                    }
                    out.push(AtomKind::Indirect(shorter));
                }
                out
            }
        }
    }

    fn build_noun_from_tree(slab: &mut NounSlab<Chaff>, tree: &NounTree) -> Noun {
        match tree {
            NounTree::Atom(AtomKind::Zero) => D(0),
            NounTree::Atom(AtomKind::Direct(value)) => D(*value),
            NounTree::Atom(AtomKind::Indirect(bytes)) => atom_from_bytes(slab, bytes).as_noun(),
            NounTree::Cell(head, tail) => {
                let h = build_noun_from_tree(slab, head);
                let t = build_noun_from_tree(slab, tail);
                Cell::new(slab, h, t).as_noun()
            }
        }
    }

    fn pair_with_self(slab: &mut NounSlab<Chaff>, noun: Noun) -> Noun {
        Cell::new(slab, noun, noun).as_noun()
    }

    fn nested_shared(slab: &mut NounSlab<Chaff>, noun: Noun) -> Noun {
        let inner = pair_with_self(slab, noun);
        Cell::new(slab, noun, inner).as_noun()
    }

    fn alternating_shared(slab: &mut NounSlab<Chaff>, noun: Noun, prefix_len: usize) -> Noun {
        let mut list = D(0);
        for idx in (0..prefix_len).rev() {
            list = Cell::new(slab, D(7_000 + idx as u64), list).as_noun();
        }
        let nested = nested_shared(slab, noun);
        T(slab, &[list, noun, nested, noun])
    }

    fn seed_noun_for_selector(slab: &mut NounSlab<Chaff>, selector: u8, tree: &NounTree) -> Noun {
        match selector % 4 {
            0 => build_noun_from_tree(slab, tree),
            1 => build_shared_noun(slab),
            2 => T(slab, &[D(1), D(2), D(3), D(4), D(5)]),
            _ => {
                let atom = atom_with_exact_bit_size(slab, 65);
                T(slab, &[D(11), atom.as_noun(), D(99)])
            }
        }
    }

    fn canonical_jam_from_case(case: &JamMutationCase) -> Bytes {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = seed_noun_for_selector(&mut slab, case.seed_selector, &case.base_tree);
        slab.set_root(noun);
        slab.jam()
    }

    fn truncate_bits(bytes: &[u8], keep_bits: usize) -> Bytes {
        if keep_bits == 0 {
            return Bytes::new();
        }
        let bit_len = bytes.len() * 8;
        let keep = keep_bits.min(bit_len);
        if keep == bit_len {
            return Bytes::copy_from_slice(bytes);
        }
        let byte_len = keep.div_ceil(8);
        let mut out = bytes[..byte_len].to_vec();
        let rem = keep & 7;
        if rem != 0 {
            let mask = (1u8 << rem) - 1;
            let last = out.len() - 1;
            out[last] &= mask;
        }
        Bytes::from(out)
    }

    fn set_bit(bytes: &mut Vec<u8>, bit_idx: usize, value: bool) {
        let byte_idx = bit_idx >> 3;
        if byte_idx >= bytes.len() {
            bytes.resize(byte_idx + 1, 0);
        }
        let bit_mask = 1u8 << (bit_idx & 7);
        if value {
            bytes[byte_idx] |= bit_mask;
        } else {
            bytes[byte_idx] &= !bit_mask;
        }
    }

    fn get_bit(bytes: &[u8], bit_idx: usize) -> bool {
        let byte_idx = bit_idx >> 3;
        if byte_idx >= bytes.len() {
            return false;
        }
        ((bytes[byte_idx] >> (bit_idx & 7)) & 1) == 1
    }

    fn clear_first_set_bit(bytes: &mut Vec<u8>, start_bit: usize) -> bool {
        let total_bits = bytes.len() * 8;
        for bit in start_bit..total_bits {
            if get_bit(bytes, bit) {
                set_bit(bytes, bit, false);
                return true;
            }
        }
        false
    }

    fn set_first_clear_bit(bytes: &mut Vec<u8>, start_bit: usize) -> bool {
        let total_bits = bytes.len() * 8;
        for bit in start_bit..total_bits {
            if !get_bit(bytes, bit) {
                set_bit(bytes, bit, true);
                return true;
            }
        }
        false
    }

    fn overwrite_prefix_bits(bytes: &mut Vec<u8>, prefix_bits: &[bool]) {
        for (idx, bit) in prefix_bits.iter().copied().enumerate() {
            set_bit(bytes, idx, bit);
        }
    }

    fn mutate_jam_bytes(canonical: &Bytes, mutation: JamMutationKind, salt: u16) -> Bytes {
        let mut bytes = canonical.to_vec();
        match mutation {
            JamMutationKind::TruncateAtBit => {
                let total_bits = bytes.len() * 8;
                if total_bits == 0 {
                    return Bytes::new();
                }
                let keep_bits = (salt as usize) % total_bits;
                truncate_bits(&bytes, keep_bits)
            }
            JamMutationKind::InflateUnaryPrefix => {
                if !clear_first_set_bit(&mut bytes, 1) && !bytes.is_empty() {
                    set_bit(&mut bytes, 0, false);
                }
                Bytes::from(bytes)
            }
            JamMutationKind::DeflateUnaryPrefix => {
                if !set_first_clear_bit(&mut bytes, 1) {
                    bytes.push(0x01);
                }
                Bytes::from(bytes)
            }
            JamMutationKind::RewriteBackrefFuture => {
                overwrite_prefix_bits(&mut bytes, &[true, true, false, true, true, false]);
                Bytes::from(bytes)
            }
            JamMutationKind::RewriteBackrefSelf => {
                overwrite_prefix_bits(&mut bytes, &[true, true, false, true, false, false]);
                Bytes::from(bytes)
            }
            JamMutationKind::FlipTagBit => {
                if bytes.is_empty() {
                    bytes.push(0);
                }
                let flip_idx = (salt as usize) % (bytes.len() * 8);
                let current = get_bit(&bytes, flip_idx);
                set_bit(&mut bytes, flip_idx, !current);
                Bytes::from(bytes)
            }
            JamMutationKind::DropPayloadBytes => {
                if bytes.len() <= 1 {
                    let keep_bits = (salt as usize) % 4;
                    return truncate_bits(&bytes, keep_bits);
                }
                let drop = 1 + ((salt as usize) % bytes.len().min(4));
                bytes.truncate(bytes.len() - drop);
                Bytes::from(bytes)
            }
            JamMutationKind::AppendNonZeroTrailingByte => {
                let trailing = (salt as u8) | 1;
                bytes.push(trailing);
                Bytes::from(bytes)
            }
        }
    }

    fn expected_cue_error(err: CueError) -> bool {
        matches!(
            err,
            CueError::TruncatedBuffer | CueError::BadBackref | CueError::BackrefTooBig
        )
    }

    fn qc_test_count(env_key: &str, default: u64) -> u64 {
        std::env::var(env_key)
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|count| *count > 0)
            .unwrap_or(default)
    }

    fn prop_chaff_roundtrip_tree_impl(tree: &NounTree) -> TestResult {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = build_noun_from_tree(&mut slab, tree);
        slab.set_root(noun);
        let jammed = slab.jam();
        let mut cued: NounSlab<Chaff> = NounSlab::new();
        let cued_noun = match cued.cue_into(jammed) {
            Ok(noun) => noun,
            Err(_) => return TestResult::failed(),
        };
        TestResult::from_bool(slab_noun_equality(unsafe { slab.root() }, &cued_noun))
    }

    fn prop_chaff_matches_nock_tree_impl(tree: &NounTree) -> TestResult {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = build_noun_from_tree(&mut slab, tree);
        slab.set_root(noun);
        let chaff_jam = slab.jam();
        let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();
        let copied = nock_slab.copy_into(noun);
        let nock_jam = NockJammer::jam(copied);
        TestResult::from_bool(chaff_jam == nock_jam)
    }

    fn prop_chaff_parity_tree_matrix_impl(tree: &NounTree) -> TestResult {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = build_noun_from_tree(&mut slab, tree);
        assert_parity_with_nock(noun, &mut slab);
        TestResult::passed()
    }

    fn prop_chaff_parity_shared_tree_impl(
        tree: &NounTree,
        mode: u8,
        prefix_seed: usize,
    ) -> TestResult {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let sub = build_noun_from_tree(&mut slab, tree);
        let shared = match mode % 3 {
            0 => pair_with_self(&mut slab, sub),
            1 => nested_shared(&mut slab, sub),
            _ => {
                let prefix = SHARED_PREFIX_BOUNDARIES[prefix_seed % SHARED_PREFIX_BOUNDARIES.len()];
                alternating_shared(&mut slab, sub, prefix)
            }
        };
        assert_parity_with_nock(shared, &mut slab);
        TestResult::passed()
    }

    fn prop_chaff_idempotence_chain_tree_impl(tree: &NounTree) -> TestResult {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = build_noun_from_tree(&mut slab, tree);
        slab.set_root(noun);
        assert_idempotence_canonical_chain(slab.jam(), None, 4);
        TestResult::passed()
    }

    fn prop_chaff_adversarial_mutations_no_panic_impl(case: JamMutationCase) -> TestResult {
        let canonical = canonical_jam_from_case(&case);
        let mutated = mutate_jam_bytes(&canonical, case.mutation, case.salt);
        let mut chaff_slab: NounSlab<Chaff> = NounSlab::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            chaff_slab.cue_into(mutated.clone())
        }));
        let cue_result = match result {
            Ok(outcome) => outcome,
            Err(_) => return TestResult::failed(),
        };

        match cue_result {
            Ok(noun) => {
                chaff_slab.set_root(noun);
                let rejam = chaff_slab.jam();
                if matches!(case.mutation, JamMutationKind::AppendNonZeroTrailingByte) {
                    return TestResult::from_bool(rejam == canonical);
                }
                TestResult::from_bool(!rejam.is_empty() || canonical.is_empty())
            }
            Err(err) => TestResult::from_bool(expected_cue_error(err)),
        }
    }

    fn prop_chaff_adversarial_mutations_differential_impl(case: JamMutationCase) -> TestResult {
        if !case.mutation.is_differential_safe() {
            return TestResult::discard();
        }

        let canonical = canonical_jam_from_case(&case);
        let mutated = mutate_jam_bytes(&canonical, case.mutation, case.salt);
        let mut chaff_slab: NounSlab<Chaff> = NounSlab::new();
        let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();

        let chaff = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            chaff_slab.cue_into(mutated.clone())
        }));
        let nock = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            nock_slab.cue_into(mutated.clone())
        }));

        let chaff = match chaff {
            Ok(value) => value,
            Err(_) => return TestResult::failed(),
        };
        let nock = match nock {
            Ok(value) => value,
            Err(_) => return TestResult::failed(),
        };

        match (chaff, nock) {
            (Err(chaff_err), Err(nock_err)) => {
                TestResult::from_bool(expected_cue_error(chaff_err) && expected_cue_error(nock_err))
            }
            (Ok(chaff_noun), Ok(nock_noun)) => {
                if !slab_noun_equality(&chaff_noun, &nock_noun) {
                    return TestResult::failed();
                }
                chaff_slab.set_root(chaff_noun);
                let chaff_rejam = chaff_slab.jam();
                let nock_rejam = NockJammer::jam(nock_noun);
                if matches!(case.mutation, JamMutationKind::AppendNonZeroTrailingByte)
                    && chaff_rejam != canonical
                {
                    return TestResult::failed();
                }
                TestResult::from_bool(chaff_rejam == nock_rejam)
            }
            _ => TestResult::failed(),
        }
    }

    fn build_list(slab: &mut NounSlab<Chaff>, leaves: &[u16]) -> Noun {
        let mut list = D(0);
        for value in leaves.iter().rev() {
            list = Cell::new(slab, D(*value as u64), list).as_noun();
        }
        list
    }

    fn parity_fixtures(slab: &mut NounSlab<Chaff>) -> Vec<Noun> {
        let mut fixtures = Vec::new();
        fixtures.push(D(0));

        let bytes = vec![0xFFu8; 64];
        let atom = atom_from_bytes(slab, &bytes);
        fixtures.push(atom.as_noun());

        fixtures.push(T(slab, &[D(1), D(2), D(3), D(4)]));
        fixtures.push(build_shared_noun(slab));
        fixtures.push(build_list(slab, &[1, 2, 3, 4, 5]));
        fixtures
    }

    fn stack_words_for_jam(bytes_len: usize) -> usize {
        let words = bytes_len.div_ceil(8);
        let scaled = words.saturating_mul(NOCKVM_STACK_WORDS_JAM_SCALE);
        scaled.max(NOCKVM_STACK_WORDS_MIN)
    }

    fn stack_words_for_large_checkpoint(bytes_len: usize) -> usize {
        stack_words_for_jam(bytes_len).max(NOCK_STACK_SIZE_TINY)
    }

    fn stack_words_for_noun(noun: Noun, jam_len: usize) -> usize {
        let jam_words = stack_words_for_jam(jam_len);
        let mass_words = noun.mass().saturating_mul(NOCKVM_STACK_WORDS_MASS_SCALE);
        jam_words.max(mass_words)
    }

    #[derive(Decode)]
    struct CheckpointEnvelope {
        magic_bytes: u64,
        version: u32,
        payload: Vec<u8>,
    }

    const SNAPSHOT_VERSION_2: u32 = 2;

    fn jammed_state_from_checkpoint(bytes: &[u8]) -> Bytes {
        let config = config::standard();

        if let Ok((envelope, _)) =
            bincode::decode_from_slice::<CheckpointEnvelope, Configuration>(bytes, config)
        {
            if envelope.magic_bytes == JAM_MAGIC_BYTES && envelope.version == SNAPSHOT_VERSION_2 {
                let (checkpoint, _) =
                    bincode::decode_from_slice::<JammedCheckpointV2, Configuration>(
                        &envelope.payload, config,
                    )
                    .expect("V2 checkpoint payload should decode");
                return checkpoint.state_jam.0;
            }
        }

        if let Ok((checkpoint, _)) =
            bincode::decode_from_slice::<JammedCheckpointV1, Configuration>(bytes, config)
        {
            if checkpoint.magic_bytes == JAM_MAGIC_BYTES {
                return checkpoint.jam.0;
            }
        }

        panic!("Failed to decode checkpoint as either V1 or V2 format");
    }

    fn legacy_pair_jam(state_value: u64, cold_value: u64) -> JammedNoun {
        let mut slab = NounSlab::<NockJammer>::new();
        let state = slab.copy_into(D(state_value));
        let cold = slab.copy_into(D(cold_value));
        let root = T(&mut slab, &[state, cold]);
        slab.set_root(root);
        JammedNoun::new(slab.coerce_jammer::<NockJammer>().jam())
    }

    fn jam_atom_with_chaff(value: u64) -> JammedNoun {
        let mut slab = NounSlab::<Chaff>::new();
        let atom = slab.copy_into(D(value));
        slab.set_root(atom);
        JammedNoun::new(slab.coerce_jammer::<Chaff>().jam())
    }

    fn atom_value(noun: Noun) -> u64 {
        noun.as_atom()
            .expect("expected atom")
            .as_u64()
            .expect("expected atom to fit in u64")
    }

    async fn load_saveable_with_chaff(temp: &TempDir, bytes: Vec<u8>) -> SaveableCheckpoint {
        std::fs::write(temp.path().join("0.chkjam"), bytes).expect("write checkpoint");
        let (_, maybe_saveable) =
            Saver::<Chaff>::try_load::<SaveableCheckpoint>(&temp.path().to_path_buf(), None)
                .await
                .expect("load checkpoint");
        maybe_saveable.expect("expected a checkpoint")
    }

    fn assert_loaded_values(
        saveable: &SaveableCheckpoint,
        expected_hash: blake3::Hash,
        event_num: u64,
        state_value: u64,
        cold_value: u64,
    ) {
        assert_eq!(saveable.ker_hash, expected_hash);
        assert_eq!(saveable.event_num, event_num);
        assert_eq!(atom_value(unsafe { *saveable.state.root() }), state_value);
        assert_eq!(atom_value(unsafe { *saveable.cold.root() }), cold_value);
    }

    fn large_checkpoint_jam() -> Bytes {
        static JAM: OnceLock<Bytes> = OnceLock::new();
        JAM.get_or_init(|| {
            let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/test-jams/0.chkjam"));
            jammed_state_from_checkpoint(bytes)
        })
        .clone()
    }

    fn jam_nockstack_trimmed(noun: Noun, expected_len: usize) -> Bytes {
        let mut jam_stack = NockStack::new(stack_words_for_noun(noun, expected_len), 0);
        let jam_noun = jam_stack.copy_into(noun);
        let jammed = nockvm::serialization::jam(&mut jam_stack, jam_noun);
        let bytes = jammed.as_ne_bytes();
        assert!(
            bytes.len() >= expected_len,
            "nockvm jam shorter than expected"
        );
        assert!(
            bytes[expected_len..].iter().all(|b| *b == 0),
            "nockvm jam has non-zero padding bytes"
        );
        Bytes::copy_from_slice(&bytes[..expected_len])
    }

    fn cue_all_with_stack(
        jam: &Bytes,
        stack_words: usize,
    ) -> (
        NounSlab<Chaff>,
        Noun,
        NounSlab<NockJammer>,
        Noun,
        NockStack,
        Noun,
    ) {
        let mut chaff_slab: NounSlab<Chaff> = NounSlab::new();
        let chaff_noun = chaff_slab
            .cue_into(jam.clone())
            .expect("chaff cue should succeed");
        chaff_slab.set_root(chaff_noun);

        let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();
        let nock_noun = nock_slab
            .cue_into(jam.clone())
            .expect("NockJammer cue should succeed");

        let mut nockvm_stack = NockStack::new(stack_words, 0);
        let nockvm_noun = <Noun as NounExt>::cue_bytes(&mut nockvm_stack, jam)
            .expect("nockvm cue should succeed");

        (
            chaff_slab, chaff_noun, nock_slab, nock_noun, nockvm_stack, nockvm_noun,
        )
    }

    fn assert_parity_with_nock(noun: Noun, chaff_slab: &mut NounSlab<Chaff>) {
        chaff_slab.set_root(noun);
        let chaff_jam = chaff_slab.jam();
        let chaff_root = unsafe { *chaff_slab.root() };

        let nock_jam = NockJammer::jam(chaff_root);
        assert_eq!(chaff_jam, nock_jam, "chaff jam differs from NockJammer");

        let stack_words = stack_words_for_noun(chaff_root, chaff_jam.len());
        let mut jam_stack = NockStack::new(stack_words, 0);
        let stack_noun = jam_stack.copy_into(chaff_root);
        let nockvm_jam = nockvm::serialization::jam(&mut jam_stack, stack_noun);
        assert!(
            nockvm_jam.eq_bytes(&chaff_jam),
            "chaff jam differs from nockvm jam (allowing zero padding)"
        );
        let nockvm_bytes = Bytes::copy_from_slice(nockvm_jam.as_ne_bytes());

        let mut chaff_cue_slab: NounSlab<Chaff> = NounSlab::new();
        let chaff_cued = chaff_cue_slab
            .cue_into(chaff_jam.clone())
            .expect("chaff cue should succeed");
        assert!(
            slab_noun_equality(&chaff_root, &chaff_cued),
            "chaff cue differs from original noun"
        );

        let mut nock_cue_slab: NounSlab<NockJammer> = NounSlab::new();
        let nock_cued = nock_cue_slab
            .cue_into(chaff_jam.clone())
            .expect("NockJammer cue should succeed");
        assert!(
            slab_noun_equality(&chaff_root, &nock_cued),
            "NockJammer cue differs from original noun"
        );

        let mut cue_stack = NockStack::new(stack_words, 0);
        let nockvm_cued = <Noun as NounExt>::cue_bytes(&mut cue_stack, &chaff_jam)
            .expect("nockvm cue should succeed");
        assert!(
            slab_noun_equality(&chaff_root, &nockvm_cued),
            "nockvm cue differs from original noun"
        );

        let mut chaff_cue_slab2: NounSlab<Chaff> = NounSlab::new();
        let chaff_cued_from_nockvm = chaff_cue_slab2
            .cue_into(nockvm_bytes.clone())
            .expect("chaff cue should accept nockvm jam bytes");
        assert!(
            slab_noun_equality(&chaff_root, &chaff_cued_from_nockvm),
            "chaff cue differs when decoding nockvm jam bytes"
        );

        let mut nock_cue_slab2: NounSlab<NockJammer> = NounSlab::new();
        let nock_cued_from_nockvm = nock_cue_slab2
            .cue_into(nockvm_bytes.clone())
            .expect("NockJammer cue should accept nockvm jam bytes");
        assert!(
            slab_noun_equality(&chaff_root, &nock_cued_from_nockvm),
            "NockJammer cue differs when decoding nockvm jam bytes"
        );
    }

    fn assert_roundtrip_matrix(jam: Bytes, full_matrix: bool) {
        let mut chaff_slab: NounSlab<Chaff> = NounSlab::new();
        let chaff_noun = chaff_slab
            .cue_into(jam.clone())
            .expect("chaff cue should succeed");
        chaff_slab.set_root(chaff_noun);

        let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();
        let nock_noun = nock_slab
            .cue_into(jam.clone())
            .expect("NockJammer cue should succeed");

        let stack_words = stack_words_for_noun(chaff_noun, jam.len());
        let mut nockvm_stack = NockStack::new(stack_words, 0);
        let nockvm_noun = <Noun as NounExt>::cue_bytes(&mut nockvm_stack, &jam)
            .expect("nockvm cue should succeed");

        assert!(
            slab_noun_equality(&chaff_noun, &nock_noun),
            "NockJammer cue differs from Chaff cue"
        );
        assert!(
            slab_noun_equality(&chaff_noun, &nockvm_noun),
            "nockvm cue differs from Chaff cue"
        );

        let chaff_jam = chaff_slab.jam();
        assert_eq!(chaff_jam, jam, "chaff jam differs from input jam");

        let nock_jam = NockJammer::jam(nock_noun);
        assert_eq!(nock_jam, jam, "NockJammer jam differs from input jam");

        let mut jam_stack = NockStack::new(stack_words, 0);
        let jam_noun = jam_stack.copy_into(chaff_noun);
        let nockvm_jam = nockvm::serialization::jam(&mut jam_stack, jam_noun);
        let nockvm_bytes = nockvm_jam.as_ne_bytes();
        assert!(
            nockvm_bytes.len() >= jam.len(),
            "nockvm jam shorter than input jam"
        );
        assert_eq!(
            &nockvm_bytes[..jam.len()],
            jam.as_ref(),
            "nockvm jam prefix differs from input jam"
        );
        assert!(
            nockvm_bytes[jam.len()..].iter().all(|b| *b == 0),
            "nockvm jam has non-zero padding bytes"
        );

        let nockvm_trimmed = Bytes::copy_from_slice(&nockvm_bytes[..jam.len()]);
        if !full_matrix {
            let mut chaff = NounSlab::<Chaff>::new();
            let chaff_rt = chaff
                .cue_into(nockvm_trimmed.clone())
                .expect("chaff cue should succeed");
            assert!(
                slab_noun_equality(&chaff_noun, &chaff_rt),
                "chaff cue mismatch on nockvm jam"
            );

            let mut nock = NounSlab::<NockJammer>::new();
            let nock_rt = nock
                .cue_into(nockvm_trimmed)
                .expect("NockJammer cue should succeed");
            assert!(
                slab_noun_equality(&chaff_noun, &nock_rt),
                "NockJammer cue mismatch on nockvm jam"
            );
            return;
        }

        for jam_bytes in [chaff_jam, nock_jam, nockvm_trimmed] {
            let mut chaff = NounSlab::<Chaff>::new();
            let chaff_rt = chaff
                .cue_into(jam_bytes.clone())
                .expect("chaff cue should succeed");
            assert!(
                slab_noun_equality(&chaff_noun, &chaff_rt),
                "chaff roundtrip noun mismatch"
            );
            chaff.set_root(chaff_rt);
            assert_eq!(chaff.jam(), jam_bytes, "chaff roundtrip jam mismatch");

            let mut nock = NounSlab::<NockJammer>::new();
            let nock_rt = nock
                .cue_into(jam_bytes.clone())
                .expect("NockJammer cue should succeed");
            assert!(
                slab_noun_equality(&chaff_noun, &nock_rt),
                "NockJammer roundtrip noun mismatch"
            );
            assert_eq!(
                NockJammer::jam(nock_rt),
                jam_bytes,
                "NockJammer roundtrip jam mismatch"
            );

            let mut stack = NockStack::new(stack_words, 0);
            let nockvm_rt = <Noun as NounExt>::cue_bytes(&mut stack, &jam_bytes)
                .expect("nockvm cue should succeed");
            assert!(
                slab_noun_equality(&chaff_noun, &nockvm_rt),
                "nockvm roundtrip noun mismatch"
            );
            let nockvm_rt_jam = nockvm::serialization::jam(&mut stack, nockvm_rt);
            let nockvm_rt_bytes = nockvm_rt_jam.as_ne_bytes();
            assert!(
                nockvm_rt_bytes.len() >= jam_bytes.len(),
                "nockvm roundtrip jam shorter than input jam"
            );
            assert_eq!(
                &nockvm_rt_bytes[..jam_bytes.len()],
                jam_bytes.as_ref(),
                "nockvm roundtrip jam prefix mismatch"
            );
            assert!(
                nockvm_rt_bytes[jam_bytes.len()..].iter().all(|b| *b == 0),
                "nockvm roundtrip jam has non-zero padding bytes"
            );
        }
    }

    fn assert_idempotence_canonical_chain(
        mut input_jam: Bytes,
        expected_canonical: Option<Bytes>,
        steps: usize,
    ) {
        assert!(steps > 0, "chain length must be positive");
        for step in 0..steps {
            let mut chaff_slab: NounSlab<Chaff> = NounSlab::new();
            let chaff_noun = chaff_slab
                .cue_into(input_jam.clone())
                .expect("chaff cue should succeed in chain");
            chaff_slab.set_root(chaff_noun);
            let chaff_jam = chaff_slab.jam();

            let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();
            let nock_noun = nock_slab
                .cue_into(input_jam.clone())
                .expect("NockJammer cue should succeed in chain");
            assert!(
                slab_noun_equality(&chaff_noun, &nock_noun),
                "noun mismatch between Chaff and NockJammer at chain step {step}"
            );
            let nock_jam = NockJammer::jam(nock_noun);
            assert_eq!(
                nock_jam, chaff_jam,
                "jam mismatch between Chaff and NockJammer at chain step {step}"
            );

            let stack_words = stack_words_for_noun(chaff_noun, chaff_jam.len());
            let mut stack = NockStack::new(stack_words, 0);
            let nockvm_noun = <Noun as NounExt>::cue_bytes(&mut stack, &input_jam)
                .expect("nockvm cue should succeed in chain");
            assert!(
                slab_noun_equality(&chaff_noun, &nockvm_noun),
                "noun mismatch between Chaff and nockvm at chain step {step}"
            );
            let nockvm_jam = nockvm::serialization::jam(&mut stack, nockvm_noun);
            let nockvm_bytes = nockvm_jam.as_ne_bytes();
            assert!(
                nockvm_bytes.len() >= chaff_jam.len(),
                "nockvm jam shorter than canonical jam at chain step {step}"
            );
            assert_eq!(
                &nockvm_bytes[..chaff_jam.len()],
                chaff_jam.as_ref(),
                "nockvm jam prefix mismatch at chain step {step}"
            );
            assert!(
                nockvm_bytes[chaff_jam.len()..]
                    .iter()
                    .all(|byte| *byte == 0),
                "nockvm jam has non-zero padding bytes at chain step {step}"
            );

            if step == 0 {
                if let Some(ref canonical) = expected_canonical {
                    assert_eq!(
                        &chaff_jam, canonical,
                        "first chain step should canonicalize to expected bytes"
                    );
                } else {
                    assert_eq!(
                        chaff_jam, input_jam,
                        "canonical jam should be idempotent on first chain step"
                    );
                }
            } else {
                assert_eq!(
                    chaff_jam, input_jam,
                    "canonical chain changed unexpectedly at step {step}"
                );
            }

            input_jam = chaff_jam;
        }
    }

    #[test]
    fn chaff_roundtrip_list_fixture() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = build_list(&mut slab, &[1, 2, 3, 4, 5]);
        slab.set_root(noun);
        let jammed = slab.jam();
        let mut cued: NounSlab<Chaff> = NounSlab::new();
        let cued_noun = cued.cue_into(jammed).expect("cue should succeed");
        assert!(slab_noun_equality(unsafe { slab.root() }, &cued_noun));
    }

    #[test]
    fn chaff_matches_nock_for_shared_fixture() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = build_shared_noun(&mut slab);
        slab.set_root(noun);
        let chaff_jam = slab.jam();
        let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();
        let copied = nock_slab.copy_into(noun);
        let nock_jam = NockJammer::jam(copied);
        assert_eq!(chaff_jam, nock_jam);
    }

    #[test]
    fn chaff_roundtrip_larger_atom_fixture() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let bytes = (0u8..64).collect::<Vec<_>>();
        let atom = atom_from_bytes(&mut slab, &bytes);
        let noun = T(&mut slab, &[D(7), atom.as_noun(), D(9)]);
        slab.set_root(noun);
        let jammed = slab.jam();
        let mut cued: NounSlab<Chaff> = NounSlab::new();
        let cued_noun = cued.cue_into(jammed).expect("cue should succeed");
        assert!(slab_noun_equality(unsafe { slab.root() }, &cued_noun));
    }

    #[test]
    fn chaff_parity_jam_fixtures() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        for noun in parity_fixtures(&mut slab) {
            slab.set_root(noun);
            let jammed = slab.jam();
            assert_roundtrip_matrix(jammed, true);
        }
    }

    #[test]
    fn chaff_idempotence_chain_jam_fixtures() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        for noun in parity_fixtures(&mut slab) {
            slab.set_root(noun);
            assert_idempotence_canonical_chain(slab.jam(), None, 4);
        }
    }

    #[test]
    fn chaff_parity_atom_bit_size_boundaries() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        for bits in [63usize, 64, 65] {
            let atom = atom_with_exact_bit_size(&mut slab, bits);
            let noun = T(&mut slab, &[D(bits as u64), atom.as_noun(), atom.as_noun()]);
            assert_parity_with_nock(noun, &mut slab);
        }
    }

    #[test]
    fn chaff_parity_backref_threshold_boundaries() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        for prefix_len in [7usize, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129] {
            let repeated = atom_with_exact_bit_size(&mut slab, 9);
            let noun = build_backref_threshold_fixture(&mut slab, prefix_len, repeated);
            assert_parity_with_nock(noun, &mut slab);
        }
    }

    #[test]
    fn cue_non_zero_trailing_bytes_policy_is_accepted_and_canonicalized() {
        let mut source: NounSlab<Chaff> = NounSlab::new();
        let original = T(&mut source, &[D(1), D(2), D(3), D(4), D(5)]);
        source.set_root(original);
        let canonical = source.jam();
        let original_root = unsafe { *source.root() };

        let mut trailing = canonical.to_vec();
        trailing.push(0b1010_0101);
        let with_trailing = Bytes::from(trailing);

        let mut chaff_slab: NounSlab<Chaff> = NounSlab::new();
        let chaff_noun = chaff_slab
            .cue_into(with_trailing.clone())
            .expect("chaff cue should accept trailing bytes");
        assert!(
            slab_noun_equality(&original_root, &chaff_noun),
            "chaff cue with non-zero trailing bytes should preserve noun"
        );
        chaff_slab.set_root(chaff_noun);
        assert_eq!(
            chaff_slab.jam(),
            canonical,
            "chaff re-jam should canonicalize trailing bytes away"
        );

        let mut nock_slab: NounSlab<NockJammer> = NounSlab::new();
        let nock_noun = nock_slab
            .cue_into(with_trailing.clone())
            .expect("NockJammer cue should accept trailing bytes");
        assert!(
            slab_noun_equality(&original_root, &nock_noun),
            "NockJammer cue with non-zero trailing bytes should preserve noun"
        );
        assert_eq!(
            NockJammer::jam(nock_noun),
            canonical,
            "NockJammer re-jam should canonicalize trailing bytes away"
        );

        let mut nockvm_stack =
            NockStack::new(stack_words_for_noun(original_root, with_trailing.len()), 0);
        let nockvm_noun = <Noun as NounExt>::cue_bytes(&mut nockvm_stack, &with_trailing)
            .expect("nockvm cue should accept trailing bytes");
        assert!(
            slab_noun_equality(&original_root, &nockvm_noun),
            "nockvm cue with non-zero trailing bytes should preserve noun"
        );
    }

    #[test]
    fn chaff_chain_canonicalizes_non_zero_trailing_bytes() {
        let mut source: NounSlab<Chaff> = NounSlab::new();
        let original = T(&mut source, &[D(1), D(2), D(3), D(4), D(5)]);
        source.set_root(original);
        let canonical = source.jam();

        let mut trailing = canonical.to_vec();
        trailing.extend_from_slice(&[0xA5, 0x01]);
        assert_idempotence_canonical_chain(Bytes::from(trailing), Some(canonical), 4);
    }

    #[test]
    fn chaff_chain_canonicalizes_nockvm_padded_jam() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        let noun = T(&mut slab, &[D(7), D(8), D(9), D(10), D(11)]);
        slab.set_root(noun);
        let canonical = slab.jam();
        let root = unsafe { *slab.root() };
        let mut stack = NockStack::new(stack_words_for_noun(root, canonical.len()), 0);
        let stack_noun = stack.copy_into(root);
        let nockvm_jam = nockvm::serialization::jam(&mut stack, stack_noun);
        let padded = Bytes::copy_from_slice(nockvm_jam.as_ne_bytes());
        assert_idempotence_canonical_chain(padded, Some(canonical), 4);
    }

    #[test]
    fn nockjammer_parity_jam_fixtures() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        for noun in parity_fixtures(&mut slab) {
            let jammed = NockJammer::jam(noun);
            assert_roundtrip_matrix(jammed, true);
        }
    }

    #[test]
    fn nockstack_parity_jam_fixtures() {
        let mut slab: NounSlab<Chaff> = NounSlab::new();
        for noun in parity_fixtures(&mut slab) {
            slab.set_root(noun);
            let expected = slab.jam();
            let jammed = jam_nockstack_trimmed(noun, expected.len());
            assert_roundtrip_matrix(jammed, true);
        }
    }

    #[test]
    fn chaff_parity_cue_large_checkpoint() {
        let jammed = large_checkpoint_jam();
        let stack_words = stack_words_for_large_checkpoint(jammed.len());
        let (chaff_slab, chaff_noun, _nock_slab, nock_noun, _stack, nockvm_noun) =
            cue_all_with_stack(&jammed, stack_words);
        assert!(
            slab_noun_equality(&chaff_noun, &nock_noun),
            "NockJammer cue differs from Chaff cue"
        );
        assert!(
            slab_noun_equality(&chaff_noun, &nockvm_noun),
            "nockvm cue differs from Chaff cue"
        );
        assert_eq!(chaff_slab.jam(), jammed, "chaff jam differs from input jam");
    }

    #[test]
    fn nockjammer_parity_cue_large_checkpoint() {
        let jammed = large_checkpoint_jam();
        let stack_words = stack_words_for_large_checkpoint(jammed.len());
        let (_chaff_slab, chaff_noun, _nock_slab, nock_noun, _stack, nockvm_noun) =
            cue_all_with_stack(&jammed, stack_words);
        assert!(
            slab_noun_equality(&nock_noun, &chaff_noun),
            "Chaff cue differs from NockJammer cue"
        );
        assert!(
            slab_noun_equality(&nock_noun, &nockvm_noun),
            "nockvm cue differs from NockJammer cue"
        );
        assert_eq!(
            NockJammer::jam(nock_noun),
            jammed,
            "NockJammer jam differs from input jam"
        );
    }

    #[test]
    fn nockstack_parity_cue_large_checkpoint() {
        let jammed = large_checkpoint_jam();
        let stack_words = stack_words_for_large_checkpoint(jammed.len());
        let (_chaff_slab, chaff_noun, _nock_slab, nock_noun, mut stack, nockvm_noun) =
            cue_all_with_stack(&jammed, stack_words);
        assert!(
            slab_noun_equality(&nockvm_noun, &chaff_noun),
            "Chaff cue differs from nockvm cue"
        );
        assert!(
            slab_noun_equality(&nockvm_noun, &nock_noun),
            "NockJammer cue differs from nockvm cue"
        );
        let nockvm_jam = nockvm::serialization::jam(&mut stack, nockvm_noun);
        let nockvm_bytes = nockvm_jam.as_ne_bytes();
        assert!(
            nockvm_bytes.len() >= jammed.len(),
            "nockvm jam shorter than input jam"
        );
        assert_eq!(
            &nockvm_bytes[..jammed.len()],
            jammed.as_ref(),
            "nockvm jam prefix differs from input jam"
        );
        assert!(
            nockvm_bytes[jammed.len()..].iter().all(|b| *b == 0),
            "nockvm jam has non-zero padding bytes"
        );
    }

    #[tokio::test]
    async fn saver_chaff_loads_v1_checkpoint() {
        let temp = TempDir::new().expect("create temp dir");
        let state_value = 5;
        let cold_value = 9;
        let checkpoint = JammedCheckpointV1::new(
            hash(b"legacy-v1-chaff"),
            7,
            legacy_pair_jam(state_value, cold_value),
        );
        let saveable =
            load_saveable_with_chaff(&temp, checkpoint.encode().expect("encode v1")).await;
        assert_loaded_values(
            &saveable,
            hash(b"legacy-v1-chaff"),
            7,
            state_value,
            cold_value,
        );
    }

    #[tokio::test]
    async fn saver_chaff_loads_v0_checkpoint() {
        let temp = TempDir::new().expect("create temp dir");
        let state_value = 11;
        let cold_value = 22;
        let checkpoint = JammedCheckpointV0::new(
            false,
            hash(b"legacy-v0-chaff"),
            3,
            legacy_pair_jam(state_value, cold_value),
        );
        let saveable =
            load_saveable_with_chaff(&temp, checkpoint.encode().expect("encode v0")).await;
        assert_loaded_values(
            &saveable,
            hash(b"legacy-v0-chaff"),
            3,
            state_value,
            cold_value,
        );
    }

    #[tokio::test]
    async fn saver_chaff_loads_v2_checkpoint() {
        let temp = TempDir::new().expect("create temp dir");
        let state_value = 1_000_001;
        let cold_value = 2_000_002;
        let checkpoint = JammedCheckpointV2::new(
            hash(b"v2-chaff"),
            42,
            jam_atom_with_chaff(cold_value),
            jam_atom_with_chaff(state_value),
        );
        let saveable =
            load_saveable_with_chaff(&temp, checkpoint.encode().expect("encode v2")).await;
        assert_loaded_values(&saveable, hash(b"v2-chaff"), 42, state_value, cold_value);
    }

    #[test]
    fn prop_chaff_roundtrip_tree_tier1() {
        let tests = qc_test_count("CHAFF_QC_TIER1_ROUNDTRIP_TESTS", 96);
        quickcheck::QuickCheck::new()
            .tests(tests)
            .quickcheck(prop_chaff_roundtrip_tree_tier1_case as fn(Tier1Tree) -> TestResult);
    }

    fn prop_chaff_roundtrip_tree_tier1_case(tree: Tier1Tree) -> TestResult {
        prop_chaff_roundtrip_tree_impl(&tree.0)
    }

    #[test]
    fn prop_chaff_roundtrip_tree_tier2() {
        let tests = qc_test_count("CHAFF_QC_TIER2_ROUNDTRIP_TESTS", 24);
        quickcheck::QuickCheck::new()
            .tests(tests)
            .quickcheck(prop_chaff_roundtrip_tree_tier2_case as fn(Tier2Tree) -> TestResult);
    }

    fn prop_chaff_roundtrip_tree_tier2_case(tree: Tier2Tree) -> TestResult {
        prop_chaff_roundtrip_tree_impl(&tree.0)
    }

    #[test]
    fn prop_chaff_roundtrip_tree_tier3() {
        let tests = qc_test_count("CHAFF_QC_TIER3_ROUNDTRIP_TESTS", 8);
        quickcheck::QuickCheck::new()
            .tests(tests)
            .quickcheck(prop_chaff_roundtrip_tree_tier3_case as fn(Tier3Tree) -> TestResult);
    }

    fn prop_chaff_roundtrip_tree_tier3_case(tree: Tier3Tree) -> TestResult {
        prop_chaff_roundtrip_tree_impl(&tree.0)
    }

    #[test]
    fn prop_chaff_matches_nock_tree() {
        let tests = qc_test_count("CHAFF_QC_MATCH_NOCK_TESTS", 64);
        quickcheck::QuickCheck::new()
            .tests(tests)
            .quickcheck(prop_chaff_matches_nock_tree_case as fn(Tier1Tree) -> TestResult);
    }

    fn prop_chaff_matches_nock_tree_case(tree: Tier1Tree) -> TestResult {
        prop_chaff_matches_nock_tree_impl(&tree.0)
    }

    #[test]
    fn prop_chaff_parity_tree_matrix() {
        let tests = qc_test_count("CHAFF_QC_PARITY_MATRIX_TESTS", 40);
        quickcheck::QuickCheck::new()
            .tests(tests)
            .quickcheck(prop_chaff_parity_tree_matrix_case as fn(Tier1Tree) -> TestResult);
    }

    fn prop_chaff_parity_tree_matrix_case(tree: Tier1Tree) -> TestResult {
        prop_chaff_parity_tree_matrix_impl(&tree.0)
    }

    #[test]
    fn prop_chaff_parity_shared_tree() {
        let tests = qc_test_count("CHAFF_QC_PARITY_SHARED_TESTS", 40);
        quickcheck::QuickCheck::new().tests(tests).quickcheck(
            prop_chaff_parity_shared_tree_case as fn(Tier1Tree, u8, usize) -> TestResult,
        );
    }

    fn prop_chaff_parity_shared_tree_case(
        tree: Tier1Tree,
        mode: u8,
        prefix_seed: usize,
    ) -> TestResult {
        prop_chaff_parity_shared_tree_impl(&tree.0, mode, prefix_seed)
    }

    #[test]
    fn prop_chaff_idempotence_chain_tree() {
        let tests = qc_test_count("CHAFF_QC_CHAIN_TESTS", 32);
        quickcheck::QuickCheck::new()
            .tests(tests)
            .quickcheck(prop_chaff_idempotence_chain_tree_case as fn(Tier1Tree) -> TestResult);
    }

    fn prop_chaff_idempotence_chain_tree_case(tree: Tier1Tree) -> TestResult {
        prop_chaff_idempotence_chain_tree_impl(&tree.0)
    }

    #[test]
    fn prop_chaff_adversarial_mutations_no_panic() {
        let tests = qc_test_count("CHAFF_QC_ADVERSARIAL_TESTS", 80);
        quickcheck::QuickCheck::new().tests(tests).quickcheck(
            prop_chaff_adversarial_mutations_no_panic_case as fn(JamMutationCase) -> TestResult,
        );
    }

    fn prop_chaff_adversarial_mutations_no_panic_case(case: JamMutationCase) -> TestResult {
        prop_chaff_adversarial_mutations_no_panic_impl(case)
    }

    #[test]
    fn prop_chaff_adversarial_mutations_differential() {
        let tests = qc_test_count("CHAFF_QC_ADVERSARIAL_DIFF_TESTS", 64);
        quickcheck::QuickCheck::new().tests(tests).quickcheck(
            prop_chaff_adversarial_mutations_differential_case as fn(JamMutationCase) -> TestResult,
        );
    }

    fn prop_chaff_adversarial_mutations_differential_case(case: JamMutationCase) -> TestResult {
        prop_chaff_adversarial_mutations_differential_impl(case)
    }
}
