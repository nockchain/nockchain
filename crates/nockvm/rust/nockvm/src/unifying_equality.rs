#![allow(clippy::missing_safety_doc)]

use either::Either::*;
use libc::{c_void, memcmp};

use crate::mem::{NockStack, ALLOC, FRAME, STACK};
use crate::noun::{Cell, Noun, NounSpace};

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

#[derive(Copy, Clone)]
enum EqualityWork {
    Compare {
        x: *mut Noun,
        y: *mut Noun,
        x_slot_readonly: bool,
        y_slot_readonly: bool,
    },
    FinishCell {
        x: *mut Noun,
        y: *mut Noun,
        x_slot_readonly: bool,
        y_slot_readonly: bool,
        x_noun_readonly: bool,
        y_noun_readonly: bool,
        xptr: *const u64,
        yptr: *const u64,
    },
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

#[inline(always)]
unsafe fn cell_child_ptrs(
    cell: Cell,
    space: &NounSpace,
    slot_readonly: bool,
) -> (*mut Noun, *mut Noun) {
    if slot_readonly {
        let raw = cell.to_raw_pointer(space);
        (
            std::ptr::addr_of!((*raw).head) as *mut Noun,
            std::ptr::addr_of!((*raw).tail) as *mut Noun,
        )
    } else {
        (cell.head_as_mut(space), cell.tail_as_mut(space))
    }
}

#[derive(Copy, Clone)]
struct RewriteSlot {
    noun: *mut Noun,
    allocated_ptr: *const u64,
    slot_readonly: bool,
    noun_readonly: bool,
}

struct RewriteDirection<'a> {
    stack: &'a NockStack,
    space: &'a NounSpace,
    current_bounds: FrameBounds,
    bounds_state: &'a mut FrameBoundsState,
    x: RewriteSlot,
    y: RewriteSlot,
}

#[inline(always)]
unsafe fn rewrite_direction(args: RewriteDirection<'_>) -> Option<bool> {
    if args.x.noun_readonly || args.y.noun_readonly {
        return None;
    }

    let x_loc = (*args.x.noun).allocated_location(args.space);
    let y_loc = (*args.y.noun).allocated_location(args.space);
    let replace_x = match (x_loc, y_loc) {
        (Some(xl), Some(yl)) if xl.is_pma() && yl.is_stack() => false,
        (Some(xl), Some(yl)) if xl.is_stack() && yl.is_pma() => true,
        (Some(xl), Some(yl)) if xl.is_pma() && yl.is_pma() => {
            args.x.allocated_ptr > args.y.allocated_ptr
        }
        _ => x_is_junior(
            args.stack, args.current_bounds, args.bounds_state, args.x.allocated_ptr,
            args.y.allocated_ptr,
        ),
    };

    if replace_x {
        (!args.x.slot_readonly).then_some(true)
    } else {
        (!args.y.slot_readonly).then_some(false)
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
    #[cfg(any(feature = "check_acyclic", feature = "check_forwarding"))]
    {
        let _space = stack.fast_noun_space();
        crate::assert_acyclic!(_space, *a);
        crate::assert_acyclic!(_space, *b);
        crate::assert_no_forwarding_pointers!(_space, *a);
        crate::assert_no_forwarding_pointers!(_space, *b);
    }
    crate::assert_no_junior_pointers!(stack, *a);
    crate::assert_no_junior_pointers!(stack, *b);

    if raw_word_eq(a, b) {
        return true;
    }

    let space = stack.fast_noun_space();
    let has_readonly_extra_ranges = space.has_readonly_extra_ptr_ranges();
    if let (Ok(aa), Ok(bb)) = ((*a).as_allocated(), (*b).as_allocated()) {
        if let (Some(am), Some(bm)) = (aa.get_cached_mug(&space), bb.get_cached_mug(&space)) {
            if am != bm {
                return false;
            }
        }
    }

    stack.frame_push(0);
    let current_bounds = current_frame_bounds(stack);
    let mut bounds_state = FrameBoundsState::new();
    *(stack.push::<EqualityWork>()) = EqualityWork::Compare {
        x: a,
        y: b,
        x_slot_readonly: false,
        y_slot_readonly: false,
    };
    let mut failed = false;

    loop {
        if stack.stack_is_empty() {
            break;
        }

        let work: EqualityWork = *(stack.top());
        stack.pop::<EqualityWork>();
        match work {
            EqualityWork::FinishCell {
                x,
                y,
                x_slot_readonly,
                y_slot_readonly,
                x_noun_readonly,
                y_noun_readonly,
                xptr,
                yptr,
            } => {
                if raw_word_eq(x, y) {
                    continue;
                }
                if let Some(replace_x) = rewrite_direction(RewriteDirection {
                    stack,
                    space: &space,
                    current_bounds,
                    bounds_state: &mut bounds_state,
                    x: RewriteSlot {
                        noun: x,
                        allocated_ptr: xptr,
                        slot_readonly: x_slot_readonly,
                        noun_readonly: x_noun_readonly,
                    },
                    y: RewriteSlot {
                        noun: y,
                        allocated_ptr: yptr,
                        slot_readonly: y_slot_readonly,
                        noun_readonly: y_noun_readonly,
                    },
                }) {
                    if replace_x {
                        *x = *y;
                    } else {
                        *y = *x;
                    }
                }
            }
            EqualityWork::Compare {
                x,
                y,
                x_slot_readonly,
                y_slot_readonly,
            } => {
                if raw_word_eq(x, y) {
                    continue;
                }

                let x_noun_readonly =
                    has_readonly_extra_ranges && space.noun_in_readonly_extra_range(*x);
                let y_noun_readonly =
                    has_readonly_extra_ranges && space.noun_in_readonly_extra_range(*y);

                if let (Ok(xa), Ok(ya)) = ((*x).as_allocated(), (*y).as_allocated()) {
                    if let (Some(xm), Some(ym)) =
                        (xa.get_cached_mug(&space), ya.get_cached_mug(&space))
                    {
                        if xm != ym {
                            failed = true;
                            break;
                        }
                    }

                    match (xa.as_either(), ya.as_either()) {
                        (Left(xi), Left(yi)) => {
                            let xi_handle = xi.as_atom().in_space(&space);
                            let yi_handle = yi.as_atom().in_space(&space);
                            let xsize = xi_handle.size();
                            let ysize = yi_handle.size();
                            if xsize != ysize {
                                failed = true;
                                break;
                            }

                            if memcmp(
                                xi_handle.data_pointer() as *const c_void,
                                yi_handle.data_pointer() as *const c_void,
                                xsize << 3,
                            ) != 0
                            {
                                failed = true;
                                break;
                            }

                            let xptr = xi_handle.raw_pointer();
                            let yptr = yi_handle.raw_pointer();
                            if let Some(replace_x) = rewrite_direction(RewriteDirection {
                                stack,
                                space: &space,
                                current_bounds,
                                bounds_state: &mut bounds_state,
                                x: RewriteSlot {
                                    noun: x,
                                    allocated_ptr: xptr,
                                    slot_readonly: x_slot_readonly,
                                    noun_readonly: x_noun_readonly,
                                },
                                y: RewriteSlot {
                                    noun: y,
                                    allocated_ptr: yptr,
                                    slot_readonly: y_slot_readonly,
                                    noun_readonly: y_noun_readonly,
                                },
                            }) {
                                if replace_x {
                                    *x = *y;
                                } else {
                                    *y = *x;
                                }
                            }
                        }

                        (Right(xc), Right(yc)) => {
                            let x_children_readonly = x_slot_readonly || x_noun_readonly;
                            let y_children_readonly = y_slot_readonly || y_noun_readonly;
                            let (xh, xt) = cell_child_ptrs(xc, &space, x_children_readonly);
                            let (yh, yt) = cell_child_ptrs(yc, &space, y_children_readonly);
                            let xptr = xc.to_raw_pointer(&space) as *const u64;
                            let yptr = yc.to_raw_pointer(&space) as *const u64;

                            if raw_word_eq(xh, yh) && raw_word_eq(xt, yt) {
                                if let Some(replace_x) = rewrite_direction(RewriteDirection {
                                    stack,
                                    space: &space,
                                    current_bounds,
                                    bounds_state: &mut bounds_state,
                                    x: RewriteSlot {
                                        noun: x,
                                        allocated_ptr: xptr,
                                        slot_readonly: x_slot_readonly,
                                        noun_readonly: x_noun_readonly,
                                    },
                                    y: RewriteSlot {
                                        noun: y,
                                        allocated_ptr: yptr,
                                        slot_readonly: y_slot_readonly,
                                        noun_readonly: y_noun_readonly,
                                    },
                                }) {
                                    if replace_x {
                                        *x = *y;
                                    } else {
                                        *y = *x;
                                    }
                                }
                                continue;
                            }

                            *(stack.push::<EqualityWork>()) = EqualityWork::FinishCell {
                                x,
                                y,
                                x_slot_readonly,
                                y_slot_readonly,
                                x_noun_readonly,
                                y_noun_readonly,
                                xptr,
                                yptr,
                            };
                            if !raw_word_eq(xt, yt) {
                                *(stack.push::<EqualityWork>()) = EqualityWork::Compare {
                                    x: xt,
                                    y: yt,
                                    x_slot_readonly: x_children_readonly,
                                    y_slot_readonly: y_children_readonly,
                                };
                            }
                            if !raw_word_eq(xh, yh) {
                                *(stack.push::<EqualityWork>()) = EqualityWork::Compare {
                                    x: xh,
                                    y: yh,
                                    x_slot_readonly: x_children_readonly,
                                    y_slot_readonly: y_children_readonly,
                                };
                            }
                        }

                        _ => {
                            failed = true;
                            break;
                        }
                    }
                } else {
                    failed = true;
                    break;
                }
            }
        }
    }

    stack.frame_pop();

    #[cfg(any(feature = "check_acyclic", feature = "check_forwarding"))]
    {
        let _space = stack.fast_noun_space();
        crate::assert_acyclic!(_space, *a);
        crate::assert_acyclic!(_space, *b);
        crate::assert_no_forwarding_pointers!(_space, *a);
        crate::assert_no_forwarding_pointers!(_space, *b);
    }
    crate::assert_no_junior_pointers!(stack, *a);
    crate::assert_no_junior_pointers!(stack, *b);

    !failed
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
    let mut stack_pointer: *const u64 = stack.get_stack_pointer();
    let mut alloc_pointer: *const u64 = stack.get_alloc_pointer();
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

#[cfg(test)]
mod tests {
    use super::unifying_equality;
    use crate::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use crate::noun::{Cell, CellMemory, Noun, D};
    use crate::pma::{test_pma_path, Pma};

    unsafe fn foreign_cell(head: Noun, tail: Noun) -> (*mut CellMemory, Noun, Vec<(usize, usize)>) {
        let ptr = Box::into_raw(Box::new(CellMemory {
            metadata: 0,
            head,
            tail,
        }));
        let start = ptr as usize;
        let end = start + std::mem::size_of::<CellMemory>();
        (
            ptr,
            Cell::from_raw_pointer(ptr).as_noun(),
            vec![(start, end)],
        )
    }

    #[test]
    fn readonly_extra_range_does_not_unify_foreign_root() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let stack_cell = Cell::new(&mut stack, D(1), D(2)).as_noun();
        let original_stack_cell = unsafe { stack_cell.as_raw() };

        unsafe {
            let (foreign_ptr, foreign_noun, ranges) = foreign_cell(D(1), D(2));
            let original_foreign = *foreign_ptr;
            let previous_ranges = stack.replace_extra_noun_ptr_ranges(ranges);

            let mut left = foreign_noun;
            let mut right = stack_cell;
            assert!(unifying_equality(&mut stack, &mut left, &mut right));
            assert_eq!(right.as_raw(), original_stack_cell);
            assert_eq!((*foreign_ptr).head.as_raw(), original_foreign.head.as_raw());
            assert_eq!((*foreign_ptr).tail.as_raw(), original_foreign.tail.as_raw());

            stack.replace_extra_noun_ptr_ranges(previous_ranges);
            drop(Box::from_raw(foreign_ptr));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn readonly_extra_range_still_unifies_pma_descendants() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let path = test_pma_path("readonly_extra_range_still_unifies_pma_descendants");
        let mut pma = Pma::new(1024, path.clone()).expect("create PMA");
        stack.install_pma_arena(std::sync::Arc::clone(pma.arena()));

        let pma_child = Cell::new(&mut pma, D(1), D(2)).as_noun();
        let stack_child = Cell::new(&mut stack, D(1), D(2)).as_noun();
        let mut stack_outer = Cell::new(&mut stack, stack_child, D(0)).as_noun();
        let original_stack_outer = unsafe { stack_outer.as_raw() };

        unsafe {
            let (foreign_ptr, foreign_noun, ranges) = foreign_cell(pma_child, D(0));
            let previous_ranges = stack.replace_extra_noun_ptr_ranges(ranges);

            let mut left = foreign_noun;
            assert!(unifying_equality(&mut stack, &mut left, &mut stack_outer));

            let space = stack.noun_space();
            let outer = stack_outer
                .in_space(&space)
                .as_cell()
                .expect("stack outer cell");
            assert_eq!(stack_outer.as_raw(), original_stack_outer);
            assert!(outer.head().noun().raw_equals(&pma_child));
            assert_eq!((*foreign_ptr).head.as_raw(), pma_child.as_raw());
            assert_eq!((*foreign_ptr).tail.as_raw(), D(0).as_raw());

            stack.replace_extra_noun_ptr_ranges(previous_ranges);
            drop(Box::from_raw(foreign_ptr));
        }

        drop(pma);
        std::fs::remove_file(path).expect("remove PMA file");
    }
}
