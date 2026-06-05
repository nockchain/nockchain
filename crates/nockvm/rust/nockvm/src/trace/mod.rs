use std::io::Error;
use std::ptr::NonNull;
use std::result::Result;
use std::time::Instant;

use either::Either::*;
use nockvm_macros::tas;

use crate::flog;
use crate::interpreter::Context;
use crate::jets::bits::util::rap;
use crate::jets::form::util::scow;
use crate::mem::NockStack;
use crate::mug::met3_usize;
use crate::noun::{Atom, DirectAtom, IndirectAtom, Noun};

mod tracing_backend;
pub use tracing_backend::*;

mod filter;
pub use filter::*;

crate::gdb!();

pub trait TraceBackend: Send {
    fn append_trace(&mut self, stack: &mut NockStack, path: Noun);

    unsafe fn write_nock_trace(
        &mut self,
        stack: &mut NockStack,
        trace_stack: *const TraceStack,
    ) -> Result<(), Error>;

    fn write_serf_trace(&mut self, _name: &str, _start: Instant) -> Result<(), Error> {
        Ok(())
    }

    fn write_metadata(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TraceStack<T = ()> {
    pub next: *const TraceStack<T>,
    pub data: T,
}

impl<T> TraceStack<T> {
    pub fn push_on_stack(stack: &mut NockStack, data: T) -> NonNull<TraceStack> {
        unsafe {
            let trace_stack = *(stack.local_noun_pointer(1) as *const *const Self);
            let new_trace_entry = stack.struct_alloc(1);
            *new_trace_entry = Self {
                next: trace_stack,
                data,
            };
            *(stack.local_noun_pointer(1) as *mut *mut Self) = new_trace_entry;
            NonNull::new_unchecked(new_trace_entry as *mut TraceStack)
        }
    }
}

impl<T> core::ops::Deref for TraceStack<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> core::ops::DerefMut for TraceStack<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

pub struct TraceInfo {
    pub backend: Box<dyn TraceBackend>,
    pub filter: Option<Box<dyn TraceFilter>>,
}

impl TraceInfo {
    pub fn append_trace(&mut self, stack: &mut NockStack, path: Noun) {
        if let Some(filter) = self.filter.as_mut() {
            if !filter.should_trace(path) {
                return;
            }
        }

        self.backend.append_trace(stack, path);
    }
}

/// Write metadata to trace file
pub fn write_metadata(info: &mut TraceInfo) -> Result<(), Error> {
    info.backend.write_metadata()
}

/// Abort writing to trace file if an error is encountered.
///
/// This should result in a well-formed partial trace file.
pub fn write_serf_trace_safe(context: &mut Context, name: &str, start: Instant) {
    if let Err(e) = write_serf_trace(
        context.trace_info.as_mut().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        }),
        name,
        start,
    ) {
        flog!(context, "\rserf: error writing event trace to file: {:?}", e);
        let info = &mut context.trace_info;
        *info = None;
    }
}

pub fn write_serf_trace(info: &mut TraceInfo, name: &str, start: Instant) -> Result<(), Error> {
    info.backend.write_serf_trace(name, start)
}

pub unsafe fn write_nock_trace(
    stack: &mut NockStack,
    info: &mut TraceInfo,
    trace_stack: *const TraceStack,
) -> Result<(), Error> {
    info.backend.write_nock_trace(stack, trace_stack)
}

//  XX: Need Rust string interpolation helper that doesn't allocate
pub fn path_to_cord(stack: &mut NockStack, path: Noun) -> Atom {
    let mut cursor = path;
    let mut length = 0usize;

    // count how much size we need
    while let Ok(c) = cursor.as_cell() {
        unsafe {
            match c.head().as_either_atom_cell() {
                Left(a) => {
                    length += 1;
                    length += met3_usize(a);
                }
                Right(ch) => {
                    if let Ok(nm) = ch.head().as_atom() {
                        if let Ok(kv) = ch.tail().as_atom() {
                            let kvt = scow(stack, DirectAtom::new_unchecked(tas!(b"ud")), kv)
                                .expect("scow should succeed in path_to_cord");
                            let kvc =
                                rap(stack, 3, kvt).expect("rap should succeed in path_to_cord");
                            length += 1;
                            length += met3_usize(nm);
                            length += met3_usize(kvc);
                        }
                    }
                }
            }
        }
        cursor = c.tail();
    }

    // reset cursor, then actually write the path
    cursor = path;
    let mut idx = 0;
    let (mut deres, buffer) = unsafe { IndirectAtom::new_raw_mut_bytes(stack, length) };
    let slash = (b"/")[0];

    while let Ok(c) = cursor.as_cell() {
        unsafe {
            match c.head().as_either_atom_cell() {
                Left(a) => {
                    buffer[idx] = slash;
                    idx += 1;
                    let bytelen = met3_usize(a);
                    buffer[idx..idx + bytelen].copy_from_slice(&a.as_ne_bytes()[0..bytelen]);
                    idx += bytelen;
                }
                Right(ch) => {
                    if let Ok(nm) = ch.head().as_atom() {
                        if let Ok(kv) = ch.tail().as_atom() {
                            let kvt = scow(stack, DirectAtom::new_unchecked(tas!(b"ud")), kv)
                                .expect("scow should succeed in path_to_cord");
                            let kvc =
                                rap(stack, 3, kvt).expect("rap should succeed in path_to_cord");
                            buffer[idx] = slash;
                            idx += 1;
                            let nmlen = met3_usize(nm);
                            buffer[idx..idx + nmlen].copy_from_slice(&nm.as_ne_bytes()[0..nmlen]);
                            idx += nmlen;
                            let kvclen = met3_usize(kvc);
                            buffer[idx..idx + kvclen]
                                .copy_from_slice(&kvc.as_ne_bytes()[0..kvclen]);
                            idx += kvclen;
                        }
                    }
                }
            }
        }
        cursor = c.tail();
    }

    unsafe { deres.normalize_as_atom() }
}
