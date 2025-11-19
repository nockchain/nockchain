pub mod bytes;
pub mod error;
pub mod scry;
pub mod slogger;

use std::ptr::copy_nonoverlapping;
use std::slice::from_raw_parts_mut;
use std::sync::atomic::AtomicIsize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use byteorder::{LittleEndian, ReadBytesExt};
pub use bytes::ToBytes;
use either::Either;
pub use error::{CrownError, Result};
use nockvm::hamt::Hamt;
use nockvm::interpreter::{self, Context, NockCancelToken};
use nockvm::jets::cold::Cold;
use nockvm::jets::hot::{Hot, HotEntry};
use nockvm::jets::warm::Warm;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, D};
use nockvm::serialization::jam;
use nockvm::trace::TraceInfo;
use slogger::CrownSlogger;

use crate::noun::slab::NounSlab;

// urbit @da timestamp
pub struct DA(pub u128);

// ~1970.1.1
const EPOCH_DA: u128 = 170141184475152167957503069145530368000;
// ~s1
const S1: u128 = 18446744073709551616;

pub const NOCK_STACK_1KB: usize = 1 << 7;

pub const NOCK_STACK_SIZE_TINY: usize = (NOCK_STACK_1KB << 10 << 10) * 2; // 2GB

pub const NOCK_STACK_SIZE_SMALL: usize = (NOCK_STACK_1KB << 10 << 10) * 4; // 4GB

// nock stack size
pub const NOCK_STACK_SIZE: usize = (NOCK_STACK_1KB << 10 << 10) * 8; // 8GB

// nock stack size
pub const NOCK_STACK_SIZE_MEDIUM: usize = (NOCK_STACK_1KB << 10 << 10) * 16; // 16GB

// LARGE nock stack size
pub const NOCK_STACK_SIZE_LARGE: usize = (NOCK_STACK_1KB << 10 << 10) * 32; // 32GB

// HUGE nock stack size
pub const NOCK_STACK_SIZE_HUGE: usize = (NOCK_STACK_1KB << 10 << 10) * 64; // 64GB

/**
 *   ::  +from-unix: unix seconds to @da
 *   ::
 *   ++  from-unix
 *     |=  timestamp=@ud
 *     ^-  @da
 *     %+  add  ~1970.1.1
 *     (mul timestamp ~s1)
 *   ::  +from-unix-ms: unix milliseconds to @da
 *   ::
 *   ++  from-unix-ms
 *     |=  timestamp=@ud
 *     ^-  @da
 *     %+  add  ~1970.1.1
 *     (div (mul ~s1 timestamp) 1.000)
*/
pub fn unix_ms_to_da(unix_ms: u128) -> DA {
    DA(EPOCH_DA + ((S1 * unix_ms) / 1000))
}

pub fn da_to_unix_ms(da: DA) -> u128 {
    ((da.0 - EPOCH_DA) * 1000) / S1
}

pub fn current_da() -> DA {
    let start = SystemTime::now();
    let since_the_epoch: u128 = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis();
    unix_ms_to_da(since_the_epoch)
}

pub fn current_epoch_ms() -> u128 {
    let start = SystemTime::now();
    start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis()
}

pub use nockvm::ext::make_tas;

// serialize a noun for writing over a socket or a file descriptor
pub fn serialize_noun(stack: &mut NockStack, noun: Noun) -> Result<Vec<u8>> {
    let atom = jam(stack, noun);
    let size = atom.size() << 3;

    let buf = unsafe { from_raw_parts_mut(stack.struct_alloc::<u8>(size + 5), size + 5) };
    buf[0] = 0u8;
    buf[1] = size as u8;
    buf[2] = (size >> 8) as u8;
    buf[3] = (size >> 16) as u8;
    buf[4] = (size >> 24) as u8;

    match atom.as_either() {
        Either::Left(direct) => unsafe {
            copy_nonoverlapping(
                &direct.data() as *const u64 as *const u8,
                buf.as_mut_ptr().add(5),
                size,
            );
        },
        Either::Right(indirect) => unsafe {
            copy_nonoverlapping(
                indirect.data_pointer() as *const u8,
                buf.as_mut_ptr().add(5),
                size,
            );
        },
    };
    Ok(buf.to_vec())
}

pub fn compute_timer_time(time: Noun) -> Result<u64> {
    let time_atom = time.as_atom()?;
    let mut time_bytes: &[u8] = time_atom.as_ne_bytes();
    let timer_time: u128 = da_to_unix_ms(DA(ReadBytesExt::read_u128::<LittleEndian>(
        &mut time_bytes,
    )?));
    let now: u128 = current_epoch_ms();
    let timer_ms: u64 = if now >= timer_time {
        1
    } else {
        (timer_time - now).try_into()?
    };
    Ok(timer_ms)
}

/// Creates a new Nock interpreter context with all necessary components
///
/// This context is used to execute Nock code and interpret Hoon-generated data.
/// It includes:
/// - A memory stack for Nock operations
/// - Hot, warm, and cold jet caches for optimized operations
/// - A hash array mapped trie (HAMT) for caching
/// - A slogger for logging
pub fn create_context(
    mut stack: NockStack,
    hot_state: &[HotEntry],
    mut cold: Cold,
    trace_info: Option<TraceInfo>,
    test_jets: Vec<NounSlab>,
) -> Context {
    let cache = Hamt::<Noun>::new(&mut stack);
    let test_jets = {
        let mut hamt = Hamt::<()>::new(&mut stack);
        for jet in test_jets {
            let mut path_noun = jet.copy_to_stack(&mut stack);
            hamt = hamt.insert(&mut stack, &mut path_noun, ());
        }

        hamt
    };
    let hot = Hot::init(&mut stack, hot_state);
    let warm = Warm::init(&mut stack, &mut cold, &hot, &test_jets);
    let slogger = Box::pin(CrownSlogger {});
    let cancel = Arc::new(AtomicIsize::new(NockCancelToken::RUNNING_IDLE));

    interpreter::Context {
        stack,
        slogger,
        cold,
        warm,
        hot,
        cache,
        scry_stack: D(0),
        trace_info,
        test_jets,
        running_status: cancel,
    }
}
