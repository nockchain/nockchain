use either::Either::*;
use libc::{c_void, memcmp};

use crate::mem::{NockStack, ALLOC, FRAME, STACK};
use crate::noun::Noun;
use crate::{assert_acyclic, assert_no_forwarding_pointers, assert_no_junior_pointers};

#[cfg(feature = "check_junior")]
#[macro_export]
macro_rules! assert_no_junior_pointers {
    ( $x:expr, $y:expr ) => {
        assert_no_alloc::permit_alloc(|| {
            assert!($x.no_junior_pointers($y));
        })
    };
}

#[cfg(not(feature = "check_junior"))]
#[macro_export]
macro_rules! assert_no_junior_pointers {
    ( $x:expr, $y:expr ) => {};
}

#[inline(always)]
unsafe fn raw_word_eq(a: *const Noun, b: *const Noun) -> bool {
    // bitwise equality of the noun representation
    (*a).as_raw() == (*b).as_raw()
}

const MAX_FRAME_BOUNDS: usize = 64;

#[derive(Copy, Clone)]
struct FrameBounds {
    frame_index: usize,
    low: *const u64,
    high: *const u64,
}

impl FrameBounds {
    #[inline(always)]
    unsafe fn contains(&self, ptr: *const u64) -> bool {
        ptr >= self.low && ptr < self.high
    }
}

struct FrameBoundsBuf {
    len: usize,
    data: [FrameBounds; MAX_FRAME_BOUNDS],
}

impl FrameBoundsBuf {
    #[inline(always)]
    fn new() -> Self {
        const EMPTY: FrameBounds = FrameBounds {
            frame_index: 0,
            low: std::ptr::null(),
            high: std::ptr::null(),
        };
        Self {
            len: 0,
            data: [EMPTY; MAX_FRAME_BOUNDS],
        }
    }

    #[inline(always)]
    fn clear(&mut self) {
        self.len = 0;
    }

    #[inline(always)]
    fn push(&mut self, bounds: FrameBounds) {
        if self.len >= MAX_FRAME_BOUNDS {
            debug_assert!(false, "FrameBoundsBuf overflow");
            return;
        }
        self.data[self.len] = bounds;
        self.len += 1;
    }

    #[inline(always)]
    fn as_slice(&self) -> &[FrameBounds] {
        &self.data[..self.len]
    }
}

unsafe fn current_frame_bounds(stack: &NockStack) -> FrameBounds {
    let start = stack.get_start();
    let end = start.add(stack.get_size());

    let alloc_ptr = stack.get_alloc_pointer();
    let prev_stack_ptr = *(stack.prev_stack_pointer_pointer()) as *const u64;

    let mut low = alloc_ptr;
    let mut high = prev_stack_ptr;
    if !stack.is_west() {
        low = prev_stack_ptr;
        high = alloc_ptr;
    }

    if low.is_null() {
        low = start;
    }
    if high.is_null() {
        high = end;
    }
    if low > high {
        core::mem::swap(&mut low, &mut high);
    }

    FrameBounds {
        frame_index: 0,
        low,
        high,
    }
}

unsafe fn collect_frame_bounds(stack: &NockStack, buf: &mut FrameBoundsBuf, current: FrameBounds) {
    buf.clear();
    let start = stack.get_start();
    let end = start.add(stack.get_size());

    let mut frame_pointer: *const u64 = stack.get_frame_pointer();
    let mut stack_pointer: *const u64 = stack.get_stack_pointer();
    let mut alloc_pointer: *const u64 = stack.get_alloc_pointer();

    buf.push(current);
    let mut next_index = 1usize;

    loop {
        if stack_pointer < alloc_pointer {
            let new_stack_pointer = *(frame_pointer.sub(STACK + 1)) as *const u64;
            let new_alloc_pointer = *(frame_pointer.sub(ALLOC + 1)) as *const u64;
            let new_frame_pointer = *(frame_pointer.sub(FRAME + 1)) as *const u64;
            if new_frame_pointer.is_null() {
                break;
            }
            if next_index >= MAX_FRAME_BOUNDS {
                break;
            }
            let mut low = *(new_frame_pointer.add(STACK)) as *const u64;
            let mut high = new_alloc_pointer;
            if low.is_null() {
                low = start;
            }
            if high.is_null() {
                high = end;
            }
            if low > high {
                core::mem::swap(&mut low, &mut high);
            }
            buf.push(FrameBounds {
                frame_index: next_index,
                low,
                high,
            });
            next_index += 1;

            frame_pointer = new_frame_pointer;
            stack_pointer = new_stack_pointer;
            alloc_pointer = new_alloc_pointer;
        } else if stack_pointer > alloc_pointer {
            let new_stack_pointer = *(frame_pointer.add(STACK)) as *const u64;
            let new_alloc_pointer = *(frame_pointer.add(ALLOC)) as *const u64;
            let new_frame_pointer = *(frame_pointer.add(FRAME)) as *const u64;
            if new_frame_pointer.is_null() {
                break;
            }
            if next_index >= MAX_FRAME_BOUNDS {
                break;
            }
            let mut low = new_alloc_pointer;
            let mut high = *(new_frame_pointer.sub(STACK + 1)) as *const u64;
            if low.is_null() {
                low = start;
            }
            if high.is_null() {
                high = end;
            }
            if low > high {
                core::mem::swap(&mut low, &mut high);
            }
            buf.push(FrameBounds {
                frame_index: next_index,
                low,
                high,
            });
            next_index += 1;

            frame_pointer = new_frame_pointer;
            stack_pointer = new_stack_pointer;
            alloc_pointer = new_alloc_pointer;
        } else {
            core::hint::cold_path();
            panic!("x_is_junior: stack_pointer == alloc_pointer");
        }
    }
}

struct FrameBoundsState {
    buf: core::mem::MaybeUninit<FrameBoundsBuf>,
    initialized: bool,
}

impl FrameBoundsState {
    fn new() -> Self {
        Self {
            buf: core::mem::MaybeUninit::uninit(),
            initialized: false,
        }
    }

    unsafe fn ensure<'a>(
        &'a mut self,
        stack: &NockStack,
        current: FrameBounds,
    ) -> &'a [FrameBounds] {
        if !self.initialized {
            self.buf.write(FrameBoundsBuf::new());
            let buf_mut = self.buf.assume_init_mut();
            collect_frame_bounds(stack, buf_mut, current);
            self.initialized = true;
        }
        self.buf.assume_init_ref().as_slice()
    }
}

// true = x is junior, false = y is junior
#[inline(always)]
unsafe fn x_is_junior(
    stack: &NockStack,
    current: FrameBounds,
    state: &mut FrameBoundsState,
    x: *const u64,
    y: *const u64,
) -> bool {
    let xin = current.contains(x);
    let yin = current.contains(y);
    if xin ^ yin {
        return xin;
    }
    if xin & yin {
        return x > y;
    }

    if !xin && !yin {
        let (_senior, junior) = senior_pointer_first(stack, x, y);
        return std::ptr::eq(x, junior);
    }

    let bounds = state.ensure(stack, current);
    let mut x_idx = None;
    let mut y_idx = None;
    for span in bounds {
        if x_idx.is_none() && span.contains(x) {
            x_idx = Some(span.frame_index);
        }
        if y_idx.is_none() && span.contains(y) {
            y_idx = Some(span.frame_index);
        }
        if x_idx.is_some() && y_idx.is_some() {
            break;
        }
    }

    match (x_idx, y_idx) {
        (Some(xi), Some(yi)) => {
            if xi != yi {
                xi < yi
            } else {
                x > y
            }
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => x > y,
    }
}

/// This version of unifying equality is not like that of vere.
/// Vere does a tree comparison (accelerated by pointer equality and short-circuited by mug
/// equality) and then unifies the nouns at the top level if they are equal.
/// Here we recursively attempt to unify nouns. Pointer-equal nouns are already unified.
/// Disequal mugs again short-circuit the unification and equality check.
/// Since we expect atoms to be normalized, direct and indirect atoms do not unify with each
/// other. For direct atoms, no unification is possible as there is no pointer involved in their
/// representation. Equality is simply direct equality on the word representation. Indirect
/// atoms require equality first of the size and then of the memory buffers' contents.
/// Cell equality is tested (after mug and pointer equality) by attempting to unify the heads and tails,
/// respectively, of cells, and then re-testing. If unification succeeds then the heads and
/// tails will be pointer-wise equal and the cell itself can be unified. A failed unification of
/// the head or the tail will already short-circuit the unification/equality test, so we will
/// not return to re-test the pointer equality.
/// When actually mutating references for unification, we must be careful to respect seniority.
/// A reference to a more junior noun should always be replaced with a reference to a more
/// senior noun, *never vice versa*, to avoid introducing references from more senior frames
/// into more junior frames, which would result in incorrect operation of the copier.
pub unsafe fn unifying_equality(stack: &mut NockStack, a: *mut Noun, b: *mut Noun) -> bool {
    assert_acyclic!(*a);
    assert_acyclic!(*b);
    assert_no_forwarding_pointers!(*a);
    assert_no_forwarding_pointers!(*b);
    assert_no_junior_pointers!(stack, *a);
    assert_no_junior_pointers!(stack, *b);

    if raw_word_eq(a, b) {
        return true;
    }

    if let (Ok(aa), Ok(bb)) = ((*a).as_allocated(), (*b).as_allocated()) {
        if let (Some(am), Some(bm)) = (aa.get_cached_mug(), bb.get_cached_mug()) {
            if am != bm {
                return false;
            }
        }
    }

    stack.frame_push(0);
    let current_bounds = current_frame_bounds(stack);
    let mut bounds_state = FrameBoundsState::new();
    *(stack.push::<(*mut Noun, *mut Noun)>()) = (a, b);

    loop {
        if stack.stack_is_empty() {
            break;
        }

        let (x, y): (*mut Noun, *mut Noun) = *(stack.top());
        if raw_word_eq(x, y) {
            stack.pop::<(*mut Noun, *mut Noun)>();
            continue;
        }

        if let (Ok(xa), Ok(ya)) = ((*x).as_allocated(), (*y).as_allocated()) {
            if let (Some(xm), Some(ym)) = (xa.get_cached_mug(), ya.get_cached_mug()) {
                if xm != ym {
                    break;
                }
            }

            match (xa.as_either(), ya.as_either()) {
                (Left(xi), Left(yi)) => {
                    if xi.size() == yi.size()
                        && memcmp(
                            xi.data_pointer() as *const c_void,
                            yi.data_pointer() as *const c_void,
                            xi.size() << 3,
                        ) == 0
                    {
                        let xptr = xi.to_raw_pointer();
                        let yptr = yi.to_raw_pointer();
                        if x_is_junior(stack, current_bounds, &mut bounds_state, xptr, yptr) {
                            *x = *y;
                        } else {
                            *y = *x;
                        }
                        stack.pop::<(*mut Noun, *mut Noun)>();
                        continue;
                    } else {
                        break;
                    }
                }

                (Right(xc), Right(yc)) => {
                    // check head; only compute tail eq if needed; push only unequal sides
                    let xh = xc.head_as_mut();
                    let yh = yc.head_as_mut();
                    if raw_word_eq(xh, yh) {
                        let xt = xc.tail_as_mut();
                        let yt = yc.tail_as_mut();
                        if raw_word_eq(xt, yt) {
                            let xptr = xc.to_raw_pointer() as *const u64;
                            let yptr = yc.to_raw_pointer() as *const u64;
                            if x_is_junior(stack, current_bounds, &mut bounds_state, xptr, yptr) {
                                *x = *y;
                            } else {
                                *y = *x;
                            }
                            stack.pop::<(*mut Noun, *mut Noun)>();
                            continue;
                        } else {
                            *(stack.push::<(*mut Noun, *mut Noun)>()) = (xt, yt);
                            continue;
                        }
                    } else {
                        // head unequal; only push tail if it is also unequal
                        let xt = xc.tail_as_mut();
                        let yt = yc.tail_as_mut();
                        if !raw_word_eq(xt, yt) {
                            *(stack.push::<(*mut Noun, *mut Noun)>()) = (xt, yt);
                        }
                        *(stack.push::<(*mut Noun, *mut Noun)>()) = (xh, yh);
                        continue;
                    }
                }

                _ => {
                    break;
                } // cells don't unify with atoms
            }
        } else {
            break; // direct atom and not raw-equal
        }
    }

    stack.frame_pop();

    assert_acyclic!(*a);
    assert_acyclic!(*b);
    assert_no_forwarding_pointers!(*a);
    assert_no_forwarding_pointers!(*b);
    assert_no_junior_pointers!(stack, *a);
    assert_no_junior_pointers!(stack, *b);

    raw_word_eq(a, b)
}

#[inline(always)]
unsafe fn normalize_bounds(
    low: &mut *const u64,
    high: &mut *const u64,
    arena_start: *const u64,
    arena_end: *const u64,
) {
    if low.is_null() {
        *low = arena_start;
    }
    if high.is_null() {
        *high = arena_end;
    }
    if *low > *high {
        core::mem::swap(low, high);
    }
}

#[inline(always)]
unsafe fn ptr_in_range(ptr: *const u64, low: *const u64, high: *const u64) -> bool {
    ptr >= low && ptr < high
}

unsafe fn senior_pointer_first(
    stack: &NockStack,
    a: *const u64,
    b: *const u64,
) -> (*const u64, *const u64) {
    let arena_start = stack.get_start();
    let arena_end = arena_start.add(stack.get_size());

    let mut frame_pointer: *const u64 = stack.get_frame_pointer();
    let mut stack_pointer: *const u64 = stack.get_stack_pointer() as *const u64;
    let mut alloc_pointer: *const u64 = stack.get_alloc_pointer() as *const u64;
    let prev_stack_pointer = *(stack.prev_stack_pointer_pointer()) as *const u64;

    let mut low_pointer;
    let mut high_pointer;

    if stack.is_west() {
        low_pointer = prev_stack_pointer;
        high_pointer = alloc_pointer;
    } else {
        low_pointer = alloc_pointer;
        high_pointer = prev_stack_pointer;
    }

    normalize_bounds(&mut low_pointer, &mut high_pointer, arena_start, arena_end);

    loop {
        let a_in = ptr_in_range(a, low_pointer, high_pointer);
        let b_in = ptr_in_range(b, low_pointer, high_pointer);

        match (a_in, b_in) {
            (true, false) => break (b, a),
            (false, true) => break (a, b),
            (true, true) => break lower_pointer_first(a, b),
            (false, false) => {
                if stack_pointer < alloc_pointer {
                    let new_stack_pointer = *(frame_pointer.sub(STACK + 1)) as *const u64;
                    let new_alloc_pointer = *(frame_pointer.sub(ALLOC + 1)) as *const u64;
                    let new_frame_pointer = *(frame_pointer.sub(FRAME + 1)) as *const u64;

                    if new_frame_pointer.is_null() {
                        break lower_pointer_first(a, b);
                    }

                    stack_pointer = new_stack_pointer;
                    alloc_pointer = new_alloc_pointer;
                    frame_pointer = new_frame_pointer;

                    high_pointer = alloc_pointer;
                    low_pointer = *(frame_pointer.add(STACK)) as *const u64;
                    normalize_bounds(&mut low_pointer, &mut high_pointer, arena_start, arena_end);
                } else if stack_pointer > alloc_pointer {
                    let new_stack_pointer = *(frame_pointer.add(STACK)) as *const u64;
                    let new_alloc_pointer = *(frame_pointer.add(ALLOC)) as *const u64;
                    let new_frame_pointer = *(frame_pointer.add(FRAME)) as *const u64;

                    if new_frame_pointer.is_null() {
                        break lower_pointer_first(a, b);
                    }

                    stack_pointer = new_stack_pointer;
                    alloc_pointer = new_alloc_pointer;
                    frame_pointer = new_frame_pointer;

                    low_pointer = alloc_pointer;
                    high_pointer = *(frame_pointer.sub(STACK + 1)) as *const u64;
                    normalize_bounds(&mut low_pointer, &mut high_pointer, arena_start, arena_end);
                } else {
                    core::hint::cold_path();
                    panic!("senior_pointer_first: stack_pointer == alloc_pointer");
                }
            }
        }
    }
}

fn lower_pointer_first(a: *const u64, b: *const u64) -> (*const u64, *const u64) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}
