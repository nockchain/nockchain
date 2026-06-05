use bytes::Bytes;

/// Sequential little-endian bit reader over an immutable byte buffer
pub struct BitReader {
    bytes: Bytes,
    bit_len: usize,
    bit_pos: usize,
}

impl BitReader {
    #[inline]
    pub fn new(bytes: Bytes) -> Self {
        let bit_len = bytes.len() * 8;
        Self {
            bytes,
            bit_len,
            bit_pos: 0,
        }
    }

    #[inline]
    pub fn position(&self) -> usize {
        self.bit_pos
    }

    #[inline]
    pub fn bits_remaining(&self) -> usize {
        self.bit_len.saturating_sub(self.bit_pos)
    }

    #[inline]
    pub fn read_bit(&mut self) -> Option<bool> {
        if self.bit_pos >= self.bit_len {
            return None;
        }
        let byte = self.bytes[self.bit_pos >> 3];
        let bit = ((byte >> (self.bit_pos & 7)) & 1) == 1;
        self.bit_pos += 1;
        Some(bit)
    }

    #[inline]
    pub fn read_bits_to_usize(&mut self, count: usize) -> Option<usize> {
        self.read_bits_to_u64(count).map(|v| v as usize)
    }

    #[inline]
    pub fn read_bits_to_bytes(&mut self, dest: &mut [u8], bits: usize) -> Option<()> {
        if bits == 0 {
            return Some(());
        }
        let bytes_needed = (bits + 7) >> 3;
        if dest.len() < bytes_needed || self.bits_remaining() < bits {
            return None;
        }
        let full_bytes = bits / 8;
        for dest_byte in dest.iter_mut().take(full_bytes) {
            *dest_byte = self.read_bits_to_u64(8)? as u8;
        }
        let rem = bits & 7;
        if rem > 0 {
            dest[full_bytes] = self.read_bits_to_u64(rem)? as u8;
        }
        Some(())
    }

    #[inline]
    pub fn read_unary(&mut self) -> Option<usize> {
        let mut zeros = 0usize;
        let mut pos = self.bit_pos;
        while pos < self.bit_len {
            let byte_idx = pos >> 3;
            let bit_offset = pos & 7;
            let available = (self.bit_len - pos).min(8 - bit_offset);
            if available == 0 {
                break;
            }
            let mut byte = self.bytes[byte_idx] >> bit_offset;
            if available < 8 - bit_offset {
                let mask = if available == 8 {
                    0xFF
                } else {
                    ((1u16 << available) - 1) as u8
                };
                byte &= mask;
            }
            if byte != 0 {
                let tz = byte.trailing_zeros() as usize;
                zeros += tz;
                pos += tz + 1;
                self.bit_pos = pos;
                return Some(zeros);
            } else {
                zeros += available;
                pos += available;
            }
        }
        None
    }

    #[inline]
    fn read_bits_to_u64(&mut self, count: usize) -> Option<u64> {
        if count == 0 {
            return Some(0);
        }
        if count > 64 || self.bits_remaining() < count {
            return None;
        }
        let mut value = 0u64;
        let mut bits_read = 0usize;
        let mut pos = self.bit_pos;
        while bits_read < count {
            let byte_idx = pos >> 3;
            let bit_offset = pos & 7;
            let take = (count - bits_read).min(8 - bit_offset);
            let mask = if take == 8 {
                0xFF
            } else {
                ((1u16 << take) - 1) as u8
            };
            let chunk = ((self.bytes[byte_idx] >> bit_offset) & mask) as u64;
            value |= chunk << bits_read;
            pos += take;
            bits_read += take;
        }
        self.bit_pos = pos;
        Some(value)
    }
}

// Fast bit writer that appends bits LSB-first into an underlying Vec<u8>
pub struct BitWriter {
    buf: Vec<u8>,   // final byte buffer (little-endian bit order per byte)
    acc: u8,        // in-progress byte accumulator
    nbits: u8,      // number of bits currently stored in `acc` (0-7)
    bit_len: usize, // total number of bits written so far
}
impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl BitWriter {
    #[inline]
    pub fn new() -> Self {
        BitWriter {
            buf: Vec::with_capacity(1024),
            acc: 0,
            nbits: 0,
            bit_len: 0,
        }
    }

    #[inline]
    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    #[inline]
    pub fn write_bit(&mut self, bit: bool) {
        if bit {
            self.acc |= 1 << self.nbits;
        }
        self.nbits += 1;
        self.bit_len += 1;
        if self.nbits == 8 {
            self.flush_acc();
        }
    }

    #[inline]
    pub fn write_zeros(&mut self, count: usize) {
        // produce `count` zero bits quickly
        // Fill partial acc first
        let mut remaining = count;
        if self.nbits != 0 {
            let space = 8 - self.nbits;
            if remaining < space as usize {
                // // just bump counters – acc already contains zeros in high bits
                // self.nbits += remaining as u8;
                // self.bit_len += remaining;
                // return;
                // keep the valid low bits we already had, clear the bits we are about to add
                let mask = (1u16 << self.nbits) - 1; // e.g. nbits = 3  -> 0b00000111
                self.acc &= mask as u8; // zero out bits [self.nbits .. 7]

                // now bump the cursors exactly as before
                self.nbits += remaining as u8;
                self.bit_len += remaining;
                return;
            } else {
                // fill acc with zeros and flush
                // self.nbits = 8;
                // self.bit_len += space as usize;
                // remaining -= space as usize;
                // zero-fill high bits we are about to claim
                let mask = (1u16 << self.nbits) - 1; // keep the `nbits` low bits
                self.acc &= mask as u8; // clear [self.nbits .. 7]

                // now top-off the byte and flush
                self.nbits = 8;
                self.bit_len += space as usize;
                remaining -= space as usize;
                self.flush_acc();
            }
        }
        // Now we are byte-aligned
        let full_bytes = remaining / 8;
        if full_bytes > 0 {
            self.buf.extend(std::iter::repeat_n(0u8, full_bytes));
            self.bit_len += full_bytes * 8;
            remaining -= full_bytes * 8;
        }
        // Remaining < 8, leave in acc (which is zero)
        self.nbits = remaining as u8;
        self.acc = 0; // already zero
        self.bit_len += remaining;
    }

    #[inline]
    pub fn write_bits_from_value(&mut self, mut value: usize, count: usize) {
        for _ in 0..count {
            self.write_bit((value & 1) == 1);
            value >>= 1;
        }
    }

    #[inline]
    pub fn write_bits_from_le_bytes(&mut self, bytes: &[u8], total_bits: usize) {
        if total_bits == 0 {
            return;
        }

        let full_bytes = total_bits / 8;
        let rem_bits: usize = total_bits % 8;

        if self.nbits == 0 {
            // Aligned path: copy full bytes directly
            if full_bytes > 0 {
                self.buf.extend_from_slice(&bytes[..full_bytes]);
                self.bit_len += full_bytes * 8;
            }
        } else if full_bytes > 0 {
            // Unaligned path: merge each byte with current accumulator
            let shift = self.nbits;
            let mut carry = self.acc;
            for &byte in &bytes[..full_bytes] {
                let combined = carry | (byte << shift);
                self.buf.push(combined);
                self.bit_len += 8;
                carry = byte >> (8 - shift);
            }
            self.acc = carry;
            // note: nbits unchanged
        }

        // Handle remaining bits (<8) from the next byte
        if rem_bits > 0 {
            let src_byte = if full_bytes < bytes.len() {
                bytes[full_bytes]
            } else {
                0
            };
            for i in 0..rem_bits {
                self.write_bit(((src_byte >> i) & 1) == 1);
            }
        }
        // Update bit_len to reflect the total number of bits written so far
        // This didn't work.
        // self.bit_len = self.buf.len() * 8 + self.nbits as usize;
    }

    #[inline]
    pub fn flush_acc(&mut self) {
        if self.nbits == 0 {
            return;
        }
        self.buf.push(self.acc);
        self.acc = 0;
        self.nbits = 0;
    }

    pub fn into_bytes(mut self) -> Bytes {
        if self.nbits > 0 {
            // Flush final partial byte (upper bits remain 0)
            self.flush_acc();
        }
        Bytes::from(self.buf)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use quickcheck::{Arbitrary, Gen, TestResult};

    use super::{BitReader, BitWriter};

    fn read_bits_to_vec(mut reader: BitReader, bits: usize) -> Option<Vec<u8>> {
        if bits == 0 {
            return Some(Vec::new());
        }
        let mut out = vec![0u8; (bits + 7) >> 3];
        reader.read_bits_to_bytes(&mut out, bits)?;
        Some(out)
    }

    const WIDTH_BOUNDARIES: [usize; 15] = [0, 1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 129];

    #[derive(Clone, Debug)]
    enum WriterOp {
        Bit(bool),
        Zeros(usize),
        Value { value: usize, bits: usize },
        Bytes { payload: Vec<u8>, bits: usize },
    }

    #[derive(Clone, Debug)]
    struct WriterProgram {
        ops: Vec<WriterOp>,
    }

    impl Arbitrary for WriterOp {
        fn arbitrary(g: &mut Gen) -> Self {
            match usize::arbitrary(g) % 4 {
                0 => Self::Bit(bool::arbitrary(g)),
                1 => Self::Zeros(boundary_or_random(g, 4096)),
                2 => Self::Value {
                    value: usize::arbitrary(g),
                    bits: boundary_or_random(g, usize::BITS as usize),
                },
                _ => {
                    let bits = boundary_or_random(g, 4096);
                    let payload_len = bits.div_ceil(8) + (usize::arbitrary(g) % 4);
                    let payload = (0..payload_len).map(|_| u8::arbitrary(g)).collect();
                    Self::Bytes { payload, bits }
                }
            }
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(std::iter::empty())
        }
    }

    impl Arbitrary for WriterProgram {
        fn arbitrary(g: &mut Gen) -> Self {
            let len = 1 + (usize::arbitrary(g) % 48);
            let mut ops = Vec::with_capacity(len);
            for _ in 0..len {
                ops.push(WriterOp::arbitrary(g));
            }
            Self { ops }
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            let mut out = Vec::new();
            if !self.ops.is_empty() {
                let mut tail = self.ops.clone();
                tail.pop();
                if !tail.is_empty() {
                    out.push(Self { ops: tail });
                }
            }
            Box::new(out.into_iter())
        }
    }

    fn boundary_or_random(g: &mut Gen, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        if (usize::arbitrary(g) % 10) < 7 {
            WIDTH_BOUNDARIES[usize::arbitrary(g) % WIDTH_BOUNDARIES.len()].min(max)
        } else {
            usize::arbitrary(g) % (max + 1)
        }
    }

    fn push_bits_from_value_model(bits: &mut Vec<bool>, mut value: usize, count: usize) {
        for _ in 0..count {
            bits.push((value & 1) == 1);
            value >>= 1;
        }
    }

    fn push_bits_from_le_bytes_model(bits: &mut Vec<bool>, payload: &[u8], total_bits: usize) {
        for bit_idx in 0..total_bits {
            let byte_idx = bit_idx >> 3;
            let bit_off = bit_idx & 7;
            let byte = payload.get(byte_idx).copied().unwrap_or(0);
            bits.push(((byte >> bit_off) & 1) == 1);
        }
    }

    fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
        let mut out = vec![0u8; bits.len().div_ceil(8)];
        for (idx, bit) in bits.iter().copied().enumerate() {
            if bit {
                out[idx >> 3] |= 1u8 << (idx & 7);
            }
        }
        out
    }

    fn truncate_bits(bytes: &[u8], keep_bits: usize) -> Vec<u8> {
        if keep_bits == 0 {
            return Vec::new();
        }
        let total = bytes.len() * 8;
        let keep = keep_bits.min(total);
        let byte_len = keep.div_ceil(8);
        let mut out = bytes[..byte_len].to_vec();
        let rem = keep & 7;
        if rem != 0 {
            let mask = (1u8 << rem) - 1;
            let idx = out.len() - 1;
            out[idx] &= mask;
        }
        out
    }

    #[derive(Clone, Copy, Debug)]
    enum ExhaustiveOp {
        Bit0,
        Bit1,
        Zeros1,
        Zeros8,
        Value1Bits1,
        Value3Bits2,
        ValueAbBits8,
        BytesA5Bits7,
        BytesA55ABits9,
    }

    const EXHAUSTIVE_OPS: [ExhaustiveOp; 9] = [
        ExhaustiveOp::Bit0,
        ExhaustiveOp::Bit1,
        ExhaustiveOp::Zeros1,
        ExhaustiveOp::Zeros8,
        ExhaustiveOp::Value1Bits1,
        ExhaustiveOp::Value3Bits2,
        ExhaustiveOp::ValueAbBits8,
        ExhaustiveOp::BytesA5Bits7,
        ExhaustiveOp::BytesA55ABits9,
    ];

    fn apply_exhaustive_op(writer: &mut BitWriter, model: &mut Vec<bool>, op: ExhaustiveOp) {
        match op {
            ExhaustiveOp::Bit0 => {
                writer.write_bit(false);
                model.push(false);
            }
            ExhaustiveOp::Bit1 => {
                writer.write_bit(true);
                model.push(true);
            }
            ExhaustiveOp::Zeros1 => {
                writer.write_zeros(1);
                model.push(false);
            }
            ExhaustiveOp::Zeros8 => {
                writer.write_zeros(8);
                model.extend(std::iter::repeat_n(false, 8));
            }
            ExhaustiveOp::Value1Bits1 => {
                writer.write_bits_from_value(1, 1);
                push_bits_from_value_model(model, 1, 1);
            }
            ExhaustiveOp::Value3Bits2 => {
                writer.write_bits_from_value(3, 2);
                push_bits_from_value_model(model, 3, 2);
            }
            ExhaustiveOp::ValueAbBits8 => {
                writer.write_bits_from_value(0xAB, 8);
                push_bits_from_value_model(model, 0xAB, 8);
            }
            ExhaustiveOp::BytesA5Bits7 => {
                writer.write_bits_from_le_bytes(&[0xA5], 7);
                push_bits_from_le_bytes_model(model, &[0xA5], 7);
            }
            ExhaustiveOp::BytesA55ABits9 => {
                writer.write_bits_from_le_bytes(&[0xA5, 0x5A], 9);
                push_bits_from_le_bytes_model(model, &[0xA5, 0x5A], 9);
            }
        }
    }

    #[test]
    fn reads_unary_across_bytes() {
        let bytes = Bytes::from(vec![0b0000_0000, 0b0001_0000]);
        let mut reader = BitReader::new(bytes);
        let zeros = reader.read_unary().expect("unary should succeed");
        assert_eq!(zeros, 12);
        assert_eq!(reader.position(), 13);
    }

    #[test]
    fn reads_bits_to_u64_with_offset() {
        let bytes = Bytes::from(vec![0b1010_1100, 0b0101_1110]);
        let mut reader = BitReader::new(bytes);
        assert_eq!(reader.read_bits_to_u64(3), Some(0b100));
        assert_eq!(reader.read_bits_to_u64(5), Some(0b10101));
        assert_eq!(reader.read_bits_to_u64(8), Some(0b0101_1110));
        assert_eq!(reader.read_bits_to_u64(1), None);
    }

    #[test]
    fn writes_zeros_preserve_existing_bits() {
        let mut writer = BitWriter::new();
        writer.write_bits_from_value(0b101, 3);
        writer.write_zeros(5);
        writer.write_bits_from_value(0b11, 2);
        assert_eq!(writer.bit_len(), 10);
        let bytes = writer.into_bytes();
        let mut reader = BitReader::new(bytes);
        let mut out = [0u8; 2];
        reader.read_bits_to_bytes(&mut out, 10).expect("read bits");
        assert_eq!(out[0], 0b0000_0101);
        assert_eq!(out[1] & 0b11, 0b11);
    }

    #[test]
    fn write_bits_from_le_bytes_roundtrip() {
        let bytes = Bytes::from(vec![0xA5, 0x5A, 0xFF]);
        let mut writer = BitWriter::new();
        writer.write_bits_from_le_bytes(&bytes, 20);
        let written = writer.into_bytes();
        let mut reader = BitReader::new(written);
        let mut out = [0u8; 3];
        reader.read_bits_to_bytes(&mut out, 20).expect("read bits");
        assert_eq!(out[0], 0xA5);
        assert_eq!(out[1], 0x5A);
        assert_eq!(out[2] & 0x0F, 0x0F);
    }

    #[test]
    fn write_zeros_then_read_zero_bits() {
        let mut writer = BitWriter::new();
        writer.write_zeros(24);
        let bytes = writer.into_bytes();
        let mut reader = BitReader::new(bytes);
        let mut out = [0xFFu8; 3];
        reader.read_bits_to_bytes(&mut out, 24).expect("read bits");
        assert_eq!(out, [0u8; 3]);
    }

    #[test]
    fn read_bits_to_bytes_exact_remaining() {
        let bytes = Bytes::from(vec![0x01]);
        let mut reader = BitReader::new(bytes);
        let mut out = [0u8; 1];
        assert!(reader.read_bits_to_bytes(&mut out, 8).is_some());
        assert_eq!(out[0], 0x01);
        assert!(reader.read_bits_to_bytes(&mut out, 1).is_none());
    }

    #[test]
    fn write_zeros_byte_aligned() {
        let mut writer = BitWriter::new();
        writer.write_zeros(16);
        assert_eq!(writer.bit_len(), 16);
        let bytes = writer.into_bytes();
        assert_eq!(bytes.as_ref(), &[0u8, 0u8]);
    }

    #[test]
    fn read_bits_returns_none_when_too_long() {
        let bytes = Bytes::from(vec![0xFF]);
        let mut reader = BitReader::new(bytes);
        assert!(reader.read_bits_to_u64(65).is_none());
    }

    #[test]
    fn exhaustive_short_writer_sequences_match_model() {
        let base = EXHAUSTIVE_OPS.len();
        let max_len = 4usize;
        for len in 0..=max_len {
            let total = base.pow(len as u32);
            for idx in 0..total {
                let mut code = idx;
                let mut writer = BitWriter::new();
                let mut model = Vec::new();
                for _ in 0..len {
                    let op = EXHAUSTIVE_OPS[code % base];
                    code /= base;
                    apply_exhaustive_op(&mut writer, &mut model, op);
                }

                assert_eq!(
                    writer.bit_len(),
                    model.len(),
                    "bit_len mismatch for len={len} idx={idx}"
                );
                let expected = bits_to_bytes(&model);
                let written = writer.into_bytes();
                assert_eq!(
                    written.as_ref(),
                    expected.as_slice(),
                    "bytes mismatch for len={len} idx={idx}"
                );

                let mut reader = BitReader::new(written);
                let mut read_back = vec![0u8; expected.len()];
                assert!(
                    reader
                        .read_bits_to_bytes(&mut read_back, model.len())
                        .is_some(),
                    "failed to read exact model bits for len={len} idx={idx}"
                );
                assert_eq!(
                    read_back, expected,
                    "read-back mismatch for len={len} idx={idx}"
                );
            }
        }
    }

    #[test]
    fn exhaustive_small_unary_prefix_space() {
        for zeros in 0..=16usize {
            for suffix in 0u16..=0xFFu16 {
                let mut writer = BitWriter::new();
                writer.write_zeros(zeros);
                writer.write_bit(true);
                writer.write_bits_from_value(suffix as usize, 8);

                let bytes = writer.into_bytes();
                let mut reader = BitReader::new(bytes);
                assert_eq!(
                    reader.read_unary(),
                    Some(zeros),
                    "unary mismatch for zeros={zeros} suffix={suffix}"
                );
                assert_eq!(
                    reader.read_bits_to_u64(8),
                    Some(suffix as u64),
                    "suffix mismatch for zeros={zeros} suffix={suffix}"
                );
            }
        }
    }

    quickcheck::quickcheck! {
        fn prop_write_then_read_bits(payload: Vec<u8>, bit_count: usize) -> TestResult {
            let bits = bit_count % 129;
            if bits == 0 {
                return TestResult::passed();
            }
            let needed = (bits + 7) >> 3;
            let mut data = payload;
            data.resize(needed, 0u8);

            let mut writer = BitWriter::new();
            writer.write_bits_from_le_bytes(&data, bits);
            let written = writer.into_bytes();

            let reader = BitReader::new(written);
            let read_back = read_bits_to_vec(reader, bits);
            if let Some(mut bytes) = read_back {
                let mask_bits = bits & 7;
                if mask_bits != 0 {
                    let mask = (1u8 << mask_bits) - 1;
                    let last_idx = needed - 1;
                    bytes[last_idx] &= mask;
                    data[last_idx] &= mask;
                }
                return TestResult::from_bool(bytes == data);
            }

            TestResult::error("failed to read written bits")
        }

        fn prop_zero_bits_roundtrip(count: usize) -> TestResult {
            let bits = count % 257;
            let mut writer = BitWriter::new();
            writer.write_zeros(bits);
            let bytes = writer.into_bytes();
            let reader = BitReader::new(bytes);
            let read_back = read_bits_to_vec(reader, bits).unwrap_or_default();
            let expected = vec![0u8; (bits + 7) >> 3];
            TestResult::from_bool(read_back == expected)
        }

        fn prop_writer_program_matches_model(program: WriterProgram) -> TestResult {
            let mut writer = BitWriter::new();
            let mut expected_bits = Vec::new();

            for op in program.ops {
                match op {
                    WriterOp::Bit(bit) => {
                        writer.write_bit(bit);
                        expected_bits.push(bit);
                    }
                    WriterOp::Zeros(count) => {
                        writer.write_zeros(count);
                        expected_bits.extend(std::iter::repeat_n(false, count));
                    }
                    WriterOp::Value { value, bits } => {
                        writer.write_bits_from_value(value, bits);
                        push_bits_from_value_model(&mut expected_bits, value, bits);
                    }
                    WriterOp::Bytes { payload, bits } => {
                        writer.write_bits_from_le_bytes(&payload, bits);
                        push_bits_from_le_bytes_model(&mut expected_bits, &payload, bits);
                    }
                }
            }

            if writer.bit_len() != expected_bits.len() {
                return TestResult::failed();
            }

            let expected_bytes = bits_to_bytes(&expected_bits);
            let written = writer.into_bytes();
            if written.as_ref() != expected_bytes.as_slice() {
                return TestResult::failed();
            }

            if expected_bits.is_empty() {
                return TestResult::passed();
            }

            let mut reader = BitReader::new(written);
            let mut read_back = vec![0u8; expected_bits.len().div_ceil(8)];
            if reader.read_bits_to_bytes(&mut read_back, expected_bits.len()).is_none() {
                return TestResult::failed();
            }
            if read_back != expected_bytes {
                return TestResult::failed();
            }

            let padded_bits = expected_bytes.len() * 8 - expected_bits.len();
            if padded_bits > 0 {
                let mut padding = vec![0u8; padded_bits.div_ceil(8)];
                if reader.read_bits_to_bytes(&mut padding, padded_bits).is_none() {
                    return TestResult::failed();
                }
                if padding.iter().any(|byte| *byte != 0) {
                    return TestResult::failed();
                }
            }

            TestResult::passed()
        }

        fn prop_read_unary_matches_model(
            zero_count: usize,
            suffix_payload: Vec<u8>,
            suffix_bits_seed: usize
        ) -> TestResult {
            let zeros = zero_count % 4097;
            let suffix_bits = suffix_bits_seed % 257;
            let needed = suffix_bits.div_ceil(8);
            let mut payload = suffix_payload;
            payload.resize(needed, 0u8);

            let mut writer = BitWriter::new();
            writer.write_zeros(zeros);
            writer.write_bit(true);
            writer.write_bits_from_le_bytes(&payload, suffix_bits);
            let bytes = writer.into_bytes();

            let mut reader = BitReader::new(bytes);
            let unary = reader.read_unary();
            if unary != Some(zeros) {
                return TestResult::failed();
            }

            let mut suffix = vec![0u8; needed];
            if reader.read_bits_to_bytes(&mut suffix, suffix_bits).is_none() {
                return TestResult::failed();
            }
            if suffix_bits & 7 != 0 && !suffix.is_empty() {
                let mask = (1u8 << (suffix_bits & 7)) - 1;
                let idx = suffix.len() - 1;
                suffix[idx] &= mask;
                payload[idx] &= mask;
            }
            TestResult::from_bool(suffix == payload)
        }

        fn prop_read_unary_truncated_returns_none(zero_count: usize) -> TestResult {
            let zeros = zero_count % 4097;
            let mut writer = BitWriter::new();
            writer.write_zeros(zeros);
            let bytes = writer.into_bytes();
            let mut reader = BitReader::new(bytes);
            TestResult::from_bool(reader.read_unary().is_none())
        }

        fn prop_read_bits_exact_or_short(payload: Vec<u8>, bit_count: usize) -> TestResult {
            let bits = bit_count % 2049;
            if bits == 0 {
                return TestResult::passed();
            }
            let needed = bits.div_ceil(8);
            let mut data = payload;
            data.resize(needed, 0u8);

            let mut writer = BitWriter::new();
            writer.write_bits_from_le_bytes(&data, bits);
            let bytes = writer.into_bytes();

            let mut exact_reader = BitReader::new(bytes.clone());
            let mut exact = vec![0u8; needed];
            if exact_reader.read_bits_to_bytes(&mut exact, bits).is_none() {
                return TestResult::failed();
            }
            if bits & 7 != 0 {
                let mask = (1u8 << (bits & 7)) - 1;
                let idx = needed - 1;
                exact[idx] &= mask;
                data[idx] &= mask;
            }
            if exact != data {
                return TestResult::failed();
            }

            let short_bits = bits.saturating_sub(8);
            let truncated = truncate_bits(bytes.as_ref(), short_bits);
            let mut short_reader = BitReader::new(Bytes::from(truncated));
            let mut short = vec![0u8; needed];
            TestResult::from_bool(short_reader.read_bits_to_bytes(&mut short, bits).is_none())
        }

        fn prop_read_bits_to_usize_width_limit(value: usize, width_seed: usize) -> TestResult {
            let bits = width_seed % (usize::BITS as usize + 1);
            let mut writer = BitWriter::new();
            writer.write_bits_from_value(value, bits);
            let mut reader = BitReader::new(writer.into_bytes());

            let expected_mask = if bits == 0 {
                0
            } else if bits >= usize::BITS as usize {
                usize::MAX
            } else {
                (1usize << bits) - 1
            };
            if reader.read_bits_to_usize(bits) != Some(value & expected_mask) {
                return TestResult::failed();
            }

            let mut too_wide = BitReader::new(Bytes::from(vec![0xFFu8; 16]));
            TestResult::from_bool(too_wide.read_bits_to_usize(usize::BITS as usize + 1).is_none())
        }
    }
}
