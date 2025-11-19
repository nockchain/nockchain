use std::ops::{DerefMut, Neg};
use std::pin::Pin;
use std::result;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use either::*;
use nockvm_macros::tas;
use tracing::trace;

use crate::hamt::Hamt;
use crate::jets::cold::Cold;
use crate::jets::hot::Hot;
use crate::jets::list::util::weld;
use crate::jets::warm::Warm;
use crate::jets::{cold, JetErr};
use crate::mem::{NockStack, Preserve};
use crate::noun::{Atom, Cell, IndirectAtom, Noun, Slots, D, T};
use crate::trace::{write_nock_trace, TraceInfo, TraceStack};
use crate::unifying_equality::unifying_equality;
use crate::{assert_acyclic, assert_no_forwarding_pointers, assert_no_junior_pointers, flog, noun};

// Previous results, not a complete sample but indicative:
// nock opcode census:
// done            : 1020331
// ret             : 66643375
// cons helper     : 250020751
// op0  (slot)     : 465843973
// op1  (constant) : 250711672
// op2  (cell)     : 1408
// op3  (type)     : 131801968
// op4  (increment): 1067304
// op5  (equals)   : 322618979
// op6  (branch)   : 350192647
// op7  (pair)     : 136269620
// op8  (pin)      : 325187439
// op9  (call)     : 262250043
// op10 (edit)     : 175014581
// op11d(hint,dis) : 77771632
// op11s(hint,sim) : 7
// op12 (scry)     : 0
// list reordered from the highest to the lowest value:
// op0  (slot)     : 465843973
// op6  (branch)   : 350192647
// op8  (pin)      : 325187439
// op5  (equals)   : 322618979
// op9  (call)     : 262250043
// op1  (constant) : 250711672
// cons helper     : 250020751
// op10 (edit)     : 175014581
// op7  (pair)     : 136269620
// op3  (type)     : 131801968
// op11d(hint,dis) : 77771632
// ret             : 66643375
// op4  (increment): 1067304
// done            : 1020331
// op2  (cell)     : 1408
// op11s(hint,sim) : 7
// op12 (scry)     : 0

#[cfg(feature = "nock_opcode_census")]
mod opcode_census {
    use std::sync::atomic::{AtomicU64, Ordering};

    macro_rules! counter {
        ($name:ident) => {
            pub(super) static $name: AtomicU64 = AtomicU64::new(0);
        };
    }

    counter!(DONE);
    counter!(RET);
    counter!(WORK_CONS);
    counter!(WORK0);
    counter!(WORK1);
    counter!(WORK2);
    counter!(WORK3);
    counter!(WORK4);
    counter!(WORK5);
    counter!(WORK6);
    counter!(WORK7);
    counter!(WORK8);
    counter!(WORK9);
    counter!(WORK10);
    counter!(WORK11D);
    counter!(WORK11S);
    counter!(WORK12);

    #[inline]
    pub(super) fn reset() {
        for counter in [
            &DONE, &RET, &WORK_CONS, &WORK0, &WORK1, &WORK2, &WORK3, &WORK4, &WORK5, &WORK6,
            &WORK7, &WORK8, &WORK9, &WORK10, &WORK11D, &WORK11S, &WORK12,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }

    #[inline]
    fn load(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }

    pub(super) fn report() {
        eprintln!("nock opcode census:");
        eprintln!("  done            : {}", load(&DONE));
        eprintln!("  ret             : {}", load(&RET));
        eprintln!("  cons helper     : {}", load(&WORK_CONS));
        eprintln!("  op0  (slot)     : {}", load(&WORK0));
        eprintln!("  op1  (constant) : {}", load(&WORK1));
        eprintln!("  op2  (cell)     : {}", load(&WORK2));
        eprintln!("  op3  (type)     : {}", load(&WORK3));
        eprintln!("  op4  (increment): {}", load(&WORK4));
        eprintln!("  op5  (equals)   : {}", load(&WORK5));
        eprintln!("  op6  (branch)   : {}", load(&WORK6));
        eprintln!("  op7  (pair)     : {}", load(&WORK7));
        eprintln!("  op8  (pin)      : {}", load(&WORK8));
        eprintln!("  op9  (call)     : {}", load(&WORK9));
        eprintln!("  op10 (edit)     : {}", load(&WORK10));
        eprintln!("  op11d(hint,dis) : {}", load(&WORK11D));
        eprintln!("  op11s(hint,sim) : {}", load(&WORK11S));
        eprintln!("  op12 (scry)     : {}", load(&WORK12));
    }
}

#[cfg(feature = "nock_opcode_census")]
macro_rules! opcode_tick {
    ($counter:ident) => {
        opcode_census::$counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    };
}

#[cfg(not(feature = "nock_opcode_census"))]
macro_rules! opcode_tick {
    ($counter:ident) => {};
}

crate::gdb!();

#[derive(Copy, Clone)]
#[repr(u8)]
enum TodoCons {
    ComputeHead,
    ComputeTail,
    Cons,
}

#[derive(Clone)]
struct NockCons {
    todo: TodoCons,
    head: Noun,
    tail: Noun,
}

#[derive(Clone)]
struct Nock0 {
    axis: Atom,
}

#[derive(Clone)]
struct Nock1 {
    noun: Noun,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo2 {
    ComputeSubject,
    ComputeFormula,
    ComputeResult,
    RestoreSubject,
}

#[derive(Clone)]
struct Nock2 {
    todo: Todo2,
    subject: Noun,
    formula: Noun,
    tail: bool,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo3 {
    ComputeChild,
    ComputeType,
}

#[derive(Clone)]
struct Nock3 {
    todo: Todo3,
    child: Noun,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo4 {
    ComputeChild,
    Increment,
}

#[derive(Clone)]
struct Nock4 {
    todo: Todo4,
    child: Noun,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo5 {
    ComputeLeftChild,
    ComputeRightChild,
    TestEquals,
}

#[derive(Clone)]
struct Nock5 {
    todo: Todo5,
    left: Noun,
    right: Noun,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo6 {
    ComputeTest,
    ComputeBranch,
}

#[derive(Clone)]
struct Nock6 {
    todo: Todo6,
    test: Noun,
    zero: Noun,
    once: Noun,
    tail: bool,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo7 {
    ComputeSubject,
    ComputeResult,
    RestoreSubject,
}

#[derive(Clone)]
struct Nock7 {
    todo: Todo7,
    subject: Noun,
    formula: Noun,
    tail: bool,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo8 {
    ComputeSubject,
    ComputeResult,
    RestoreSubject,
}

#[derive(Clone)]
struct Nock8 {
    todo: Todo8,
    pin: Noun,
    formula: Noun,
    tail: bool,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo9 {
    ComputeCore,
    ComputeResult,
    RestoreSubject,
}

#[derive(Clone)]
struct Nock9 {
    todo: Todo9,
    axis: Atom,
    core: Noun,
    tail: bool,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo10 {
    ComputeTree,
    ComputePatch,
    Edit,
}

#[derive(Clone)]
struct Nock10 {
    todo: Todo10,
    axis: Atom,
    tree: Noun,
    patch: Noun,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Todo11D {
    ComputeHint,
    ComputeResult,
    Done,
}

#[derive(Clone)]
struct Nock11D {
    todo: Todo11D,
    tag: Atom,
    hint: Noun,
    body: Noun,
    tail: bool,
}

#[derive(Copy, Clone)]
enum Todo11S {
    ComputeResult,
    Done,
}

#[derive(Clone)]
struct Nock11S {
    todo: Todo11S,
    tag: Atom,
    body: Noun,
    tail: bool,
}

#[derive(Copy, Clone)]
enum Todo12 {
    ComputeReff,
    ComputePath,
    Scry,
}

#[derive(Clone)]
struct Nock12 {
    todo: Todo12,
    reff: Noun,
    path: Noun,
}

#[derive(Clone)]
enum NockWork {
    Done,
    Ret,
    WorkCons(NockCons),
    Work0(Nock0),
    Work1(Nock1),
    Work2(Nock2),
    Work3(Nock3),
    Work4(Nock4),
    Work5(Nock5),
    Work6(Nock6),
    Work7(Nock7),
    Work8(Nock8),
    Work9(Nock9),
    Work10(Nock10),
    Work11D(Nock11D),
    Work11S(Nock11S),
    Work12(Nock12),
}

impl NockWork {
    fn opcode(&self) -> u8 {
        match self {
            NockWork::Done => 0,
            NockWork::Ret => 1,
            NockWork::WorkCons(_) => 2,
            NockWork::Work0(_) => 3,
            NockWork::Work1(_) => 4,
            NockWork::Work2(_) => 5,
            NockWork::Work3(_) => 6,
            NockWork::Work4(_) => 7,
            NockWork::Work5(_) => 8,
            NockWork::Work6(_) => 9,
            NockWork::Work7(_) => 10,
            NockWork::Work8(_) => 11,
            NockWork::Work9(_) => 12,
            NockWork::Work10(_) => 13,
            NockWork::Work11D(_) => 14,
            NockWork::Work11S(_) => 15,
            NockWork::Work12(_) => 16,
        }
    }
}

pub trait Slogger {
    // type SlogTarget;
    // DerefMut<Target = SlogTarget>;
    /** Send %slog, pretty-printed debug output.
     *
     * pri  =   debug priority
     * tank =   output as tank
     */
    fn slog(&mut self, stack: &mut NockStack, pri: u64, tank: Noun);

    /** Send %flog, raw debug output. */
    fn flog(&mut self, stack: &mut NockStack, cord: Noun);
}

impl<T: Slogger + DerefMut + Unpin + Sized> Slogger for Pin<&mut T>
where
    T::Target: Slogger + DerefMut + Unpin + Sized,
{
    // + Unpin
    // type SlogTarget = T::Target;
    fn flog(&mut self, stack: &mut NockStack, cord: Noun) {
        (*self).deref_mut().flog(stack, cord);
    }

    fn slog(&mut self, stack: &mut NockStack, pri: u64, tank: Noun) {
        (**self).slog(stack, pri, tank);
    }
}

pub struct ContextSnapshot {
    cold: Cold,
    warm: Warm,
    cache: Hamt<Noun>,
}

pub struct Context {
    pub stack: NockStack,
    pub slogger: Pin<Box<dyn Slogger + Unpin>>,
    pub cold: Cold,
    pub warm: Warm,
    pub hot: Hot,
    pub cache: Hamt<Noun>,
    pub scry_stack: Noun,
    pub trace_info: Option<TraceInfo>,
    pub running_status: Arc<AtomicIsize>,
    pub test_jets: Hamt<()>,
}

#[derive(Debug, Clone)]
pub struct NockCancelToken {
    running_status: Arc<AtomicIsize>,
}

impl NockCancelToken {
    pub const RUNNING_IDLE: isize = 0;

    pub fn cancel(&self) -> bool {
        loop {
            let running = self.running_status.load(Ordering::SeqCst);
            if running == Self::RUNNING_IDLE {
                trace!("nock cancellation: already idle");
                break false;
            } else if running < Self::RUNNING_IDLE {
                trace!("nock cancellation: already cancelled");
                break false;
            } else {
                trace!("Nock cancellation: cancelling");
                if self
                    .running_status
                    .compare_exchange(running, running.neg(), Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break true;
                }
            }
        }
    }
}

impl Context {
    pub fn save(&self) -> ContextSnapshot {
        ContextSnapshot {
            cold: self.cold,
            warm: self.warm,
            cache: self.cache,
        }
    }

    pub fn restore(&mut self, saved: &ContextSnapshot) {
        self.cold = saved.cold;
        self.warm = saved.warm;
        self.cache = saved.cache;
    }

    pub fn cancel_token(&self) -> NockCancelToken {
        NockCancelToken {
            running_status: self.running_status.clone(),
        }
    }

    /**
     * For jets that need a stack frame internally.
     *
     * This ensures that the frame is cleaned up even if the closure short-circuites to an error
     * result using e.g. the ? syntax. We need this method separately from with_frame to allow the
     * jet to use the entire context without the borrow checker complaining about the mutable
     * references.
     */
    pub unsafe fn with_stack_frame<F, O>(&mut self, slots: usize, f: F) -> O
    where
        F: FnOnce(&mut Context) -> O,
        O: Preserve,
    {
        self.stack.frame_push(slots);
        let mut ret = f(self);
        ret.preserve(&mut self.stack);
        self.cache.preserve(&mut self.stack);
        self.cold.preserve(&mut self.stack);
        self.warm.preserve(&mut self.stack);
        self.stack.frame_pop();
        ret
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Mote {
    Exit = tas!(b"exit") as isize,
    Fail = tas!(b"fail") as isize,
    Intr = tas!(b"intr") as isize,
    Meme = tas!(b"meme") as isize,
    Jest = tas!(b"jest") as isize,
}

#[derive(Clone, Copy, Debug)]
pub enum Error {
    ScryBlocked(Noun),            // path
    ScryCrashed(Noun),            // trace
    Deterministic(Mote, Noun),    // mote, trace
    NonDeterministic(Mote, Noun), // mote, trace
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::ScryBlocked(ref path) => write!(f, "ScryBlocked({:?})", path),
            Error::ScryCrashed(ref trace) => write!(f, "ScryCrashed({:?})", trace),
            Error::Deterministic(ref mote, ref trace) => {
                write!(f, "Deterministic({:?}, {:?})", mote, trace)
            }
            Error::NonDeterministic(ref mote, ref trace) => {
                write!(f, "NonDeterministic({:?}, {:?})", mote, trace)
            }
        }
    }
}

impl Preserve for Error {
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        match self {
            Error::ScryBlocked(ref mut path) => path.preserve(stack),
            Error::ScryCrashed(ref mut trace) => trace.preserve(stack),
            Error::Deterministic(_, ref mut trace) => trace.preserve(stack),
            Error::NonDeterministic(_, ref mut trace) => trace.preserve(stack),
        }
    }

    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        match self {
            Error::ScryBlocked(ref path) => path.assert_in_stack(stack),
            Error::ScryCrashed(ref trace) => trace.assert_in_stack(stack),
            Error::Deterministic(_, ref trace) => trace.assert_in_stack(stack),
            Error::NonDeterministic(_, ref trace) => trace.assert_in_stack(stack),
        }
    }
}

impl From<noun::Error> for Error {
    fn from(_: noun::Error) -> Self {
        Error::Deterministic(Mote::Exit, D(0))
    }
}

impl From<cold::Error> for Error {
    fn from(_: cold::Error) -> Self {
        Error::Deterministic(Mote::Exit, D(0))
    }
}

pub type Result = result::Result<Noun, Error>;

pub const OK_CONTINUE: Result = Ok(D(0));
pub const BAIL_EXIT: Result = Err(Error::Deterministic(Mote::Exit, D(0)));
pub const BAIL_FAIL: Result = Err(Error::NonDeterministic(Mote::Fail, D(0)));
pub const BAIL_INTR: Result = Err(Error::NonDeterministic(Mote::Intr, D(0)));
pub(crate) const BAIL_JEST: Result = Err(Error::NonDeterministic(Mote::Jest, D(0)));

#[inline(always)]
#[cold]
fn cold_err<E>(err: E) -> Result
where
    Error: From<E>,
{
    Err(err.into())
}

macro_rules! try_or_bail {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(err) => return cold_err(err),
        }
    };
}

#[allow(unused_variables)]
#[inline(always)]
fn debug_assertions(stack: &mut NockStack, noun: Noun) {
    assert_acyclic!(noun);
    assert_no_forwarding_pointers!(noun);
    assert_no_junior_pointers!(stack, noun);
}

/** Interpret nock */
pub fn interpret(context: &mut Context, mut subject: Noun, formula: Noun) -> Result {
    let orig_subject = subject; // for debugging
    let snapshot = context.save();
    let virtual_frame: *const u64 = context.stack.get_frame_pointer();
    let mut res: Noun = D(0);
    loop {
        let running_status = context.running_status.load(Ordering::SeqCst);
        if running_status >= NockCancelToken::RUNNING_IDLE {
            if context
                .running_status
                .compare_exchange(
                    running_status,
                    running_status + 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                break;
            }
        } else {
            return Err(exit(
                context,
                &snapshot,
                virtual_frame,
                Error::NonDeterministic(Mote::Intr, D(0)),
            ));
        }
    }

    // Setup stack for Nock computation
    unsafe {
        context.stack.frame_push(2);

        // Bottom of mean stack
        *(context.stack.local_noun_pointer(0)) = D(0);
        // Bottom of trace stack
        *(context.stack.local_noun_pointer(1) as *mut *const TraceStack) = std::ptr::null();

        *(context.stack.push()) = NockWork::Done;
    };

    // DO NOT REMOVE THIS COMMENT
    //
    // If you need to allocate for debugging, wrap the debugging code in
    //
    // ```
    // permit_alloc(|| {
    //   your.code.goes.here()
    // })
    // ```
    //
    // (See https://docs.rs/assert_no_alloc/latest/assert_no_alloc/#advanced-use)
    let nock = unsafe {
        try_or_bail!(push_formula(&mut context.stack, formula, true));

        loop {
            let work_ptr = context.stack.top::<NockWork>();
            match &mut *work_ptr {
                NockWork::Work0(zero) => {
                    opcode_tick!(WORK0);
                    if let Ok(noun) = subject.slot_atom(zero.axis) {
                        res = noun;
                        context.stack.pop::<NockWork>();
                    } else {
                        // Axis invalid for input Noun
                        break BAIL_EXIT;
                    }
                }
                NockWork::Work6(ref mut cond) => match cond.todo {
                    Todo6::ComputeTest => {
                        opcode_tick!(WORK6);
                        cond.todo = Todo6::ComputeBranch;
                        *work_ptr = NockWork::Work6(cond.clone());
                        try_or_bail!(push_formula(&mut context.stack, cond.test, false));
                    }
                    Todo6::ComputeBranch => {
                        opcode_tick!(WORK6);
                        let stack = &mut context.stack;
                        stack.pop::<NockWork>();
                        if res.is_direct() {
                            let direct = res.as_direct().unwrap_unchecked();
                            if direct.data() == 0 {
                                try_or_bail!(push_formula(stack, cond.zero, cond.tail));
                            } else if direct.data() == 1 {
                                try_or_bail!(push_formula(stack, cond.once, cond.tail));
                            } else {
                                // Test branch of Nock 6 must return 0 or 1
                                break BAIL_EXIT;
                            }
                        } else {
                            // Test branch of Nock 6 must return a direct atom
                            break BAIL_EXIT;
                        }
                    }
                },
                NockWork::Work8(ref mut pins) => match pins.todo {
                    Todo8::ComputeSubject => {
                        opcode_tick!(WORK8);
                        pins.todo = Todo8::ComputeResult;
                        *work_ptr = NockWork::Work8(pins.clone());
                        try_or_bail!(push_formula(&mut context.stack, pins.pin, false));
                    }
                    Todo8::ComputeResult => {
                        opcode_tick!(WORK8);
                        let stack = &mut context.stack;
                        if pins.tail {
                            subject = T(stack, &[res, subject]);
                            stack.pop::<NockWork>();
                            try_or_bail!(push_formula(stack, pins.formula, true));
                        } else {
                            pins.todo = Todo8::RestoreSubject;
                            pins.pin = subject;
                            subject = T(stack, &[res, subject]);
                            try_or_bail!(push_formula(stack, pins.formula, false));
                        }
                    }
                    Todo8::RestoreSubject => {
                        opcode_tick!(WORK8);
                        subject = pins.pin;
                        context.stack.pop::<NockWork>();
                    }
                },
                NockWork::Work5(ref mut five) => match five.todo {
                    Todo5::ComputeLeftChild => {
                        opcode_tick!(WORK5);
                        five.todo = Todo5::ComputeRightChild;
                        *work_ptr = NockWork::Work5(five.clone());
                        try_or_bail!(push_formula(&mut context.stack, five.left, false));
                    }
                    Todo5::ComputeRightChild => {
                        opcode_tick!(WORK5);
                        five.todo = Todo5::TestEquals;
                        five.left = res;
                        *work_ptr = NockWork::Work5(five.clone());
                        try_or_bail!(push_formula(&mut context.stack, five.right, false));
                    }
                    Todo5::TestEquals => {
                        opcode_tick!(WORK5);
                        let stack = &mut context.stack;
                        let saved_value_ptr = &mut five.left;
                        res = if unifying_equality(stack, &mut res, saved_value_ptr) {
                            D(0)
                        } else {
                            D(1)
                        };
                        stack.pop::<NockWork>();
                    }
                },
                NockWork::Work9(ref mut kale) => {
                    match kale.todo {
                        Todo9::ComputeCore => {
                            opcode_tick!(WORK9);
                            kale.todo = Todo9::ComputeResult;
                            *work_ptr = NockWork::Work9(kale.clone());
                            try_or_bail!(push_formula(&mut context.stack, kale.core, false));
                        }
                        Todo9::ComputeResult => {
                            opcode_tick!(WORK9);
                            if let Ok(mut formula) = res.slot_atom(kale.axis) {
                                if !cfg!(feature = "sham_hints") {
                                    if let Some((jet, _path, test)) = context
                                        .warm
                                        .find_jet(&mut context.stack, &mut res, &mut formula)
                                        .next()
                                    {
                                        match jet(context, res) {
                                            Ok(mut jet_res) => {
                                                if test {
                                                    let mut test_res = try_or_bail!(interpret(
                                                        context, res, formula
                                                    ));
                                                    if !unifying_equality(
                                                        &mut context.stack, &mut test_res,
                                                        &mut jet_res,
                                                    ) {
                                                        break BAIL_JEST;
                                                    }
                                                }
                                                res = jet_res;
                                                context.stack.pop::<NockWork>();
                                                continue;
                                            }
                                            Err(JetErr::Punt) => {}
                                            Err(err) => {
                                                break Err(err.into());
                                            }
                                        }
                                    }
                                };

                                let stack = &mut context.stack;
                                if kale.tail {
                                    stack.pop::<NockWork>();

                                    // We could trace on 2 as well, but 2 only comes from Hoon via
                                    // '.*', so we can assume it's never directly used to invoke
                                    // jetted code.
                                    if let Some((path, trace_info)) =
                                        context.trace_info.as_mut().and_then(|v| {
                                            context.cold.matches(stack, &mut res).zip(Some(v))
                                        })
                                    {
                                        trace_info.append_trace(stack, path);
                                    }

                                    subject = res;
                                    try_or_bail!(push_formula(stack, formula, true));
                                } else {
                                    kale.todo = Todo9::RestoreSubject;
                                    kale.core = subject;

                                    debug_assertions(stack, orig_subject);
                                    debug_assertions(stack, subject);
                                    debug_assertions(stack, res);

                                    subject = res;
                                    mean_frame_push(stack, 0);
                                    *stack.push() = NockWork::Ret;
                                    try_or_bail!(push_formula(stack, formula, true));

                                    // We could trace on 2 as well, but 2 only comes from Hoon via
                                    // '.*', so we can assume it's never directly used to invoke
                                    // jetted code.
                                    if let Some((path, trace_info)) =
                                        context.trace_info.as_mut().and_then(|v| {
                                            context.cold.matches(stack, &mut res).zip(Some(v))
                                        })
                                    {
                                        trace_info.append_trace(stack, path);
                                    }
                                }
                            } else {
                                // Axis into core must be atom
                                break BAIL_EXIT;
                            }
                        }
                        Todo9::RestoreSubject => {
                            opcode_tick!(WORK9);
                            let stack = &mut context.stack;

                            subject = kale.core;
                            stack.pop::<NockWork>();

                            debug_assertions(stack, orig_subject);
                            debug_assertions(stack, subject);
                            debug_assertions(stack, res);
                        }
                    }
                }
                NockWork::Work1(once) => {
                    opcode_tick!(WORK1);
                    res = once.noun;
                    context.stack.pop::<NockWork>();
                }
                NockWork::WorkCons(ref mut cons) => match cons.todo {
                    TodoCons::ComputeHead => {
                        opcode_tick!(WORK_CONS);
                        cons.todo = TodoCons::ComputeTail;
                        try_or_bail!(push_formula(&mut context.stack, cons.head, false));
                    }
                    TodoCons::ComputeTail => {
                        opcode_tick!(WORK_CONS);
                        cons.todo = TodoCons::Cons;
                        cons.head = res;
                        try_or_bail!(push_formula(&mut context.stack, cons.tail, false));
                    }
                    TodoCons::Cons => {
                        opcode_tick!(WORK_CONS);
                        let stack = &mut context.stack;
                        res = T(stack, &[cons.head, res]);
                        stack.pop::<NockWork>();
                    }
                },
                NockWork::Work10(ref mut diet) => {
                    opcode_tick!(WORK10);
                    match diet.todo {
                        Todo10::ComputeTree => {
                            diet.todo = Todo10::ComputePatch; // should we compute patch then tree?
                            try_or_bail!(push_formula(&mut context.stack, diet.tree, false));
                        }
                        Todo10::ComputePatch => {
                            diet.todo = Todo10::Edit;
                            diet.tree = res;
                            try_or_bail!(push_formula(&mut context.stack, diet.patch, false));
                        }
                        Todo10::Edit => {
                            res = edit(&mut context.stack, diet.axis, res, diet.tree);
                            context.stack.pop::<NockWork>();
                        }
                    }
                }
                NockWork::Work7(ref mut pose) => match pose.todo {
                    Todo7::ComputeSubject => {
                        opcode_tick!(WORK7);
                        pose.todo = Todo7::ComputeResult;
                        try_or_bail!(push_formula(&mut context.stack, pose.subject, false));
                    }
                    Todo7::ComputeResult => {
                        opcode_tick!(WORK7);
                        let stack = &mut context.stack;
                        if pose.tail {
                            stack.pop::<NockWork>();
                            subject = res;
                            try_or_bail!(push_formula(stack, pose.formula, true));
                        } else {
                            pose.todo = Todo7::RestoreSubject;
                            pose.subject = subject;
                            subject = res;
                            try_or_bail!(push_formula(stack, pose.formula, false));
                        }
                    }
                    Todo7::RestoreSubject => {
                        opcode_tick!(WORK7);
                        subject = pose.subject;
                        context.stack.pop::<NockWork>();
                    }
                },
                NockWork::Work3(ref mut thee) => match thee.todo {
                    Todo3::ComputeChild => {
                        opcode_tick!(WORK3);
                        thee.todo = Todo3::ComputeType;
                        try_or_bail!(push_formula(&mut context.stack, thee.child, false));
                    }
                    Todo3::ComputeType => {
                        opcode_tick!(WORK3);
                        res = if res.is_cell() { D(0) } else { D(1) };
                        context.stack.pop::<NockWork>();
                    }
                },
                NockWork::Work11D(ref mut dint) => match dint.todo {
                    Todo11D::ComputeHint => {
                        opcode_tick!(WORK11D);
                        if let Some(ret) =
                            hint::match_pre_hint(context, subject, dint.tag, dint.hint, dint.body)
                        {
                            match ret {
                                Ok(found) => {
                                    res = found;
                                    context.stack.pop::<NockWork>();
                                }
                                Err(err) => {
                                    break Err(err);
                                }
                            }
                        } else {
                            dint.todo = Todo11D::ComputeResult;
                            try_or_bail!(push_formula(&mut context.stack, dint.hint, false));
                        }
                    }
                    Todo11D::ComputeResult => {
                        opcode_tick!(WORK11D);
                        if let Some(ret) = hint::match_pre_nock(
                            context,
                            subject,
                            dint.tag,
                            Some((dint.hint, res)),
                            dint.body,
                        ) {
                            match ret {
                                Ok(found) => {
                                    res = found;
                                    context.stack.pop::<NockWork>();
                                }
                                Err(err) => {
                                    break Err(err);
                                }
                            }
                        } else {
                            if dint.tail {
                                context.stack.pop::<NockWork>();
                            } else {
                                dint.todo = Todo11D::Done;
                                dint.hint = res;
                            }
                            try_or_bail!(push_formula(&mut context.stack, dint.body, dint.tail));
                        }
                    }
                    Todo11D::Done => {
                        opcode_tick!(WORK11D);
                        if let Some(found) = hint::match_post_nock(
                            context,
                            subject,
                            dint.tag,
                            Some(dint.hint),
                            dint.body,
                            res,
                        ) {
                            res = found;
                        }
                        context.stack.pop::<NockWork>();
                    }
                },
                NockWork::Ret => {
                    opcode_tick!(RET);
                    write_trace(context);

                    let stack = &mut context.stack;
                    debug_assertions(stack, orig_subject);
                    debug_assertions(stack, subject);
                    debug_assertions(stack, res);

                    stack.preserve(&mut context.cache);
                    stack.preserve(&mut context.cold);
                    stack.preserve(&mut context.warm);
                    stack.preserve(&mut res);
                    stack.frame_pop();

                    debug_assertions(stack, orig_subject);
                    debug_assertions(stack, res);
                }
                NockWork::Work4(ref mut four) => match four.todo {
                    Todo4::ComputeChild => {
                        opcode_tick!(WORK4);
                        four.todo = Todo4::Increment;
                        try_or_bail!(push_formula(&mut context.stack, four.child, false));
                    }
                    Todo4::Increment => {
                        opcode_tick!(WORK4);
                        if let Ok(atom) = res.as_atom() {
                            res = inc(&mut context.stack, atom).as_noun();
                            context.stack.pop::<NockWork>();
                        } else {
                            // Cannot increment (Nock 4) a cell
                            break BAIL_EXIT;
                        }
                    }
                },
                NockWork::Done => {
                    opcode_tick!(DONE);
                    write_trace(context);

                    let stack = &mut context.stack;
                    debug_assertions(stack, orig_subject);
                    debug_assertions(stack, subject);
                    debug_assertions(stack, res);

                    stack.preserve(&mut context.cache);
                    stack.preserve(&mut context.cold);
                    stack.preserve(&mut context.warm);
                    stack.preserve(&mut res);
                    stack.frame_pop();

                    debug_assertions(stack, orig_subject);
                    debug_assertions(stack, res);

                    break Ok(res);
                }
                NockWork::Work2(ref mut vale) => {
                    cold_paths::step_work2(context, vale, &mut subject, &mut res)?;
                    continue;
                }
                NockWork::Work11S(ref mut sint) => {
                    match cold_paths::step_work11s(context, sint, &mut subject, &mut res) {
                        Ok(_) => {}
                        Err(err) => {
                            break Err(err);
                        }
                    };
                }
                NockWork::Work12(ref mut scry) => {
                    cold_paths::step_work12(context, scry, &mut res)?;
                    continue;
                }
            };
        }
    };

    #[cfg(feature = "nock_opcode_census")]
    opcode_census::report();

    loop {
        let running_status = context.running_status.load(Ordering::SeqCst);
        if running_status < NockCancelToken::RUNNING_IDLE {
            if context
                .running_status
                .compare_exchange(
                    running_status,
                    running_status + 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                break;
            }
        } else if running_status == NockCancelToken::RUNNING_IDLE {
            break;
        } else if context
            .running_status
            .compare_exchange(
                running_status,
                running_status - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            break;
        }
    }

    match nock {
        Ok(res) => Ok(res),
        Err(err) => Err(exit(context, &snapshot, virtual_frame, err)),
    }
}

mod cold_paths {
    use super::*;

    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn step_work2(
        context: &mut Context,
        vale: &mut Nock2,
        subject: &mut Noun,
        res: &mut Noun,
    ) -> Result {
        match vale.todo {
            Todo2::ComputeSubject => {
                opcode_tick!(WORK2);
                vale.todo = Todo2::ComputeFormula;
                try_or_bail!(push_formula(&mut context.stack, vale.subject, false));
                return OK_CONTINUE;
            }
            Todo2::ComputeFormula => {
                opcode_tick!(WORK2);
                vale.todo = Todo2::ComputeResult;
                vale.subject = *res;
                try_or_bail!(push_formula(&mut context.stack, vale.formula, false));
                return OK_CONTINUE;
            }
            Todo2::ComputeResult => {
                opcode_tick!(WORK2);
                let stack = &mut context.stack;
                if vale.tail {
                    stack.pop::<NockWork>();
                    *subject = vale.subject;
                    try_or_bail!(push_formula(stack, res.clone(), true));
                    return OK_CONTINUE;
                } else {
                    vale.todo = Todo2::RestoreSubject;
                    std::mem::swap(&mut vale.subject, subject);

                    // debug_assertions(stack, orig_subject);
                    debug_assertions(stack, *subject);
                    debug_assertions(stack, *res);

                    mean_frame_push(stack, 0);
                    *stack.push() = NockWork::Ret;
                    try_or_bail!(push_formula(stack, *res, true));
                    return OK_CONTINUE;
                }
            }
            Todo2::RestoreSubject => {
                opcode_tick!(WORK2);
                let stack = &mut context.stack;

                *subject = vale.subject;
                stack.pop::<NockWork>();

                // debug_assertions(stack, orig_subject);
                debug_assertions(stack, *subject);
                debug_assertions(stack, *res);
                return OK_CONTINUE;
            }
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn step_work11s(
        context: &mut Context,
        sint: &mut Nock11S,
        subject: &mut Noun,
        res: &mut Noun,
    ) -> Result {
        match sint.todo {
            Todo11S::ComputeResult => {
                opcode_tick!(WORK11S);
                if let Some(ret) =
                    hint::match_pre_nock(context, *subject, sint.tag, None, sint.body)
                {
                    match ret {
                        Ok(found) => {
                            *res = found;
                            context.stack.pop::<NockWork>();
                            return OK_CONTINUE;
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    }
                } else {
                    if sint.tail {
                        context.stack.pop::<NockWork>();
                    } else {
                        sint.todo = Todo11S::Done;
                    }
                    push_formula(&mut context.stack, sint.body, sint.tail)
                }
            }
            Todo11S::Done => {
                opcode_tick!(WORK11S);
                if let Some(found) =
                    hint::match_post_nock(context, *subject, sint.tag, None, sint.body, res.clone())
                {
                    *res = found;
                }
                context.stack.pop::<NockWork>();
                return OK_CONTINUE;
            }
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn step_work12(
        context: &mut Context,
        scry: &mut Nock12,
        // subject: &mut Noun,
        res: &mut Noun,
    ) -> Result {
        match scry.todo {
            Todo12::ComputeReff => {
                opcode_tick!(WORK12);
                scry.todo = Todo12::ComputePath;
                try_or_bail!(push_formula(&mut context.stack, scry.reff, false));
                return OK_CONTINUE;
            }
            Todo12::ComputePath => {
                opcode_tick!(WORK12);
                scry.todo = Todo12::Scry;
                scry.reff = res.clone();
                try_or_bail!(push_formula(&mut context.stack, scry.path, false));
                return OK_CONTINUE;
            }
            Todo12::Scry => {
                if let Some(cell) = context.scry_stack.cell() {
                    scry.path = res.clone();
                    let scry_stack = context.scry_stack;
                    let scry_handler = cell.head();
                    let scry_gate = scry_handler.as_cell()?;
                    let payload = T(&mut context.stack, &[scry.reff, res.clone()]);
                    let scry_core = T(
                        &mut context.stack,
                        &[scry_gate.head(), payload, scry_gate.tail().as_cell()?.tail()],
                    );
                    let scry_form = T(&mut context.stack, &[D(9), D(2), D(1), scry_core]);

                    context.scry_stack = cell.tail();
                    // Alternately, we could use scry_core as the subject and [9 2 0 1] as
                    // the formula. It's unclear if performance will be better with a purely
                    // static formula.
                    match interpret(context, D(0), scry_form) {
                        Ok(noun) => match noun.as_either_atom_cell() {
                            Left(atom) => {
                                if atom.as_noun().raw_equals(&D(0)) {
                                    return Err(Error::ScryBlocked(scry.path));
                                } else {
                                    return Err(Error::ScryCrashed(D(0)));
                                }
                            }
                            Right(cell) => match cell.tail().as_either_atom_cell() {
                                Left(_) => {
                                    let stack = &mut context.stack;
                                    let hunk = T(stack, &[D(tas!(b"hunk")), scry.reff, scry.path]);
                                    mean_push(stack, hunk);
                                    return Err(Error::ScryCrashed(D(0)));
                                }
                                Right(cell) => {
                                    *res = cell.tail();
                                    context.scry_stack = scry_stack;
                                    context.stack.pop::<NockWork>();
                                    return OK_CONTINUE;
                                }
                            },
                        },
                        Err(error) => match error {
                            Error::Deterministic(_, trace) | Error::ScryCrashed(trace) => {
                                return Err(Error::ScryCrashed(trace));
                            }
                            Error::NonDeterministic(_, _) => {
                                return Err(error);
                            }
                            Error::ScryBlocked(_) => {
                                return BAIL_FAIL;
                            }
                        },
                    }
                } else {
                    return BAIL_EXIT;
                }
            }
        }
    }
}

fn push_formula(stack: &mut NockStack, formula: Noun, tail: bool) -> Result {
    unsafe {
        if let Ok(formula_cell) = formula.as_cell() {
            // Formula
            match formula_cell.head().as_either_atom_cell() {
                Right(_cell) => {
                    *stack.push() = NockWork::WorkCons(NockCons {
                        todo: TodoCons::ComputeHead,
                        head: formula_cell.head(),
                        tail: formula_cell.tail(),
                    });
                }
                Left(atom) => {
                    if let Ok(direct) = atom.as_direct() {
                        match direct.data() {
                            0 => {
                                if let Ok(axis_atom) = formula_cell.tail().as_atom() {
                                    *stack.push() = NockWork::Work0(Nock0 { axis: axis_atom });
                                } else {
                                    // Axis for Nock 0 must be an atom
                                    return BAIL_EXIT;
                                }
                            }
                            1 => {
                                *stack.push() = NockWork::Work1(Nock1 {
                                    noun: formula_cell.tail(),
                                });
                            }
                            2 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    *stack.push() = NockWork::Work2(Nock2 {
                                        todo: Todo2::ComputeSubject,
                                        subject: arg_cell.head(),
                                        formula: arg_cell.tail(),
                                        tail,
                                    });
                                } else {
                                    // Argument to Nock 2 must be cell
                                    return BAIL_EXIT;
                                };
                            }
                            3 => {
                                *stack.push() = NockWork::Work3(Nock3 {
                                    todo: Todo3::ComputeChild,
                                    child: formula_cell.tail(),
                                });
                            }
                            4 => {
                                *stack.push() = NockWork::Work4(Nock4 {
                                    todo: Todo4::ComputeChild,
                                    child: formula_cell.tail(),
                                });
                            }
                            5 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    *stack.push() = NockWork::Work5(Nock5 {
                                        todo: Todo5::ComputeLeftChild,
                                        left: arg_cell.head(),
                                        right: arg_cell.tail(),
                                    });
                                } else {
                                    // Argument to Nock 5 must be cell
                                    return BAIL_EXIT;
                                };
                            }
                            6 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    if let Ok(branch_cell) = arg_cell.tail().as_cell() {
                                        *stack.push() = NockWork::Work6(Nock6 {
                                            todo: Todo6::ComputeTest,
                                            test: arg_cell.head(),
                                            zero: branch_cell.head(),
                                            once: branch_cell.tail(),
                                            tail,
                                        });
                                    } else {
                                        // Argument tail to Nock 6 must be cell
                                        return BAIL_EXIT;
                                    };
                                } else {
                                    // Argument to Nock 6 must be cell
                                    return BAIL_EXIT;
                                }
                            }
                            7 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    *stack.push() = NockWork::Work7(Nock7 {
                                        todo: Todo7::ComputeSubject,
                                        subject: arg_cell.head(),
                                        formula: arg_cell.tail(),
                                        tail,
                                    });
                                } else {
                                    // Argument to Nock 7 must be cell
                                    return BAIL_EXIT;
                                };
                            }
                            8 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    *stack.push() = NockWork::Work8(Nock8 {
                                        todo: Todo8::ComputeSubject,
                                        pin: arg_cell.head(),
                                        formula: arg_cell.tail(),
                                        tail,
                                    });
                                } else {
                                    // Argument to Nock 8 must be cell
                                    return BAIL_EXIT;
                                };
                            }
                            9 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    if let Ok(axis_atom) = arg_cell.head().as_atom() {
                                        let p = stack.push();
                                        *p = NockWork::Work9(Nock9 {
                                            todo: Todo9::ComputeCore,
                                            axis: axis_atom,
                                            core: arg_cell.tail(),
                                            tail,
                                        });
                                    } else {
                                        // Axis for Nock 9 must be an atom
                                        return BAIL_EXIT;
                                    }
                                } else {
                                    // Argument to Nock 9 must be cell
                                    return BAIL_EXIT;
                                };
                            }
                            10 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    if let Ok(patch_cell) = arg_cell.head().as_cell() {
                                        if let Ok(axis_atom) = patch_cell.head().as_atom() {
                                            *stack.push() = NockWork::Work10(Nock10 {
                                                todo: Todo10::ComputeTree,
                                                axis: axis_atom,
                                                tree: arg_cell.tail(),
                                                patch: patch_cell.tail(),
                                            });
                                        } else {
                                            // Axis for Nock 10 must be an atom
                                            return BAIL_EXIT;
                                        }
                                    } else {
                                        // Head of argument to Nock 10 must be a cell
                                        return BAIL_EXIT;
                                    };
                                } else {
                                    // Argument to Nock 10 must be a cell
                                    return BAIL_EXIT;
                                };
                            }
                            11 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    match arg_cell.head().as_either_atom_cell() {
                                        Left(tag_atom) => {
                                            *stack.push() = NockWork::Work11S(Nock11S {
                                                todo: Todo11S::ComputeResult,
                                                tag: tag_atom,
                                                body: arg_cell.tail(),
                                                tail: tail && hint::is_tail(tag_atom),
                                            });
                                        }
                                        Right(hint_cell) => {
                                            if let Ok(tag_atom) = hint_cell.head().as_atom() {
                                                *stack.push() = NockWork::Work11D(Nock11D {
                                                    todo: Todo11D::ComputeHint,
                                                    tag: tag_atom,
                                                    hint: hint_cell.tail(),
                                                    body: arg_cell.tail(),
                                                    tail: tail && hint::is_tail(tag_atom),
                                                });
                                            } else {
                                                // Hint tag must be an atom
                                                return BAIL_EXIT;
                                            }
                                        }
                                    };
                                } else {
                                    // Argument for Nock 11 must be cell
                                    return BAIL_EXIT;
                                };
                            }
                            12 => {
                                if let Ok(arg_cell) = formula_cell.tail().as_cell() {
                                    *stack.push() = NockWork::Work12(Nock12 {
                                        todo: Todo12::ComputeReff,
                                        reff: arg_cell.head(),
                                        path: arg_cell.tail(),
                                    });
                                } else {
                                    // Argument for Nock 12 must be cell
                                    return BAIL_EXIT;
                                }
                            }
                            _ => {
                                // Invalid formula opcode
                                return BAIL_EXIT;
                            }
                        }
                    } else {
                        // Formula opcode must be direct atom
                        return BAIL_EXIT;
                    }
                }
            }
        } else {
            // Bad formula: atoms are not formulas
            return BAIL_EXIT;
        }
    }
    OK_CONTINUE
}

fn exit(
    context: &mut Context,
    snapshot: &ContextSnapshot,
    virtual_frame: *const u64,
    error: Error,
) -> Error {
    unsafe {
        context.restore(snapshot);

        if context.stack.copying() {
            assert!(!std::ptr::eq(
                context.stack.get_frame_pointer(),
                virtual_frame
            ));
            context.stack.frame_pop();
        }

        let stack = &mut context.stack;
        let mut preserve = match error {
            Error::ScryBlocked(path) => path,
            Error::Deterministic(_, t) | Error::NonDeterministic(_, t) | Error::ScryCrashed(t) => {
                // Return $tang of traces
                let h = *(stack.local_noun_pointer(0));
                // XX: Small chance of clobbering something important after OOM?
                // XX: what if we OOM while making a stack trace
                match weld(stack, t, h) {
                    Ok(trace) => trace,
                    Err(_) => h,
                }
            }
        };

        while !std::ptr::eq(stack.get_frame_pointer(), virtual_frame) {
            stack.preserve(&mut preserve);
            stack.frame_pop();
        }

        match error {
            Error::Deterministic(mote, _) => Error::Deterministic(mote, preserve),
            Error::NonDeterministic(mote, _) => Error::NonDeterministic(mote, preserve),
            Error::ScryCrashed(_) => Error::ScryCrashed(preserve),
            Error::ScryBlocked(_) => error,
        }
    }
}

/** Push frame onto NockStack while preserving the mean stack.
 */
fn mean_frame_push(stack: &mut NockStack, slots: usize) {
    unsafe {
        let trace = *(stack.local_noun_pointer(0));
        stack.frame_push(slots + 2);
        *(stack.local_noun_pointer(0)) = trace;
        *(stack.local_noun_pointer(1) as *mut *const Noun) = std::ptr::null();
    }
}

/** Push onto the mean stack.
 */
fn mean_push(stack: &mut NockStack, noun: Noun) {
    unsafe {
        let cur_trace = *(stack.local_noun_pointer(0));
        let new_trace = T(stack, &[noun, cur_trace]);
        *(stack.local_noun_pointer(0)) = new_trace;
    }
}

/** Pop off of the mean stack.
 */
fn mean_pop(stack: &mut NockStack) {
    unsafe {
        *(stack.local_noun_pointer(0)) = (*(stack.local_noun_pointer(0)))
            .as_cell()
            .expect("serf: unexpected end of mean stack\r")
            .tail();
    }
}

fn edit(stack: &mut NockStack, edit_axis: Atom, patch: Noun, mut tree: Noun) -> Noun {
    use either::{Left, Right};

    use crate::noun::{DirectAxisIterator, IndirectAxisIterator};

    let mut res = patch;
    let mut dest: *mut Noun = &mut res;

    match edit_axis.as_either() {
        Left(direct) => {
            let mut axis_iter =
                DirectAxisIterator::new(direct.data()).expect("0 is not allowed as an edit axis");

            while let Some(descend_tail) = axis_iter.next() {
                let tree_cell = tree.as_cell().expect("Invalid axis for edit");
                if descend_tail {
                    unsafe {
                        let (cell, cellmem) = Cell::new_raw_mut(stack);
                        *dest = cell.as_noun();
                        (*cellmem).head = tree_cell.head();
                        dest = &mut ((*cellmem).tail);
                    }
                    tree = tree_cell.tail();
                } else {
                    unsafe {
                        let (cell, cellmem) = Cell::new_raw_mut(stack);
                        *dest = cell.as_noun();
                        (*cellmem).tail = tree_cell.tail();
                        dest = &mut ((*cellmem).head);
                    }
                    tree = tree_cell.head();
                }
            }
        }
        Right(indirect) => {
            let mut axis_iter = IndirectAxisIterator::new(indirect.as_slice())
                .expect("0 is not allowed as an edit axis");

            while let Some(descend_tail) = axis_iter.next() {
                let tree_cell = tree.as_cell().expect("Invalid axis for edit");
                if descend_tail {
                    unsafe {
                        let (cell, cellmem) = Cell::new_raw_mut(stack);
                        *dest = cell.as_noun();
                        (*cellmem).head = tree_cell.head();
                        dest = &mut ((*cellmem).tail);
                    }
                    tree = tree_cell.tail();
                } else {
                    unsafe {
                        let (cell, cellmem) = Cell::new_raw_mut(stack);
                        *dest = cell.as_noun();
                        (*cellmem).tail = tree_cell.tail();
                        dest = &mut ((*cellmem).head);
                    }
                    tree = tree_cell.head();
                }
            }
        }
    }

    unsafe {
        *dest = patch;
    }
    res
}

pub fn inc(stack: &mut NockStack, atom: Atom) -> Atom {
    match atom.as_either() {
        Left(direct) => Atom::new(stack, direct.data() + 1),
        Right(indirect) => {
            let indirect_slice = indirect.as_bitslice();
            match indirect_slice.first_zero() {
                None => {
                    // all ones, make an indirect one word bigger
                    let (new_indirect, new_slice) =
                        unsafe { IndirectAtom::new_raw_mut_bitslice(stack, indirect.size() + 1) };
                    new_slice.set(indirect_slice.len(), true);
                    new_indirect.as_atom()
                }
                Some(first_zero) => {
                    let (new_indirect, new_slice) =
                        unsafe { IndirectAtom::new_raw_mut_bitslice(stack, indirect.size()) };
                    new_slice.set(first_zero, true);
                    new_slice[first_zero + 1..]
                        .copy_from_bitslice(&indirect_slice[first_zero + 1..]);
                    new_indirect.as_atom()
                }
            }
        }
    }
}

/// Write fast-hinted traces to trace file
unsafe fn write_trace(context: &mut Context) {
    if let Some(ref mut info) = &mut context.trace_info {
        let trace_stack = *(context.stack.local_noun_pointer(1) as *mut *const TraceStack);
        // Abort writing to trace file if we encountered an error. This should
        // result in a well-formed partial trace file.
        if let Err(_e) = write_nock_trace(&mut context.stack, info, trace_stack) {
            flog!(context, "\rserf: error writing nock trace to file: {:?}", _e);
            context.trace_info = None;
        }
    }
}

mod hint {
    use nockvm_macros::tas;

    use super::*;
    use crate::jets;
    use crate::jets::cold;
    use crate::jets::nock::util::{mook, LEAF};
    use crate::noun::{tape, Atom, Cell, Noun, D, T};
    use crate::unifying_equality::unifying_equality;

    pub(super) fn is_tail(tag: Atom) -> bool {
        //  XX: handle IndirectAtom tags
        match tag.direct() {
            #[allow(clippy::match_like_matches_macro)]
            Some(dtag) => match dtag.data() {
                tas!(b"fast") => false,
                tas!(b"memo") => false,
                _ => true,
            },
            None => true,
        }
    }

    /** Match dynamic hints before the hint formula is evaluated */
    pub(super) fn match_pre_hint(
        context: &mut Context,
        subject: Noun,
        tag: Atom,
        hint: Noun,
        body: Noun,
    ) -> Option<Result> {
        //  XX: handle IndirectAtom tags
        match tag.direct()?.data() {
            tas!(b"sham") => {
                if cfg!(feature = "sham_hints") {
                    let jet_formula = hint.cell()?;
                    // XX: what is the head here?
                    let jet_name = jet_formula.tail();

                    if let Some(jet) = jets::get_jet(context, jet_name) {
                        match jet(context, subject) {
                            Ok(mut jet_res) => {
                                //  XX: simplify this by moving jet test mode into the 11 code in interpret, or into its own function?
                                // if in test mode, check that the jet returns the same result as the raw nock
                                if jets::get_jet_test_mode(jet_name) {
                                    //  XX: we throw away trace, which might matter for non-deterministic errors
                                    //      maybe mook and slog it?
                                    match interpret(context, subject, body) {
                                        Ok(mut nock_res) => {
                                            let stack = &mut context.stack;
                                            if unsafe {
                                                !unifying_equality(
                                                    stack, &mut nock_res, &mut jet_res,
                                                )
                                            } {
                                                //  XX: need NockStack allocated string interpolation
                                                // let tape = tape(stack, "jet mismatch in {}, raw: {}, jetted: {}", jet_name, nock_res, jet_res);
                                                // let mean = T(stack, &[D(tas!(b"mean")), tape]);
                                                // mean_push(stack, mean);
                                                Some(BAIL_EXIT)
                                            } else {
                                                Some(Ok(nock_res))
                                            }
                                        }
                                        Err(error) => {
                                            //  XX: need NockStack allocated string interpolation
                                            // let stack = &mut context.stack;
                                            // let tape = tape(stack, "jet mismatch in {}, raw: {}, jetted: {}", jet_name, err, jet_res);
                                            // let mean = T(stack, &[D(tas!(b"mean")), tape]);
                                            // mean_push(stack, mean);

                                            match error {
                                                Error::NonDeterministic(mote, _) => {
                                                    Some(Err(Error::NonDeterministic(mote, D(0))))
                                                }
                                                _ => Some(BAIL_EXIT),
                                            }
                                        }
                                    }
                                } else {
                                    Some(Ok(jet_res))
                                }
                            }
                            Err(JetErr::Punt) => None,
                            Err(err) => {
                                //  XX: need NockStack allocated string interpolation
                                // let stack = &mut context.stack;
                                // let tape = tape(stack, "{} jet error in {}", err, jet_name);
                                // let mean = T(stack, &[D(tas!(b"mean")), tape]);
                                // mean_push(stack, mean);
                                Some(Err(err.into()))
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            tas!(b"memo") => {
                let stack = &mut context.stack;
                let mut key = Cell::new(stack, subject, body).as_noun();
                context.cache.lookup(stack, &mut key).map(Ok)
            }
            _ => None,
        }
    }

    /** Match static and dynamic hints before the nock formula is evaluated */
    pub(super) fn match_pre_nock(
        context: &mut Context,
        subject: Noun,
        tag: Atom,
        hint: Option<(Noun, Noun)>,
        body: Noun,
    ) -> Option<Result> {
        //  XX: handle IndirectAtom tags
        match tag.direct()?.data() {
            tas!(b"dont") => {
                if cfg!(feature = "hint_dont") {
                    Some(Err(Error::NonDeterministic(Mote::Fail, D(0))))
                } else {
                    None
                }
            }
            tas!(b"bout") => {
                let start = Instant::now();
                let res = interpret(context, subject, body);
                if res.is_ok() {
                    let duration = start.elapsed();
                    flog!(context, "took: {duration:?}");
                }
                Some(res)
            }
            tas!(b"slog") => {
                let stack = &mut context.stack;
                let slogger = &mut context.slogger;

                let (_form, clue) = hint?;
                let slog_cell = clue.cell()?;
                let pri = slog_cell.head().direct()?.data();
                let tank = slog_cell.tail();

                let s = (*slogger).deref_mut();
                s.slog(stack, pri, tank);
                None
            }
            tas!(b"hand") | tas!(b"hunk") | tas!(b"lose") | tas!(b"mean") | tas!(b"spot") => {
                let stack = &mut context.stack;
                let (_form, clue) = hint?;
                let noun = T(stack, &[tag.as_noun(), clue]);
                mean_push(stack, noun);
                None
            }
            tas!(b"hela") => {
                //  XX: This only prints the trace down to the bottom of THIS
                //      interpret call, making this neither a %nara nor a %hela
                //      hint, as Vere understands them. We'll need to
                //      recursively work down frames to get the stack trace all
                //      the way to the root.
                let mean = unsafe { *(context.stack.local_noun_pointer(0)) };
                let tone = Cell::new(&mut context.stack, D(2), mean);

                match mook(context, tone, true) {
                    Ok(toon) => {
                        let stack = &mut context.stack;
                        let slogger = &mut context.slogger;

                        if unsafe { !toon.head().raw_equals(&D(2)) } {
                            // +mook will only ever return a $toon with non-%2 head if that's what it was given as
                            // input. Since we control the input for this call exactly, there must exist a programming
                            // error in Ares if this occurs.
                            panic!("serf: %hela: mook returned invalid tone");
                        }

                        let mut list = toon.tail();
                        loop {
                            if unsafe { list.raw_equals(&D(0)) } {
                                break;
                            }

                            if let Ok(cell) = list.as_cell() {
                                slogger.slog(stack, 0, cell.head());
                                list = cell.tail();
                            } else {
                                flog!(context, "serf: %hela: list ends without ~");
                                break;
                            }
                        }
                        None
                    }
                    Err(_) => {
                        // +mook should only ever bail if the input is not [%2 (list)]. Since we control the input
                        // for this call exactly, there must exist a programming error in Ares if this occurs.
                        panic!("serf: unrecoverable stack trace error");
                    }
                }
            }
            _ => None,
        }
    }

    /** Match static and dynamic hints after the nock formula is evaluated */
    pub(super) fn match_post_nock(
        context: &mut Context,
        subject: Noun,
        tag: Atom,
        hint: Option<Noun>,
        body: Noun,
        res: Noun,
    ) -> Option<Noun> {
        let stack = &mut context.stack;
        let slogger = &mut context.slogger;
        let cold = &mut context.cold;
        let hot = &context.hot;
        let cache = &mut context.cache;

        //  XX: handle IndirectAtom tags
        match tag.direct()?.data() {
            tas!(b"memo") => {
                let mut key = Cell::new(stack, subject, body).as_noun();
                context.cache = cache.insert(stack, &mut key, res);
            }
            tas!(b"hand") | tas!(b"hunk") | tas!(b"lose") | tas!(b"mean") | tas!(b"spot") => {
                mean_pop(stack);
            }
            tas!(b"fast") => {
                if !cfg!(feature = "sham_hints") {
                    if let Some(clue) = hint {
                        let chum = clue.slot(2).ok()?;
                        let mut parent = clue.slot(6).ok()?;
                        loop {
                            if let Ok(parent_cell) = parent.as_cell() {
                                if unsafe { parent_cell.head().raw_equals(&D(11)) } {
                                    match parent.slot(7) {
                                        Ok(noun) => {
                                            parent = noun;
                                        }
                                        Err(_) => {
                                            return None;
                                        }
                                    }
                                } else {
                                    break;
                                }
                            } else {
                                return None;
                            }
                        }
                        let parent_formula_op = parent.slot(2).ok()?.atom()?.direct()?;
                        let parent_formula_ax = parent.slot(3).ok()?.atom()?;

                        let cold_res: cold::Result = {
                            if parent_formula_op.data() == 1 {
                                if parent_formula_ax.direct()?.data() == 0 {
                                    cold.register(stack, res, parent_formula_ax, chum)
                                } else {
                                    //  XX: flog! is ideal, but it runs afoul of the borrow checker
                                    // flog!(context, "invalid root parent formula: {} {}", chum, parent);
                                    let tape = tape(
                                        stack, "serf: cold: register: invalid root parent axis",
                                    );
                                    slog_leaf(stack, slogger, tape);
                                    Ok(false)
                                }
                            } else {
                                cold.register(stack, res, parent_formula_ax, chum)
                            }
                        };

                        match cold_res {
                            Ok(true) => {
                                context.warm = Warm::init(stack, cold, hot, &context.test_jets)
                            }
                            Err(cold::Error::NoParent) => {
                                let Ok(chum_atom) = chum.as_atom() else {
                                    flog!(context, "serf: cold: register: cell chum");
                                    return None;
                                };
                                let chum_bytes = Vec::from(chum_atom.as_ne_bytes());
                                let Ok(chum_str) = String::from_utf8(chum_bytes) else {
                                    flog!(context, "serf: cold: register: unprintable chum");
                                    return None;
                                };
                                flog!(context, "serf: cold: register: could not match parent battery at given axis: {} {:?}", chum_str, parent_formula_ax);
                            }
                            Err(cold::Error::BadNock) => {
                                flog!(
                                    context, "serf: cold: register: bad clue formula: {:?}", clue
                                );
                            }
                            _ => {}
                        }
                    } else {
                        flog!(context, "serf: cold: register: no clue for %fast");
                    }
                }
            }
            _ => {}
        }

        None
    }

    fn slog_leaf(stack: &mut NockStack, slogger: &mut Pin<Box<dyn Slogger + Unpin>>, tape: Noun) {
        let tank = T(stack, &[LEAF, tape]);
        slogger.slog(stack, 0u64, tank);
    }
}

mod debug {
    use either::Either::*;

    use crate::noun::Noun;

    #[allow(dead_code)]
    pub(super) fn assert_normalized(noun: Noun, path: Noun) {
        assert_normalized_helper(noun, path, None);
    }

    #[allow(dead_code)]
    pub(super) fn assert_normalized_depth(noun: Noun, path: Noun, depth: usize) {
        assert_normalized_helper(noun, path, Some(depth));
    }

    #[allow(dead_code)]
    fn assert_normalized_helper(noun: Noun, path: Noun, depth: Option<usize>) {
        match noun.as_either_atom_cell() {
            Left(atom) => {
                if !atom.is_normalized() {
                    if atom.size() == 1 {
                        panic!(
                            "Un-normalized indirect_atom (should be direct) returned from jet for {path:?}",
                        );
                    } else {
                        panic!(
                            "Un-normalized indirect_atom (last word 0) returned from jet for {path:?}",
                        );
                    }
                }
            }
            Right(cell) => {
                if depth.is_none_or(|d| d != 0) {
                    let new_depth = depth.map(|x| x - 1);
                    assert_normalized_helper(cell.head(), path, new_depth);
                    assert_normalized_helper(cell.tail(), path, new_depth);
                }
            }
        }
    }
}
