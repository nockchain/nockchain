use std::collections::HashMap;
use std::io::{self, Write};

use either::Either;
use nockvm::noun::{Atom, Noun, NounSpace, D};
use nockvm::pma::{classify_pma_noun, PmaDirectJamError, PmaDirectReader, PmaRawNounKind};
use nockvm::serialization::{met0_u64_to_usize, met0_usize};

const STREAM_BUF_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct JamStreamStats {
    pub bit_len: usize,
    pub byte_len: usize,
}

pub struct StreamingBitWriter<'a, W: Write> {
    writer: &'a mut W,
    buf: Vec<u8>,
    current_byte: u8,
    current_bits: u8,
    bit_len: usize,
}

impl<'a, W: Write> StreamingBitWriter<'a, W> {
    pub fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            buf: Vec::with_capacity(STREAM_BUF_BYTES),
            current_byte: 0,
            current_bits: 0,
            bit_len: 0,
        }
    }

    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    fn flush_buf(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            self.writer.write_all(&self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.buf.is_empty() && bytes.len() >= STREAM_BUF_BYTES {
            self.writer.write_all(bytes)?;
            return Ok(());
        }
        let mut start = 0;
        while start < bytes.len() {
            let remaining = STREAM_BUF_BYTES - self.buf.len();
            if remaining == 0 {
                self.flush_buf()?;
            }
            let end = (start + remaining).min(bytes.len());
            self.buf.extend_from_slice(&bytes[start..end]);
            start = end;
        }
        Ok(())
    }

    fn push_zero_bytes(&mut self, mut count: usize) -> io::Result<()> {
        let zeros = [0u8; 4096];
        while count > 0 {
            let chunk = count.min(zeros.len());
            self.push_bytes(&zeros[..chunk])?;
            count -= chunk;
        }
        Ok(())
    }

    pub fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        if bit {
            self.current_byte |= 1u8 << self.current_bits;
        }
        self.current_bits += 1;
        self.bit_len += 1;
        if self.current_bits == 8 {
            self.push_bytes(&[self.current_byte])?;
            self.current_byte = 0;
            self.current_bits = 0;
        }
        Ok(())
    }

    pub fn write_zeros(&mut self, count: usize) -> io::Result<()> {
        let mut remaining = count;
        if self.current_bits == 0 {
            let full_bytes = remaining / 8;
            if full_bytes > 0 {
                self.push_zero_bytes(full_bytes)?;
                self.bit_len += full_bytes * 8;
                remaining -= full_bytes * 8;
            }
        }
        for _ in 0..remaining {
            self.write_bit(false)?;
        }
        Ok(())
    }

    pub fn write_bits_from_value(&mut self, mut value: u64, bits: usize) -> io::Result<()> {
        let mut remaining = bits;
        if self.current_bits == 0 {
            while remaining >= 8 {
                self.push_bytes(&[value as u8])?;
                self.bit_len += 8;
                value >>= 8;
                remaining -= 8;
            }
        }
        for _ in 0..remaining {
            self.write_bit((value & 1) != 0)?;
            value >>= 1;
        }
        Ok(())
    }

    pub fn write_bits_from_le_bytes(&mut self, bytes: &[u8], bits: usize) -> io::Result<()> {
        let mut remaining = bits;
        let mut offset = 0usize;
        if self.current_bits == 0 {
            let full_bytes = remaining / 8;
            if full_bytes > 0 {
                self.push_bytes(&bytes[..full_bytes])?;
                self.bit_len += full_bytes * 8;
                remaining -= full_bytes * 8;
                offset = full_bytes;
            }
        }
        if remaining > 0 {
            if self.current_bits == 0 {
                let byte = bytes
                    .get(offset)
                    .copied()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "atom bytes"))?;
                for bit in 0..remaining {
                    self.write_bit((byte & (1u8 << bit)) != 0)?;
                }
            } else {
                let mut idx = offset;
                while remaining > 0 {
                    let byte = bytes.get(idx).copied().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::UnexpectedEof, "atom bytes")
                    })?;
                    let take = remaining.min(8);
                    for bit in 0..take {
                        self.write_bit((byte & (1u8 << bit)) != 0)?;
                    }
                    remaining -= take;
                    idx += 1;
                }
            }
        }
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<JamStreamStats> {
        if self.current_bits != 0 {
            self.push_bytes(&[self.current_byte])?;
            self.current_byte = 0;
            self.current_bits = 0;
        }
        self.flush_buf()?;
        self.writer.flush()?;
        let byte_len = (self.bit_len + 7) / 8;
        Ok(JamStreamStats {
            bit_len: self.bit_len,
            byte_len,
        })
    }
}

pub fn jam_pma_to_writer<W: Write>(
    reader: &mut PmaDirectReader,
    root_raw: u64,
    writer: &mut W,
) -> Result<JamStreamStats, PmaDirectJamError> {
    let mut bit_writer = StreamingBitWriter::new(writer);
    let mut backrefs: HashMap<u64, usize> = HashMap::new();
    let mut stack = Vec::new();
    stack.push(root_raw);

    while let Some(noun_raw) = stack.pop() {
        let kind = classify_pma_noun(noun_raw)?;
        if let Some(backref) = backrefs.get(&noun_raw).copied() {
            match kind {
                PmaRawNounKind::Direct(value) => {
                    let atom_bits = met0_u64_to_usize(value);
                    if met0_u64_to_usize(backref as u64) < atom_bits {
                        mat_backref(&mut bit_writer, backref)?;
                    } else {
                        mat_direct_atom(&mut bit_writer, value)?;
                    }
                }
                PmaRawNounKind::Indirect { offset } => {
                    let atom_bits = reader.indirect_atom_bits(offset)?;
                    if met0_u64_to_usize(backref as u64) < atom_bits {
                        mat_backref(&mut bit_writer, backref)?;
                    } else {
                        mat_indirect_atom(reader, &mut bit_writer, offset, atom_bits)?;
                    }
                }
                PmaRawNounKind::Cell { .. } => {
                    mat_backref(&mut bit_writer, backref)?;
                }
            }
            continue;
        }

        backrefs.insert(noun_raw, bit_writer.bit_len());
        match kind {
            PmaRawNounKind::Direct(value) => {
                mat_direct_atom(&mut bit_writer, value)?;
            }
            PmaRawNounKind::Indirect { offset } => {
                let atom_bits = reader.indirect_atom_bits(offset)?;
                mat_indirect_atom(reader, &mut bit_writer, offset, atom_bits)?;
            }
            PmaRawNounKind::Cell { offset } => {
                mat_cell(&mut bit_writer)?;
                let (head, tail) = reader.read_cell(offset)?;
                stack.push(tail);
                stack.push(head);
            }
        }
    }

    Ok(bit_writer.finish()?)
}

pub fn jam_noun_to_writer<W: Write>(
    noun: Noun,
    space: &NounSpace,
    writer: &mut W,
) -> io::Result<JamStreamStats> {
    fn mat_backref<W: Write>(
        writer: &mut StreamingBitWriter<'_, W>,
        backref: usize,
    ) -> io::Result<()> {
        if backref == 0 {
            writer.write_bits_from_value(0b111, 3)?;
            return Ok(());
        }
        let backref_sz = met0_u64_to_usize(backref as u64);
        let backref_sz_sz = met0_u64_to_usize(backref_sz as u64);
        writer.write_bit(true)?;
        writer.write_bit(true)?;
        writer.write_zeros(backref_sz_sz)?;
        writer.write_bit(true)?;
        writer.write_bits_from_value(backref_sz as u64, backref_sz_sz - 1)?;
        writer.write_bits_from_value(backref as u64, backref_sz)?;
        Ok(())
    }

    fn mat_atom<W: Write>(
        writer: &mut StreamingBitWriter<'_, W>,
        atom: Atom,
        space: &NounSpace,
    ) -> io::Result<()> {
        unsafe {
            if atom.as_noun().raw_equals(&D(0)) {
                writer.write_bits_from_value(0b10, 2)?;
                return Ok(());
            }
        }
        let atom_sz = met0_usize(atom, space);
        let atom_sz_sz = met0_u64_to_usize(atom_sz as u64);
        writer.write_bit(false)?;
        writer.write_zeros(atom_sz_sz)?;
        writer.write_bit(true)?;
        writer.write_bits_from_value(atom_sz as u64, atom_sz_sz - 1)?;
        writer.write_bits_from_le_bytes(atom.in_space(space).as_ne_bytes(), atom_sz)?;
        Ok(())
    }

    let mut bit_writer = StreamingBitWriter::new(writer);
    let mut backrefs: HashMap<u64, usize> = HashMap::new();
    let mut stack = vec![noun];

    while let Some(noun) = stack.pop() {
        let raw = unsafe { noun.as_raw() };
        if let Some(backref) = backrefs.get(&raw).copied() {
            if let Ok(atom) = noun.as_atom() {
                if met0_u64_to_usize(backref as u64) < met0_usize(atom, space) {
                    mat_backref(&mut bit_writer, backref)?;
                } else {
                    mat_atom(&mut bit_writer, atom, space)?;
                }
            } else {
                mat_backref(&mut bit_writer, backref)?;
            }
            continue;
        }

        backrefs.insert(raw, bit_writer.bit_len());
        match noun.as_either_atom_cell() {
            Either::Left(atom) => {
                mat_atom(&mut bit_writer, atom, space)?;
            }
            Either::Right(cell) => {
                bit_writer.write_bit(true)?;
                bit_writer.write_bit(false)?;
                let cell = cell.in_space(space);
                stack.push(cell.tail().noun());
                stack.push(cell.head().noun());
            }
        }
    }

    bit_writer.finish()
}

fn mat_backref<W: Write>(
    writer: &mut StreamingBitWriter<'_, W>,
    backref: usize,
) -> Result<(), PmaDirectJamError> {
    if backref == 0 {
        writer.write_bits_from_value(0b111, 3)?;
        return Ok(());
    }
    let backref_sz = met0_u64_to_usize(backref as u64);
    let backref_sz_sz = met0_u64_to_usize(backref_sz as u64);
    writer.write_bit(true)?;
    writer.write_bit(true)?;
    writer.write_zeros(backref_sz_sz)?;
    writer.write_bit(true)?;
    writer.write_bits_from_value(backref_sz as u64, backref_sz_sz - 1)?;
    writer.write_bits_from_value(backref as u64, backref_sz)?;
    Ok(())
}

fn mat_direct_atom<W: Write>(
    writer: &mut StreamingBitWriter<'_, W>,
    value: u64,
) -> Result<(), PmaDirectJamError> {
    if value == 0 {
        writer.write_bits_from_value(0b10, 2)?;
        return Ok(());
    }
    let atom_bits = met0_u64_to_usize(value);
    mat_atom_header(writer, atom_bits)?;
    writer.write_bits_from_value(value, atom_bits)?;
    Ok(())
}

fn mat_indirect_atom<W: Write>(
    reader: &mut PmaDirectReader,
    writer: &mut StreamingBitWriter<'_, W>,
    offset: u64,
    atom_bits: usize,
) -> Result<(), PmaDirectJamError> {
    if atom_bits == 0 {
        writer.write_bits_from_value(0b10, 2)?;
        return Ok(());
    }
    let size_words = reader.indirect_atom_words(offset)?;
    mat_atom_header(writer, atom_bits)?;
    let last_bits = atom_bits.saturating_sub((size_words - 1).saturating_mul(64));
    for i in 0..size_words {
        let word = reader.read_u64(offset + 2 + i as u64)?;
        let bits = if i + 1 == size_words { last_bits } else { 64 };
        writer.write_bits_from_value(word, bits)?;
    }
    Ok(())
}

fn mat_atom_header<W: Write>(
    writer: &mut StreamingBitWriter<'_, W>,
    atom_bits: usize,
) -> Result<(), PmaDirectJamError> {
    let atom_sz_sz = met0_u64_to_usize(atom_bits as u64);
    writer.write_bit(false)?;
    writer.write_zeros(atom_sz_sz)?;
    writer.write_bit(true)?;
    writer.write_bits_from_value(atom_bits as u64, atom_sz_sz - 1)?;
    Ok(())
}

fn mat_cell<W: Write>(writer: &mut StreamingBitWriter<'_, W>) -> Result<(), PmaDirectJamError> {
    writer.write_bits_from_value(0b01, 2)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use nockvm::ext::AtomExt;
    use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use nockvm::noun::{Atom, D, T};

    use crate::stream::jam_noun_to_writer;
    use crate::Chaff;

    const TOP_SLOTS: usize = 1 << 10;

    fn fresh_stack() -> NockStack {
        NockStack::new(NOCK_STACK_SIZE_TINY, TOP_SLOTS)
    }

    #[test]
    fn stream_jam_matches_chaff_simple_cell() {
        let mut stack = fresh_stack();
        let noun = T(&mut stack, &[D(1), D(2), D(3)]);
        let jammed = Chaff::jam(noun, &stack.noun_space());
        let space = stack.noun_space();
        let mut out = Vec::new();
        let _stats = jam_noun_to_writer(noun, &space, &mut out).expect("stream jam");
        assert_eq!(out, jammed.as_ref());
    }

    #[test]
    fn stream_jam_matches_chaff_indirect_atom() {
        let mut stack = fresh_stack();
        let bytes = vec![0xA5u8; 64];
        let atom = <Atom as AtomExt>::from_bytes(&mut stack, &bytes);
        let noun = T(&mut stack, &[D(7), atom.as_noun()]);
        let jammed = Chaff::jam(noun, &stack.noun_space());
        let space = stack.noun_space();
        let mut out = Vec::new();
        let _stats = jam_noun_to_writer(noun, &space, &mut out).expect("stream jam");
        assert_eq!(out, jammed.as_ref());
    }
}
