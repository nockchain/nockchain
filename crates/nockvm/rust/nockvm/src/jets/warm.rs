use std::ptr::{copy_nonoverlapping, null_mut};
use std::time::Instant;

use nockvm_macros::tas;
use tracing::debug;

use crate::hamt::Hamt;
use crate::jets::cold::{Batteries, Cold};
use crate::jets::hot::Hot;
use crate::jets::{Jet, JetDispatchMode};
use crate::mem::{NockStack, Preserve};
use crate::noun::{Noun, NounAllocator, NounSpace, Slots, T};
use crate::pma::{Pma, PmaCopy, PmaCopyFrom};

/// key = formula
#[derive(Copy, Clone)]
pub struct Warm(Hamt<WarmEntry>);

impl Preserve for Warm {
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        self.0.assert_in_stack(stack);
    }
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        self.0.preserve(stack);
    }
}

impl PmaCopy for Warm {
    #[cfg(feature = "pma-assert")]
    fn assert_in_pma(&self, pma: &Pma) {
        self.0.assert_in_pma(pma);
    }

    #[cfg(not(feature = "pma-assert"))]
    #[inline(always)]
    fn assert_in_pma(&self, _pma: &Pma) {}

    unsafe fn copy_to_pma(&mut self, stack: &NockStack, pma: &mut Pma) {
        let trace = std::env::var_os("NOCK_PMA_TRACE").is_some();
        if trace {
            debug!("pma-copy: warm stats start");
            let stats_start = Instant::now();
            let mut last_progress = Instant::now();
            let mut key_count = 0usize;
            let mut list_count = 0usize;
            let mut total_nodes = 0usize;
            let mut nodes_stack = 0usize;
            let mut nodes_pma = 0usize;
            let mut prefix_nodes = 0usize;
            let mut lists_all_stack = 0usize;
            let mut lists_all_pma = 0usize;
            let mut lists_mixed = 0usize;
            let mut lists_mixed_after_pma = 0usize;

            for leaf in self.0.iter() {
                key_count += leaf.len();
                for (_key, entry) in leaf {
                    list_count += 1;
                    let mut cursor = *entry;
                    let mut seen_pma = false;
                    let mut list_stack = 0usize;
                    let mut list_pma = 0usize;
                    let mut list_prefix = 0usize;
                    let mut list_mixed_after_pma = false;
                    while !cursor.0.is_null() {
                        total_nodes += 1;
                        if pma.contains_ptr(cursor.0 as *const u8) {
                            nodes_pma += 1;
                            list_pma += 1;
                            seen_pma = true;
                        } else {
                            nodes_stack += 1;
                            list_stack += 1;
                            if seen_pma {
                                list_mixed_after_pma = true;
                            } else {
                                list_prefix += 1;
                            }
                        }
                        cursor = (*cursor.0).next;
                        if total_nodes & 0x3fff == 0 {
                            let now = Instant::now();
                            if now.duration_since(last_progress).as_millis() >= 2000 {
                                debug!(
                                    "pma-copy: warm stats progress: lists={}, nodes={}, stack_nodes={}, pma_nodes={}, elapsed_ms={}",
                                    list_count,
                                    total_nodes,
                                    nodes_stack,
                                    nodes_pma,
                                    stats_start.elapsed().as_millis()
                                );
                                last_progress = now;
                            }
                        }
                    }
                    prefix_nodes += list_prefix;
                    if list_pma == 0 {
                        lists_all_stack += 1;
                    } else if list_stack == 0 {
                        lists_all_pma += 1;
                    } else {
                        lists_mixed += 1;
                    }
                    if list_mixed_after_pma {
                        lists_mixed_after_pma += 1;
                    }
                }
            }

            let stats_ms = stats_start.elapsed().as_millis();
            debug!(
                "pma-copy: warm stats done: keys={}, lists={}, nodes={}, stack_nodes={}, pma_nodes={}, prefix_nodes={}, lists_all_stack={}, lists_all_pma={}, lists_mixed={}, lists_mixed_after_pma={}, stats_ms={}",
                key_count,
                list_count,
                total_nodes,
                nodes_stack,
                nodes_pma,
                prefix_nodes,
                lists_all_stack,
                lists_all_pma,
                lists_mixed,
                lists_mixed_after_pma,
                stats_ms
            );
            let alloc_before = pma.alloc_offset();
            debug!("pma-copy: warm copy start");
            let copy_start = Instant::now();
            self.0.copy_to_pma(stack, pma);
            let copy_ms = copy_start.elapsed().as_millis();
            let alloc_after = pma.alloc_offset();
            let alloc_words = alloc_after.saturating_sub(alloc_before);

            debug!(
                "pma-copy: warm copy done: alloc_words={}, copy_ms={}",
                alloc_words, copy_ms
            );
        } else {
            self.0.copy_to_pma(stack, pma);
        }
    }
}

impl PmaCopyFrom for Warm {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        self.0.copy_from_pma(from_pma, to_pma);
    }
}

#[derive(Copy, Clone)]
struct WarmEntry(*mut WarmEntryMem);

const WARM_ENTRY_NIL: WarmEntry = WarmEntry(null_mut());

struct WarmEntryMem {
    batteries: Batteries,
    jet: Jet,
    path: Noun, // useful for profiling/debugging
    test: bool, // Whether to *also* run the hoon for this jet
    next: WarmEntry,
}
#[derive(Copy, Clone)]
struct WarmRegistration {
    path: Noun,
    batteries: Batteries,
    jet: Jet,
    test: bool,
}

impl Preserve for WarmEntry {
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        if self.0.is_null() {
            return;
        };
        let mut cursor = *self;
        loop {
            stack.assert_struct_is_in(cursor.0, 1);
            (*cursor.0).batteries.assert_in_stack(stack);
            (*cursor.0).path.assert_in_stack(stack);
            if (*cursor.0).next.0.is_null() {
                break;
            };
            cursor = (*cursor.0).next;
        }
    }
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        if self.0.is_null() {
            return;
        }
        let mut ptr: *mut *mut WarmEntryMem = &mut self.0;
        loop {
            if stack.is_in_frame(*ptr) {
                (**ptr).batteries.preserve(stack);
                (**ptr).path.preserve(stack);
                let dest_mem: *mut WarmEntryMem = stack.struct_alloc_in_previous_frame(1);
                copy_nonoverlapping(*ptr, dest_mem, 1);
                *ptr = dest_mem;
                ptr = &mut ((*dest_mem).next.0);
                if (*dest_mem).next.0.is_null() {
                    break;
                };
            } else {
                break;
            }
        }
    }
}

impl PmaCopy for WarmEntry {
    #[cfg(feature = "pma-assert")]
    fn assert_in_pma(&self, pma: &Pma) {
        if self.0.is_null() {
            return;
        }
        let mut cursor = *self;
        loop {
            unsafe {
                assert!(
                    pma.contains_ptr(cursor.0 as *const u8),
                    "WarmEntry node should be in PMA"
                );
                (*cursor.0).batteries.assert_in_pma(pma);
                (*cursor.0).path.assert_in_pma(pma);
                if (*cursor.0).next.0.is_null() {
                    break;
                }
                cursor = (*cursor.0).next;
            }
        }
    }

    #[cfg(not(feature = "pma-assert"))]
    #[inline(always)]
    fn assert_in_pma(&self, _pma: &Pma) {}

    unsafe fn copy_to_pma(&mut self, stack: &NockStack, pma: &mut Pma) {
        if self.0.is_null() {
            return;
        }
        let trace = std::env::var_os("NOCK_PMA_TRACE_WARM_ENTRY").is_some();
        let mut ptr: *mut WarmEntry = self;
        loop {
            if trace {
                debug!("pma-copy: warm entry start: node_ptr={:p}", (*ptr).0);
            }
            if pma.contains_ptr((*ptr).0 as *const u8) {
                break;
            }
            // Copy batteries and path to PMA
            (*(*ptr).0).batteries.copy_to_pma(stack, pma);
            if trace {
                debug!("pma-copy: warm entry batteries done");
            }
            (*(*ptr).0).path.copy_to_pma(stack, pma);
            if trace {
                debug!("pma-copy: warm entry path done");
            }
            // Allocate new WarmEntryMem in PMA and copy
            let dest_mem: *mut WarmEntryMem = pma.alloc_struct(1);
            copy_nonoverlapping((*ptr).0, dest_mem, 1);
            // Update pointer to point to PMA copy
            *ptr = WarmEntry(dest_mem);
            if trace {
                debug!("pma-copy: warm entry done: dest_ptr={:p}", dest_mem);
            }
            // Move to next node
            ptr = &mut (*dest_mem).next;
            if (*dest_mem).next.0.is_null() {
                break;
            }
        }
    }
}

impl PmaCopyFrom for WarmEntry {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        if self.0.is_null() {
            return;
        }
        let mut ptr: *mut WarmEntry = self;
        loop {
            let src_mem = (*ptr).0;
            let dest_mem: *mut WarmEntryMem = to_pma.alloc_struct(1);
            copy_nonoverlapping(src_mem, dest_mem, 1);
            *ptr = WarmEntry(dest_mem);
            (*dest_mem).batteries.copy_from_pma(from_pma, to_pma);
            (*dest_mem).path.copy_from_pma(from_pma, to_pma);
            ptr = &mut (*dest_mem).next;
            if (*dest_mem).next.0.is_null() {
                break;
            }
        }
    }
}

impl Iterator for WarmEntry {
    type Item = (Noun, Batteries, Jet, bool);
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_null() {
            return None;
        }
        unsafe {
            let res = (
                (*(self.0)).path,
                (*(self.0)).batteries,
                (*(self.0)).jet,
                (*(self.0)).test,
            );
            *self = (*(self.0)).next;
            Some(res)
        }
    }
}

#[derive(Default)]
pub enum JetLookupResult {
    Run {
        jet: Jet,
        path: Noun,
    },
    Test {
        jet: Jet,
        path: Noun,
    },
    #[default]
    NoJet,
}

impl Iterator for JetLookupResult {
    type Item = (Jet, Noun, bool);
    fn next(&mut self) -> Option<Self::Item> {
        match std::mem::take(self) {
            JetLookupResult::Run { jet, path } => Some((jet, path, false)),
            JetLookupResult::Test { jet, path } => Some((jet, path, true)),
            JetLookupResult::NoJet => None,
        }
    }
}

#[inline(always)]
fn transparent_hint_tag(hint: Noun, space: &NounSpace) -> Option<u64> {
    let tag = if let Ok(cell) = hint.in_space(space).as_cell() {
        cell.head().noun()
    } else {
        hint
    };
    tag.as_direct().ok().map(|direct| direct.data())
}

#[inline(always)]
fn is_transparent_hint_tag(tag: u64) -> bool {
    matches!(
        tag,
        tas!(b"hand") | tas!(b"hunk") | tas!(b"lose") | tas!(b"mean") | tas!(b"spot")
    )
}

#[inline(always)]
fn direct_atom_is(noun: Noun, value: u64, space: &NounSpace) -> bool {
    noun.in_space(space)
        .as_atom()
        .ok()
        .and_then(|atom| atom.as_u64().ok())
        == Some(value)
}

#[inline(always)]
fn transparent_hint_body(formula: Noun, space: &NounSpace) -> Option<Noun> {
    let cell = formula.in_space(space).as_cell().ok()?;
    if !direct_atom_is(cell.head().noun(), 11, space) {
        return None;
    }
    let args = cell.tail().as_cell().ok()?;
    let tag = transparent_hint_tag(args.head().noun(), space)?;
    if is_transparent_hint_tag(tag) {
        Some(args.tail().noun())
    } else {
        None
    }
}

fn normalize_unary_formula(
    stack: &mut NockStack,
    op: Noun,
    body: Noun,
    original: Noun,
    changed: bool,
    space: &NounSpace,
) -> (Noun, bool) {
    let (body, body_changed) = normalize_transparent_hints(stack, body, space);
    if changed || body_changed {
        (T(stack, &[op, body]), true)
    } else {
        (original, false)
    }
}
fn normalize_pair_formula(
    stack: &mut NockStack,
    op: Noun,
    args: Noun,
    original: Noun,
    changed: bool,
    space: &NounSpace,
) -> (Noun, bool) {
    let Ok(args_cell) = args.in_space(space).as_cell() else {
        return (original, changed);
    };
    let left = args_cell.head().noun();
    let right = args_cell.tail().noun();
    let (left, left_changed) = normalize_transparent_hints(stack, left, space);
    let (right, right_changed) = normalize_transparent_hints(stack, right, space);
    if changed || left_changed || right_changed {
        (T(stack, &[op, left, right]), true)
    } else {
        (original, false)
    }
}

fn normalize_triple_formula(
    stack: &mut NockStack,
    op: Noun,
    args: Noun,
    original: Noun,
    changed: bool,
    space: &NounSpace,
) -> (Noun, bool) {
    let Ok(args_cell) = args.in_space(space).as_cell() else {
        return (original, changed);
    };
    let left = args_cell.head().noun();
    let Ok(rest_cell) = args_cell.tail().as_cell() else {
        return (original, changed);
    };
    let middle = rest_cell.head().noun();
    let right = rest_cell.tail().noun();
    let (left, left_changed) = normalize_transparent_hints(stack, left, space);
    let (middle, middle_changed) = normalize_transparent_hints(stack, middle, space);
    let (right, right_changed) = normalize_transparent_hints(stack, right, space);
    if changed || left_changed || middle_changed || right_changed {
        (T(stack, &[op, left, middle, right]), true)
    } else {
        (original, false)
    }
}

fn normalize_axis_formula(
    stack: &mut NockStack,
    op: Noun,
    args: Noun,
    original: Noun,
    changed: bool,
    space: &NounSpace,
) -> (Noun, bool) {
    let Ok(args_cell) = args.in_space(space).as_cell() else {
        return (original, changed);
    };
    let axis = args_cell.head().noun();
    let formula = args_cell.tail().noun();
    let (formula, formula_changed) = normalize_transparent_hints(stack, formula, space);
    if changed || formula_changed {
        (T(stack, &[op, axis, formula]), true)
    } else {
        (original, false)
    }
}

fn normalize_edit_formula(
    stack: &mut NockStack,
    op: Noun,
    args: Noun,
    original: Noun,
    changed: bool,
    space: &NounSpace,
) -> (Noun, bool) {
    let Ok(args_cell) = args.in_space(space).as_cell() else {
        return (original, changed);
    };
    let Ok(edit_cell) = args_cell.head().as_cell() else {
        return (original, changed);
    };
    let axis = edit_cell.head().noun();
    let replacement = edit_cell.tail().noun();
    let subject = args_cell.tail().noun();
    let (replacement, replacement_changed) = normalize_transparent_hints(stack, replacement, space);
    let (subject, subject_changed) = normalize_transparent_hints(stack, subject, space);
    if changed || replacement_changed || subject_changed {
        let edit = T(stack, &[axis, replacement]);
        (T(stack, &[op, edit, subject]), true)
    } else {
        (original, false)
    }
}

fn normalize_hint_formula(
    stack: &mut NockStack,
    op: Noun,
    args: Noun,
    original: Noun,
    changed: bool,
    space: &NounSpace,
) -> (Noun, bool) {
    let Ok(args_cell) = args.in_space(space).as_cell() else {
        return (original, changed);
    };
    let hint = args_cell.head().noun();
    let body = args_cell.tail().noun();
    let (body, body_changed) = normalize_transparent_hints(stack, body, space);
    let (hint, hint_changed) = if let Ok(hint_cell) = hint.in_space(space).as_cell() {
        let tag = hint_cell.head().noun();
        let clue = hint_cell.tail().noun();
        let (clue, clue_changed) = normalize_transparent_hints(stack, clue, space);
        if clue_changed {
            (T(stack, &[tag, clue]), true)
        } else {
            (hint, false)
        }
    } else {
        (hint, false)
    };
    if changed || hint_changed || body_changed {
        (T(stack, &[op, hint, body]), true)
    } else {
        (original, false)
    }
}

fn normalize_transparent_hints(
    stack: &mut NockStack,
    formula: Noun,
    space: &NounSpace,
) -> (Noun, bool) {
    let mut formula = formula;
    let mut changed = false;
    while let Some(body) = transparent_hint_body(formula, space) {
        formula = body;
        changed = true;
    }

    let Ok(cell) = formula.in_space(space).as_cell() else {
        return (formula, changed);
    };
    let head = cell.head().noun();
    let tail = cell.tail().noun();

    if head.in_space(space).as_cell().is_ok() {
        let (head, head_changed) = normalize_transparent_hints(stack, head, space);
        let (tail, tail_changed) = normalize_transparent_hints(stack, tail, space);
        if changed || head_changed || tail_changed {
            return (T(stack, &[head, tail]), true);
        }
        return (formula, false);
    }

    let Some(op) = head
        .in_space(space)
        .as_atom()
        .ok()
        .and_then(|atom| atom.as_u64().ok())
    else {
        return (formula, changed);
    };

    match op {
        0 | 1 => (formula, changed),
        2 | 5 | 7 | 8 => normalize_pair_formula(stack, head, tail, formula, changed, space),
        3 | 4 => normalize_unary_formula(stack, head, tail, formula, changed, space),
        6 => normalize_triple_formula(stack, head, tail, formula, changed, space),
        9 => normalize_axis_formula(stack, head, tail, formula, changed, space),
        10 => normalize_edit_formula(stack, head, tail, formula, changed, space),
        11 => normalize_hint_formula(stack, head, tail, formula, changed, space),
        _ => (formula, changed),
    }
}

#[inline(always)]
fn raw_noun_eq(left: Noun, right: Noun) -> bool {
    unsafe { left.raw_equals(&right) }
}

fn match_warm_entries(
    stack: &mut NockStack,
    warm_it: WarmEntry,
    subject: Noun,
    space: &NounSpace,
    dispatch: JetDispatchMode,
) -> Option<(Noun, Batteries, Jet, bool)> {
    for (path, batteries, jet, test) in warm_it {
        if batteries.matches(stack, subject, space, dispatch) {
            return Some((path, batteries, jet, test));
        }
    }
    None
}

#[inline]
fn jet_result(jet: Jet, path: Noun, test: bool) -> JetLookupResult {
    if test {
        JetLookupResult::Test { jet, path }
    } else {
        JetLookupResult::Run { jet, path }
    }
}

impl Warm {
    #[allow(clippy::new_without_default)]
    pub fn new(stack: &mut NockStack) -> Self {
        Warm(Hamt::new(stack))
    }

    fn insert(&mut self, stack: &mut NockStack, formula: &mut Noun, entry: WarmRegistration) {
        let current_warm_entry = self.0.lookup(stack, formula).unwrap_or(WARM_ENTRY_NIL);
        unsafe {
            let warm_entry_mem_ptr: *mut WarmEntryMem = stack.struct_alloc(1);
            *warm_entry_mem_ptr = WarmEntryMem {
                batteries: entry.batteries,
                jet: entry.jet,
                path: entry.path,
                test: entry.test,
                next: current_warm_entry,
            };
            self.0 = self.0.insert(stack, formula, WarmEntry(warm_entry_mem_ptr));
        }
    }

    fn insert_with_transparent_hints(
        &mut self,
        stack: &mut NockStack,
        formula: &mut Noun,
        entry: WarmRegistration,
        space: &NounSpace,
    ) {
        let (normalized_formula, normalized) = normalize_transparent_hints(stack, *formula, space);
        self.insert(stack, formula, entry);
        if normalized && !raw_noun_eq(*formula, normalized_formula) {
            let mut formula = normalized_formula;
            self.insert(stack, &mut formula, entry);
        }
    }
    pub fn init(
        stack: &mut NockStack,
        cold: &mut Cold,
        hot: &Hot,
        test_jets: &Hamt<()>,
        dispatch: JetDispatchMode,
    ) -> Self {
        let mut warm = Self::new(stack);
        let space = stack.fast_noun_space();
        for (mut path, axis, jet) in *hot {
            let test_path = test_jets.lookup(stack, &mut path).is_some();
            let batteries_list = cold.find(stack, &mut path);
            for batteries in batteries_list {
                let mut batteries_tmp = batteries;
                let (battery, _parent_axis) = batteries_tmp
                    .next()
                    .expect("IMPOSSIBLE: empty battery entry in cold state");
                if let Ok(mut formula) = unsafe { (*battery).slot_atom(axis, &space) } {
                    let entry = WarmRegistration {
                        path,
                        batteries,
                        jet,
                        test: test_path,
                    };
                    match dispatch {
                        JetDispatchMode::Exact => warm.insert(stack, &mut formula, entry),
                        JetDispatchMode::HintBlind => {
                            warm.insert_with_transparent_hints(stack, &mut formula, entry, &space)
                        }
                    }
                } else {
                    //  XX: need NockStack allocated string interpolation
                    // eprintln!("Bad axis {} into formula {:?}", axis, battery);
                    continue;
                }
            }
        }
        warm
    }

    /// Walk through the linked list of WarmEntry objects and do a partial check
    /// against the subject using Batteries (walk to root of parent batteries).
    /// If there's a match, then we've found a valid jet.
    pub fn find_jet(
        &mut self,
        stack: &mut NockStack,
        s: &mut Noun,
        f: &mut Noun,
        dispatch: JetDispatchMode,
    ) -> JetLookupResult {
        let space = stack.fast_noun_space();
        if let Some(warm_it) = self.0.lookup(stack, f) {
            if let Some((path, _batteries, jet, test)) =
                match_warm_entries(stack, warm_it, *s, &space, dispatch)
            {
                return jet_result(jet, path, test);
            }
        }

        if dispatch == JetDispatchMode::HintBlind {
            let (normalized_formula, normalized) = normalize_transparent_hints(stack, *f, &space);
            if normalized && !raw_noun_eq(*f, normalized_formula) {
                let mut formula = normalized_formula;
                if let Some(warm_it) = self.0.lookup(stack, &mut formula) {
                    if let Some((path, batteries, jet, test)) =
                        match_warm_entries(stack, warm_it, *s, &space, dispatch)
                    {
                        self.maybe_insert_alias(
                            stack,
                            f,
                            &space,
                            WarmRegistration {
                                path,
                                batteries,
                                jet,
                                test,
                            },
                        );
                        return jet_result(jet, path, test);
                    }
                }
            }
        }

        JetLookupResult::NoJet
    }

    /// Memoize a hint-blind fallback hit under the raw query formula so the
    /// next dispatch of the same formula hits the first HAMT lookup instead
    /// of re-normalizing. Inserting mid-interpret is sound for the same
    /// reason `%memo` cache inserts are: every Ret/Done work item preserves
    /// `context.warm` outward frame-by-frame, and `exit()` restores the
    /// entry snapshot when a computation bails, so alias nodes never outlive
    /// a discarded region. Aliases live only in the warm table — never in
    /// cold state, which honk serializes via `--dump-wrapper-assets`.
    ///
    /// Skip formulas whose memory sits in the current frame: the HAMT would
    /// retain the formula as a key and `preserve` would deep-copy the whole
    /// tree once per frame pop, costing more than the normalize-per-miss it
    /// avoids. (Honk's hot case — arm formulas of cores interned at the eval
    /// stack root — is always senior to the dispatching frame.)
    fn maybe_insert_alias(
        &mut self,
        stack: &mut NockStack,
        formula: &mut Noun,
        space: &NounSpace,
        entry: WarmRegistration,
    ) {
        let Ok(allocated) = (*formula).as_allocated() else {
            // Direct atoms can't carry hint wrappers; nothing to alias.
            return;
        };
        let ptr = unsafe { allocated.to_raw_pointer(space) };
        if unsafe { stack.is_in_frame(ptr) } {
            return;
        }
        self.insert(stack, formula, entry);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::interpreter::Context;
    use crate::jets::cold::{Batteries, BatteriesMem, NO_BATTERIES};
    use crate::jets::JetErr;
    use crate::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use crate::noun::{AllocLocation, NounAllocator, NounSpace, D, T};
    use crate::pma::{test_pma_path, Pma, PmaCopy};

    const DEFAULT_STACK_SIZE: usize = NOCK_STACK_SIZE_TINY;

    fn make_test_stack(size: usize) -> NockStack {
        NockStack::new(size, 0)
    }

    /// Dummy jet function for testing
    fn dummy_jet(_ctx: &mut Context, _subj: Noun) -> Result<Noun, JetErr> {
        Ok(D(42))
    }

    /// Another dummy jet function to differentiate entries
    fn dummy_jet_2(_ctx: &mut Context, _subj: Noun) -> Result<Noun, JetErr> {
        Ok(D(99))
    }

    /// Create a simple Batteries for testing (single entry with given battery value)
    fn make_simple_batteries(stack: &mut NockStack, battery_value: u64) -> Batteries {
        let batteries_mem: *mut BatteriesMem = unsafe { stack.alloc_struct(1) };
        unsafe {
            batteries_mem.write(BatteriesMem {
                battery: D(battery_value),
                parent_axis: D(0).as_atom().expect("0 is a valid atom"),
                parent_batteries: NO_BATTERIES,
            });
        }
        Batteries::new(batteries_mem)
    }

    /// Create a WarmEntry linked list for testing
    fn make_warm_entry(stack: &mut NockStack, entries: &[(u64, Jet, u64, bool)]) -> WarmEntry {
        let mut warm_entry = WARM_ENTRY_NIL;
        for &(battery_value, jet, path_value, test) in entries.iter().rev() {
            let batteries = make_simple_batteries(stack, battery_value);
            let warm_entry_mem: *mut WarmEntryMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                warm_entry_mem.write(WarmEntryMem {
                    batteries,
                    jet,
                    path: D(path_value),
                    test,
                    next: warm_entry,
                });
            }
            warm_entry = WarmEntry(warm_entry_mem);
        }
        warm_entry
    }

    fn hinted_formula(stack: &mut NockStack, tag: u64, clue: Noun, body: Noun) -> Noun {
        let hint = T(stack, &[D(tag), clue]);
        T(stack, &[D(11), hint, body])
    }

    #[test]
    fn test_warm_lookup_strips_transparent_hints() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let batteries = make_simple_batteries(&mut stack, 10);
        let mut formula = D(100);
        warm.insert(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
        );

        let mean = hinted_formula(&mut stack, tas!(b"mean"), D(0), D(100));
        let mut query = hinted_formula(&mut stack, tas!(b"spot"), D(1), mean);
        let mut subject = D(10);

        match warm.find_jet(
            &mut stack,
            &mut subject,
            &mut query,
            JetDispatchMode::HintBlind,
        ) {
            JetLookupResult::Run { jet, path } => {
                assert!(std::ptr::fn_addr_eq(jet, dummy_jet as Jet));
                assert!(unsafe { path.raw_equals(&D(1000)) });
            }
            JetLookupResult::Test { .. } => panic!("expected run-mode jet lookup"),
            JetLookupResult::NoJet => panic!("expected transparent hint wrapper to match warm jet"),
        }
    }

    #[test]
    fn test_warm_init_registers_stripped_hint_key() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let space = stack.fast_noun_space();
        let batteries = make_simple_batteries(&mut stack, 10);
        let mut formula = hinted_formula(&mut stack, tas!(b"spot"), D(1), D(100));
        warm.insert_with_transparent_hints(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
            &space,
        );

        let mut query = D(100);
        let mut subject = D(10);
        match warm.find_jet(
            &mut stack,
            &mut subject,
            &mut query,
            JetDispatchMode::HintBlind,
        ) {
            JetLookupResult::Run { jet, path } => {
                assert!(std::ptr::fn_addr_eq(jet, dummy_jet as Jet));
                assert!(unsafe { path.raw_equals(&D(1000)) });
            }
            JetLookupResult::Test { .. } => panic!("expected run-mode jet lookup"),
            JetLookupResult::NoJet => panic!("expected stripped hint key to match warm jet"),
        }
    }

    #[test]
    fn test_warm_lookup_strips_nested_transparent_hints() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let batteries = make_simple_batteries(&mut stack, 10);
        let slot_three = T(&mut stack, &[D(0), D(3)]);
        let slot_four = T(&mut stack, &[D(0), D(4)]);
        let mut formula = T(&mut stack, &[D(2), slot_three, slot_four]);
        warm.insert(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
        );

        let hinted_slot_three = hinted_formula(&mut stack, tas!(b"spot"), D(1), slot_three);
        let mut query = T(&mut stack, &[D(2), hinted_slot_three, slot_four]);
        let mut subject = D(10);
        match warm.find_jet(
            &mut stack,
            &mut subject,
            &mut query,
            JetDispatchMode::HintBlind,
        ) {
            JetLookupResult::Run { jet, path } => {
                assert!(std::ptr::fn_addr_eq(jet, dummy_jet as Jet));
                assert!(unsafe { path.raw_equals(&D(1000)) });
            }
            JetLookupResult::Test { .. } => panic!("expected run-mode jet lookup"),
            JetLookupResult::NoJet => panic!("expected nested transparent hint to match warm jet"),
        }
    }

    #[test]
    fn test_warm_lookup_preserves_constant_hint_data() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let space = stack.fast_noun_space();
        let batteries = make_simple_batteries(&mut stack, 10);
        let hinted_data = hinted_formula(&mut stack, tas!(b"spot"), D(1), D(100));
        let mut formula = T(&mut stack, &[D(1), hinted_data]);
        warm.insert_with_transparent_hints(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
            &space,
        );

        let mut query = T(&mut stack, &[D(1), D(100)]);
        let mut subject = D(10);
        assert!(matches!(
            warm.find_jet(
            &mut stack,
            &mut subject,
            &mut query,
            JetDispatchMode::HintBlind,
        ),
            JetLookupResult::NoJet
        ));
    }

    #[test]
    fn test_warm_lookup_keeps_effectful_hints() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let batteries = make_simple_batteries(&mut stack, 10);
        let mut formula = D(100);
        warm.insert(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
        );

        let mut query = hinted_formula(&mut stack, tas!(b"memo"), D(0), D(100));
        let mut subject = D(10);
        assert!(matches!(
            warm.find_jet(
            &mut stack,
            &mut subject,
            &mut query,
            JetDispatchMode::HintBlind,
        ),
            JetLookupResult::NoJet
        ));
    }

    #[test]
    fn test_warm_exact_lookup_ignores_hinted_query() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let batteries = make_simple_batteries(&mut stack, 10);
        let mut formula = D(100);
        warm.insert(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
        );

        let mut query = hinted_formula(&mut stack, tas!(b"spot"), D(1), D(100));
        let mut subject = D(10);

        // Exact dispatch never strips hints: the hinted query misses.
        assert!(matches!(
            warm.find_jet(
                &mut stack,
                &mut subject,
                &mut query,
                JetDispatchMode::Exact,
            ),
            JetLookupResult::NoJet
        ));

        // The same query under hint-blind dispatch hits.
        assert!(matches!(
            warm.find_jet(
                &mut stack,
                &mut subject,
                &mut query,
                JetDispatchMode::HintBlind,
            ),
            JetLookupResult::Run { .. }
        ));
    }

    #[test]
    fn test_warm_hint_blind_matches_hint_divergent_battery() {
        // End-to-end honk scenario at the warm level: both the arm formula
        // and the registered root battery carry %spot hints the querying
        // core lacks. HintBlind dispatch must match; Exact must not.
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let space = stack.fast_noun_space();

        let hinted_arm = hinted_formula(&mut stack, tas!(b"spot"), D(1), D(100));
        let mut registered_root = T(&mut stack, &[hinted_arm, D(7)]);

        let batteries_mem: *mut BatteriesMem = unsafe { stack.alloc_struct(1) };
        unsafe {
            batteries_mem.write(BatteriesMem {
                battery: registered_root,
                parent_axis: D(0).as_atom().expect("0 is a valid atom"),
                parent_batteries: NO_BATTERIES,
            });
        }
        let batteries = Batteries::new(batteries_mem);

        warm.insert_with_transparent_hints(
            &mut stack,
            &mut registered_root,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
            &space,
        );

        // The querying core: same code, no %spot wrapper.
        let mut subject = T(&mut stack, &[D(100), D(7)]);
        // The query formula: differently-hinted variant of the registered key.
        let mean_arm = hinted_formula(&mut stack, tas!(b"mean"), D(2), D(100));
        let mut query = T(&mut stack, &[mean_arm, D(7)]);

        assert!(matches!(
            warm.find_jet(
                &mut stack,
                &mut subject,
                &mut query,
                JetDispatchMode::Exact,
            ),
            JetLookupResult::NoJet
        ));
        assert!(matches!(
            warm.find_jet(
                &mut stack,
                &mut subject,
                &mut query,
                JetDispatchMode::HintBlind,
            ),
            JetLookupResult::Run { .. }
        ));
    }

    #[test]
    fn test_warm_hint_blind_fallback_inserts_alias_for_senior_formula() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let batteries = make_simple_batteries(&mut stack, 10);
        let mut formula = D(100);
        warm.insert(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
        );

        // Allocate the hinted query in the root frame (senior), then
        // dispatch from a pushed frame, mirroring how interpret calls
        // find_jet on arm formulas of long-lived cores.
        let mut query = hinted_formula(&mut stack, tas!(b"spot"), D(1), D(100));
        let mut subject = D(10);

        stack.frame_push(0);
        assert!(matches!(
            warm.find_jet(
                &mut stack,
                &mut subject,
                &mut query,
                JetDispatchMode::HintBlind,
            ),
            JetLookupResult::Run { .. }
        ));
        // The raw (hinted) key is now directly present in the warm table.
        assert!(warm.0.lookup(&mut stack, &mut query).is_some());

        // The alias survives frame pop via the same preserve discipline
        // interpret applies to context.warm at every Ret/Done.
        unsafe {
            stack.preserve(&mut warm);
            stack.frame_pop();
        }
        assert!(warm.0.lookup(&mut stack, &mut query).is_some());
        assert!(matches!(
            warm.find_jet(
                &mut stack,
                &mut subject,
                &mut query,
                JetDispatchMode::HintBlind,
            ),
            JetLookupResult::Run { .. }
        ));
    }

    #[test]
    fn test_warm_hint_blind_fallback_skips_alias_for_junior_formula() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut warm = Warm::new(&mut stack);
        let batteries = make_simple_batteries(&mut stack, 10);
        let mut formula = D(100);
        warm.insert(
            &mut stack,
            &mut formula,
            WarmRegistration {
                path: D(1000),
                batteries,
                jet: dummy_jet,
                test: false,
            },
        );

        // Allocate the hinted query inside the dispatching frame: the
        // fallback must still hit, but no alias may be inserted (the HAMT
        // would retain a frame-local key).
        stack.frame_push(0);
        let mut query = hinted_formula(&mut stack, tas!(b"spot"), D(1), D(100));
        let mut subject = D(10);
        assert!(matches!(
            warm.find_jet(
                &mut stack,
                &mut subject,
                &mut query,
                JetDispatchMode::HintBlind,
            ),
            JetLookupResult::Run { .. }
        ));
        assert!(warm.0.lookup(&mut stack, &mut query).is_none());
        unsafe { stack.frame_pop() };
    }

    /// Helper to verify a noun is not stack-allocated (is in offset form)
    fn verify_noun_not_stack_allocated(noun: Noun, space: &NounSpace, context: &str) {
        if noun.is_direct() {
            return;
        }
        let location = noun.in_space(space).allocated_location();
        assert!(
            !matches!(location, Some(AllocLocation::Stack)),
            "{} should be in offset form after evacuation",
            context
        );
        if let Ok(cell) = noun.in_space(space).as_cell() {
            verify_noun_not_stack_allocated(cell.head().noun(), space, context);
            verify_noun_not_stack_allocated(cell.tail().noun(), space, context);
        }
    }

    /// Verifies WarmEntry can be evacuated to PMA and remains functional.
    ///
    /// This test exercises:
    /// - Creating a WarmEntry linked list with multiple entries
    /// - Evacuating the WarmEntry to PMA via copy_to_pma
    /// - Verifying all entries are still accessible after evacuation
    /// - Verifying all nouns are in offset form (not stack-allocated)
    /// - Verifying the WarmEntry passes assert_in_pma
    ///
    /// Note: copy_to_pma sets forwarding pointers in the source nouns, which corrupts
    /// them for normal use. We use expected values for comparison.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_evacuate_warm_entry_round_trip() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut pma =
            Pma::new(100000, test_pma_path("warm_entry")).expect("Failed to create test PMA");
        let space = NounSpace::new(&stack, &pma);

        // Create WarmEntry linked list with two entries
        // (battery_value, jet, path_value, test)
        let entries: Vec<(u64, Jet, u64, bool)> =
            vec![(10, dummy_jet, 100, false), (20, dummy_jet_2, 200, true)];
        let mut warm_entry = make_warm_entry(&mut stack, &entries);

        // Evacuate WarmEntry to PMA
        unsafe {
            warm_entry.copy_to_pma(&stack, &mut pma);
        }

        // Iterate over evacuated warm_entry and verify values
        let mut expected_iter = entries.iter();
        for (path, batteries, jet, test) in warm_entry {
            let (expected_battery, expected_jet, expected_path, expected_test) = expected_iter
                .next()
                .expect("WarmEntry has more entries than expected");

            // Verify path
            assert_eq!(
                unsafe { path.as_raw() },
                *expected_path,
                "Path should match"
            );

            // Verify jet function pointer
            assert!(
                std::ptr::fn_addr_eq(jet, *expected_jet),
                "Jet function pointer should match"
            );

            // Verify test flag
            assert_eq!(test, *expected_test, "Test flag should match");

            // Verify batteries (first entry only for simplicity)
            let mut batteries_iter = batteries.into_iter();
            let (battery_ptr, parent_axis) = batteries_iter
                .next()
                .expect("Batteries should have at least one entry");
            let battery = unsafe { *battery_ptr };
            assert_eq!(
                unsafe { battery.as_raw() },
                *expected_battery,
                "Battery value should match"
            );
            assert_eq!(
                parent_axis
                    .in_space(&space)
                    .as_u64()
                    .expect("parent axis should fit in u64"),
                0,
                "Parent axis should be 0"
            );

            // Verify nouns are in offset form
            verify_noun_not_stack_allocated(path, &space, "WarmEntry path");
            verify_noun_not_stack_allocated(battery, &space, "WarmEntry battery");
        }

        assert!(
            expected_iter.next().is_none(),
            "WarmEntry has fewer entries than expected"
        );

        // Verify the WarmEntry passes assert_in_pma
        warm_entry.assert_in_pma(&pma);
    }

    /// Verifies Warm state can be evacuated to PMA and remains functional.
    ///
    /// This test exercises:
    /// - Creating a Warm HAMT with multiple formula->WarmEntry mappings
    /// - Evacuating the Warm to PMA via copy_to_pma
    /// - Verifying all entries are still accessible via lookup after evacuation
    /// - Verifying all nouns are in offset form (not stack-allocated)
    /// - Verifying the Warm passes assert_in_pma
    ///
    /// Note: copy_to_pma sets forwarding pointers in the source nouns, which corrupts
    /// them for normal use. We use expected values for comparison.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_evacuate_warm_round_trip() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut pma = Pma::new(100000, test_pma_path("warm")).expect("Failed to create test PMA");
        let space = NounSpace::new(&stack, &pma);

        // Create a Warm and insert some entries
        let mut warm = Warm::new(&mut stack);

        // Insert entry 1: formula D(100) -> (battery=10, jet=dummy_jet, path=1000, test=false)
        let batteries1 = make_simple_batteries(&mut stack, 10);
        let mut formula1 = D(100);
        warm.insert(
            &mut stack,
            &mut formula1,
            WarmRegistration {
                path: D(1000),
                batteries: batteries1,
                jet: dummy_jet,
                test: false,
            },
        );

        // Insert entry 2: formula D(200) -> (battery=20, jet=dummy_jet_2, path=2000, test=true)
        let batteries2 = make_simple_batteries(&mut stack, 20);
        let mut formula2 = D(200);
        warm.insert(
            &mut stack,
            &mut formula2,
            WarmRegistration {
                path: D(2000),
                batteries: batteries2,
                jet: dummy_jet_2,
                test: true,
            },
        );

        // Insert entry 3: same formula as entry 1, different jet (creates linked list)
        let batteries3 = make_simple_batteries(&mut stack, 30);
        let mut formula3 = D(100);
        warm.insert(
            &mut stack,
            &mut formula3,
            WarmRegistration {
                path: D(3000),
                batteries: batteries3,
                jet: dummy_jet_2,
                test: true,
            },
        );

        // Expected values for verification
        // formula D(100) should have two entries: (30, dummy_jet_2, 3000, true) -> (10, dummy_jet, 1000, false)
        // formula D(200) should have one entry: (20, dummy_jet_2, 2000, true)
        let expected_formula_100: Vec<(u64, Jet, u64, bool)> =
            vec![(30, dummy_jet_2, 3000, true), (10, dummy_jet, 1000, false)];
        let expected_formula_200: Vec<(u64, Jet, u64, bool)> = vec![(20, dummy_jet_2, 2000, true)];

        // Evacuate Warm to PMA
        unsafe {
            warm.copy_to_pma(&stack, &mut pma);
        }

        // Verify lookup for formula D(100)
        let mut lookup_key1 = D(100);
        let warm_entry1 = warm
            .0
            .lookup(&mut stack, &mut lookup_key1)
            .expect("Should find entry for formula D(100)");

        let mut expected_iter1 = expected_formula_100.iter();
        for (path, batteries, jet, test) in warm_entry1 {
            let (expected_battery, expected_jet, expected_path, expected_test) = expected_iter1
                .next()
                .expect("WarmEntry has more entries than expected");

            assert_eq!(
                unsafe { path.as_raw() },
                *expected_path,
                "Path should match"
            );
            assert!(std::ptr::fn_addr_eq(jet, *expected_jet), "Jet should match");
            assert_eq!(test, *expected_test, "Test flag should match");

            // Verify battery
            let mut batteries_iter = batteries.into_iter();
            let (battery_ptr, _) = batteries_iter.next().expect("Batteries should have entry");
            let battery = unsafe { *battery_ptr };
            assert_eq!(
                unsafe { battery.as_raw() },
                *expected_battery,
                "Battery should match"
            );

            // Verify nouns are in offset form
            verify_noun_not_stack_allocated(path, &space, "Warm path");
            verify_noun_not_stack_allocated(battery, &space, "Warm battery");
        }
        assert!(
            expected_iter1.next().is_none(),
            "Missing entries for formula D(100)"
        );

        // Verify lookup for formula D(200)
        let mut lookup_key2 = D(200);
        let warm_entry2 = warm
            .0
            .lookup(&mut stack, &mut lookup_key2)
            .expect("Should find entry for formula D(200)");

        let mut expected_iter2 = expected_formula_200.iter();
        for (path, batteries, jet, test) in warm_entry2 {
            let (expected_battery, expected_jet, expected_path, expected_test) = expected_iter2
                .next()
                .expect("WarmEntry has more entries than expected");

            assert_eq!(
                unsafe { path.as_raw() },
                *expected_path,
                "Path should match"
            );
            assert!(std::ptr::fn_addr_eq(jet, *expected_jet), "Jet should match");
            assert_eq!(test, *expected_test, "Test flag should match");

            // Verify battery
            let mut batteries_iter = batteries.into_iter();
            let (battery_ptr, _) = batteries_iter.next().expect("Batteries should have entry");
            let battery = unsafe { *battery_ptr };
            assert_eq!(
                unsafe { battery.as_raw() },
                *expected_battery,
                "Battery should match"
            );

            // Verify nouns are in offset form
            verify_noun_not_stack_allocated(path, &space, "Warm path");
            verify_noun_not_stack_allocated(battery, &space, "Warm battery");
        }
        assert!(
            expected_iter2.next().is_none(),
            "Missing entries for formula D(200)"
        );

        // Verify non-existent lookup returns None
        let mut lookup_key3 = D(999);
        assert!(
            warm.0.lookup(&mut stack, &mut lookup_key3).is_none(),
            "Lookup for non-existent formula should return None"
        );

        // Verify the Warm passes assert_in_pma
        warm.assert_in_pma(&pma);
    }
}
