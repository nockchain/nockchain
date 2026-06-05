use std::ptr::{copy_nonoverlapping, null_mut};

use crate::hamt::Hamt;
use crate::mem::{self, NockStack, Preserve};
use crate::noun::{self, Atom, DirectAtom, IndirectAtom, Noun, NounAllocator, Slots, D, T};
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

const NO_BATTERIES: Batteries = Batteries(null_mut());

#[derive(Copy, Clone)]
struct BatteriesMem {
    battery: Noun,
    parent_axis: Atom,
    parent_batteries: Batteries,
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
    pub fn matches(self, stack: &mut NockStack, mut core: Noun) -> bool {
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
            if let Ok(mut core_battery) = core.slot(2) {
                if unsafe { !unifying_equality(stack, &mut core_battery, battery) } {
                    return false;
                };
                if let Ok(core_parent) = core.slot_atom(parent_axis) {
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
    fn matches(mut self, stack: &mut NockStack, core: Noun) -> Option<Batteries> {
        self.find(|&batteries| batteries.matches(stack, core))
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
        let mut battery = (*core).slot(2).ok()?;
        unsafe {
            let paths = (*(self.0)).battery_to_paths.lookup(stack, &mut battery)?;
            for path in paths {
                if let Some(batteries_list) =
                    (*(self.0)).path_to_batteries.lookup(stack, &mut (*path))
                {
                    if let Some(_batt) = batteries_list.matches(stack, *core) {
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

            let mut battery = core.slot(2)?;
            let mut parent = core.slot_atom(parent_axis)?;
            // Check if we already registered this core
            if let Some(paths) = (*(self.0)).battery_to_paths.lookup(stack, &mut battery) {
                for path in paths {
                    if let Ok(path_cell) = (*path).as_cell() {
                        if unifying_equality(stack, &mut path_cell.head(), &mut chum) {
                            if let Some(batteries_list) =
                                (*(self.0)).path_to_batteries.lookup(stack, &mut *path)
                            {
                                if let Some(_batteries) = batteries_list.matches(stack, core) {
                                    return Ok(false);
                                }
                            }
                        }
                    }
                }
            }

            let mut parent_battery = parent.slot(2)?;

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
                    if let Some(parent_batteries) = battery_list.matches(stack, parent) {
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
                    if let Some(parent_batteries) = battery_list.matches(stack, parent) {
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

pub struct NounListIterator(Noun);

impl Iterator for NounListIterator {
    type Item = Noun;
    fn next(&mut self) -> Option<Self::Item> {
        if let Ok(it) = self.0.as_cell() {
            self.0 = it.tail();
            Some(it.head())
        } else if unsafe { self.0.raw_equals(&D(0)) } {
            None
        } else {
            panic!("Improper list terminator: {:?}", self.0)
        }
    }
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
    fn from_noun<A: NounAllocator>(stack: &mut A, noun: &Noun) -> NounableResult<Self::Target>
    where
        Self: Sized;
}

impl Nounable for Atom {
    type Target = Self;

    fn into_noun<A: NounAllocator>(self, _stack: &mut A) -> Noun {
        self.as_noun()
    }
    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        noun.atom().ok_or(FromNounError::NotAtom)
    }
}

impl Nounable for u64 {
    type Target = Self;
    fn into_noun<A: NounAllocator>(self, _stack: &mut A) -> Noun {
        // Copied from Crown's IntoNoun, not sure why this isn't D(*self)
        unsafe { Atom::from_raw(self).into_noun(_stack) }
    }
    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let atom = noun.atom().ok_or(FromNounError::NotAtom)?;
        let as_u64 = atom.as_u64()?;
        Ok(as_u64)
    }
}

impl Nounable for Noun {
    type Target = Self;
    fn into_noun<A: NounAllocator>(self, _stack: &mut A) -> Noun {
        self
    }

    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Self) -> NounableResult<Self::Target> {
        Ok(*noun)
    }
}

impl Nounable for &str {
    type Target = String;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let contents_atom = unsafe {
            let bytes = self.bytes().collect::<Vec<u8>>();
            IndirectAtom::new_raw_bytes_ref(stack, bytes.as_slice()).normalize_as_atom()
        };
        contents_atom.into_noun(stack)
    }
    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let atom = noun.as_atom()?;
        let bytes = atom.as_ne_bytes();
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

    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let mut items: Vec<<T as Nounable>::Target> = vec![];
        for item in NounListIterator(*noun) {
            let item = T::from_noun(_stack, &item)?;
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

    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        // it's a three tuple now
        let cell = noun.cell().ok_or(FromNounError::NotCell)?;
        let head = cell.head();
        let tail = cell.tail();
        let a = T::from_noun(_stack, &head)?;
        let cell = tail.as_cell()?;
        let b = U::from_noun(_stack, &cell.head())?;
        let c = V::from_noun(_stack, &cell.tail())?;
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

    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let cell = noun.cell().ok_or(FromNounError::NotCell)?;
        let head = cell.head();
        let tail = cell.tail();
        let a = T::from_noun(_stack, &head)?;
        let b = U::from_noun(_stack, &tail)?;
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

    fn from_noun<A: NounAllocator>(stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let mut result = NOUN_LIST_NIL;
        for item in NounListIterator(*noun) {
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

    fn from_noun<A: NounAllocator>(stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let mut batteries = NO_BATTERIES;
        for item in NounListIterator(*noun) {
            let cell = item.cell().ok_or(FromNounError::NotCell)?;
            let battery = cell.head();
            let parent_axis = cell.tail().as_atom()?;
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

    fn from_noun<A: NounAllocator>(stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let mut batteries_list = BATTERIES_LIST_NIL;
        for item in NounListIterator(*noun) {
            let batteries = Batteries::from_noun(stack, &item)?;
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

    fn from_noun<A: NounAllocator>(stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let mut items = Vec::new();
        for item in NounListIterator(*noun) {
            let cell = item.cell().ok_or(FromNounError::NotCell)?;
            let key = cell.head();
            let value = T::from_noun(stack, &cell.tail())?;
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

    fn from_noun<A: NounAllocator>(stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let mut battery_to_paths = Vec::new();
        let mut root_to_paths = Vec::new();
        let mut path_to_batteries = Vec::new();

        let battery_to_paths_noun = noun.slot(2)?;
        let root_to_paths_noun = noun.slot(6)?;
        let path_to_batteries_noun = noun.slot(7)?;

        // iterate over battery_to_paths_noun
        for item in NounListIterator(battery_to_paths_noun) {
            let cell = item.cell().ok_or(FromNounError::NotCell)?;
            let key = cell.head();
            let value = NounList::from_noun(stack, &cell.tail())?;
            battery_to_paths.push((key, value));
        }

        // iterate over root_to_paths_noun
        for item in NounListIterator(root_to_paths_noun) {
            let cell = item.cell().ok_or(FromNounError::NotCell)?;
            let key = cell.head();
            let value = NounList::from_noun(stack, &cell.tail())?;
            root_to_paths.push((key, value));
        }

        // iterate over path_to_batteries_noun
        for item in NounListIterator(path_to_batteries_noun) {
            let cell = item.cell().ok_or(FromNounError::NotCell)?;
            let key = cell.head();
            let value = BatteriesList::from_noun(stack, &cell.tail())?;
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
    use crate::hamt::Hamt;
    use crate::mem::NockStack;
    use crate::noun::{Cell, Noun, D};
    /// Default stack size for tests where you aren't intending to run out of space
    pub(crate) const DEFAULT_STACK_SIZE: usize = 1 << 27;
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
        let cold = make_cold_state(&mut stack);
        let cold_noun = cold.into_noun(&mut stack);
        let new_cold =
            Cold::from_noun(&mut stack, &cold_noun).expect("Failed to convert noun to cold");

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
        let items = vec![(D(0), D(1)), (D(2), D(3))];
        let hamt = super::hamt_from_vec(&mut stack, items);
        let noun = hamt.into_noun(&mut stack);
        let new_hamt: Vec<(Noun, Noun)> = <Hamt<Noun> as Nounable>::from_noun::<NockStack>(
            &mut stack, &noun,
        )
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
        let batteries_list2 = make_batteries_list(&mut stack, &[1, 2]);
        let batteries_list_noun = batteries_list2.into_noun(&mut stack);
        let new_batteries_list2 = BatteriesList::from_noun(&mut stack, &batteries_list_noun)
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
        let batteries2 = make_batteries(&mut stack);
        let batteries_noun = batteries2.into_noun(&mut stack);
        let new_batteries = Batteries::from_noun(&mut stack, &batteries_noun)
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
                a_atom.as_u64(),
                b_atom.as_u64()
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn tuple_bidirectional_conversion() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
        let tup = (D(1), D(2), D(3));
        let noun = tup.into_noun(&mut stack);
        let new_tup: (Noun, Noun, Noun) =
            <(Noun, Noun, Noun) as Nounable>::from_noun::<NockStack>(&mut stack, &noun)
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
        let new_noun_list: NounList = <NounList as Nounable>::from_noun::<NockStack>(
            &mut stack, &noun,
        )
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
    fn how_to_noun() {
        let mut stack = make_test_stack(DEFAULT_STACK_SIZE);
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
            .head()
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
            .tail()
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
            .head()
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
            .tail()
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
}
