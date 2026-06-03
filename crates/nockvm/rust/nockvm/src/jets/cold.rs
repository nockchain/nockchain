use std::ptr::{copy_nonoverlapping, null_mut};

use intmap::IntMap;
use tracing::debug;

use crate::hamt::Hamt;
use crate::mem::{self, NockStack, Preserve};
use crate::noun::{
    self, Atom, DirectAtom, IndirectAtom, Noun, NounAllocator, NounSpace, Slots, D, T,
};
use crate::offset::PmaOffsetWords;
use crate::pma::{Pma, PmaCopy, PmaCopyFrom};
use crate::unifying_equality::unifying_equality;

pub enum Error {
    NoParent,
    BadNock,
}

impl From<noun::Error> for Error {
    fn from(_: noun::Error) -> Self {
        Error::BadNock
    }
}

pub type Result = std::result::Result<bool, Error>;

// Batteries is a core hierarchy (e.g. a path of parent batteries to a root)
#[derive(Copy, Clone)]
pub struct Batteries(*mut BatteriesMem);

pub(crate) const NO_BATTERIES: Batteries = Batteries(null_mut());

#[derive(Copy, Clone)]
pub(crate) struct BatteriesMem {
    pub(crate) battery: Noun,
    pub(crate) parent_axis: Atom,
    pub(crate) parent_batteries: Batteries,
}

impl Preserve for Batteries {
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        if self.0.is_null() {
            return;
        };
        let mut cursor = *self;
        loop {
            stack.assert_struct_is_in(cursor.0, 1);
            (*cursor.0).battery.assert_in_stack(stack);
            (*cursor.0).parent_axis.assert_in_stack(stack);
            if (*cursor.0).parent_batteries.0.is_null() {
                break;
            };
            cursor = (*cursor.0).parent_batteries;
        }
    }
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        if self.0.is_null() {
            return;
        };
        let mut ptr: *mut *mut BatteriesMem = &mut self.0;
        loop {
            if stack.is_in_frame(*ptr) {
                (**ptr).battery.preserve(stack);
                (**ptr).parent_axis.preserve(stack);
                let dest_mem: *mut BatteriesMem = stack.struct_alloc_in_previous_frame(1);
                copy_nonoverlapping(*ptr, dest_mem, 1);
                *ptr = dest_mem;
                ptr = &mut ((**ptr).parent_batteries.0);
                if (*dest_mem).parent_batteries.0.is_null() {
                    break;
                };
            } else {
                break;
            }
        }
    }
}

impl PmaCopy for Batteries {
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
                    "Batteries node should be in PMA"
                );
                (*cursor.0).battery.assert_in_pma(pma);
                (*cursor.0).parent_axis.assert_in_pma(pma);
                if (*cursor.0).parent_batteries.0.is_null() {
                    break;
                }
                cursor = (*cursor.0).parent_batteries;
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
        let start = std::time::Instant::now();
        let mut last_progress = start;
        let mut steps = 0usize;
        if trace {
            debug!("pma-copy: batteries start: head_ptr={:p}", self.0);
        }
        let mut ptr: *mut Batteries = self;
        loop {
            if pma.contains_ptr((*ptr).0 as *const u8) {
                break;
            }
            if trace {
                let space = stack.noun_space();
                let battery = (*(*ptr).0).battery;
                let axis = (*(*ptr).0).parent_axis;
                let axis_noun = axis.as_noun();
                debug!(
                    "pma-copy: batteries node: node_ptr={:p} battery_raw=0x{:x} battery_repr={:?} axis_raw=0x{:x} axis_repr={:?}",
                    (*ptr).0,
                    unsafe { battery.as_raw() },
                    battery.repr(&space),
                    unsafe { axis_noun.as_raw() },
                    axis_noun.repr(&space)
                );
                debug!("pma-copy: batteries battery copy start");
            }
            // Copy the battery noun and parent_axis to PMA
            (*(*ptr).0).battery.copy_to_pma(stack, pma);
            if trace {
                debug!("pma-copy: batteries battery copy done");
                debug!("pma-copy: batteries axis copy start");
            }
            (*(*ptr).0).parent_axis.copy_to_pma(stack, pma);
            if trace {
                debug!("pma-copy: batteries axis copy done");
            }
            // Allocate new BatteriesMem in PMA and copy
            let dest_mem: *mut BatteriesMem = pma.alloc_struct(1);
            copy_nonoverlapping((*ptr).0, dest_mem, 1);
            // Update pointer to point to PMA copy
            *ptr = Batteries(dest_mem);
            // Move to next node
            ptr = &mut (*dest_mem).parent_batteries;
            if (*dest_mem).parent_batteries.0.is_null() {
                break;
            }
            steps += 1;
            if trace && (steps & 0x3ff == 0) {
                let now = std::time::Instant::now();
                if now.duration_since(last_progress).as_millis() >= 2000 {
                    debug!(
                        "pma-copy: batteries progress: steps={}, elapsed_ms={}",
                        steps,
                        start.elapsed().as_millis()
                    );
                    last_progress = now;
                }
            }
        }
        if trace {
            debug!(
                "pma-copy: batteries done: steps={}, elapsed_ms={}",
                steps,
                start.elapsed().as_millis()
            );
        }
    }
}

impl PmaCopyFrom for Batteries {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        if self.0.is_null() {
            return;
        }
        let mut ptr: *mut Batteries = self;
        loop {
            let src_mem = (*ptr).0;
            let dest_mem: *mut BatteriesMem = to_pma.alloc_struct(1);
            copy_nonoverlapping(src_mem, dest_mem, 1);
            *ptr = Batteries(dest_mem);
            (*dest_mem).battery.copy_from_pma(from_pma, to_pma);
            (*dest_mem).parent_axis.copy_from_pma(from_pma, to_pma);
            ptr = &mut (*dest_mem).parent_batteries;
            if (*dest_mem).parent_batteries.0.is_null() {
                break;
            }
        }
    }
}

impl Iterator for Batteries {
    type Item = (*mut Noun, Atom);
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_null() {
            None
        } else {
            unsafe {
                let res = (
                    &mut (*(self.0)).battery as *mut Noun,
                    (*(self.0)).parent_axis,
                );
                *self = (*(self.0)).parent_batteries;
                Some(res)
            }
        }
    }
}

impl Batteries {
    pub(crate) fn new(ptr: *mut BatteriesMem) -> Self {
        Batteries(ptr)
    }

    pub fn matches(self, stack: &mut NockStack, mut core: Noun, space: &NounSpace) -> bool {
        let mut root_found: bool = false;

        for (battery, parent_axis) in self {
            if root_found {
                panic!("cold: core matched to root, but more data remains in path");
            }

            if let Ok(d) = parent_axis.as_direct() {
                if d.data() == 0 {
                    if unsafe { unifying_equality(stack, &mut core, battery) } {
                        root_found = true;
                        continue;
                    } else {
                        return false;
                    };
                };
            };
            if let Ok(mut core_battery) = core.slot(2, space) {
                if unsafe { !unifying_equality(stack, &mut core_battery, battery) } {
                    return false;
                };
                if let Ok(core_parent) = core.slot_atom(parent_axis, space) {
                    core = core_parent;
                    continue;
                } else {
                    return false;
                }
            } else {
                return false;
            }
        }

        if !root_found {
            panic!("cold: core matched exactly, but never matched root");
        }

        true
    }
}

// BatteriesList is a linked list of core hierarchies with an iterator; used to
// store all possible parent hierarchies for a core
#[derive(Copy, Clone)]
pub struct BatteriesList(*mut BatteriesListMem);

const BATTERIES_LIST_NIL: BatteriesList = BatteriesList(null_mut());

#[derive(Copy, Clone)]
struct BatteriesListMem {
    batteries: Batteries,
    next: BatteriesList,
}

impl PmaCopy for BatteriesList {
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
                    "BatteriesList node should be in PMA"
                );
                (*cursor.0).batteries.assert_in_pma(pma);
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
        let mut ptr: *mut BatteriesList = self;
        loop {
            if pma.contains_ptr((*ptr).0 as *const u8) {
                break;
            }
            // Copy the batteries to PMA
            (*(*ptr).0).batteries.copy_to_pma(stack, pma);
            // Allocate new BatteriesListMem in PMA and copy
            let dest_mem: *mut BatteriesListMem = pma.alloc_struct(1);
            copy_nonoverlapping((*ptr).0, dest_mem, 1);
            // Update pointer to point to PMA copy
            *ptr = BatteriesList(dest_mem);
            // Move to next node
            ptr = &mut (*dest_mem).next;
            if (*dest_mem).next.0.is_null() {
                break;
            }
        }
    }
}

impl PmaCopyFrom for BatteriesList {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        if self.0.is_null() {
            return;
        }
        let mut ptr: *mut BatteriesList = self;
        loop {
            let src_mem = (*ptr).0;
            let dest_mem: *mut BatteriesListMem = to_pma.alloc_struct(1);
            copy_nonoverlapping(src_mem, dest_mem, 1);
            *ptr = BatteriesList(dest_mem);
            (*dest_mem).batteries.copy_from_pma(from_pma, to_pma);
            ptr = &mut (*dest_mem).next;
            if (*dest_mem).next.0.is_null() {
                break;
            }
        }
    }
}

impl Preserve for BatteriesList {
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        if self.0.is_null() {
            return;
        }
        let mut cursor = *self;
        loop {
            stack.assert_struct_is_in(cursor.0, 1);
            (*cursor.0).batteries.assert_in_stack(stack);
            if (*cursor.0).next.0.is_null() {
                break;
            };
            cursor = (*cursor.0).next;
        }
    }
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        if self.0.is_null() {
            return;
        };
        let mut ptr: *mut *mut BatteriesListMem = &mut self.0;
        loop {
            if stack.is_in_frame(*ptr) {
                (**ptr).batteries.preserve(stack);
                let dest_mem: *mut BatteriesListMem = stack.struct_alloc_in_previous_frame(1);
                copy_nonoverlapping(*ptr, dest_mem, 1);
                *ptr = dest_mem;
                ptr = &mut ((**ptr).next.0);
                if (*dest_mem).next.0.is_null() {
                    break;
                };
            } else {
                break;
            }
        }
    }
}

impl Iterator for BatteriesList {
    type Item = Batteries;
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_null() {
            None
        } else {
            unsafe {
                let mem = *(self.0);
                let res = mem.batteries;
                *self = mem.next;
                Some(res)
            }
        }
    }
}

impl BatteriesList {
    fn matches(
        mut self,
        stack: &mut NockStack,
        core: Noun,
        space: &NounSpace,
    ) -> Option<Batteries> {
        self.find(|&batteries| batteries.matches(stack, core, space))
    }
}

// NounList is a linked list of paths (path = list of nested core names) with an
// iterator; used to store all possible registered paths for a core
#[derive(Copy, Clone)]
pub struct NounList(pub(crate) *mut NounListMem);

const NOUN_LIST_NIL: NounList = NounList(null_mut());

#[derive(Copy, Clone)]
pub(crate) struct NounListMem {
    element: Noun,
    next: NounList,
}

impl Preserve for NounList {
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        if self.0.is_null() {
            return;
        };
        let mut cursor = *self;
        loop {
            stack.assert_struct_is_in(cursor.0, 1);
            (*cursor.0).element.assert_in_stack(stack);
            if (*cursor.0).next.0.is_null() {
                break;
            };
            cursor = (*cursor.0).next;
        }
    }
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        if self.0.is_null() {
            return;
        };
        let mut ptr: *mut NounList = self;
        loop {
            if stack.is_in_frame((*ptr).0) {
                (*(*ptr).0).element.preserve(stack);
                let dest_mem: *mut NounListMem = stack.struct_alloc_in_previous_frame(1);
                copy_nonoverlapping((*ptr).0, dest_mem, 1);
                *ptr = NounList(dest_mem);
                ptr = &mut ((*(*ptr).0).next);
                if (*dest_mem).next.0.is_null() {
                    break;
                };
            } else {
                break;
            }
        }
    }
}

impl PmaCopy for NounList {
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
                    "NounList node should be in PMA"
                );
                (*cursor.0).element.assert_in_pma(pma);
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
        let mut ptr: *mut NounList = self;
        loop {
            if pma.contains_ptr((*ptr).0 as *const u8) {
                break;
            }
            // Copy the element noun to PMA
            (*(*ptr).0).element.copy_to_pma(stack, pma);
            // Allocate new NounListMem in PMA and copy
            let dest_mem: *mut NounListMem = pma.alloc_struct(1);
            copy_nonoverlapping((*ptr).0, dest_mem, 1);
            // Update pointer to point to PMA copy
            *ptr = NounList(dest_mem);
            // Move to next node
            ptr = &mut (*dest_mem).next;
            if (*dest_mem).next.0.is_null() {
                break;
            }
        }
    }
}

impl PmaCopyFrom for NounList {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        if self.0.is_null() {
            return;
        }
        let mut ptr: *mut NounList = self;
        loop {
            let src_mem = (*ptr).0;
            let dest_mem: *mut NounListMem = to_pma.alloc_struct(1);
            copy_nonoverlapping(src_mem, dest_mem, 1);
            *ptr = NounList(dest_mem);
            (*dest_mem).element.copy_from_pma(from_pma, to_pma);
            ptr = &mut (*dest_mem).next;
            if (*dest_mem).next.0.is_null() {
                break;
            }
        }
    }
}

impl Iterator for NounList {
    type Item = *mut Noun;
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_null() {
            None
        } else {
            unsafe {
                let res = &mut (*(self.0)).element;
                *self = (*(self.0)).next;
                Some(res)
            }
        }
    }
}

#[derive(Copy, Clone)]
pub struct Cold(*mut ColdMem);

struct ColdMem {
    /// key: outermost battery (e.g. furthest battery from root for a core)
    /// value: possible registered paths for core
    ///
    /// Identical nock can exist in multiple places, so the outermost battery
    /// yields multiple paths. Instead of matching on the entire core in the Hamt
    /// (which would require iterating through every possible pair), we match
    /// the outermost battery to a path, then compare the core to the registered
    /// cores for that path.
    battery_to_paths: Hamt<NounList>,
    /// Roots
    /// key: root noun
    /// value: root path
    ///
    /// Just like battery_to_paths, but for roots (which refer to themselves as
    /// their parent).
    root_to_paths: Hamt<NounList>,
    /// key: registered path to core
    /// value: linked list of a sequence of nested batteries
    path_to_batteries: Hamt<BatteriesList>,
}

impl PmaCopy for Cold {
    #[cfg(feature = "pma-assert")]
    fn assert_in_pma(&self, pma: &Pma) {
        unsafe {
            assert!(
                pma.contains_ptr(self.0 as *const u8),
                "Cold struct should be in PMA"
            );
            (*self.0).battery_to_paths.assert_in_pma(pma);
            (*self.0).root_to_paths.assert_in_pma(pma);
            (*self.0).path_to_batteries.assert_in_pma(pma);
        }
    }

    #[cfg(not(feature = "pma-assert"))]
    #[inline(always)]
    fn assert_in_pma(&self, _pma: &Pma) {}

    unsafe fn copy_to_pma(&mut self, stack: &NockStack, pma: &mut Pma) {
        if pma.contains_ptr(self.0 as *const u8) {
            return;
        }
        // Copy each HAMT to PMA
        (*self.0).battery_to_paths.copy_to_pma(stack, pma);
        (*self.0).root_to_paths.copy_to_pma(stack, pma);
        (*self.0).path_to_batteries.copy_to_pma(stack, pma);
        // Allocate ColdMem in PMA and copy
        let dest_mem: *mut ColdMem = pma.alloc_struct(1);
        copy_nonoverlapping(self.0, dest_mem, 1);
        // Update pointer to point to PMA copy
        self.0 = dest_mem;
    }
}

impl PmaCopyFrom for Cold {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        if self.0.is_null() {
            return;
        }
        let src_mem = self.0;
        let dest_mem: *mut ColdMem = to_pma.alloc_struct(1);
        copy_nonoverlapping(src_mem, dest_mem, 1);
        self.0 = dest_mem;
        (*dest_mem).battery_to_paths.copy_from_pma(from_pma, to_pma);
        (*dest_mem).root_to_paths.copy_from_pma(from_pma, to_pma);
        (*dest_mem)
            .path_to_batteries
            .copy_from_pma(from_pma, to_pma);
    }
}

impl Preserve for Cold {
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        stack.assert_struct_is_in(self.0, 1);
        (*self.0).battery_to_paths.assert_in_stack(stack);
        (*self.0).root_to_paths.assert_in_stack(stack);
        (*self.0).path_to_batteries.assert_in_stack(stack);
    }
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        (*(self.0)).battery_to_paths.preserve(stack);
        (*(self.0)).root_to_paths.preserve(stack);
        (*(self.0)).path_to_batteries.preserve(stack);
        let new_dest: *mut ColdMem = stack.struct_alloc_in_previous_frame(1);
        copy_nonoverlapping(self.0, new_dest, 1);
        self.0 = new_dest;
    }
}

impl Cold {
    pub fn is_null(&self) -> bool {
        unsafe {
            (*self.0).battery_to_paths.is_null()
                || (*self.0).battery_to_paths.is_null()
                || (*self.0).root_to_paths.is_null()
        }
    }

    pub fn pma_offset(&self, pma: &Pma) -> Option<PmaOffsetWords> {
        let ptr = self.0 as *const u8;
        if pma.contains_ptr(ptr) {
            Some(pma.offset_from_ptr(ptr))
        } else {
            None
        }
    }

    pub unsafe fn from_pma_offset(pma: &Pma, offset: PmaOffsetWords) -> Self {
        let ptr = pma.ptr_from_offset(offset) as *mut ColdMem;
        Cold(ptr)
    }

    pub fn new(stack: &mut NockStack) -> Self {
        let battery_to_paths = Hamt::new(stack);
        let root_to_paths = Hamt::new(stack);
        let path_to_batteries = Hamt::new(stack);
        unsafe {
            let cold_mem_ptr: *mut ColdMem = stack.struct_alloc(1);
            *cold_mem_ptr = ColdMem {
                battery_to_paths,
                root_to_paths,
                path_to_batteries,
            };
            Cold(cold_mem_ptr)
        }
    }

    pub fn from_vecs(
        stack: &mut NockStack,
        battery_to_paths: Vec<(Noun, NounList)>,
        root_to_paths: Vec<(Noun, NounList)>,
        path_to_batteries: Vec<(Noun, BatteriesList)>,
    ) -> Self {
        let battery_to_paths = hamt_from_vec(stack, battery_to_paths);
        let root_to_paths = hamt_from_vec(stack, root_to_paths);
        let path_to_batteries = hamt_from_vec(stack, path_to_batteries);
        unsafe {
            let cold_mem_ptr: *mut ColdMem = stack.struct_alloc(1);
            *cold_mem_ptr = ColdMem {
                battery_to_paths,
                root_to_paths,
                path_to_batteries,
            };
            Cold(cold_mem_ptr)
        }
    }

    pub fn find(&mut self, stack: &mut NockStack, path: &mut Noun) -> BatteriesList {
        unsafe {
            (*(self.0))
                .path_to_batteries
                .lookup(stack, path)
                .unwrap_or(BATTERIES_LIST_NIL)
        }
    }

    /** Try to match a core directly to the cold state, print the resulting path if found
     */
    pub fn matches(&mut self, stack: &mut NockStack, core: &mut Noun) -> Option<Noun> {
        let space = stack.fast_noun_space();
        let mut battery = (*core).slot(2, &space).ok()?;
        unsafe {
            let paths = (*(self.0)).battery_to_paths.lookup(stack, &mut battery)?;
            for path in paths {
                if let Some(batteries_list) =
                    (*(self.0)).path_to_batteries.lookup(stack, &mut (*path))
                {
                    if let Some(_batt) = batteries_list.matches(stack, *core, &space) {
                        return Some(*path);
                    }
                }
            }
        };
        None
    }

    /// register a core, return a boolean of whether we actually needed to register (false ->
    /// already registered)
    ///
    /// XX: validate chum Noun as $chum
    #[allow(clippy::result_unit_err)]
    pub fn register(
        &mut self,
        stack: &mut NockStack,
        mut core: Noun,
        parent_axis: Atom,
        mut chum: Noun,
    ) -> Result {
        let space = stack.fast_noun_space();
        unsafe {
            // Are we registering a root?
            if let Ok(parent_axis_direct) = parent_axis.as_direct() {
                if parent_axis_direct.data() == 0 {
                    let mut root_path = T(stack, &[chum, D(0)]);
                    if let Some(paths) = (*(self.0)).root_to_paths.lookup(stack, &mut core) {
                        for a_path in paths {
                            if unifying_equality(stack, &mut root_path, a_path) {
                                return Ok(false); // it's already in here
                            }
                        }
                    }
                    let batteries_mem_ptr: *mut BatteriesMem = stack.struct_alloc(1);
                    *batteries_mem_ptr = BatteriesMem {
                        battery: core,
                        parent_axis: DirectAtom::new_unchecked(0).as_atom(),
                        parent_batteries: NO_BATTERIES,
                    };

                    let current_batteries_list: BatteriesList = (*(self.0))
                        .path_to_batteries
                        .lookup(stack, &mut root_path)
                        .unwrap_or(BATTERIES_LIST_NIL);

                    let batteries_list_mem_ptr: *mut BatteriesListMem = stack.struct_alloc(1);
                    *batteries_list_mem_ptr = BatteriesListMem {
                        batteries: Batteries(batteries_mem_ptr),
                        next: current_batteries_list,
                    };

                    let current_paths_list: NounList = (*(self.0))
                        .root_to_paths
                        .lookup(stack, &mut core)
                        .unwrap_or(NOUN_LIST_NIL);

                    let paths_list_mem_ptr: *mut NounListMem = stack.struct_alloc(1);
                    *paths_list_mem_ptr = NounListMem {
                        element: root_path,
                        next: current_paths_list,
                    };

                    let cold_mem_ptr: *mut ColdMem = stack.struct_alloc(1);
                    *cold_mem_ptr = ColdMem {
                        battery_to_paths: (*(self.0)).battery_to_paths,
                        root_to_paths: (*(self.0)).root_to_paths.insert(
                            stack,
                            &mut core,
                            NounList(paths_list_mem_ptr),
                        ),
                        path_to_batteries: (*(self.0)).path_to_batteries.insert(
                            stack,
                            &mut root_path,
                            BatteriesList(batteries_list_mem_ptr),
                        ),
                    };

                    *self = Cold(cold_mem_ptr);
                    return Ok(true);
                }
            }

            let mut battery = core.slot(2, &space)?;
            let mut parent = core.slot_atom(parent_axis, &space)?;
            // Check if we already registered this core
            if let Some(paths) = (*(self.0)).battery_to_paths.lookup(stack, &mut battery) {
                for path in paths {
                    if let Ok(path_cell) = (*path).in_space(&space).as_cell() {
                        let mut head = path_cell.head().noun();
                        if unifying_equality(stack, &mut head, &mut chum) {
                            if let Some(batteries_list) =
                                (*(self.0)).path_to_batteries.lookup(stack, &mut *path)
                            {
                                if let Some(_batteries) =
                                    batteries_list.matches(stack, core, &space)
                                {
                                    return Ok(false);
                                }
                            }
                        }
                    }
                }
            }

            let mut parent_battery = parent.slot(2, &space)?;

            // err until we actually found a parent
            let mut ret: Result = Err(Error::NoParent);

            let mut path_to_batteries = (*(self.0)).path_to_batteries;
            let mut battery_to_paths = (*(self.0)).battery_to_paths;
            let root_to_paths = (*(self.0)).root_to_paths;

            if let Some(paths) = battery_to_paths.lookup(stack, &mut parent_battery) {
                for a_path in paths {
                    // path is a reserved word lol
                    let battery_list = path_to_batteries
                        .lookup(stack, &mut *a_path)
                        .unwrap_or(BATTERIES_LIST_NIL);
                    if let Some(parent_batteries) = battery_list.matches(stack, parent, &space) {
                        let mut my_path = T(stack, &[chum, *a_path]);

                        let batteries_mem_ptr: *mut BatteriesMem = stack.struct_alloc(1);
                        *batteries_mem_ptr = BatteriesMem {
                            battery,
                            parent_axis,
                            parent_batteries,
                        };

                        let current_batteries_list = path_to_batteries
                            .lookup(stack, &mut my_path)
                            .unwrap_or(BATTERIES_LIST_NIL);
                        let batteries_list_mem_ptr: *mut BatteriesListMem = stack.struct_alloc(1);
                        *batteries_list_mem_ptr = BatteriesListMem {
                            batteries: Batteries(batteries_mem_ptr),
                            next: current_batteries_list,
                        };

                        let current_paths_list = battery_to_paths
                            .lookup(stack, &mut battery)
                            .unwrap_or(NOUN_LIST_NIL);
                        let paths_list_mem_ptr: *mut NounListMem = stack.struct_alloc(1);
                        *paths_list_mem_ptr = NounListMem {
                            element: my_path,
                            next: current_paths_list,
                        };

                        path_to_batteries = path_to_batteries.insert(
                            stack,
                            &mut my_path,
                            BatteriesList(batteries_list_mem_ptr),
                        );
                        battery_to_paths = battery_to_paths.insert(
                            stack,
                            &mut battery,
                            NounList(paths_list_mem_ptr),
                        );
                        ret = Ok(true);
                    }
                }
            };

            if let Some(paths) = root_to_paths.lookup(stack, &mut parent) {
                for a_path in paths {
                    // path is a reserved word lol
                    let battery_list = path_to_batteries
                        .lookup(stack, &mut *a_path)
                        .unwrap_or(BATTERIES_LIST_NIL);
                    if let Some(parent_batteries) = battery_list.matches(stack, parent, &space) {
                        let mut my_path = T(stack, &[chum, *a_path]);

                        let batteries_mem_ptr: *mut BatteriesMem = stack.struct_alloc(1);
                        *batteries_mem_ptr = BatteriesMem {
                            battery,
                            parent_axis,
                            parent_batteries,
                        };

                        let current_batteries_list = path_to_batteries
                            .lookup(stack, &mut my_path)
                            .unwrap_or(BATTERIES_LIST_NIL);
                        let batteries_list_mem_ptr: *mut BatteriesListMem = stack.struct_alloc(1);
                        *batteries_list_mem_ptr = BatteriesListMem {
                            batteries: Batteries(batteries_mem_ptr),
                            next: current_batteries_list,
                        };

                        let current_paths_list = battery_to_paths
                            .lookup(stack, &mut battery)
                            .unwrap_or(NOUN_LIST_NIL);
                        let paths_list_mem_ptr: *mut NounListMem = stack.struct_alloc(1);
                        *paths_list_mem_ptr = NounListMem {
                            element: my_path,
                            next: current_paths_list,
                        };

                        path_to_batteries = path_to_batteries.insert(
                            stack,
                            &mut my_path,
                            BatteriesList(batteries_list_mem_ptr),
                        );
                        battery_to_paths = battery_to_paths.insert(
                            stack,
                            &mut battery,
                            NounList(paths_list_mem_ptr),
                        );
                        ret = Ok(true);
                    }
                }
            };

            let cold_mem_ptr: *mut ColdMem = stack.struct_alloc(1);
            *cold_mem_ptr = ColdMem {
                battery_to_paths,
                root_to_paths,
                path_to_batteries,
            };

            *self = Cold(cold_mem_ptr);
            ret
        }
    }
}

pub struct NounListIterator<'a> {
    noun: Noun,
    space: &'a NounSpace,
}

impl<'a> NounListIterator<'a> {
    fn new(noun: Noun, space: &'a NounSpace) -> Self {
        Self { noun, space }
    }
}

impl<'a> Iterator for NounListIterator<'a> {
    type Item = Noun;
    fn next(&mut self) -> Option<Self::Item> {
        if let Ok(it) = self.noun.as_cell() {
            self.noun = it.tail(self.space);
            Some(it.head(self.space))
        } else if unsafe { self.noun.raw_equals(&D(0)) } {
            None
        } else {
            panic!("Improper list terminator: {:?}", self.noun)
        }
    }
}

fn copy_noun_into_allocator<A: NounAllocator>(
    stack: &mut A,
    noun: Noun,
    space: &NounSpace,
) -> Noun {
    let mut copied: IntMap<u64, Noun> = IntMap::new();
    let mut result = D(0);
    let mut copy_stack = vec![(noun, std::ptr::addr_of_mut!(result))];

    while let Some((noun, dest)) = copy_stack.pop() {
        match noun.as_either_direct_allocated() {
            either::Either::Left(direct) => unsafe {
                *dest = direct.as_noun();
            },
            either::Either::Right(allocated) => match allocated.as_either() {
                either::Either::Left(indirect) => {
                    let atom_handle = indirect.as_atom().in_space(space);
                    let raw_pointer = unsafe { atom_handle.raw_pointer() };
                    let raw_size = atom_handle.raw_size();
                    if let Some(copied_noun) = copied.get(raw_pointer as u64) {
                        unsafe { *dest = *copied_noun };
                        continue;
                    }

                    let indirect_mem = unsafe { stack.alloc_indirect(atom_handle.size()) };
                    unsafe {
                        copy_nonoverlapping(raw_pointer, indirect_mem, raw_size);
                    }
                    let copied_noun = unsafe {
                        IndirectAtom::from_raw_pointer(indirect_mem)
                            .as_atom()
                            .as_noun()
                    };
                    copied.insert(raw_pointer as u64, copied_noun);
                    unsafe { *dest = copied_noun };
                }
                either::Either::Right(cell) => {
                    let cell_handle = cell.in_space(space);
                    let raw_pointer = unsafe { cell_handle.raw_pointer() };
                    if let Some(copied_noun) = copied.get(raw_pointer as u64) {
                        unsafe { *dest = *copied_noun };
                        continue;
                    }

                    let cell_mem = unsafe { stack.alloc_cell() };
                    unsafe {
                        copy_nonoverlapping(raw_pointer, cell_mem, 1);
                    }
                    let copied_noun =
                        unsafe { crate::noun::Cell::from_raw_pointer(cell_mem).as_noun() };
                    copied.insert(raw_pointer as u64, copied_noun);
                    unsafe {
                        *dest = copied_noun;
                        copy_stack.push((
                            cell_handle.tail().noun(),
                            std::ptr::addr_of_mut!((*cell_mem).tail),
                        ));
                        copy_stack.push((
                            cell_handle.head().noun(),
                            std::ptr::addr_of_mut!((*cell_mem).head),
                        ));
                    }
                }
            },
        }
    }

    result
}

#[derive(thiserror::Error, Debug)]
pub enum FromNounError {
    #[error("Not an atom")]
    NotAtom,
    #[error("Not a u64")]
    NotU64,
    #[error("Not a cell")]
    NotCell,
    #[error("Noun error: {0}")]
    NounError(#[from] noun::Error),
    #[error("UTF-8 error: {0}")]
    Utf8Error(#[from] std::str::Utf8Error),
}

pub type NounableResult<T> = std::result::Result<T, FromNounError>;

pub trait Nounable {
    type Target;
    // type Allocator;

    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun;
    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target>
    where
        Self: Sized;
}

impl Nounable for Atom {
    type Target = Self;

    fn into_noun<A: NounAllocator>(self, _stack: &mut A) -> Noun {
        self.as_noun()
    }
    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let copied = copy_noun_into_allocator(stack, *noun, space);
        copied.as_atom().map_err(|_| FromNounError::NotAtom)
    }
}

impl Nounable for u64 {
    type Target = Self;
    fn into_noun<A: NounAllocator>(self, _stack: &mut A) -> Noun {
        // Copied from Crown's IntoNoun, not sure why this isn't D(*self)
        unsafe { Atom::from_raw(self).into_noun(_stack) }
    }
    fn from_noun<A: NounAllocator>(
        _stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let atom = noun.atom().ok_or(FromNounError::NotAtom)?;
        let as_u64 = atom.in_space(space).as_u64()?;
        Ok(as_u64)
    }
}

impl Nounable for Noun {
    type Target = Self;
    fn into_noun<A: NounAllocator>(self, _stack: &mut A) -> Noun {
        self
    }

    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Self,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        Ok(copy_noun_into_allocator(stack, *noun, space))
    }
}

impl Nounable for &str {
    type Target = String;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let contents_atom = unsafe {
            let bytes = self.bytes().collect::<Vec<u8>>();
            let space = stack.noun_space();
            IndirectAtom::new_raw_bytes_ref(stack, bytes.as_slice()).normalize_as_atom(&space)
        };
        contents_atom.into_noun(stack)
    }
    fn from_noun<A: NounAllocator>(
        _stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let atom = noun.as_atom()?;
        let atom_handle = atom.in_space(space);
        let bytes = atom_handle.as_ne_bytes();
        let utf8 = std::str::from_utf8(bytes)?;
        let allocated = utf8.to_string();
        Ok(allocated)
    }
}

impl<T: Nounable + Copy> Nounable for &[T] {
    type Target = Vec<T::Target>;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let mut list = D(0);
        for item in self.iter().rev() {
            let item_noun = item.into_noun(stack);
            list = T(stack, &[item_noun, list]);
        }
        list
    }

    fn from_noun<A: NounAllocator>(
        _stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let mut items: Vec<<T as Nounable>::Target> = vec![];
        for item in NounListIterator::new(*noun, space) {
            let item = T::from_noun(_stack, &item, space)?;
            items.push(item);
        }
        Ok(items)
    }
}

impl<T: Nounable, U: Nounable, V: Nounable> Nounable for (T, U, V) {
    type Target = (T::Target, U::Target, V::Target);
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        // It's a three-tuple now
        let (a, b, c) = self;
        let a_noun = a.into_noun(stack);
        let b_noun = b.into_noun(stack);
        let c_noun = c.into_noun(stack);
        T(stack, &[a_noun, b_noun, c_noun])
    }

    fn from_noun<A: NounAllocator>(
        _stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        // it's a three tuple now
        let cell = noun.in_space(space).as_cell()?;
        let head = cell.head().noun();
        let tail = cell.tail().noun();
        let a = T::from_noun(_stack, &head, space)?;
        let cell = tail.in_space(space).as_cell()?;
        let b = U::from_noun(_stack, &cell.head().noun(), space)?;
        let c = V::from_noun(_stack, &cell.tail().noun(), space)?;
        Ok((a, b, c))
    }
}

impl<T: Nounable, U: Nounable> Nounable for (T, U) {
    type Target = (T::Target, U::Target);
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let (a, b) = self;
        let a_noun = a.into_noun(stack);
        let b_noun = b.into_noun(stack);
        T(stack, &[a_noun, b_noun])
    }

    fn from_noun<A: NounAllocator>(
        _stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let cell = noun.in_space(space).as_cell()?;
        let head = cell.head().noun();
        let tail = cell.tail().noun();
        let a = T::from_noun(_stack, &head, space)?;
        let b = U::from_noun(_stack, &tail, space)?;
        Ok((a, b))
    }
}

impl Nounable for NounList {
    type Target = NounList;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let mut list = D(0);
        for item in self {
            list = T(stack, &[unsafe { *item }, list]);
        }
        list
    }

    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let mut result = NOUN_LIST_NIL;
        for item in NounListIterator::new(*noun, space) {
            let item = <Noun as Nounable>::from_noun(stack, &item, space)?;
            let list_mem_ptr: *mut NounListMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                list_mem_ptr.write(NounListMem {
                    element: item,
                    next: result,
                });
            }
            result = NounList(list_mem_ptr);
        }
        Ok(result)
    }
}

impl Nounable for Batteries {
    type Target = Batteries;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let mut list = D(0);
        for (battery, parent_axis) in self {
            let battery_noun = unsafe { *battery };
            let parent_axis_noun = parent_axis.as_noun();
            let item = T(stack, &[battery_noun, parent_axis_noun]);
            list = T(stack, &[item, list]);
        }
        list
    }

    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let mut batteries = NO_BATTERIES;
        for item in NounListIterator::new(*noun, space) {
            let cell = item.in_space(space).as_cell()?;
            let battery = <Noun as Nounable>::from_noun(stack, &cell.head().noun(), space)?;
            let parent_axis = <Atom as Nounable>::from_noun(stack, &cell.tail().noun(), space)?;
            let batteries_mem: *mut BatteriesMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                batteries_mem.write(BatteriesMem {
                    battery,
                    parent_axis,
                    parent_batteries: batteries,
                });
            }
            batteries = Batteries(batteries_mem);
        }
        Ok(batteries)
    }
}

impl Nounable for BatteriesList {
    type Target = BatteriesList;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let mut list = D(0);
        for batteries in self {
            let batteries_noun = batteries.into_noun(stack);
            list = T(stack, &[batteries_noun, list]);
        }
        list
    }

    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let mut batteries_list = BATTERIES_LIST_NIL;
        for item in NounListIterator::new(*noun, space) {
            let batteries = Batteries::from_noun(stack, &item, space)?;
            let batteries_list_mem: *mut BatteriesListMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                batteries_list_mem.write(BatteriesListMem {
                    batteries,
                    next: batteries_list,
                });
            }
            batteries_list = BatteriesList(batteries_list_mem);
        }
        Ok(batteries_list)
    }
}

impl<T: Nounable + Copy + mem::Preserve> Nounable for Hamt<T> {
    type Target = Vec<(Noun, T::Target)>;

    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let mut list = D(0);
        let mut reverse = Vec::new();
        for item in self.iter() {
            reverse.push(item);
        }
        reverse.reverse();
        for slice in reverse {
            for (key, value) in slice {
                let value_noun = value.into_noun(stack);
                let items = T(stack, &[*key, value_noun]);
                list = T(stack, &[items, list]);
            }
        }
        list
    }

    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let mut items = Vec::new();
        for item in NounListIterator::new(*noun, space) {
            let cell = item.in_space(space).as_cell()?;
            let key = <Noun as Nounable>::from_noun(stack, &cell.head().noun(), space)?;
            let value = T::from_noun(stack, &cell.tail().noun(), space)?;
            items.push((key, value));
        }
        // items.reverse();
        Ok(items)
    }
}

// This blows up into an ugly refactor around a concrete NockStack, better to have a separate conversion function
pub fn hamt_from_vec<T: Nounable + Copy + mem::Preserve>(
    stack: &mut NockStack,
    items: Vec<(Noun, T)>,
) -> Hamt<T> {
    let mut hamt = Hamt::new(stack);
    for (mut key, value) in items {
        hamt = hamt.insert(stack, &mut key, value);
    }
    hamt
}

impl Nounable for Cold {
    type Target = (
        Vec<(Noun, NounList)>,
        Vec<(Noun, NounList)>,
        Vec<(Noun, BatteriesList)>,
    );

    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let cold_mem = self.0;
        let mut battery_to_paths_noun = D(0);
        let mut root_to_paths_noun = D(0);
        let mut path_to_batteries_noun = D(0);
        unsafe {
            for slice in (*cold_mem).battery_to_paths.iter() {
                for (battery, paths) in slice {
                    let battery_noun = battery.into_noun(stack);
                    let paths_noun = paths.into_noun(stack);
                    // two-step the cons'ing for correct associativity
                    let items = T(stack, &[battery_noun, paths_noun]);
                    battery_to_paths_noun = T(stack, &[items, battery_to_paths_noun]);
                }
            }
            for slice in (*cold_mem).root_to_paths.iter() {
                for (root, paths) in slice {
                    let root_noun = root.into_noun(stack);
                    let paths_noun = paths.into_noun(stack);
                    // two-step the cons'ing for correct associativity
                    let items = T(stack, &[root_noun, paths_noun]);
                    root_to_paths_noun = T(stack, &[items, root_to_paths_noun]);
                }
            }
            for slice in (*cold_mem).path_to_batteries.iter() {
                for (path, batteries) in slice {
                    let path_noun = path.into_noun(stack);
                    let batteries_noun = batteries.into_noun(stack);
                    // two-step the cons'ing for correct associativity
                    let items = T(stack, &[path_noun, batteries_noun]);
                    path_to_batteries_noun = T(stack, &[items, path_to_batteries_noun]);
                }
            }
        }

        T(
            stack,
            &[battery_to_paths_noun, root_to_paths_noun, path_to_batteries_noun],
        )
    }

    fn from_noun<A: NounAllocator>(
        stack: &mut A,
        noun: &Noun,
        space: &NounSpace,
    ) -> NounableResult<Self::Target> {
        let mut battery_to_paths = Vec::new();
        let mut root_to_paths = Vec::new();
        let mut path_to_batteries = Vec::new();

        let battery_to_paths_noun = noun.slot(2, space)?;
        let root_to_paths_noun = noun.slot(6, space)?;
        let path_to_batteries_noun = noun.slot(7, space)?;

        // iterate over battery_to_paths_noun
        for item in NounListIterator::new(battery_to_paths_noun, space) {
            let cell = item.in_space(space).as_cell()?;
            let key = cell.head().noun();
            let value = NounList::from_noun(stack, &cell.tail().noun(), space)?;
            battery_to_paths.push((key, value));
        }

        // iterate over root_to_paths_noun
        for item in NounListIterator::new(root_to_paths_noun, space) {
            let cell = item.in_space(space).as_cell()?;
            let key = cell.head().noun();
            let value = NounList::from_noun(stack, &cell.tail().noun(), space)?;
            root_to_paths.push((key, value));
        }

        // iterate over path_to_batteries_noun
        for item in NounListIterator::new(path_to_batteries_noun, space) {
            let cell = item.in_space(space).as_cell()?;
            let key = cell.head().noun();
            let value = BatteriesList::from_noun(stack, &cell.tail().noun(), space)?;
            path_to_batteries.push((key, value));
        }
        battery_to_paths.reverse();
        root_to_paths.reverse();
        path_to_batteries.reverse();

        let result = (battery_to_paths, root_to_paths, path_to_batteries);
        Ok(result)
    }
}

#[cfg(test)]
pub(crate) mod test {
    use std::iter::FromIterator;

    use super::*;
    use crate::ext::noun_equality;
    use crate::hamt::Hamt;
    use crate::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use crate::noun::{AllocLocation, Cell, Noun, NounSpace, D};
    /// Default stack size for tests where you aren't intending to run out of space
    pub(crate) const DEFAULT_STACK_SIZE: usize = NOCK_STACK_SIZE_TINY;
    pub(crate) fn make_test_stack(size: usize) -> NockStack {
        let top_slots = 3;

        NockStack::new(size, top_slots)
    }

    fn make_cold_state(stack: &mut NockStack) -> Cold {
        let cold = Cold::new(stack);
        unsafe {
            let battery_to_paths_list = make_noun_list(stack, &[5, 6]);
            (*cold.0).battery_to_paths =
                (*cold.0)
                    .battery_to_paths
                    .insert(stack, &mut D(200), battery_to_paths_list);
            let root_noun_list = make_noun_list(stack, &[1, 2]);
            (*cold.0).root_to_paths =
                (*cold.0)
                    .root_to_paths
                    .insert(stack, &mut D(100), root_noun_list);
            let root_noun_list = make_noun_list(stack, &[3, 4]);
            (*cold.0).root_to_paths =
                (*cold.0)
                    .root_to_paths
                    .insert(stack, &mut D(101), root_noun_list);

            let batteries_list = make_batteries_list(stack, &[7, 8]);
            (*cold.0).path_to_batteries =
                (*cold.0)
                    .path_to_batteries
                    .insert(stack, &mut D(300), batteries_list);
        }
        cold
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn cold_bidirectional_conversion() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let space = stack.noun_space();
        let cold = make_cold_state(&mut stack);
        let cold_noun = cold.into_noun(&mut stack);
        let new_cold = Cold::from_noun(&mut stack, &cold_noun, &space)
            .expect("Failed to convert noun to cold");

        // battery_to_paths
        let old_battery_to_paths = unsafe { &(*cold.0).battery_to_paths };
        let new_battery_to_paths = new_cold.0.clone();
        for (a, b) in old_battery_to_paths
            .iter()
            .flatten()
            .zip(new_battery_to_paths.iter())
        {
            let key_a = &mut a.0.clone() as *mut Noun;
            let key_b = &mut b.0.clone() as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, key_a, key_b) },
                "Keys don't match: {:?} {:?}",
                a.0,
                b.0
            );
            let mut value_a_noun = a.1.into_noun(&mut stack);
            let mut value_b_noun = b.1.into_noun(&mut stack);
            let value_a = &mut value_a_noun as *mut Noun;
            let value_b = &mut value_b_noun as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, value_a, value_b) },
                "Values don't match: old: {:?} new: {:?}",
                value_a_noun,
                value_b_noun
            );
        }
        // Use zipped iteration to compare the two cold states
        let old_root_to_paths = unsafe { &(*cold.0).root_to_paths };
        let new_root_to_paths = new_cold.1.clone();
        for (a, b) in old_root_to_paths
            .iter()
            .flatten()
            .zip(new_root_to_paths.iter())
        {
            let key_a = &mut a.0.clone() as *mut Noun;
            let key_b = &mut b.0.clone() as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, key_a, key_b) },
                "Keys don't match: {:?} {:?}",
                a.0,
                b.0
            );
            let mut value_a_noun = a.1.into_noun(&mut stack);
            let mut value_b_noun = b.1.into_noun(&mut stack);
            let value_a = &mut value_a_noun as *mut Noun;
            let value_b = &mut value_b_noun as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, value_a, value_b) },
                "Values don't match: old: {:?} new: {:?}",
                value_a_noun,
                value_b_noun
            );
        }
        // path_to_batteries
        let old_path_to_batteries = unsafe { &(*cold.0).path_to_batteries };
        let new_path_to_batteries = new_cold.2.clone();
        for (a, b) in old_path_to_batteries
            .iter()
            .flatten()
            .zip(new_path_to_batteries.iter())
        {
            let key_a = &mut a.0.clone() as *mut Noun;
            let key_b = &mut b.0.clone() as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, key_a, key_b) },
                "Keys don't match: {:?} {:?}",
                a.0,
                b.0
            );
            let mut value_a_noun = a.1.into_noun(&mut stack);
            let mut value_b_noun = b.1.into_noun(&mut stack);
            let value_a = &mut value_a_noun as *mut Noun;
            let value_b = &mut value_b_noun as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, value_a, value_b) },
                "Values don't match: old: {:?} new: {:?}",
                value_a_noun,
                value_b_noun
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn hamt_bidirectional_conversion() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let space = stack.noun_space();
        let items = vec![(D(0), D(1)), (D(2), D(3))];
        let hamt = super::hamt_from_vec(&mut stack, items);
        let noun = hamt.into_noun(&mut stack);
        let new_hamt: Vec<(Noun, Noun)> =
            <Hamt<Noun> as Nounable>::from_noun::<NockStack>(&mut stack, &noun, &space)
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
        let flat_hamt: Vec<(Noun, Noun)> = hamt.iter().flatten().cloned().collect();
        for (a, b) in new_hamt.iter().zip(flat_hamt.iter()) {
            let key_a = &mut a.0.clone() as *mut Noun;
            let key_b = &mut b.0.clone() as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, key_a, key_b) },
                "Keys don't match: {:?} {:?}",
                a.0,
                b.0
            );
            let value_a = &mut a.1.clone() as *mut Noun;
            let value_b = &mut b.1.clone() as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, value_a, value_b) },
                "Values don't match: {:?} {:?}",
                a.1,
                b.1
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn hamt_from_noun_rehomes_foreign_keys_and_values() {
        let mut source = make_test_stack(DEFAULT_STACK_SIZE);
        let source_space = source.noun_space();
        let foreign_key = T(&mut source, &[D(10), D(11)]);
        let foreign_value = T(&mut source, &[D(12), D(13)]);
        let hamt = super::hamt_from_vec(&mut source, vec![(foreign_key, foreign_value)]);
        let noun = hamt.into_noun(&mut source);

        let mut dest = make_test_stack(DEFAULT_STACK_SIZE);
        let decoded: Vec<(Noun, Noun)> =
            <Hamt<Noun> as Nounable>::from_noun::<NockStack>(&mut dest, &noun, &source_space)
                .expect("decode hamt from foreign stack");

        assert_eq!(decoded.len(), 1);
        let dest_space = dest.noun_space();
        let (decoded_key, decoded_value) = decoded[0];
        assert!(
            !unsafe { decoded_key.raw_equals(&foreign_key) },
            "decoded hamt key should not retain the foreign pointer"
        );
        assert!(
            !unsafe { decoded_value.raw_equals(&foreign_value) },
            "decoded hamt value should not retain the foreign pointer"
        );
        verify_noun_stack_allocated(decoded_key, &dest_space, "decoded hamt key");
        verify_noun_stack_allocated(decoded_value, &dest_space, "decoded hamt value");
    }

    fn make_batteries_list(stack: &mut NockStack, v: &[u64]) -> BatteriesList {
        let mut batteries_list = BATTERIES_LIST_NIL;
        for &item in v.iter().rev() {
            let batteries_mem: *mut BatteriesMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                batteries_mem.write(BatteriesMem {
                    battery: D(item),
                    parent_axis: D(0).as_atom().unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    }),
                    parent_batteries: NO_BATTERIES,
                });
            }
            let batteries = Batteries(batteries_mem);
            let batteries_list_mem: *mut BatteriesListMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                batteries_list_mem.write(BatteriesListMem {
                    batteries,
                    next: batteries_list,
                });
            }
            batteries_list = BatteriesList(batteries_list_mem);
        }
        batteries_list
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn batteries_list_bidirectional_conversion() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let space = stack.noun_space();
        let batteries_list2 = make_batteries_list(&mut stack, &[1, 2]);
        let batteries_list_noun = batteries_list2.into_noun(&mut stack);
        let new_batteries_list2 =
            BatteriesList::from_noun(&mut stack, &batteries_list_noun, &space)
                .expect("Failed to convert noun to batteries list");
        for (a, b) in batteries_list2.zip(new_batteries_list2) {
            let mut a_noun = a.into_noun(&mut stack);
            let mut b_noun = b.into_noun(&mut stack);
            let a_ptr = &mut a_noun as *mut Noun;
            let b_ptr = &mut b_noun as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, a_ptr, b_ptr) },
                "Items don't match"
            );
        }
    }

    fn make_batteries(stack: &mut NockStack) -> Batteries {
        let batteries_mem: *mut BatteriesMem = unsafe { stack.alloc_struct(1) };
        unsafe {
            batteries_mem.write(BatteriesMem {
                battery: D(0),
                parent_axis: D(1).as_atom().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                }),
                parent_batteries: NO_BATTERIES,
            });
        }
        let batteries = Batteries(batteries_mem);
        let batteries_mem2: *mut BatteriesMem = unsafe { stack.alloc_struct(1) };
        unsafe {
            batteries_mem2.write(BatteriesMem {
                battery: D(2),
                parent_axis: D(3).as_atom().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                }),
                parent_batteries: batteries,
            });
        }

        Batteries(batteries_mem2)
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn batteries_bidirectional_conversion() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let space = stack.noun_space();
        let batteries2 = make_batteries(&mut stack);
        let batteries_noun = batteries2.into_noun(&mut stack);
        let new_batteries = Batteries::from_noun(&mut stack, &batteries_noun, &space)
            .expect("Failed to convert noun to batteries");
        assert_eq!(new_batteries.count(), 2);
        assert_eq!(batteries2.count(), 2);
        for ((a, a_atom), (b, b_atom)) in new_batteries.zip(batteries2) {
            let a_ptr = a;
            let b_ptr = b;
            let a_val = unsafe { *a_ptr };
            let b_val = unsafe { *b_ptr };
            assert!(
                unsafe { unifying_equality(&mut stack, a_ptr, b_ptr) },
                "Items don't match: {:?} {:?}",
                a_val,
                b_val
            );
            let a_atom_noun = a_atom.into_noun(&mut stack);
            let b_atom_noun = b_atom.into_noun(&mut stack);
            let a_atom_noun_ptr = &mut a_atom_noun.clone() as *mut Noun;
            let b_atom_noun_ptr = &mut b_atom_noun.clone() as *mut Noun;
            assert!(
                unsafe { unifying_equality(&mut stack, a_atom_noun_ptr, b_atom_noun_ptr) },
                "Parent axes don't match: {:?} {:?}",
                a_atom.in_space(&space).as_u64(),
                b_atom.in_space(&space).as_u64()
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn batteries_from_noun_rehomes_foreign_payloads() {
        let mut source = make_test_stack(DEFAULT_STACK_SIZE);
        let source_space = source.noun_space();
        let foreign_battery = T(&mut source, &[D(20), D(21)]);
        let foreign_axis = Atom::new(&mut source, u64::MAX);
        let batteries_item = T(&mut source, &[foreign_battery, foreign_axis.as_noun()]);
        let batteries_noun = T(&mut source, &[batteries_item, D(0)]);

        let mut dest = make_test_stack(DEFAULT_STACK_SIZE);
        let decoded = Batteries::from_noun(&mut dest, &batteries_noun, &source_space)
            .expect("decode batteries from foreign stack");
        unsafe { decoded.assert_in_stack(&dest) };

        let dest_space = dest.noun_space();
        let (battery_ptr, parent_axis) = decoded.into_iter().next().expect("decoded batteries");
        let battery = unsafe { *battery_ptr };
        assert!(
            !unsafe { battery.raw_equals(&foreign_battery) },
            "decoded battery should not retain the foreign pointer"
        );
        assert!(
            !unsafe { parent_axis.as_noun().raw_equals(&foreign_axis.as_noun()) },
            "decoded parent axis should not retain the foreign pointer"
        );
        verify_noun_stack_allocated(battery, &dest_space, "decoded battery");
        verify_noun_stack_allocated(parent_axis.as_noun(), &dest_space, "decoded parent axis");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn tuple_bidirectional_conversion() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let space = stack.noun_space();
        let tup = (D(1), D(2), D(3));
        let noun = tup.into_noun(&mut stack);
        let new_tup: (Noun, Noun, Noun) =
            <(Noun, Noun, Noun) as Nounable>::from_noun::<NockStack>(&mut stack, &noun, &space)
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
        let (a, b, c) = new_tup;
        let a_ptr = &mut a.clone() as *mut Noun;
        let b_ptr = &mut b.clone() as *mut Noun;
        let c_ptr = &mut c.clone() as *mut Noun;
        assert!(
            unsafe { unifying_equality(&mut stack, a_ptr, &mut D(1) as *mut Noun) },
            "First item doesn't match"
        );
        assert!(
            unsafe { unifying_equality(&mut stack, b_ptr, &mut D(2) as *mut Noun) },
            "Second item doesn't match"
        );
        assert!(
            unsafe { unifying_equality(&mut stack, c_ptr, &mut D(3) as *mut Noun) },
            "Third item doesn't match"
        );
    }

    pub(crate) fn make_noun_list(stack: &mut NockStack, v: &[u64]) -> NounList {
        let mut noun_list = NOUN_LIST_NIL;
        for &item in v.iter().rev() {
            let noun_list_mem: *mut NounListMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                noun_list_mem.write(NounListMem {
                    element: D(item),
                    next: noun_list,
                });
            }
            noun_list = NounList(noun_list_mem);
        }
        noun_list
    }

    fn make_noun_list_from_nouns(stack: &mut NockStack, nouns: &[Noun]) -> NounList {
        let mut noun_list = NOUN_LIST_NIL;
        for &item in nouns.iter().rev() {
            let noun_list_mem: *mut NounListMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                noun_list_mem.write(NounListMem {
                    element: item,
                    next: noun_list,
                });
            }
            noun_list = NounList(noun_list_mem);
        }
        noun_list
    }

    fn verify_noun_stack_allocated(noun: Noun, space: &NounSpace, context: &str) {
        if noun.is_direct() {
            return;
        }

        let location = noun.in_space(space).allocated_location();
        assert!(
            matches!(location, Some(AllocLocation::Stack)),
            "{} should be stack-allocated after decode",
            context
        );

        if let Ok(cell) = noun.in_space(space).as_cell() {
            verify_noun_stack_allocated(cell.head().noun(), space, context);
            verify_noun_stack_allocated(cell.tail().noun(), space, context);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn noun_list_bidirectional_conversion() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        const ITEM_COUNT: u64 = 2;
        let vec = Vec::from_iter(1..=ITEM_COUNT);
        let items = vec.iter().map(|&x| D(x)).collect::<Vec<Noun>>();
        let slice = vec.as_slice();
        let noun_list = make_noun_list(&mut stack, slice);
        let noun = noun_list.into_noun(&mut stack);
        let space = stack.noun_space();
        let new_noun_list: NounList =
            <NounList as Nounable>::from_noun::<NockStack>(&mut stack, &noun, &space)
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
        let mut item_count = 0;
        for (a, b) in new_noun_list.zip(items.iter()) {
            let a_ptr = a;
            let b_ptr = &mut b.clone() as *mut Noun;
            let a_val = unsafe { *a_ptr };
            item_count += 1;
            assert!(
                unsafe { unifying_equality(&mut stack, a_ptr, b_ptr) },
                "Items don't match: {:?} {:?}",
                a_val,
                b
            );
        }
        assert_eq!(item_count, ITEM_COUNT as usize);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn noun_list_from_noun_rehomes_foreign_elements() {
        let mut source = make_test_stack(DEFAULT_STACK_SIZE);
        let source_space = source.noun_space();
        let foreign_elem = T(&mut source, &[D(1), D(2)]);
        let noun = make_noun_list_from_nouns(&mut source, &[foreign_elem]).into_noun(&mut source);

        let mut dest = make_test_stack(DEFAULT_STACK_SIZE);
        let decoded = NounList::from_noun(&mut dest, &noun, &source_space)
            .expect("decode noun list from foreign stack");
        unsafe { decoded.assert_in_stack(&dest) };

        let dest_space = dest.noun_space();
        let elem_ptr = decoded
            .into_iter()
            .next()
            .expect("decoded noun list element");
        let elem = unsafe { *elem_ptr };
        assert!(
            !unsafe { elem.raw_equals(&foreign_elem) },
            "decoded element should not retain the foreign pointer"
        );
        verify_noun_stack_allocated(elem, &dest_space, "decoded noun list element");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn how_to_noun() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let space = stack.noun_space();
        let tup: &[Noun] = &[D(0), D(1)];
        let cell = Cell::new_tuple(&mut stack, tup);
        let noun: Noun = cell.as_noun();
        let car = noun
            .cell()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .in_space(&space)
            .head()
            .noun()
            .direct()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .data();
        let cdr = noun
            .cell()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .in_space(&space)
            .tail()
            .noun()
            .direct()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .data();
        assert_eq!(car, 0);
        assert_eq!(cdr, 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn how_to_noun_but_listy() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let space = stack.noun_space();
        let tup: &[Noun] = &[D(0), D(1)];
        let cell = Cell::new_tuple(&mut stack, tup);
        let noun: Noun = cell.as_noun();
        let car = noun
            .cell()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .in_space(&space)
            .head()
            .noun()
            .direct()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .data();
        let cdr = noun
            .cell()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .in_space(&space)
            .tail()
            .noun()
            .direct()
            .unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .data();
        assert_eq!(car, 0);
        assert_eq!(cdr, 1);
    }

    /// Helper to recursively verify a noun is not stack-allocated
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

    /// Verifies NounList can be evacuated to PMA and remains functional.
    ///
    /// This test exercises:
    /// - Creating a NounList with multiple elements
    /// - Evacuating the NounList to PMA via copy_to_pma
    /// - Verifying all elements are still accessible after evacuation
    /// - Verifying all nouns are in offset form (not stack-allocated)
    /// - Verifying the NounList passes assert_in_pma
    ///
    /// Note: copy_to_pma sets forwarding pointers in the source nouns, which corrupts
    /// them for normal use. We use expected_values (raw u64s) for comparison since
    /// those aren't affected by forwarding pointers.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_evacuate_noun_list_round_trip() {
        use crate::pma::{test_pma_path, Pma, PmaCopy};

        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut pma =
            Pma::new(100000, test_pma_path("noun_list")).expect("Failed to create test PMA");
        let space = NounSpace::new(&stack, &pma);

        // The expected values - we use these for comparison since the source
        // nouns will have forwarding pointers set after evacuation
        let expected_values: Vec<u64> = vec![10, 20, 30, 40, 50];

        // Create a NounList with test data
        let mut noun_list = make_noun_list(&mut stack, &expected_values);

        // Count elements before evacuation
        let count_before: usize = noun_list.into_iter().count();
        assert_eq!(count_before, 5, "Should have 5 elements before evacuation");

        // Evacuate NounList to PMA
        unsafe {
            noun_list.copy_to_pma(&stack, &mut pma);
        }

        // Count elements and collect values after evacuation
        let mut values_after = Vec::new();
        for elem_ptr in noun_list {
            let elem = unsafe { *elem_ptr };
            values_after.push(unsafe { elem.as_raw() });
        }

        assert_eq!(
            values_after.len(),
            expected_values.len(),
            "Element count should be preserved"
        );
        assert_eq!(
            values_after, expected_values,
            "Element values should be preserved"
        );

        // Verify all nouns in the list are in offset form
        for elem_ptr in noun_list {
            let elem = unsafe { *elem_ptr };
            verify_noun_not_stack_allocated(elem, &space, "NounList element");
        }

        // Verify the NounList passes assert_in_pma
        noun_list.assert_in_pma(&pma);
    }

    /// Verifies NounList with complex nouns (Cells, IndirectAtoms) can be evacuated to PMA.
    ///
    /// This test exercises evacuation of NounList elements that are not direct atoms,
    /// ensuring that Cells and IndirectAtoms are correctly copied to the PMA.
    ///
    /// Note: copy_to_pma sets forwarding pointers in the source nouns, which corrupts
    /// them for normal use. We use a ref_stack to create reference copies for comparison.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_evacuate_noun_list_complex_nouns() {
        use crate::noun::{Cell, IndirectAtom};
        use crate::pma::{test_pma_path, Pma, PmaCopy};

        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut ref_stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut pma = Pma::new(100000, test_pma_path("noun_list_complex"))
            .expect("Failed to create test PMA");
        let space = NounSpace::new(&stack, &pma);

        // Create complex nouns on the main stack
        // Element 0: A cell [1 2]
        let cell1 = Cell::new(&mut stack, D(1), D(2)).as_noun();
        // Element 1: An indirect atom (larger than 63 bits)
        let big_data: [u64; 2] = [0xDEADBEEF_CAFEBABE, 0x12345678_9ABCDEF0];
        let indirect1 =
            unsafe { IndirectAtom::new_raw(&mut stack, 2, big_data.as_ptr()).as_noun() };
        // Element 2: A nested cell [[3 4] 5]
        let inner_cell = Cell::new(&mut stack, D(3), D(4)).as_noun();
        let nested_cell = Cell::new(&mut stack, inner_cell, D(5)).as_noun();
        // Element 3: A direct atom for variety
        let direct1 = D(42);
        // Element 4: A cell with structural sharing [[a b] [a b]] where a,b are IndirectAtoms
        let big_a: [u64; 2] = [0x1111111111111111, 0x2222222222222222];
        let big_b: [u64; 2] = [0x3333333333333333, 0x4444444444444444];
        let indirect_a = unsafe { IndirectAtom::new_raw(&mut stack, 2, big_a.as_ptr()).as_noun() };
        let indirect_b = unsafe { IndirectAtom::new_raw(&mut stack, 2, big_b.as_ptr()).as_noun() };
        let shared_cell = Cell::new(&mut stack, indirect_a, indirect_b).as_noun();
        let structural_sharing = Cell::new(&mut stack, shared_cell, shared_cell).as_noun();

        // Build the NounList manually with complex nouns
        let mut noun_list = NOUN_LIST_NIL;
        for noun in [direct1, nested_cell, indirect1, cell1, structural_sharing]
            .iter()
            .rev()
        {
            let mem: *mut NounListMem = unsafe { stack.alloc_struct(1) };
            unsafe {
                mem.write(NounListMem {
                    element: *noun,
                    next: noun_list,
                });
            }
            noun_list = NounList(mem);
        }

        // Create reference copies on ref_stack for comparison after evacuation
        let ref_cell1 = Cell::new(&mut ref_stack, D(1), D(2)).as_noun();
        let ref_indirect1 =
            unsafe { IndirectAtom::new_raw(&mut ref_stack, 2, big_data.as_ptr()).as_noun() };
        let ref_inner_cell = Cell::new(&mut ref_stack, D(3), D(4)).as_noun();
        let ref_nested_cell = Cell::new(&mut ref_stack, ref_inner_cell, D(5)).as_noun();
        let ref_direct1 = D(42);
        let ref_indirect_a =
            unsafe { IndirectAtom::new_raw(&mut ref_stack, 2, big_a.as_ptr()).as_noun() };
        let ref_indirect_b =
            unsafe { IndirectAtom::new_raw(&mut ref_stack, 2, big_b.as_ptr()).as_noun() };
        let ref_shared_cell = Cell::new(&mut ref_stack, ref_indirect_a, ref_indirect_b).as_noun();
        let ref_structural_sharing =
            Cell::new(&mut ref_stack, ref_shared_cell, ref_shared_cell).as_noun();
        // Order must match iteration order of noun_list: direct1, nested_cell, indirect1, cell1, structural_sharing
        let ref_nouns =
            [ref_direct1, ref_nested_cell, ref_indirect1, ref_cell1, ref_structural_sharing];

        // Count elements before evacuation
        let count_before: usize = noun_list.into_iter().count();
        assert_eq!(count_before, 5, "Should have 5 elements before evacuation");

        // Evacuate NounList to PMA
        unsafe {
            noun_list.copy_to_pma(&stack, &mut pma);
        }

        // Verify element count after evacuation
        let count_after: usize = noun_list.into_iter().count();
        assert_eq!(count_after, 5, "Should have 5 elements after evacuation");

        // Verify elements match reference copies using unifying_equality
        let ref_space = NounSpace::new(&ref_stack, &pma);
        for (i, elem_ptr) in noun_list.into_iter().enumerate() {
            let elem = unsafe { *elem_ptr };
            let ref_noun = ref_nouns[i];
            assert!(
                noun_equality(ref_noun.in_space(&ref_space), elem.in_space(&ref_space),),
                "Element {} should match reference after evacuation",
                i
            );
        }

        // Verify all nouns in the list are in offset form
        for elem_ptr in noun_list {
            let elem = unsafe { *elem_ptr };
            verify_noun_not_stack_allocated(elem, &space, "NounList complex element");
        }

        // Verify the NounList passes assert_in_pma
        noun_list.assert_in_pma(&pma);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_copy_from_pma_does_not_mutate_source_noun_list() {
        use crate::noun::Cell;
        use crate::pma::{test_pma_path, Pma, PmaCopy, PmaCopyFrom};

        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut from_pma = Pma::new(100000, test_pma_path("noun_list_copy_from_source"))
            .expect("Failed to create source PMA");
        let mut to_pma = Pma::new(100000, test_pma_path("noun_list_copy_from_dest"))
            .expect("Failed to create destination PMA");

        // Deliberately shift destination offsets so source mutation cannot hide
        // behind matching allocation offsets in the two slabs.
        unsafe {
            to_pma.alloc_struct::<u64>(17);
        }

        let cell = Cell::new(&mut stack, D(1), D(2)).as_noun();
        let mem: *mut NounListMem = unsafe { stack.alloc_struct(1) };
        unsafe {
            mem.write(NounListMem {
                element: cell,
                next: NOUN_LIST_NIL,
            });
        }
        let mut source = NounList(mem);
        unsafe {
            source.copy_to_pma(&stack, &mut from_pma);
        }

        let source_element_before = unsafe { (*source.0).element.as_raw() };
        let mut copied = source;
        unsafe {
            copied.copy_from_pma(&from_pma, &mut to_pma);
        }
        let source_element_after = unsafe { (*source.0).element.as_raw() };

        assert_eq!(
            source_element_after, source_element_before,
            "copy_from_pma must not rewrite nouns stored in the source PMA"
        );
    }

    /// Verifies Batteries can be evacuated to PMA and remains functional.
    ///
    /// This test exercises:
    /// - Creating a Batteries linked list with multiple entries
    /// - Evacuating the Batteries to PMA via copy_to_pma
    /// - Verifying all entries are still accessible after evacuation
    /// - Verifying all nouns are in offset form (not stack-allocated)
    /// - Verifying the Batteries passes assert_in_pma
    ///
    /// Note: copy_to_pma sets forwarding pointers in the source nouns, which corrupts
    /// them for normal use. We use expected values for comparison.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_evacuate_batteries_round_trip() {
        use crate::pma::{test_pma_path, Pma, PmaCopy};

        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut pma =
            Pma::new(100000, test_pma_path("batteries")).expect("Failed to create test PMA");
        let space = NounSpace::new(&stack, &pma);

        // Create a Batteries list using the test helper
        // This creates: [battery=D(2), axis=D(3)] -> [battery=D(0), axis=D(1)] -> NIL
        let mut batteries = make_batteries(&mut stack);

        // Expected values (battery, parent_axis) in iteration order
        let expected_values: Vec<(u64, u64)> = vec![(2, 3), (0, 1)];

        // Evacuate Batteries to PMA
        unsafe {
            batteries.copy_to_pma(&stack, &mut pma);
        }

        // Iterate over evacuated batteries and verify values, count, and offset form
        let mut expected_iter = expected_values.iter();
        for (battery_ptr, parent_axis) in batteries {
            let (expected_battery, expected_axis) = expected_iter
                .next()
                .expect("Batteries has more entries than expected");

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
                *expected_axis,
                "Parent axis should match"
            );

            // Verify nouns are in offset form
            verify_noun_not_stack_allocated(battery, &space, "Batteries battery");
            verify_noun_not_stack_allocated(parent_axis.as_noun(), &space, "Batteries parent_axis");
        }
        assert!(
            expected_iter.next().is_none(),
            "Batteries has fewer entries than expected"
        );

        // Verify the Batteries passes assert_in_pma
        batteries.assert_in_pma(&pma);
    }

    /// Verifies BatteriesList can be evacuated to PMA and remains functional.
    ///
    /// This test exercises:
    /// - Creating a BatteriesList with multiple Batteries entries
    /// - Evacuating the BatteriesList to PMA via copy_to_pma
    /// - Verifying all entries are still accessible after evacuation
    /// - Verifying all nouns are in offset form (not stack-allocated)
    /// - Verifying the BatteriesList passes assert_in_pma
    ///
    /// Note: copy_to_pma sets forwarding pointers in the source nouns, which corrupts
    /// them for normal use. We use expected values for comparison.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_evacuate_batteries_list_round_trip() {
        use crate::pma::{test_pma_path, Pma, PmaCopy};

        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut pma =
            Pma::new(100000, test_pma_path("batteries_list")).expect("Failed to create test PMA");
        let space = NounSpace::new(&stack, &pma);

        // Create a BatteriesList using the test helper
        // make_batteries_list(&[7, 8]) creates a list with two Batteries entries,
        // each with a single battery noun (D(7) and D(8))
        let mut batteries_list = make_batteries_list(&mut stack, &[7, 8]);

        // Expected battery values in iteration order
        let expected_batteries: Vec<u64> = vec![7, 8];

        // Evacuate BatteriesList to PMA
        unsafe {
            batteries_list.copy_to_pma(&stack, &mut pma);
        }

        // Iterate over evacuated batteries_list and verify values, count, and offset form
        let mut expected_iter = expected_batteries.iter();
        for batteries in batteries_list {
            let expected_battery = expected_iter
                .next()
                .expect("BatteriesList has more entries than expected");

            // Each Batteries in this test has a single entry
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
            verify_noun_not_stack_allocated(battery, &space, "BatteriesList battery");
            verify_noun_not_stack_allocated(
                parent_axis.as_noun(),
                &space,
                "BatteriesList parent_axis",
            );

            // Verify no more entries in this Batteries
            assert!(
                batteries_iter.next().is_none(),
                "Batteries should have exactly one entry"
            );
        }
        assert!(
            expected_iter.next().is_none(),
            "BatteriesList has fewer entries than expected"
        );

        // Verify the BatteriesList passes assert_in_pma
        batteries_list.assert_in_pma(&pma);
    }

    /// Verifies Cold jet state can be evacuated to PMA and remains functional.
    ///
    /// This test exercises:
    /// - Creating a Cold state with populated HAMTs (battery_to_paths, root_to_paths, path_to_batteries)
    /// - Evacuating the entire Cold structure to PMA via copy_to_pma
    /// - Verifying all three HAMTs are accessible after evacuation
    /// - Verifying all internal nouns are in offset form (not stack-allocated)
    /// - Verifying the Cold structure passes assert_in_pma
    ///
    /// Cold is a critical component of the jet matching system, storing mappings between:
    /// - Batteries (code) -> paths (registered names/hierarchies)
    /// - Roots (base cores) -> paths
    /// - Paths -> battery lists (for matching cores to jets)
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_evacuate_cold_round_trip() {
        use crate::pma::{test_pma_path, Pma, PmaCopy};

        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let mut pma = Pma::new(100000, test_pma_path("cold")).expect("Failed to create test PMA");
        let space = NounSpace::new(&stack, &pma);

        // Create a Cold state using make_cold_state
        let mut cold = make_cold_state(&mut stack);

        // Count entries before evacuation
        let count_battery_to_paths_before: usize =
            unsafe { (*cold.0).battery_to_paths.iter().map(|e| e.len()).sum() };
        let count_root_to_paths_before: usize =
            unsafe { (*cold.0).root_to_paths.iter().map(|e| e.len()).sum() };
        let count_path_to_batteries_before: usize =
            unsafe { (*cold.0).path_to_batteries.iter().map(|e| e.len()).sum() };

        assert_eq!(
            count_battery_to_paths_before, 1,
            "Should have 1 battery_to_paths entry"
        );
        assert_eq!(
            count_root_to_paths_before, 2,
            "Should have 2 root_to_paths entries"
        );
        assert_eq!(
            count_path_to_batteries_before, 1,
            "Should have 1 path_to_batteries entry"
        );

        // Evacuate Cold to PMA
        unsafe {
            cold.copy_to_pma(&stack, &mut pma);
        }

        // Verify entry counts are preserved after evacuation
        let count_battery_to_paths_after: usize =
            unsafe { (*cold.0).battery_to_paths.iter().map(|e| e.len()).sum() };
        let count_root_to_paths_after: usize =
            unsafe { (*cold.0).root_to_paths.iter().map(|e| e.len()).sum() };
        let count_path_to_batteries_after: usize =
            unsafe { (*cold.0).path_to_batteries.iter().map(|e| e.len()).sum() };

        assert_eq!(
            count_battery_to_paths_after, count_battery_to_paths_before,
            "battery_to_paths entry count should be preserved"
        );
        assert_eq!(
            count_root_to_paths_after, count_root_to_paths_before,
            "root_to_paths entry count should be preserved"
        );
        assert_eq!(
            count_path_to_batteries_after, count_path_to_batteries_before,
            "path_to_batteries entry count should be preserved"
        );

        // Verify lookups still work after evacuation
        // Note: We use fresh D(x) atoms for lookup since the original keys
        // may have forwarding pointers set during evacuation
        let lookup_battery = unsafe { (*cold.0).battery_to_paths.lookup(&mut stack, &mut D(200)) };
        assert!(
            lookup_battery.is_some(),
            "battery_to_paths lookup for D(200) should succeed after evacuation"
        );

        let lookup_root1 = unsafe { (*cold.0).root_to_paths.lookup(&mut stack, &mut D(100)) };
        assert!(
            lookup_root1.is_some(),
            "root_to_paths lookup for D(100) should succeed after evacuation"
        );

        let lookup_root2 = unsafe { (*cold.0).root_to_paths.lookup(&mut stack, &mut D(101)) };
        assert!(
            lookup_root2.is_some(),
            "root_to_paths lookup for D(101) should succeed after evacuation"
        );

        let lookup_path = unsafe { (*cold.0).path_to_batteries.lookup(&mut stack, &mut D(300)) };
        assert!(
            lookup_path.is_some(),
            "path_to_batteries lookup for D(300) should succeed after evacuation"
        );

        // Verify all nouns in the Cold HAMTs are in offset form
        // Check battery_to_paths
        for entries in unsafe { (*cold.0).battery_to_paths.iter() } {
            for (key, noun_list) in entries {
                verify_noun_not_stack_allocated(*key, &space, "battery_to_paths key");
                // Verify NounList elements
                for elem_ptr in *noun_list {
                    let elem = unsafe { *elem_ptr };
                    verify_noun_not_stack_allocated(
                        elem, &space, "battery_to_paths NounList element",
                    );
                }
            }
        }

        // Check root_to_paths
        for entries in unsafe { (*cold.0).root_to_paths.iter() } {
            for (key, noun_list) in entries {
                verify_noun_not_stack_allocated(*key, &space, "root_to_paths key");
                for elem_ptr in *noun_list {
                    let elem = unsafe { *elem_ptr };
                    verify_noun_not_stack_allocated(elem, &space, "root_to_paths NounList element");
                }
            }
        }

        // Check path_to_batteries
        for entries in unsafe { (*cold.0).path_to_batteries.iter() } {
            for (key, batteries_list) in entries {
                verify_noun_not_stack_allocated(*key, &space, "path_to_batteries key");
                // Verify BatteriesList elements
                for batteries in *batteries_list {
                    for (battery_ptr, _parent_axis) in batteries {
                        let battery = unsafe { *battery_ptr };
                        verify_noun_not_stack_allocated(
                            battery, &space, "path_to_batteries battery",
                        );
                    }
                }
            }
        }

        // Verify the Cold structure passes assert_in_pma
        cold.assert_in_pma(&pma);
    }
}
