use std::fmt::Arguments;
use std::io::{Result, Write};

use crate::interpreter::Context;
use crate::mem::NockStack;
use crate::noun::{Atom, IndirectAtom};

struct NockWriter<'s, 'b> {
    stack: &'s mut NockStack,
    buffer: &'b mut [u8],
    indirect: IndirectAtom,
    cursor: usize, //bytes
}

const INITIAL_CAPACITY_BYTES: usize = 256;

impl<'s> NockWriter<'s, '_> {
    unsafe fn new(stack: &'s mut NockStack) -> Self {
        let (indirect, buffer) = IndirectAtom::new_raw_mut_bytes(stack, INITIAL_CAPACITY_BYTES);
        NockWriter {
            stack,
            buffer,
            indirect,
            cursor: 0,
        }
    }

    unsafe fn finalize(mut self) -> Atom {
        self.indirect.normalize_as_atom()
    }

    unsafe fn expand(&mut self) {
        let sz = self.buffer.len();
        let (new_indirect, new_buffer) = IndirectAtom::new_raw_mut_bytes(self.stack, sz * 2);
        new_buffer[0..sz].copy_from_slice(self.buffer);
        self.buffer = new_buffer;
        self.indirect = new_indirect;
    }
}

impl Write for NockWriter<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let sz = buf.len();
        while (self.buffer.len() - self.cursor) < sz {
            unsafe { self.expand() };
        }
        self.buffer[self.cursor..self.cursor + sz].copy_from_slice(buf);
        self.cursor += sz;
        Ok(sz)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn nock_fmt(context: &mut Context, fmt: Arguments<'_>) -> Result<Atom> {
    let mut nw = unsafe { NockWriter::new(&mut context.stack) };
    nw.write_fmt(fmt)?;
    Ok(unsafe { nw.finalize() })
}

pub(crate) fn flog_fmt(context: &mut Context, fmt: Arguments<'_>) -> Result<()> {
    let cord = nock_fmt(context, fmt)?;
    context.slogger.flog(&mut context.stack, cord.as_noun());
    Ok(())
}

#[macro_export]
macro_rules! flog {
    ($ctx:expr, $($arg:tt)*) => {
        let _ = $crate::flog::flog_fmt($ctx, std::format_args!($($arg)*));
    }
}
