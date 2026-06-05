use nockvm::jets::sort::util::gor;
use nockvm::mem::NockStack;
use nockvm::noun::{Cell, CellHandle, Noun, NounHandle, NounSpace};
use nockvm::unifying_equality::unifying_equality;

use crate::noun_ext::NounMathExt;

#[derive(Copy, Clone)]
pub struct HoonList<'a> {
    pub(super) next: Option<Noun>,
    pub(super) space: &'a NounSpace,
}

impl<'a> Iterator for HoonList<'a> {
    type Item = Noun;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        self.next.take().map(|noun| {
            let cell = noun.in_space(self.space).as_cell().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let tail = cell.tail().noun();
            self.next = if tail.is_cell() { Some(tail) } else { None };
            cell.head().noun()
        })
    }
}

impl<'a> HoonList<'a> {
    pub fn try_from(
        n: Noun,
        space: &'a NounSpace,
    ) -> core::result::Result<Self, nockvm::noun::Error> {
        if n.is_cell() {
            let _ = n.in_space(space).as_cell().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            Ok(HoonList {
                next: Some(n),
                space,
            })
        } else {
            Ok(HoonList { next: None, space })
        }
    }

    pub fn from_cell(c: Cell, space: &'a NounSpace) -> Self {
        Self {
            next: Some(c.as_noun()),
            space,
        }
    }

    pub fn space(&self) -> &'a NounSpace {
        self.space
    }
}

pub fn next_cell(cell: Cell, space: &NounSpace) -> Option<Cell> {
    let cell_handle = CellHandle::new(cell, space);
    let tail = cell_handle.tail().noun();
    if tail.is_cell() {
        let handle = tail.in_space(space).as_cell().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        Some(handle.cell())
    } else {
        None
    }
}

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub struct HoonMap<'a> {
    pub(super) node: Noun,
    pub(super) left: Option<Noun>,
    pub(super) right: Option<Noun>,
    pub(super) space: &'a NounSpace,
}

impl<'a> HoonMap<'a> {
    pub fn get(&self, stack: &mut NockStack, mut k: Noun) -> Option<Noun> {
        let [mut ck, cv] = self.node.uncell(self.space).ok()?;

        if unsafe { unifying_equality(stack, &mut ck, &mut k) } {
            // ?:  =(b p.n.a)
            //   (some q.n.a)
            Some(cv)
        } else if gor(stack, k, ck, self.space).as_direct().map(|v| v.data()) == Ok(0) {
            // ?:  (gor b p.n.a)
            //   $(a l.a)
            let map = Self::try_from(self.left?.in_space(self.space)).ok()?;
            map.get(stack, k)
        } else {
            // $(a r.a)
            let map = Self::try_from(self.right?.in_space(self.space)).ok()?;
            map.get(stack, k)
        }
    }

    pub fn try_from(n: NounHandle<'a>) -> std::result::Result<Self, nockvm::noun::Error> {
        if n.is_cell() {
            let cell = n.as_cell().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            HoonMap::try_from_cell(cell)
        } else {
            not_cell()
        }
    }

    pub fn try_from_cell(c: CellHandle<'a>) -> std::result::Result<Self, nockvm::noun::Error> {
        let tail = c.tail();
        if let Ok(cell_tail) = tail.as_cell() {
            let left = cell_tail.head().noun();
            let right = cell_tail.tail().noun();

            Ok(Self {
                node: c.head().noun(),
                left: left.is_cell().then_some(left),
                right: right.is_cell().then_some(right),
                space: c.space(),
            })
        } else {
            not_cell()
        }
    }
}
#[allow(dead_code)]
#[derive(Clone)]
pub struct HoonMapIter<'a> {
    pub(super) stack: Vec<Option<NounHandle<'a>>>,
    pub(super) space: &'a NounSpace,
}

impl<'a> Iterator for HoonMapIter<'a> {
    type Item = NounHandle<'a>;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(maybe_cell) = self.stack.pop() {
            if let Some(noun) = maybe_cell {
                if let Ok(cell_trie) = HoonMap::try_from(noun) {
                    self.stack
                        .push(cell_trie.right.map(|n| n.in_space(self.space)));
                    self.stack
                        .push(cell_trie.left.map(|n| n.in_space(self.space)));
                    return Some(cell_trie.node.in_space(self.space));
                } else {
                    return self.next();
                }
            } else {
                return self.next();
            }
        }
        None
    }
}
fn not_cell<T>() -> core::result::Result<T, nockvm::noun::Error> {
    Err(nockvm::noun::Error::NotCell)
}

impl<'a> HoonMapIter<'a> {
    pub fn new(n: &'a NounHandle) -> Self {
        if n.is_cell() {
            Self {
                stack: vec![Some(*n)],
                space: n.space(),
            }
        } else {
            Self {
                stack: vec![None],
                space: n.space(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nockvm::noun::{AllocLocation, NounRepr, D, T};
    use nockvm::pma::{Pma, PmaCopy};
    use noun_serde::NounEncode;

    use super::*;
    use crate::zoon::zmap::ZMap;

    fn test_pma_path(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let mut path = std::env::temp_dir();
        path.push(format!("nockchain_math_{label}_{pid}_{id}.mmap"));
        path
    }

    fn test_pma(label: &str) -> Pma {
        Pma::new(100000, test_pma_path(label)).expect("failed to create test PMA")
    }

    #[test]
    #[cfg_attr(miri, ignore = "file-backed PMA unsupported in Miri")]
    fn hoon_list_reads_pma_backed_cells() {
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma("hoon_list");

        let first = T(&mut stack, &[D(1), D(2)]);
        let second = T(&mut stack, &[D(3), D(4)]);
        let tail = T(&mut stack, &[second, D(0)]);
        let mut list = T(&mut stack, &[first, tail]);

        unsafe {
            list.copy_to_pma(&stack, &mut pma);
        }

        let space = NounSpace::pma_only(&pma);
        let items: Vec<Noun> = HoonList::try_from(list, &space)
            .expect("PMA-backed list should decode")
            .collect();

        assert_eq!(items.len(), 2, "list length should be preserved");
        assert_eq!(
            items[0].in_space(&space).repr(),
            NounRepr::Cell(AllocLocation::PmaOffset),
            "first element should remain a PMA offset-form cell"
        );
        assert_eq!(
            items[1].in_space(&space).repr(),
            NounRepr::Cell(AllocLocation::PmaOffset),
            "second element should remain a PMA offset-form cell"
        );

        let first = items[0]
            .in_space(&space)
            .as_cell()
            .expect("first item should be a cell");
        assert_eq!(
            first
                .head()
                .as_atom()
                .expect("first head should be an atom")
                .as_u64()
                .expect("first head should fit in u64"),
            1
        );
        assert_eq!(
            first
                .tail()
                .as_atom()
                .expect("first tail should be an atom")
                .as_u64()
                .expect("first tail should fit in u64"),
            2
        );

        let second = items[1]
            .in_space(&space)
            .as_cell()
            .expect("second item should be a cell");
        assert_eq!(
            second
                .head()
                .as_atom()
                .expect("second head should be an atom")
                .as_u64()
                .expect("second head should fit in u64"),
            3
        );
        assert_eq!(
            second
                .tail()
                .as_atom()
                .expect("second tail should be an atom")
                .as_u64()
                .expect("second tail should fit in u64"),
            4
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "file-backed PMA unsupported in Miri")]
    fn hoon_map_get_and_iter_work_for_pma_backed_maps() {
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let map = ZMap::try_from_entries(vec![(7u64, 70u64), (3u64, 30u64), (11u64, 110u64)])
            .expect("owned z-map should build");
        let mut map_noun = map.to_noun(&mut stack);
        let mut pma = test_pma("hoon_map");

        unsafe {
            map_noun.copy_to_pma(&stack, &mut pma);
        }

        let space = NounSpace::pma_only(&pma);
        let hoon_map =
            HoonMap::try_from(map_noun.in_space(&space)).expect("PMA-backed map should decode");

        let mut lookup_stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let value = hoon_map
            .get(&mut lookup_stack, D(11))
            .expect("existing key should be found");
        assert_eq!(
            value
                .in_space(&space)
                .as_atom()
                .expect("map lookup value should be an atom")
                .as_u64()
                .expect("map lookup value should fit in u64"),
            110
        );
        assert!(
            hoon_map.get(&mut lookup_stack, D(99)).is_none(),
            "missing key should not resolve"
        );

        let map_handle = map_noun.in_space(&space);
        let mut entries: Vec<(u64, u64)> = HoonMapIter::new(&map_handle)
            .map(|entry| {
                let pair = entry
                    .as_cell()
                    .expect("map iterator should yield entry pairs");
                let key = pair
                    .head()
                    .as_atom()
                    .expect("map iterator key should be an atom")
                    .as_u64()
                    .expect("map iterator key should fit in u64");
                let value = pair
                    .tail()
                    .as_atom()
                    .expect("map iterator value should be an atom")
                    .as_u64()
                    .expect("map iterator value should fit in u64");
                (key, value)
            })
            .collect();
        entries.sort_unstable();

        assert_eq!(entries, vec![(3, 30), (7, 70), (11, 110)]);
    }
}
