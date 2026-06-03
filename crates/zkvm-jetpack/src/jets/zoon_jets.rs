//! Zoon and h-zoon jets, including the balance state-migration fast path.
//!
//! The slow consensus migration is not ordinary z-map to h-map conversion. The
//! balance field is a z-mip whose outer key is a block id and whose value is a
//! full note-balance z-map snapshot for that block:
//!
//! ```text
//! blocks:   block-id -> page(parent-id, ...)
//! balance:  block-id -> full note-balance snapshot
//! ```
//!
//! The Hoon arm `+zh-balmilt` still means `(zh-milt balance)`. That simple
//! fallback is the protocol oracle. The Rust jet gets `blocks` only as an
//! evaluation hint, then uses it to recover parent order across snapshots.
//!
//! The speedup comes from exploiting persistent z-map structure. Adjacent blocks
//! usually spend or create a few notes, while the child snapshot shares most of
//! its source tree cells with the parent snapshot. Generic `zh-milt` cannot see
//! those parent links, which makes it repeatedly walk nearly identical full
//! balance maps. The balance jet converts parents first, memoizes exact
//! repeated z-map roots by noun identity, and converts a child from the
//! already-converted parent by collecting z-map deltas.
//!
//! Delta conversion is deliberately conservative:
//!
//! - identical z-map roots reuse the parent h-map;
//! - matching treap nodes recurse only through changed subtrees;
//! - small local rotations flatten and compare by compact hashed key;
//! - duplicate compact keys, malformed trees, missing parents, cycles, and large
//!   rotation regions bail to generic conversion;
//! - deletes run before puts, preserving changes where two key nouns have the
//!   same compact digest.
//!
//! The fast path is permitted to reduce work only. Outer entries are rebuilt
//! with the original key nouns, snapshot-level fallback still shares the generic
//! memo, and test-jet mode compares the Rust noun against the Hoon oracle.

use std::cmp::Ordering;
use std::collections::HashMap;

use nockchain_math::belt::Belt;
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::tip5::hash::{hash_10, hash_noun_varlen_digest};
use nockchain_math::zoon::common::{dor_tip, lth_tip};
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Cell, CellMemory, Noun, NounAllocator, NounSpace, D, NO, T, YES};
use nockvm_macros::tas;
use noun_serde::NounDecode;
use rayon::slice::ParallelSliceMut;

use crate::jets::tip5_jets::digest_to_noundigest;

const TIP_CACHE_TAG: u64 = tas!(b"zntip");
const DOUBLE_TIP_CACHE_TAG: u64 = tas!(b"zndtip");
const PARALLEL_SORT_THRESHOLD: usize = 4096;
const SMALL_HASHED_KEY_LIMIT: usize = 2;
const Z_DIFF_ROTATION_ENTRY_LIMIT: usize = 512;

pub fn dor_tip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let mut a = slot(sam, 2, &space)?;
    let mut b = slot(sam, 3, &space)?;

    Ok(bool_to_noun(dor_tip(&mut context.stack, &mut a, &mut b)?))
}

pub fn gor_tip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let mut a = slot(sam, 2, &space)?;
    let mut b = slot(sam, 3, &space)?;

    let a_tip = get_tip_digest(context, a)?;
    let b_tip = get_tip_digest(context, b)?;

    let ordered = if a_tip == b_tip {
        dor_tip(&mut context.stack, &mut a, &mut b)?
    } else {
        lth_tip(&a_tip, &b_tip)
    };

    Ok(bool_to_noun(ordered))
}

pub fn mor_tip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let mut a = slot(sam, 2, &space)?;
    let mut b = slot(sam, 3, &space)?;

    let a_tip = get_double_tip_digest(context, a)?;
    let b_tip = get_double_tip_digest(context, b)?;

    let ordered = if a_tip == b_tip {
        dor_tip(&mut context.stack, &mut a, &mut b)?
    } else {
        lth_tip(&a_tip, &b_tip)
    };

    Ok(bool_to_noun(ordered))
}

/// Jet for `+gor-hip`, the h-zoon key ordering over existing digest limbs.
///
/// Keys are decoded as a digest list. A direct digest is a one-item list. Each
/// digest compares limbs `[4, 3, 2, 1, 0]`; equal digests continue to the next
/// list item. Equal lists return false, and an equal-prefix shorter list loses.
/// There is no `dor` fallback because h-zoon keys are already consensus digest
/// commitments.
pub fn gor_hip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    Ok(bool_to_noun(gor_hip(a, b, &space)?))
}

/// Jet for `+mor-hip`, the h-zoon priority ordering over existing digest limbs.
///
/// This uses the same digest-list rules as `+gor-hip`, but compares digest
/// limbs `[0, 1, 2, 3, 4]`. That is the priority order used to keep the h-tree
/// balanced without hashing the key noun again.
pub fn mor_hip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    Ok(bool_to_noun(mor_hip(a, b, &space)?))
}

/// Jet for `+zh-molt`, converting a legacy z-map tree into an h-map tree.
pub fn zh_molt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let mut memo = HashMap::new();
    let space = context.stack.noun_space();
    let a = slot(subject, 6, &space)?;
    z_map_to_h_map(&mut context.stack, a, &space, &mut memo)
}

/// Jet for `+zh-silt`, converting a legacy z-set tree into an h-set tree.
pub fn zh_silt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let mut memo = HashMap::new();
    let space = context.stack.noun_space();
    let a = slot(subject, 6, &space)?;
    z_set_to_h_set(&mut context.stack, a, &space, &mut memo)
}

/// Jet for `+zh-milt`, converting a legacy z-mip tree into an h-mip tree.
pub fn zh_milt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let mut memo = ZHConversionMemo::default();
    let space = context.stack.noun_space();
    let a = slot(subject, 6, &space)?;
    z_mip_to_h_mip(&mut context.stack, a, &space, &mut memo)
}

/// Jet for `+zh-balmilt`, the balance-specific state migration converter.
///
/// This arm exists because balance history has a shape the generic converter
/// cannot see:
///
/// ```text
/// blocks:   block-id -> page(parent-id, ...)
/// balance:  block-id -> full note-balance snapshot
/// ```
///
/// Each balance snapshot is a complete z-map of spendable notes at that block.
/// Adjacent blocks usually change only a few notes, and the source z-maps share
/// most of their underlying tree cells. Generic `zh-milt` is still perfectly
/// correct, but it only receives `balance`; it does not know which snapshot is
/// the parent of another snapshot. Its memo catches exactly repeated roots, but
/// a changed root with shared subtrees is still walked as a full independent
/// map. High checkpoints turned that into billion-scale logical tree visits.
///
/// Consensus rule for this jet:
///
/// - the Hoon fallback is `(zh-milt balance)`, which remains the protocol oracle;
/// - `blocks` is only an optimization hint for choosing parent-first order;
/// - the fast path may reuse a converted parent only by applying z-map diffs;
/// - snapshots without a known converted parent use generic conversion;
/// - outer h-map entries are rebuilt from the original key nouns;
/// - exact key nouns matter, not just compact digest identity;
/// - anything malformed, cyclic, unsupported, or too wide falls back to the
///   generic converter.
///
/// In test-jet mode the interpreter runs this Rust arm and then evaluates the
/// Hoon fallback on the same sample. Any noun mismatch becomes a `%jest` bail.
pub fn zh_balance_milt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let blocks = slot(sam, 2, &space)?;
    let balance = slot(sam, 3, &space)?;
    z_balance_mip_to_h_mip_with_blocks(&mut context.stack, blocks, balance, &space)
}

/// Jet for `+zh-jult`, converting a legacy z-jug tree into an h-jug tree.
pub fn zh_jult_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let mut memo = ZHConversionMemo::default();
    let space = context.stack.noun_space();
    let a = slot(subject, 6, &space)?;
    z_jug_to_h_jug(&mut context.stack, a, &space, &mut memo)
}

#[derive(Default)]
struct ZHConversionMemo {
    maps: HashMap<u64, Noun>,
    sets: HashMap<u64, Noun>,
    mip_outer: HashMap<u64, Noun>,
    jug_outer: HashMap<u64, Noun>,
}

fn z_map_to_h_map<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut HashMap<u64, Noun>,
) -> Result<Noun, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_map_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.get(&tree_key) {
        return Ok(*converted);
    }

    let mut convert_value = |_stack: &mut A, value: Noun| Ok(value);
    let converted = z_map_to_h_map_with(stack, tree, space, memo, &mut convert_value)?;
    memo.insert(tree_key, converted);
    Ok(converted)
}

fn z_set_to_h_set<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut HashMap<u64, Noun>,
) -> Result<Noun, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_set_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.get(&tree_key) {
        return Ok(*converted);
    }

    let converted = z_set_to_h_set_with(stack, tree, space, memo)?;
    memo.insert(tree_key, converted);
    Ok(converted)
}

fn z_mip_to_h_mip<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut ZHConversionMemo,
) -> Result<Noun, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_map_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.mip_outer.get(&tree_key) {
        return Ok(*converted);
    }

    let mut outer_memo = std::mem::take(&mut memo.mip_outer);
    let mut convert_value =
        |stack: &mut A, value: Noun| z_map_to_hashed_h_map(stack, value, space, memo);
    let converted = z_map_to_h_map_with(stack, tree, space, &mut outer_memo, &mut convert_value);
    memo.mip_outer = outer_memo;
    let converted = converted?;
    memo.mip_outer.insert(tree_key, converted);
    Ok(converted)
}

fn z_map_to_hashed_h_map<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut ZHConversionMemo,
) -> Result<Noun, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_map_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.maps.get(&tree_key) {
        return Ok(*converted);
    }

    let mut convert_value = |_stack: &mut A, value: Noun| Ok(value);
    let converted = z_map_to_h_map_with(stack, tree, space, &mut memo.maps, &mut convert_value)?;
    memo.maps.insert(tree_key, converted);
    Ok(converted)
}

fn z_balance_mip_to_h_mip_with_blocks<A: NounAllocator>(
    stack: &mut A,
    blocks: Noun,
    balance: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if unsafe { balance.raw_equals(&D(0)) } {
        return Ok(h_map_empty());
    }
    if balance.is_atom() {
        return Err(BAIL_FAIL);
    }

    // Parent links decide only the fast-path order. They do not define the
    // result. If the block map is not shaped like consensus state, convert the
    // whole balance through the generic `zh-milt` path.
    let parents = match collect_block_parents(blocks, space) {
        Ok(parents) => parents,
        Err(_) => {
            let mut memo = ZHConversionMemo::default();
            return z_mip_to_h_mip(stack, balance, space, &mut memo);
        }
    };

    let mut entries = Vec::new();
    collect_z_map_entries(balance, space, &mut entries)?;
    let mut snapshots = Vec::with_capacity(entries.len());
    let mut index_by_block = HashMap::with_capacity(entries.len());
    for entry in entries {
        let block_digest = match digest_from_noun(entry.key, space) {
            Ok(block_digest) => block_digest,
            Err(_) => {
                // Generic h-zoon accepts every valid hashed key noun. The
                // balance fast path needs direct block digests to join a
                // balance snapshot to `blocks`, and it must also preserve the
                // original outer key noun when rebuilding the output. If those
                // two facts diverge, the whole balance goes through the oracle
                // path.
                let mut memo = ZHConversionMemo::default();
                return z_mip_to_h_mip(stack, balance, space, &mut memo);
            }
        };
        let index = snapshots.len();
        snapshots.push(BalanceSnapshot {
            block_id: entry.key,
            block_digest,
            z_balance: entry.value,
        });
        index_by_block.insert(block_digest, index);
    }

    let mut state = BalanceConversionState {
        stack,
        snapshots,
        index_by_block,
        parents,
        space,
        converted: Vec::new(),
        visiting: Vec::new(),
        memo: ZHConversionMemo::default(),
    };
    state.converted.resize(state.snapshots.len(), None);
    state.visiting.resize(state.snapshots.len(), false);

    // Convert every snapshot. `memo.maps` catches exact repeated balance roots.
    // Parent-derived diffs handle the common persistent-tree case: a new root
    // with mostly shared descendants and a small note delta. A missing parent
    // only disables this optimization for the affected snapshot.
    for index in 0..state.snapshots.len() {
        state.convert_index(index)?;
    }

    let mut outer_entries = Vec::with_capacity(state.snapshots.len());
    for index in 0..state.snapshots.len() {
        let key = state.snapshots[index].block_id;
        let value = state.converted[index].ok_or(BAIL_FAIL)?;
        outer_entries.push(HMapEntry {
            noun: T(state.stack, &[key, value]),
            key,
        });
    }
    h_map_from_entries(state.stack, outer_entries, state.space)
}

struct BalanceSnapshot {
    block_id: Noun,
    block_digest: [u64; 5],
    z_balance: Noun,
}

// Migration-local state for one balance field.
//
// `converted` is indexed by the flattened outer balance entries and stores the
// h-map for each block's note balance. `memo.maps` is the same cache used by
// generic z-map conversion, which keeps repeated roots identical across the
// oracle path and the balance-aware path.
struct BalanceConversionState<'a, A: NounAllocator> {
    stack: &'a mut A,
    snapshots: Vec<BalanceSnapshot>,
    index_by_block: HashMap<[u64; 5], usize>,
    parents: HashMap<[u64; 5], [u64; 5]>,
    space: &'a NounSpace,
    converted: Vec<Option<Noun>>,
    visiting: Vec<bool>,
    memo: ZHConversionMemo,
}

impl<A: NounAllocator> BalanceConversionState<'_, A> {
    fn convert_index(&mut self, index: usize) -> Result<Noun, JetErr> {
        if let Some(converted) = self.cached_conversion(index) {
            return Ok(converted);
        }

        let mut path = Vec::new();
        let mut current = index;

        // Build the ancestor path iteratively. Earlier recursive code worked on
        // the sample checkpoint, but it spent stack on a migration already close
        // to the allocator floor. The loop stops at the nearest converted,
        // missing, self-parented, or cyclic ancestor. Any snapshot still lacking
        // a converted parent then uses generic conversion.
        loop {
            if self.cached_conversion(current).is_some() {
                break;
            }
            if self.visiting[current] {
                break;
            }

            self.visiting[current] = true;
            path.push(current);

            let Some(parent_index) = self.parent_index(current) else {
                break;
            };
            if parent_index == current || self.visiting[parent_index] {
                break;
            }
            if self.cached_conversion(parent_index).is_some() {
                break;
            }

            current = parent_index;
        }

        for &path_index in &path {
            self.visiting[path_index] = false;
        }

        for path_index in path.into_iter().rev() {
            if self.cached_conversion(path_index).is_some() {
                continue;
            }

            // Try the delta path only when the parent h-map already exists. If
            // the source trees do not prove a bounded local change, this single
            // snapshot falls back to generic conversion.
            let converted = match self.try_convert_from_converted_parent(path_index) {
                Ok(Some(converted)) => converted,
                Ok(None) | Err(_) => self.convert_generic(path_index)?,
            };
            self.store_conversion(path_index, converted);
        }

        self.converted[index].ok_or(BAIL_FAIL)
    }

    fn cached_conversion(&mut self, index: usize) -> Option<Noun> {
        if let Some(converted) = self.converted[index] {
            return Some(converted);
        }

        let z_balance = self.snapshots[index].z_balance;
        if let Some(converted) = self.memo.maps.get(&noun_identity(z_balance)).copied() {
            self.converted[index] = Some(converted);
            return Some(converted);
        }

        None
    }

    fn store_conversion(&mut self, index: usize, converted: Noun) {
        let z_balance = self.snapshots[index].z_balance;
        self.memo.maps.insert(noun_identity(z_balance), converted);
        self.converted[index] = Some(converted);
    }

    fn parent_index(&self, index: usize) -> Option<usize> {
        let block_digest = self.snapshots[index].block_digest;
        let parent_digest = self.parents.get(&block_digest)?;
        self.index_by_block.get(parent_digest).copied()
    }

    fn try_convert_from_converted_parent(&mut self, index: usize) -> Result<Option<Noun>, JetErr> {
        let Some(parent_index) = self.parent_index(index) else {
            return Ok(None);
        };

        let Some(parent_h_balance) = self.converted[parent_index] else {
            return Ok(None);
        };
        let parent_z_balance = self.snapshots[parent_index].z_balance;
        let child_z_balance = self.snapshots[index].z_balance;
        h_map_from_parent_z_diff(
            self.stack, parent_z_balance, parent_h_balance, child_z_balance, self.space,
        )
        .map(Some)
    }

    fn convert_generic(&mut self, index: usize) -> Result<Noun, JetErr> {
        z_map_to_hashed_h_map(
            self.stack, self.snapshots[index].z_balance, self.space, &mut self.memo,
        )
    }
}

fn collect_block_parents(
    blocks: Noun,
    space: &NounSpace,
) -> Result<HashMap<[u64; 5], [u64; 5]>, JetErr> {
    let mut entries = Vec::new();
    collect_z_map_entries(blocks, space, &mut entries)?;
    let mut parents = HashMap::with_capacity(entries.len());
    for entry in entries {
        let block_id = digest_from_noun(entry.key, space)?;
        let parent = local_page_parent(entry.value, space)?;
        parents.insert(block_id, digest_from_noun(parent, space)?);
    }
    Ok(parents)
}

fn local_page_parent(page: Noun, space: &NounSpace) -> Result<Noun, JetErr> {
    let page_cell = page.in_space(space).as_cell().map_err(|_| BAIL_FAIL)?;
    if page_cell.head().noun().is_atom() {
        slot(page, 30, space)
    } else {
        slot(page, 14, space)
    }
}

fn z_jug_to_h_jug<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut ZHConversionMemo,
) -> Result<Noun, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_map_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.jug_outer.get(&tree_key) {
        return Ok(*converted);
    }

    let mut outer_memo = std::mem::take(&mut memo.jug_outer);
    let mut convert_value =
        |stack: &mut A, value: Noun| z_set_to_hashed_h_set(stack, value, space, memo);
    let converted = z_map_to_h_map_with(stack, tree, space, &mut outer_memo, &mut convert_value);
    memo.jug_outer = outer_memo;
    let converted = converted?;
    memo.jug_outer.insert(tree_key, converted);
    Ok(converted)
}

fn z_set_to_hashed_h_set<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut ZHConversionMemo,
) -> Result<Noun, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_set_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.sets.get(&tree_key) {
        return Ok(*converted);
    }

    let converted = z_set_to_h_set_with(stack, tree, space, &mut memo.sets)?;
    memo.sets.insert(tree_key, converted);
    Ok(converted)
}

fn z_map_to_h_map_with<A, F>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut HashMap<u64, Noun>,
    convert_value: &mut F,
) -> Result<Noun, JetErr>
where
    A: NounAllocator,
    F: FnMut(&mut A, Noun) -> Result<Noun, JetErr>,
{
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_map_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.get(&tree_key) {
        return Ok(*converted);
    }

    let mut entries = Vec::new();
    collect_z_map_entries(tree, space, &mut entries)?;
    let mut converted_entries = Vec::with_capacity(entries.len());
    for entry in entries {
        let key = entry.key;
        let value = entry.value;
        let converted_value = convert_value(stack, value)?;
        let converted_entry = if unsafe { converted_value.raw_equals(&value) } {
            entry.noun
        } else {
            T(stack, &[key, converted_value])
        };
        converted_entries.push(HMapEntry {
            noun: converted_entry,
            key,
        });
    }
    let converted = h_map_from_entries(stack, converted_entries, space)?;
    memo.insert(tree_key, converted);
    Ok(converted)
}

fn z_set_to_h_set_with<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    space: &NounSpace,
    memo: &mut HashMap<u64, Noun>,
) -> Result<Noun, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(h_set_empty());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let tree_key = noun_identity(tree);
    if let Some(converted) = memo.get(&tree_key) {
        return Ok(*converted);
    }

    let mut items = Vec::new();
    collect_z_set_items(tree, space, &mut items)?;
    let converted = h_set_from_items(stack, items, space)?;
    memo.insert(tree_key, converted);
    Ok(converted)
}

struct HMapEntry {
    noun: Noun,
    key: Noun,
}

struct ZMapEntry {
    noun: Noun,
    key: Noun,
    value: Noun,
}

fn collect_z_map_entries(
    tree: Noun,
    space: &NounSpace,
    entries: &mut Vec<ZMapEntry>,
) -> Result<(), JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let [entry, left, right] = tree.uncell(space)?;
    let [key, value] = entry.uncell(space)?;
    entries.push(ZMapEntry {
        noun: entry,
        key,
        value,
    });
    collect_z_map_entries(left, space, entries)?;
    collect_z_map_entries(right, space, entries)
}

fn collect_z_set_items(tree: Noun, space: &NounSpace, items: &mut Vec<Noun>) -> Result<(), JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let [value, left, right] = tree.uncell(space)?;
    items.push(value);
    collect_z_set_items(left, space, items)?;
    collect_z_set_items(right, space, items)
}

enum HMapDiffAction {
    Put { key: Noun, value: Noun },
    Del { key: Noun },
}

struct ZMapDiffEntry {
    key_hash: SmallHashedKey,
    key: Noun,
    value: Noun,
}

fn h_map_from_parent_z_diff<A: NounAllocator>(
    stack: &mut A,
    parent_z: Noun,
    parent_h: Noun,
    child_z: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if unsafe { parent_z.raw_equals(&child_z) } {
        return Ok(parent_h);
    }

    let mut actions = Vec::new();
    collect_z_map_diff(stack, parent_z, child_z, space, &mut actions)?;

    let mut result = parent_h;
    // Apply deletions first. Generic `zh-milt` preserves exact key nouns, and
    // the diff path needs to model replacements as delete old key noun, then put
    // new key noun.
    for action in &actions {
        if let HMapDiffAction::Del { key } = action {
            result = h_map_del(stack, result, *key, space)?;
        }
    }
    for action in actions {
        if let HMapDiffAction::Put { key, value } = action {
            result = h_map_put(stack, result, key, value, space)?;
        }
    }
    Ok(result)
}

fn collect_z_map_diff<A: NounAllocator>(
    stack: &mut A,
    parent_z: Noun,
    child_z: Noun,
    space: &NounSpace,
    actions: &mut Vec<HMapDiffAction>,
) -> Result<(), JetErr> {
    if unsafe { parent_z.raw_equals(&child_z) } {
        return Ok(());
    }

    if unsafe { parent_z.raw_equals(&D(0)) } {
        collect_z_map_put_actions(child_z, space, actions)?;
        return Ok(());
    }
    if unsafe { child_z.raw_equals(&D(0)) } {
        collect_z_map_del_actions(parent_z, space, actions)?;
        return Ok(());
    }
    if parent_z.is_atom() || child_z.is_atom() {
        return Err(BAIL_FAIL);
    }

    let [parent_entry, parent_left, parent_right] = parent_z.uncell(space)?;
    let [parent_key, parent_value] = parent_entry.uncell(space)?;
    let [child_entry, child_left, child_right] = child_z.uncell(space)?;
    let [child_key, child_value] = child_entry.uncell(space)?;

    if noun_equal(stack, parent_key, child_key) {
        if !noun_equal(stack, parent_value, child_value) {
            actions.push(HMapDiffAction::Put {
                key: child_key,
                value: child_value,
            });
        }
        collect_z_map_diff(stack, parent_left, child_left, space, actions)?;
        collect_z_map_diff(stack, parent_right, child_right, space, actions)
    } else {
        collect_rotated_z_map_diff(stack, parent_z, child_z, space, actions)
    }
}

fn collect_rotated_z_map_diff<A: NounAllocator>(
    stack: &mut A,
    parent_z: Noun,
    child_z: Noun,
    space: &NounSpace,
    actions: &mut Vec<HMapDiffAction>,
) -> Result<(), JetErr> {
    // Zoon maps are treaps. A small logical update can rotate the local root,
    // making the root keys differ even when most of the two subtrees match. For
    // small regions we flatten and compare by hashed key. Duplicate compact keys
    // make that proof ambiguous, and large regions would cost the work this jet
    // is avoiding; both cases bail to generic conversion.
    let parent_len = count_z_map_entries_limited(parent_z, Z_DIFF_ROTATION_ENTRY_LIMIT + 1, space)?;
    let child_len = count_z_map_entries_limited(child_z, Z_DIFF_ROTATION_ENTRY_LIMIT + 1, space)?;
    if parent_len + child_len > Z_DIFF_ROTATION_ENTRY_LIMIT {
        return Err(BAIL_FAIL);
    }

    let mut parent_entries = Vec::with_capacity(parent_len);
    let mut child_entries = Vec::with_capacity(child_len);
    collect_z_map_diff_entries(parent_z, space, &mut parent_entries)?;
    collect_z_map_diff_entries(child_z, space, &mut child_entries)?;

    let mut parent_by_key = HashMap::with_capacity(parent_entries.len());
    for entry in parent_entries {
        if parent_by_key
            .insert(entry.key_hash, (entry.key, entry.value))
            .is_some()
        {
            return Err(BAIL_FAIL);
        }
    }

    let mut child_by_key = HashMap::with_capacity(child_entries.len());
    for entry in child_entries {
        if child_by_key
            .insert(entry.key_hash, (entry.key, entry.value))
            .is_some()
        {
            return Err(BAIL_FAIL);
        }
    }

    for (key_hash, (parent_key, _value)) in &parent_by_key {
        match child_by_key.get(key_hash) {
            Some((child_key, _value)) if noun_equal(stack, *parent_key, *child_key) => {}
            _ => actions.push(HMapDiffAction::Del { key: *parent_key }),
        }
    }

    for (key_hash, (child_key, child_value)) in child_by_key {
        match parent_by_key.get(&key_hash) {
            Some((parent_key, parent_value))
                if noun_equal(stack, *parent_key, child_key)
                    && noun_equal(stack, *parent_value, child_value) => {}
            _ => actions.push(HMapDiffAction::Put {
                key: child_key,
                value: child_value,
            }),
        }
    }

    Ok(())
}

fn collect_z_map_put_actions(
    tree: Noun,
    space: &NounSpace,
    actions: &mut Vec<HMapDiffAction>,
) -> Result<(), JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let [entry, left, right] = tree.uncell(space)?;
    let [key, value] = entry.uncell(space)?;
    small_hashed_key_from_noun(key, space).ok_or(BAIL_FAIL)?;
    actions.push(HMapDiffAction::Put { key, value });
    collect_z_map_put_actions(left, space, actions)?;
    collect_z_map_put_actions(right, space, actions)
}

fn collect_z_map_del_actions(
    tree: Noun,
    space: &NounSpace,
    actions: &mut Vec<HMapDiffAction>,
) -> Result<(), JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let [entry, left, right] = tree.uncell(space)?;
    let [key, _value] = entry.uncell(space)?;
    small_hashed_key_from_noun(key, space).ok_or(BAIL_FAIL)?;
    actions.push(HMapDiffAction::Del { key });
    collect_z_map_del_actions(left, space, actions)?;
    collect_z_map_del_actions(right, space, actions)
}

fn collect_z_map_diff_entries(
    tree: Noun,
    space: &NounSpace,
    entries: &mut Vec<ZMapDiffEntry>,
) -> Result<(), JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }

    let [entry, left, right] = tree.uncell(space)?;
    let [key, value] = entry.uncell(space)?;
    entries.push(ZMapDiffEntry {
        key_hash: small_hashed_key_from_noun(key, space).ok_or(BAIL_FAIL)?,
        key,
        value,
    });
    collect_z_map_diff_entries(left, space, entries)?;
    collect_z_map_diff_entries(right, space, entries)
}

fn count_z_map_entries_limited(
    tree: Noun,
    limit: usize,
    space: &NounSpace,
) -> Result<usize, JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(0);
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }
    if limit == 0 {
        return Ok(1);
    }

    let [_entry, left, right] = tree.uncell(space)?;
    let left_count = count_z_map_entries_limited(left, limit, space)?;
    if left_count > limit {
        return Ok(left_count);
    }
    let remaining = limit.saturating_sub(left_count + 1);
    let right_count = count_z_map_entries_limited(right, remaining, space)?;
    Ok(left_count + 1 + right_count)
}

struct HMapBuildNode {
    entry: Noun,
    digests: HashedKey,
    original_index: usize,
    left: Option<usize>,
    right: Option<usize>,
}

struct SmallHMapBuildNode {
    entry: Noun,
    key: SmallHashedKey,
    original_index: usize,
    left: Option<usize>,
    right: Option<usize>,
}

struct HSetBuildNode {
    value: Noun,
    digests: HashedKey,
    original_index: usize,
    left: Option<usize>,
    right: Option<usize>,
}

struct SmallHSetBuildNode {
    value: Noun,
    key: SmallHashedKey,
    original_index: usize,
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SmallHashedKey {
    len: usize,
    digests: [[u64; 5]; SMALL_HASHED_KEY_LIMIT],
}

impl SmallHashedKey {
    fn empty() -> Self {
        Self {
            len: 0,
            digests: [[0; 5]; SMALL_HASHED_KEY_LIMIT],
        }
    }

    fn single(digest: [u64; 5]) -> Self {
        Self {
            len: 1,
            digests: [digest, [0; 5]],
        }
    }

    fn pair(first: [u64; 5], second: [u64; 5]) -> Self {
        Self {
            len: 2,
            digests: [first, second],
        }
    }
}

enum HashedKey {
    Single([u64; 5]),
    List(Vec<[u64; 5]>),
}

impl HashedKey {
    fn as_slice(&self) -> &[[u64; 5]] {
        match self {
            HashedKey::Single(digest) => std::slice::from_ref(digest),
            HashedKey::List(digests) => digests.as_slice(),
        }
    }
}

fn h_map_from_entries<A: NounAllocator>(
    stack: &mut A,
    entries: Vec<HMapEntry>,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if entries.is_empty() {
        return Ok(h_map_empty());
    }

    if let Some(mut nodes) = small_h_map_build_nodes(&entries, space) {
        sort_small_h_map_build_nodes(&mut nodes);
        ensure_strict_small_h_map_order(&nodes)?;
        let root = link_small_h_map_nodes_by_priority(&mut nodes);
        return build_small_h_map_node(stack, &nodes, root);
    }

    let mut nodes = Vec::with_capacity(entries.len());
    for (original_index, entry) in entries.into_iter().enumerate() {
        nodes.push(HMapBuildNode {
            entry: entry.noun,
            digests: hashed_to_digests(entry.key, space)?,
            original_index,
            left: None,
            right: None,
        });
    }
    sort_h_map_build_nodes(&mut nodes);
    ensure_strict_h_map_order(&nodes)?;
    let root = link_h_map_nodes_by_priority(&mut nodes);
    build_h_map_node(stack, &nodes, root)
}

fn h_set_from_items<A: NounAllocator>(
    stack: &mut A,
    items: Vec<Noun>,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if items.is_empty() {
        return Ok(h_set_empty());
    }

    if let Some(mut nodes) = small_h_set_build_nodes(&items, space) {
        sort_small_h_set_build_nodes(&mut nodes);
        ensure_strict_small_h_set_order(&nodes)?;
        let root = link_small_h_set_nodes_by_priority(&mut nodes);
        return build_small_h_set_node(stack, &nodes, root);
    }

    let mut nodes = Vec::with_capacity(items.len());
    for (original_index, value) in items.into_iter().enumerate() {
        nodes.push(HSetBuildNode {
            value,
            digests: hashed_to_digests(value, space)?,
            original_index,
            left: None,
            right: None,
        });
    }
    sort_h_set_build_nodes(&mut nodes);
    ensure_strict_h_set_order(&nodes)?;
    let root = link_h_set_nodes_by_priority(&mut nodes);
    build_h_set_node(stack, &nodes, root)
}

fn small_h_map_build_nodes(
    entries: &[HMapEntry],
    space: &NounSpace,
) -> Option<Vec<SmallHMapBuildNode>> {
    let mut nodes = Vec::with_capacity(entries.len());
    for (original_index, entry) in entries.iter().enumerate() {
        let key = small_hashed_key_from_noun(entry.key, space)?;
        nodes.push(SmallHMapBuildNode {
            entry: entry.noun,
            key,
            original_index,
            left: None,
            right: None,
        });
    }
    Some(nodes)
}

fn small_h_set_build_nodes(items: &[Noun], space: &NounSpace) -> Option<Vec<SmallHSetBuildNode>> {
    let mut nodes = Vec::with_capacity(items.len());
    for (original_index, value) in items.iter().enumerate() {
        let key = small_hashed_key_from_noun(*value, space)?;
        nodes.push(SmallHSetBuildNode {
            value: *value,
            key,
            original_index,
            left: None,
            right: None,
        });
    }
    Some(nodes)
}

fn small_hashed_key_from_noun(noun: Noun, space: &NounSpace) -> Option<SmallHashedKey> {
    if unsafe { noun.raw_equals(&D(0)) } {
        return Some(SmallHashedKey::empty());
    }

    let cell = noun.in_space(space).as_cell().ok()?;
    let head = cell.head().noun();
    let tail = cell.tail().noun();
    if head.is_atom() {
        return small_digest_from_noun(noun, space).map(SmallHashedKey::single);
    }

    let first = small_digest_from_noun(head, space)?;
    if unsafe { tail.raw_equals(&D(0)) } {
        return None;
    }

    let second_cell = tail.in_space(space).as_cell().ok()?;
    let second_noun = second_cell.head().noun();
    let rest = second_cell.tail().noun();
    let second = small_digest_from_noun(second_noun, space)?;
    if unsafe { !rest.raw_equals(&D(0)) } {
        return None;
    }

    Some(SmallHashedKey::pair(first, second))
}

fn small_digest_from_noun(noun: Noun, space: &NounSpace) -> Option<[u64; 5]> {
    let first = noun.in_space(space).as_cell().ok()?;
    let first_head = first.head().noun();
    let first_tail = first.tail().noun();
    let second = first_tail.in_space(space).as_cell().ok()?;
    let second_head = second.head().noun();
    let second_tail = second.tail().noun();
    let third = second_tail.in_space(space).as_cell().ok()?;
    let third_head = third.head().noun();
    let third_tail = third.tail().noun();
    let fourth = third_tail.in_space(space).as_cell().ok()?;
    let fourth_head = fourth.head().noun();
    let fourth_tail = fourth.tail().noun();
    Some([
        atom_to_u64_opt(first_head, space)?,
        atom_to_u64_opt(second_head, space)?,
        atom_to_u64_opt(third_head, space)?,
        atom_to_u64_opt(fourth_head, space)?,
        atom_to_u64_opt(fourth_tail, space)?,
    ])
}

fn atom_to_u64_opt(noun: Noun, space: &NounSpace) -> Option<u64> {
    noun.in_space(space).as_atom().ok()?.as_u64().ok()
}

fn compare_h_map_build_nodes(a: &HMapBuildNode, b: &HMapBuildNode) -> Ordering {
    descending_gor_order(a.digests.as_slice(), b.digests.as_slice())
        .then_with(|| a.original_index.cmp(&b.original_index))
}

fn compare_h_set_build_nodes(a: &HSetBuildNode, b: &HSetBuildNode) -> Ordering {
    descending_gor_order(a.digests.as_slice(), b.digests.as_slice())
        .then_with(|| a.original_index.cmp(&b.original_index))
}

fn compare_small_h_map_build_nodes(a: &SmallHMapBuildNode, b: &SmallHMapBuildNode) -> Ordering {
    descending_small_gor_order(&a.key, &b.key).then_with(|| a.original_index.cmp(&b.original_index))
}

fn compare_small_h_set_build_nodes(a: &SmallHSetBuildNode, b: &SmallHSetBuildNode) -> Ordering {
    descending_small_gor_order(&a.key, &b.key).then_with(|| a.original_index.cmp(&b.original_index))
}

fn sort_h_map_build_nodes(nodes: &mut [HMapBuildNode]) {
    if nodes.len() >= PARALLEL_SORT_THRESHOLD {
        nodes.par_sort_unstable_by(compare_h_map_build_nodes);
    } else {
        nodes.sort_unstable_by(compare_h_map_build_nodes);
    }
}

fn sort_h_set_build_nodes(nodes: &mut [HSetBuildNode]) {
    if nodes.len() >= PARALLEL_SORT_THRESHOLD {
        nodes.par_sort_unstable_by(compare_h_set_build_nodes);
    } else {
        nodes.sort_unstable_by(compare_h_set_build_nodes);
    }
}

fn sort_small_h_map_build_nodes(nodes: &mut [SmallHMapBuildNode]) {
    if nodes.len() >= PARALLEL_SORT_THRESHOLD {
        nodes.par_sort_unstable_by(compare_small_h_map_build_nodes);
    } else {
        nodes.sort_unstable_by(compare_small_h_map_build_nodes);
    }
}

fn sort_small_h_set_build_nodes(nodes: &mut [SmallHSetBuildNode]) {
    if nodes.len() >= PARALLEL_SORT_THRESHOLD {
        nodes.par_sort_unstable_by(compare_small_h_set_build_nodes);
    } else {
        nodes.sort_unstable_by(compare_small_h_set_build_nodes);
    }
}

fn ensure_strict_h_map_order(nodes: &[HMapBuildNode]) -> Result<(), JetErr> {
    for pair in nodes.windows(2) {
        if descending_gor_order(pair[0].digests.as_slice(), pair[1].digests.as_slice())
            == Ordering::Equal
        {
            return Err(BAIL_FAIL);
        }
    }
    Ok(())
}

fn ensure_strict_h_set_order(nodes: &[HSetBuildNode]) -> Result<(), JetErr> {
    for pair in nodes.windows(2) {
        if descending_gor_order(pair[0].digests.as_slice(), pair[1].digests.as_slice())
            == Ordering::Equal
        {
            return Err(BAIL_FAIL);
        }
    }
    Ok(())
}

fn ensure_strict_small_h_map_order(nodes: &[SmallHMapBuildNode]) -> Result<(), JetErr> {
    for pair in nodes.windows(2) {
        if pair[0].key == pair[1].key {
            return Err(BAIL_FAIL);
        }
    }
    Ok(())
}

fn ensure_strict_small_h_set_order(nodes: &[SmallHSetBuildNode]) -> Result<(), JetErr> {
    for pair in nodes.windows(2) {
        if pair[0].key == pair[1].key {
            return Err(BAIL_FAIL);
        }
    }
    Ok(())
}

fn link_h_map_nodes_by_priority(nodes: &mut [HMapBuildNode]) -> usize {
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..nodes.len() {
        let mut left = None;
        while let Some(&top) = stack.last() {
            if hashed_order_low_to_high(
                nodes[top].digests.as_slice(),
                nodes[index].digests.as_slice(),
            ) == Ordering::Greater
            {
                break;
            }
            left = stack.pop();
        }
        nodes[index].left = left;
        if let Some(&top) = stack.last() {
            nodes[top].right = Some(index);
        }
        stack.push(index);
    }
    stack[0]
}

fn link_h_set_nodes_by_priority(nodes: &mut [HSetBuildNode]) -> usize {
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..nodes.len() {
        let mut left = None;
        while let Some(&top) = stack.last() {
            if hashed_order_low_to_high(
                nodes[top].digests.as_slice(),
                nodes[index].digests.as_slice(),
            ) == Ordering::Greater
            {
                break;
            }
            left = stack.pop();
        }
        nodes[index].left = left;
        if let Some(&top) = stack.last() {
            nodes[top].right = Some(index);
        }
        stack.push(index);
    }
    stack[0]
}

fn link_small_h_map_nodes_by_priority(nodes: &mut [SmallHMapBuildNode]) -> usize {
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..nodes.len() {
        let mut left = None;
        while let Some(&top) = stack.last() {
            if small_mor_order(&nodes[top].key, &nodes[index].key) == Ordering::Greater {
                break;
            }
            left = stack.pop();
        }
        nodes[index].left = left;
        if let Some(&top) = stack.last() {
            nodes[top].right = Some(index);
        }
        stack.push(index);
    }
    stack[0]
}

fn link_small_h_set_nodes_by_priority(nodes: &mut [SmallHSetBuildNode]) -> usize {
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..nodes.len() {
        let mut left = None;
        while let Some(&top) = stack.last() {
            if small_mor_order(&nodes[top].key, &nodes[index].key) == Ordering::Greater {
                break;
            }
            left = stack.pop();
        }
        nodes[index].left = left;
        if let Some(&top) = stack.last() {
            nodes[top].right = Some(index);
        }
        stack.push(index);
    }
    stack[0]
}

fn build_h_map_node<A: NounAllocator>(
    stack: &mut A,
    nodes: &[HMapBuildNode],
    index: usize,
) -> Result<Noun, JetErr> {
    let cells = allocate_tree_cells(stack, nodes.len());
    for (node_index, node) in nodes.iter().enumerate() {
        fill_tree_cell(
            cells[node_index],
            node.entry,
            node.left
                .map_or_else(h_map_empty, |left| cells[left].outer.as_noun()),
            node.right
                .map_or_else(h_map_empty, |right| cells[right].outer.as_noun()),
        );
    }
    Ok(cells[index].outer.as_noun())
}

fn build_h_set_node<A: NounAllocator>(
    stack: &mut A,
    nodes: &[HSetBuildNode],
    index: usize,
) -> Result<Noun, JetErr> {
    let cells = allocate_tree_cells(stack, nodes.len());
    for (node_index, node) in nodes.iter().enumerate() {
        fill_tree_cell(
            cells[node_index],
            node.value,
            node.left
                .map_or_else(h_set_empty, |left| cells[left].outer.as_noun()),
            node.right
                .map_or_else(h_set_empty, |right| cells[right].outer.as_noun()),
        );
    }
    Ok(cells[index].outer.as_noun())
}

fn build_small_h_map_node<A: NounAllocator>(
    stack: &mut A,
    nodes: &[SmallHMapBuildNode],
    index: usize,
) -> Result<Noun, JetErr> {
    let cells = allocate_tree_cells(stack, nodes.len());
    for (node_index, node) in nodes.iter().enumerate() {
        fill_tree_cell(
            cells[node_index],
            node.entry,
            node.left
                .map_or_else(h_map_empty, |left| cells[left].outer.as_noun()),
            node.right
                .map_or_else(h_map_empty, |right| cells[right].outer.as_noun()),
        );
    }
    Ok(cells[index].outer.as_noun())
}

fn build_small_h_set_node<A: NounAllocator>(
    stack: &mut A,
    nodes: &[SmallHSetBuildNode],
    index: usize,
) -> Result<Noun, JetErr> {
    let cells = allocate_tree_cells(stack, nodes.len());
    for (node_index, node) in nodes.iter().enumerate() {
        fill_tree_cell(
            cells[node_index],
            node.value,
            node.left
                .map_or_else(h_set_empty, |left| cells[left].outer.as_noun()),
            node.right
                .map_or_else(h_set_empty, |right| cells[right].outer.as_noun()),
        );
    }
    Ok(cells[index].outer.as_noun())
}

#[derive(Clone, Copy)]
struct TreeCellPair {
    outer: Cell,
    outer_memory: *mut CellMemory,
    inner: Cell,
    inner_memory: *mut CellMemory,
}

fn allocate_tree_cells<A: NounAllocator>(stack: &mut A, len: usize) -> Vec<TreeCellPair> {
    let mut cells = Vec::with_capacity(len);
    for _ in 0..len {
        let outer = unsafe { Cell::new_raw_mut(stack) };
        let inner = unsafe { Cell::new_raw_mut(stack) };
        cells.push(TreeCellPair {
            outer: outer.0,
            outer_memory: outer.1,
            inner: inner.0,
            inner_memory: inner.1,
        });
    }
    cells
}

fn fill_tree_cell(cells: TreeCellPair, node: Noun, left: Noun, right: Noun) {
    unsafe {
        (*cells.outer_memory).head = node;
        (*cells.outer_memory).tail = cells.inner.as_noun();
        (*cells.inner_memory).head = left;
        (*cells.inner_memory).tail = right;
    }
}

fn descending_gor_order(a: &[[u64; 5]], b: &[[u64; 5]]) -> Ordering {
    hashed_order_high_to_low(a, b).reverse()
}

fn descending_small_gor_order(a: &SmallHashedKey, b: &SmallHashedKey) -> Ordering {
    small_gor_order(a, b).reverse()
}

fn small_gor_order(a: &SmallHashedKey, b: &SmallHashedKey) -> Ordering {
    let shared_len = a.len.min(b.len);
    for digest_index in 0..shared_len {
        for limb_index in (0..5).rev() {
            let ordering =
                a.digests[digest_index][limb_index].cmp(&b.digests[digest_index][limb_index]);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
    }
    a.len.cmp(&b.len)
}

fn small_mor_order(a: &SmallHashedKey, b: &SmallHashedKey) -> Ordering {
    let shared_len = a.len.min(b.len);
    for digest_index in 0..shared_len {
        for limb_index in 0..5 {
            let ordering =
                a.digests[digest_index][limb_index].cmp(&b.digests[digest_index][limb_index]);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
    }
    a.len.cmp(&b.len)
}

fn hashed_order_high_to_low(a: &[[u64; 5]], b: &[[u64; 5]]) -> Ordering {
    compare_digest_lists_ordering(a, b, compare_digest_high_to_low)
}

fn hashed_order_low_to_high(a: &[[u64; 5]], b: &[[u64; 5]]) -> Ordering {
    compare_digest_lists_ordering(a, b, compare_digest_low_to_high)
}

fn h_map_put<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    key: Noun,
    value: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    hashed_to_digests(key, space)?;
    if tree.is_atom() {
        let entry = T(stack, &[key, value]);
        return Ok(T(stack, &[entry, h_map_empty(), h_map_empty()]));
    }

    let [entry, left, right] = tree.uncell(space)?;
    let [node_key, node_value] = entry.uncell(space)?;
    hashed_to_digests(node_key, space)?;
    if noun_equal(stack, key, node_key) {
        if noun_equal(stack, value, node_value) {
            return Ok(tree);
        }
        let entry = T(stack, &[key, value]);
        return Ok(T(stack, &[entry, left, right]));
    }

    if gor_hip(key, node_key, space)? {
        let child = h_map_put(stack, left, key, value, space)?;
        let [child_entry, child_left, child_right] = child.uncell(space)?;
        let [child_key, _child_value] = child_entry.uncell(space)?;
        if mor_hip(node_key, child_key, space)? {
            Ok(T(stack, &[entry, child, right]))
        } else {
            let demoted = T(stack, &[entry, child_right, right]);
            Ok(T(stack, &[child_entry, child_left, demoted]))
        }
    } else {
        let child = h_map_put(stack, right, key, value, space)?;
        let [child_entry, child_left, child_right] = child.uncell(space)?;
        let [child_key, _child_value] = child_entry.uncell(space)?;
        if mor_hip(node_key, child_key, space)? {
            Ok(T(stack, &[entry, left, child]))
        } else {
            let demoted = T(stack, &[entry, left, child_left]);
            Ok(T(stack, &[child_entry, demoted, child_right]))
        }
    }
}

fn h_map_del<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    key: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    hashed_to_digests(key, space)?;
    if tree.is_atom() {
        return Ok(h_map_empty());
    }

    let [entry, left, right] = tree.uncell(space)?;
    let [node_key, _node_value] = entry.uncell(space)?;
    hashed_to_digests(node_key, space)?;
    if noun_equal(stack, key, node_key) {
        return h_map_join(stack, left, right, space);
    }

    if gor_hip(key, node_key, space)? {
        let child = h_map_del(stack, left, key, space)?;
        if unsafe { child.raw_equals(&left) } {
            Ok(tree)
        } else {
            Ok(T(stack, &[entry, child, right]))
        }
    } else {
        let child = h_map_del(stack, right, key, space)?;
        if unsafe { child.raw_equals(&right) } {
            Ok(tree)
        } else {
            Ok(T(stack, &[entry, left, child]))
        }
    }
}

fn h_map_join<A: NounAllocator>(
    stack: &mut A,
    left: Noun,
    right: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if left.is_atom() {
        return Ok(right);
    }
    if right.is_atom() {
        return Ok(left);
    }

    let [left_entry, left_left, left_right] = left.uncell(space)?;
    let [left_key, _left_value] = left_entry.uncell(space)?;
    let [right_entry, right_left, right_right] = right.uncell(space)?;
    let [right_key, _right_value] = right_entry.uncell(space)?;
    hashed_to_digests(left_key, space)?;
    hashed_to_digests(right_key, space)?;

    if mor_hip(left_key, right_key, space)? {
        let joined = h_map_join(stack, left_right, right, space)?;
        Ok(T(stack, &[left_entry, left_left, joined]))
    } else {
        let joined = h_map_join(stack, left, right_left, space)?;
        Ok(T(stack, &[right_entry, joined, right_right]))
    }
}

fn h_set_put<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    value: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    hashed_to_digests(value, space)?;
    if tree.is_atom() {
        return Ok(T(stack, &[value, h_set_empty(), h_set_empty()]));
    }

    let [node_value, left, right] = tree.uncell(space)?;
    hashed_to_digests(node_value, space)?;
    if noun_equal(stack, value, node_value) {
        return Ok(tree);
    }

    if gor_hip(value, node_value, space)? {
        let child = h_set_put(stack, left, value, space)?;
        let [child_value, child_left, child_right] = child.uncell(space)?;
        if mor_hip(node_value, child_value, space)? {
            Ok(T(stack, &[node_value, child, right]))
        } else {
            let demoted = T(stack, &[node_value, child_right, right]);
            Ok(T(stack, &[child_value, child_left, demoted]))
        }
    } else {
        let child = h_set_put(stack, right, value, space)?;
        let [child_value, child_left, child_right] = child.uncell(space)?;
        if mor_hip(node_value, child_value, space)? {
            Ok(T(stack, &[node_value, left, child]))
        } else {
            let demoted = T(stack, &[node_value, left, child_left]);
            Ok(T(stack, &[child_value, demoted, child_right]))
        }
    }
}

fn h_map_empty() -> Noun {
    D(tas!(b"hmap"))
}

fn h_set_empty() -> Noun {
    D(tas!(b"hset"))
}

fn noun_identity(noun: Noun) -> u64 {
    unsafe { noun.as_raw() }
}

fn noun_equal<A: NounAllocator>(stack: &mut A, mut a: Noun, mut b: Noun) -> bool {
    unsafe { stack.equals(&mut a, &mut b) }
}

fn bool_to_noun(value: bool) -> Noun {
    if value {
        YES
    } else {
        NO
    }
}

fn gor_hip(a: Noun, b: Noun, space: &NounSpace) -> Result<bool, JetErr> {
    let a_digests = hashed_to_digests(a, space)?;
    let b_digests = hashed_to_digests(b, space)?;
    Ok(compare_digest_lists(
        a_digests.as_slice(),
        b_digests.as_slice(),
        compare_digest_high_to_low,
    ))
}

fn mor_hip(a: Noun, b: Noun, space: &NounSpace) -> Result<bool, JetErr> {
    let a_digests = hashed_to_digests(a, space)?;
    let b_digests = hashed_to_digests(b, space)?;
    Ok(compare_digest_lists(
        a_digests.as_slice(),
        b_digests.as_slice(),
        compare_digest_low_to_high,
    ))
}

fn hashed_to_digests(noun: Noun, space: &NounSpace) -> Result<HashedKey, JetErr> {
    if let Ok(digest) = digest_from_noun(noun, space) {
        return Ok(HashedKey::Single(digest));
    }

    let mut rest = noun;
    let mut digests = Vec::new();
    loop {
        if unsafe { rest.raw_equals(&D(0)) } {
            if digests.len() == 1 {
                return Err(BAIL_FAIL);
            }
            return Ok(HashedKey::List(digests));
        }
        let cell = rest.in_space(space).as_cell().map_err(|_| BAIL_FAIL)?;
        let head = cell.head().noun();
        digests.push(digest_from_noun(head, space)?);
        rest = cell.tail().noun();
    }
}

fn digest_from_noun(noun: Noun, space: &NounSpace) -> Result<[u64; 5], JetErr> {
    let first = noun.in_space(space).as_cell().map_err(|_| BAIL_FAIL)?;
    let first_head = first.head().noun();
    let first_tail = first.tail().noun();
    let second = first_tail
        .in_space(space)
        .as_cell()
        .map_err(|_| BAIL_FAIL)?;
    let second_head = second.head().noun();
    let second_tail = second.tail().noun();
    let third = second_tail
        .in_space(space)
        .as_cell()
        .map_err(|_| BAIL_FAIL)?;
    let third_head = third.head().noun();
    let third_tail = third.tail().noun();
    let fourth = third_tail
        .in_space(space)
        .as_cell()
        .map_err(|_| BAIL_FAIL)?;
    let fourth_head = fourth.head().noun();
    let fourth_tail = fourth.tail().noun();
    Ok([
        atom_to_u64(first_head, space)?,
        atom_to_u64(second_head, space)?,
        atom_to_u64(third_head, space)?,
        atom_to_u64(fourth_head, space)?,
        atom_to_u64(fourth_tail, space)?,
    ])
}

fn atom_to_u64(noun: Noun, space: &NounSpace) -> Result<u64, JetErr> {
    noun.in_space(space)
        .as_atom()
        .map_err(|_| BAIL_FAIL)?
        .as_u64()
        .map_err(|_| BAIL_FAIL)
}

fn compare_digest_lists(
    a: &[[u64; 5]],
    b: &[[u64; 5]],
    compare_digest: fn(&[u64; 5], &[u64; 5]) -> Option<bool>,
) -> bool {
    let mut index = 0;
    loop {
        match (a.get(index), b.get(index)) {
            (None, _) => return false,
            (Some(_), None) => return true,
            (Some(a_digest), Some(b_digest)) => {
                if let Some(ordered) = compare_digest(a_digest, b_digest) {
                    return ordered;
                }
            }
        }
        index += 1;
    }
}

fn compare_digest_lists_ordering(
    a: &[[u64; 5]],
    b: &[[u64; 5]],
    compare_digest: fn(&[u64; 5], &[u64; 5]) -> Option<bool>,
) -> Ordering {
    let mut index = 0;
    loop {
        match (a.get(index), b.get(index)) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a_digest), Some(b_digest)) => {
                if let Some(ordered) = compare_digest(a_digest, b_digest) {
                    return if ordered {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    };
                }
            }
        }
        index += 1;
    }
}

fn compare_digest_high_to_low(a: &[u64; 5], b: &[u64; 5]) -> Option<bool> {
    for index in (0..5).rev() {
        if a[index] > b[index] {
            return Some(true);
        }
        if a[index] < b[index] {
            return Some(false);
        }
    }
    None
}

fn compare_digest_low_to_high(a: &[u64; 5], b: &[u64; 5]) -> Option<bool> {
    for index in 0..5 {
        if a[index] > b[index] {
            return Some(true);
        }
        if a[index] < b[index] {
            return Some(false);
        }
    }
    None
}

fn cache_lookup_digest(
    context: &mut Context,
    tag: u64,
    noun: Noun,
) -> Result<Option<[u64; 5]>, JetErr> {
    let space = context.stack.noun_space();
    let mut key = T(&mut context.stack, &[D(tag), noun]);
    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(cached) => Ok(Some(<[u64; 5]>::from_noun(&cached, &space)?)),
        None => Ok(None),
    }
}

fn cache_insert_digest(context: &mut Context, tag: u64, noun: Noun, digest: [u64; 5]) {
    let mut key = T(&mut context.stack, &[D(tag), noun]);
    let value = digest_to_noundigest(&mut context.stack, digest);
    context.cache = context.cache.insert(&mut context.stack, &mut key, value);
}

fn get_tip_digest(context: &mut Context, noun: Noun) -> Result<[u64; 5], JetErr> {
    if let Some(cached) = cache_lookup_digest(context, TIP_CACHE_TAG, noun)? {
        return Ok(cached);
    }
    let space = context.stack.noun_space();
    let digest = hash_noun_varlen_digest(&mut context.stack, noun, &space)?;
    cache_insert_digest(context, TIP_CACHE_TAG, noun, digest);
    Ok(digest)
}

fn get_double_tip_digest(context: &mut Context, noun: Noun) -> Result<[u64; 5], JetErr> {
    if let Some(cached) = cache_lookup_digest(context, DOUBLE_TIP_CACHE_TAG, noun)? {
        return Ok(cached);
    }

    let tip_digest = get_tip_digest(context, noun)?;
    let mut input: Vec<Belt> = Vec::with_capacity(10);
    input.extend(tip_digest.into_iter().map(Belt));
    input.extend(tip_digest.into_iter().map(Belt));
    let digest = hash_10(&mut input);

    cache_insert_digest(context, DOUBLE_TIP_CACHE_TAG, noun, digest);
    Ok(digest)
}

// ===========================================================================
// h-by / h-in container arm jets (non-gate arms only).
//
// Each jet is a faithful reimplementation of the corresponding `~/`-hinted
// arm in open/hoon/common/h-zoon.hoon. Door arms: the arm sample is at
// `slot(subject, 6)`; the door sample (the map/set) is reached via the arm
// context `slot(subject, 7)` then `slot(door, 6)` (same convention as the
// nockvm +by/+in jets). Empty markers are %hmap / %hset; key ordering is
// gor-hip (descent) and mor-hip (priority), never `gor`/`mor`. Reuses the
// production-proven h_map_put / h_map_del / h_map_join / h_set_put helpers.
// ===========================================================================

fn h_door_sample(subject: Noun, space: &NounSpace) -> Result<Noun, JetErr> {
    let door = slot(subject, 7, space)?;
    slot(door, 6, space)
}

// ---- h-map (h-by) helpers ----

// +get: `(unit value)` — D(0) for ~, [0 value] for [~ u].
fn h_map_get_unit<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    key: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let mut node = tree;
    loop {
        if node.is_atom() {
            return Ok(D(0));
        }
        let [entry, left, right] = node.uncell(space)?;
        let [node_key, node_value] = entry.uncell(space)?;
        if noun_equal(stack, key, node_key) {
            return Ok(T(stack, &[D(0), node_value]));
        }
        node = if gor_hip(key, node_key, space)? {
            left
        } else {
            right
        };
    }
}

// +uni: treap merge; on equal key the right operand (b) wins.
fn h_map_uni<A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    b: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if b.is_atom() {
        return Ok(a);
    }
    if a.is_atom() {
        return Ok(b);
    }
    let [ae, al, ar] = a.uncell(space)?;
    let [ak, _av] = ae.uncell(space)?;
    let [be, bl, br] = b.uncell(space)?;
    let [bk, _bv] = be.uncell(space)?;
    if noun_equal(stack, bk, ak) {
        let nl = h_map_uni(stack, al, bl, space)?;
        let nr = h_map_uni(stack, ar, br, space)?;
        return Ok(T(stack, &[be, nl, nr]));
    }
    if mor_hip(ak, bk, space)? {
        if gor_hip(bk, ak, space)? {
            let bmod = T(stack, &[be, bl, h_map_empty()]);
            let inner = h_map_uni(stack, al, bmod, space)?;
            let a2 = T(stack, &[ae, inner, ar]);
            h_map_uni(stack, a2, br, space)
        } else {
            let bmod = T(stack, &[be, h_map_empty(), br]);
            let inner = h_map_uni(stack, ar, bmod, space)?;
            let a2 = T(stack, &[ae, al, inner]);
            h_map_uni(stack, a2, bl, space)
        }
    } else if gor_hip(ak, bk, space)? {
        let amod = T(stack, &[ae, al, h_map_empty()]);
        let inner = h_map_uni(stack, amod, bl, space)?;
        let b2 = T(stack, &[be, inner, br]);
        h_map_uni(stack, ar, b2, space)
    } else {
        let amod = T(stack, &[ae, h_map_empty(), ar]);
        let inner = h_map_uni(stack, amod, br, space)?;
        let b2 = T(stack, &[be, bl, inner]);
        h_map_uni(stack, al, b2, space)
    }
}

// +bif: split `tree` at `key`; returns (left, right), key dropped.
fn h_map_bif<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    key: Noun,
    space: &NounSpace,
) -> Result<(Noun, Noun), JetErr> {
    if tree.is_atom() {
        return Ok((h_map_empty(), h_map_empty()));
    }
    let [entry, left, right] = tree.uncell(space)?;
    let [node_key, _node_value] = entry.uncell(space)?;
    if noun_equal(stack, key, node_key) {
        return Ok((left, right));
    }
    if gor_hip(key, node_key, space)? {
        let (dl, dr) = h_map_bif(stack, left, key, space)?;
        Ok((dl, T(stack, &[entry, dr, right])))
    } else {
        let (dl, dr) = h_map_bif(stack, right, key, space)?;
        Ok((T(stack, &[entry, left, dl]), dr))
    }
}

// +int: treap intersection.
fn h_map_int<A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    b: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if b.is_atom() || a.is_atom() {
        return Ok(h_map_empty());
    }
    let [ae, al, ar] = a.uncell(space)?;
    let [ak, _av] = ae.uncell(space)?;
    let [be, bl, br] = b.uncell(space)?;
    let [bk, _bv] = be.uncell(space)?;
    if mor_hip(ak, bk, space)? {
        if noun_equal(stack, bk, ak) {
            let nl = h_map_int(stack, al, bl, space)?;
            let nr = h_map_int(stack, ar, br, space)?;
            Ok(T(stack, &[be, nl, nr]))
        } else if gor_hip(bk, ak, space)? {
            let bmod = T(stack, &[be, bl, h_map_empty()]);
            let x = h_map_int(stack, al, bmod, space)?;
            let y = h_map_int(stack, a, br, space)?;
            h_map_uni(stack, x, y, space)
        } else {
            let bmod = T(stack, &[be, h_map_empty(), br]);
            let x = h_map_int(stack, ar, bmod, space)?;
            let y = h_map_int(stack, a, bl, space)?;
            h_map_uni(stack, x, y, space)
        }
    } else if noun_equal(stack, ak, bk) {
        let nl = h_map_int(stack, al, bl, space)?;
        let nr = h_map_int(stack, ar, br, space)?;
        Ok(T(stack, &[be, nl, nr]))
    } else if gor_hip(ak, bk, space)? {
        let amod = T(stack, &[ae, al, h_map_empty()]);
        let x = h_map_int(stack, amod, bl, space)?;
        let y = h_map_int(stack, ar, b, space)?;
        h_map_uni(stack, x, y, space)
    } else {
        let amod = T(stack, &[ae, h_map_empty(), ar]);
        let x = h_map_int(stack, amod, br, space)?;
        let y = h_map_int(stack, al, b, space)?;
        h_map_uni(stack, x, y, space)
    }
}

// +dif: a without any key in b.
fn h_map_dif<A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    b: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if b.is_atom() {
        return Ok(a);
    }
    let [be, bl, br] = b.uncell(space)?;
    let [bk, _bv] = be.uncell(space)?;
    let (cl, cr) = h_map_bif(stack, a, bk, space)?;
    let d = h_map_dif(stack, cl, bl, space)?;
    let e = h_map_dif(stack, cr, br, space)?;
    h_map_join(stack, d, e, space)
}

// peg axis composition; None on u128 overflow (jet then punts to hoon).
fn peg(a: u128, b: u128) -> Option<u128> {
    if b == 0 {
        return None;
    }
    if b == 1 {
        return Some(a);
    }
    let d = 127u32 - b.leading_zeros();
    let span = 1u128.checked_shl(d)?;
    let low = b - span;
    a.checked_mul(span)?.checked_add(low)
}

fn axis_to_u64(axis: u128) -> Result<u64, JetErr> {
    if axis >= (1u128 << 62) {
        return Err(JetErr::Punt);
    }
    Ok(axis as u64)
}

// +dig: axis of key as `(unit @)`.
fn h_map_dig<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    key: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let mut node = tree;
    let mut axis: u128 = 1;
    loop {
        if node.is_atom() {
            return Ok(D(0));
        }
        let [entry, left, right] = node.uncell(space)?;
        let [node_key, _node_value] = entry.uncell(space)?;
        if noun_equal(stack, key, node_key) {
            let ax = peg(axis, 2).ok_or(JetErr::Punt)?;
            let ax = axis_to_u64(ax)?;
            return Ok(T(stack, &[D(0), D(ax)]));
        }
        if gor_hip(key, node_key, space)? {
            axis = peg(axis, 6).ok_or(JetErr::Punt)?;
            node = left;
        } else {
            axis = peg(axis, 7).ok_or(JetErr::Punt)?;
            node = right;
        }
    }
}

// fold +put over a `(list [p q])`.
fn h_map_gas<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    list: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let mut acc = tree;
    let mut rest = list;
    loop {
        if rest.is_atom() {
            return Ok(acc);
        }
        let [item, tail] = rest.uncell(space)?;
        let [pair_key, pair_value] = item.uncell(space)?;
        acc = h_map_put(stack, acc, pair_key, pair_value, space)?;
        rest = tail;
    }
}

// ---- h-set (h-in) helpers ----

fn h_set_has<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    item: Noun,
    space: &NounSpace,
) -> Result<bool, JetErr> {
    let mut node = tree;
    loop {
        if node.is_atom() {
            return Ok(false);
        }
        let [node_item, left, right] = node.uncell(space)?;
        if noun_equal(stack, item, node_item) {
            return Ok(true);
        }
        node = if gor_hip(item, node_item, space)? {
            left
        } else {
            right
        };
    }
}

fn h_set_join<A: NounAllocator>(
    stack: &mut A,
    left: Noun,
    right: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if left.is_atom() {
        return Ok(right);
    }
    if right.is_atom() {
        return Ok(left);
    }
    let [li, ll, lr] = left.uncell(space)?;
    let [ri, rl, rr] = right.uncell(space)?;
    if mor_hip(li, ri, space)? {
        let joined = h_set_join(stack, lr, right, space)?;
        Ok(T(stack, &[li, ll, joined]))
    } else {
        let joined = h_set_join(stack, left, rl, space)?;
        Ok(T(stack, &[ri, joined, rr]))
    }
}

fn h_set_del<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    item: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if tree.is_atom() {
        return Ok(h_set_empty());
    }
    let [node_item, left, right] = tree.uncell(space)?;
    if noun_equal(stack, item, node_item) {
        return h_set_join(stack, left, right, space);
    }
    if gor_hip(item, node_item, space)? {
        let child = h_set_del(stack, left, item, space)?;
        Ok(T(stack, &[node_item, child, right]))
    } else {
        let child = h_set_del(stack, right, item, space)?;
        Ok(T(stack, &[node_item, left, child]))
    }
}

fn h_set_uni<A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    b: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if noun_equal(stack, a, b) {
        return Ok(a);
    }
    if b.is_atom() {
        return Ok(a);
    }
    if a.is_atom() {
        return Ok(b);
    }
    let [an, al, ar] = a.uncell(space)?;
    let [bn, bl, br] = b.uncell(space)?;
    if noun_equal(stack, bn, an) {
        let nl = h_set_uni(stack, al, bl, space)?;
        let nr = h_set_uni(stack, ar, br, space)?;
        return Ok(T(stack, &[bn, nl, nr]));
    }
    if mor_hip(an, bn, space)? {
        if gor_hip(bn, an, space)? {
            let bmod = T(stack, &[bn, bl, h_set_empty()]);
            let inner = h_set_uni(stack, al, bmod, space)?;
            let a2 = T(stack, &[an, inner, ar]);
            h_set_uni(stack, a2, br, space)
        } else {
            let bmod = T(stack, &[bn, h_set_empty(), br]);
            let inner = h_set_uni(stack, ar, bmod, space)?;
            let a2 = T(stack, &[an, al, inner]);
            h_set_uni(stack, a2, bl, space)
        }
    } else if gor_hip(an, bn, space)? {
        let amod = T(stack, &[an, al, h_set_empty()]);
        let inner = h_set_uni(stack, amod, bl, space)?;
        let b2 = T(stack, &[bn, inner, br]);
        h_set_uni(stack, ar, b2, space)
    } else {
        let amod = T(stack, &[an, h_set_empty(), ar]);
        let inner = h_set_uni(stack, amod, br, space)?;
        let b2 = T(stack, &[bn, bl, inner]);
        h_set_uni(stack, al, b2, space)
    }
}

fn h_set_int<A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    b: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if b.is_atom() || a.is_atom() {
        return Ok(h_set_empty());
    }
    let [an, al, ar] = a.uncell(space)?;
    let [bn, bl, br] = b.uncell(space)?;
    if noun_equal(stack, bn, an) {
        let nl = h_set_int(stack, al, bl, space)?;
        let nr = h_set_int(stack, ar, br, space)?;
        return Ok(T(stack, &[an, nl, nr]));
    }
    if !mor_hip(an, bn, space)? {
        return h_set_int(stack, b, a, space);
    }
    if gor_hip(bn, an, space)? {
        let bmod = T(stack, &[bn, bl, h_set_empty()]);
        let x = h_set_int(stack, al, bmod, space)?;
        let y = h_set_int(stack, a, br, space)?;
        h_set_uni(stack, x, y, space)
    } else {
        let bmod = T(stack, &[bn, h_set_empty(), br]);
        let x = h_set_int(stack, ar, bmod, space)?;
        let y = h_set_int(stack, a, bl, space)?;
        h_set_uni(stack, x, y, space)
    }
}

// +bif (set): mirrors the `=< +` node-builder; tail is (left, right).
fn h_set_bif_node<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    item: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if tree.is_atom() {
        return Ok(T(stack, &[item, h_set_empty(), h_set_empty()]));
    }
    let [n, l, r] = tree.uncell(space)?;
    if noun_equal(stack, item, n) {
        return Ok(tree);
    }
    if gor_hip(item, n, space)? {
        let c = h_set_bif_node(stack, l, item, space)?;
        let [cn, cl, cr] = c.uncell(space)?;
        let a2 = T(stack, &[n, cr, r]);
        Ok(T(stack, &[cn, cl, a2]))
    } else {
        let c = h_set_bif_node(stack, r, item, space)?;
        let [cn, cl, cr] = c.uncell(space)?;
        let a2 = T(stack, &[n, l, cl]);
        Ok(T(stack, &[cn, a2, cr]))
    }
}

fn h_set_bif<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    item: Noun,
    space: &NounSpace,
) -> Result<(Noun, Noun), JetErr> {
    let node = h_set_bif_node(stack, tree, item, space)?;
    let [_n, l, r] = node.uncell(space)?;
    Ok((l, r))
}

fn h_set_dif<A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    b: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if b.is_atom() {
        return Ok(a);
    }
    let [bn, bl, br] = b.uncell(space)?;
    let (cl, cr) = h_set_bif(stack, a, bn, space)?;
    let d = h_set_dif(stack, cl, bl, space)?;
    let e = h_set_dif(stack, cr, br, space)?;
    h_set_join(stack, d, e, space)
}

fn h_set_dig<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    item: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let mut node = tree;
    let mut axis: u128 = 1;
    loop {
        if node.is_atom() {
            return Ok(D(0));
        }
        let [node_item, left, right] = node.uncell(space)?;
        if noun_equal(stack, item, node_item) {
            let ax = peg(axis, 2).ok_or(JetErr::Punt)?;
            let ax = axis_to_u64(ax)?;
            return Ok(T(stack, &[D(0), D(ax)]));
        }
        if gor_hip(item, node_item, space)? {
            axis = peg(axis, 6).ok_or(JetErr::Punt)?;
            node = left;
        } else {
            axis = peg(axis, 7).ok_or(JetErr::Punt)?;
            node = right;
        }
    }
}

fn h_set_gas<A: NounAllocator>(
    stack: &mut A,
    tree: Noun,
    list: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let mut acc = tree;
    let mut rest = list;
    loop {
        if rest.is_atom() {
            return Ok(acc);
        }
        let [item, tail] = rest.uncell(space)?;
        acc = h_set_put(stack, acc, item, space)?;
        rest = tail;
    }
}

// ---- h-by jet entrypoints ----

pub fn h_by_get_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let key = slot(subject, 6, &space)?;
    let map = h_door_sample(subject, &space)?;
    h_map_get_unit(&mut context.stack, map, key, &space)
}

pub fn h_by_got_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let key = slot(subject, 6, &space)?;
    let map = h_door_sample(subject, &space)?;
    let unit = h_map_get_unit(&mut context.stack, map, key, &space)?;
    if unit.is_atom() {
        return Err(BAIL_FAIL);
    }
    slot(unit, 3, &space)
}

pub fn h_by_gut_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let key = slot(sam, 2, &space)?;
    let default = slot(sam, 3, &space)?;
    let map = h_door_sample(subject, &space)?;
    let unit = h_map_get_unit(&mut context.stack, map, key, &space)?;
    if unit.is_atom() {
        Ok(default)
    } else {
        slot(unit, 3, &space)
    }
}

pub fn h_by_has_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let key = slot(subject, 6, &space)?;
    let map = h_door_sample(subject, &space)?;
    let unit = h_map_get_unit(&mut context.stack, map, key, &space)?;
    Ok(bool_to_noun(!unit.is_atom()))
}

pub fn h_by_put_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let key = slot(sam, 2, &space)?;
    let value = slot(sam, 3, &space)?;
    let map = h_door_sample(subject, &space)?;
    h_map_put(&mut context.stack, map, key, value, &space)
}

pub fn h_by_del_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let key = slot(subject, 6, &space)?;
    let map = h_door_sample(subject, &space)?;
    h_map_del(&mut context.stack, map, key, &space)
}

pub fn h_by_mar_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let key = slot(sam, 2, &space)?;
    let unit = slot(sam, 3, &space)?;
    let map = h_door_sample(subject, &space)?;
    if unit.is_atom() {
        h_map_del(&mut context.stack, map, key, &space)
    } else {
        let value = slot(unit, 3, &space)?;
        h_map_put(&mut context.stack, map, key, value, &space)
    }
}

pub fn h_by_gas_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let list = slot(subject, 6, &space)?;
    let map = h_door_sample(subject, &space)?;
    h_map_gas(&mut context.stack, map, list, &space)
}

pub fn h_by_uni_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let b = slot(subject, 6, &space)?;
    let a = h_door_sample(subject, &space)?;
    h_map_uni(&mut context.stack, a, b, &space)
}

pub fn h_by_int_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let b = slot(subject, 6, &space)?;
    let a = h_door_sample(subject, &space)?;
    h_map_int(&mut context.stack, a, b, &space)
}

pub fn h_by_dif_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let b = slot(subject, 6, &space)?;
    let a = h_door_sample(subject, &space)?;
    h_map_dif(&mut context.stack, a, b, &space)
}

pub fn h_by_bif_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let key = slot(subject, 6, &space)?;
    let map = h_door_sample(subject, &space)?;
    let (left, right) = h_map_bif(&mut context.stack, map, key, &space)?;
    Ok(T(&mut context.stack, &[left, right]))
}

pub fn h_by_dig_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let key = slot(subject, 6, &space)?;
    let map = h_door_sample(subject, &space)?;
    h_map_dig(&mut context.stack, map, key, &space)
}

// ---- h-in jet entrypoints ----

pub fn h_in_has_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let item = slot(subject, 6, &space)?;
    let set = h_door_sample(subject, &space)?;
    Ok(bool_to_noun(h_set_has(
        &mut context.stack, set, item, &space,
    )?))
}

pub fn h_in_put_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let item = slot(subject, 6, &space)?;
    let set = h_door_sample(subject, &space)?;
    h_set_put(&mut context.stack, set, item, &space)
}

pub fn h_in_del_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let item = slot(subject, 6, &space)?;
    let set = h_door_sample(subject, &space)?;
    h_set_del(&mut context.stack, set, item, &space)
}

pub fn h_in_gas_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let list = slot(subject, 6, &space)?;
    let set = h_door_sample(subject, &space)?;
    h_set_gas(&mut context.stack, set, list, &space)
}

pub fn h_in_uni_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let b = slot(subject, 6, &space)?;
    let a = h_door_sample(subject, &space)?;
    h_set_uni(&mut context.stack, a, b, &space)
}

pub fn h_in_int_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let b = slot(subject, 6, &space)?;
    let a = h_door_sample(subject, &space)?;
    h_set_int(&mut context.stack, a, b, &space)
}

pub fn h_in_dif_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let b = slot(subject, 6, &space)?;
    let a = h_door_sample(subject, &space)?;
    h_set_dif(&mut context.stack, a, b, &space)
}

pub fn h_in_bif_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let item = slot(subject, 6, &space)?;
    let set = h_door_sample(subject, &space)?;
    let (left, right) = h_set_bif(&mut context.stack, set, item, &space)?;
    Ok(T(&mut context.stack, &[left, right]))
}

pub fn h_in_dig_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let item = slot(subject, 6, &space)?;
    let set = h_door_sample(subject, &space)?;
    h_set_dig(&mut context.stack, set, item, &space)
}

#[cfg(test)]
mod tests {
    use ibig::UBig;
    use nockchain_math::zoon::common::DefaultTipHasher;
    use nockchain_math::zoon::zmap::z_map_put;
    use nockchain_math::zoon::zset::z_set_put;
    use nockvm::interpreter::Context;
    use nockvm::jets::util::test::{init_context, A};
    use nockvm::jets::util::BAIL_FAIL;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Noun, D, T};
    use nockvm::unifying_equality::unifying_equality;
    use quickcheck::{Arbitrary, Gen, QuickCheck};

    use super::*;

    #[test]
    fn dor_tip_matches_hoon_for_mixed_atom_cell_inputs() {
        let c = &mut init_context();
        let atom = D(7);
        let cell = T(&mut c.stack, &[D(1), D(2)]);

        assert!(
            cmp_with_jet(c, dor_tip_jet, atom, cell).expect("dor-tip jet should succeed"),
            "expected dor-tip atom<cell case to be true"
        );
        assert!(
            !cmp_with_jet(c, dor_tip_jet, cell, atom).expect("dor-tip jet should succeed"),
            "expected dor-tip cell<atom case to be false"
        );
    }

    #[test]
    fn gor_tip_reuses_tip_cache() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(1), D(2)]);
        let b = T(&mut c.stack, &[D(3), D(4)]);
        let subject = jet_subject(&mut c.stack, a, b);

        assert_eq!(cache_entries(c), 0);
        let _first = gor_tip_jet(c, subject).expect("gor-tip should succeed");
        let after_first = cache_entries(c);
        assert!(
            after_first >= 2,
            "expected tip cache entries for both nouns, found {after_first}"
        );

        let _second = gor_tip_jet(c, subject).expect("gor-tip should succeed");
        let after_second = cache_entries(c);
        assert_eq!(after_second, after_first);
    }

    #[test]
    fn mor_tip_reuses_tip_and_double_tip_cache() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(17), D(23)]);
        let b = T(&mut c.stack, &[D(19), D(29)]);
        let subject = jet_subject(&mut c.stack, a, b);

        assert_eq!(cache_entries(c), 0);
        let _first = mor_tip_jet(c, subject).expect("mor-tip should succeed");
        let after_mor = cache_entries(c);
        assert!(
            after_mor >= 4,
            "expected tip + double-tip cache entries for both nouns, found {after_mor}"
        );

        let _second = mor_tip_jet(c, subject).expect("mor-tip should succeed");
        let after_second_mor = cache_entries(c);
        assert_eq!(after_second_mor, after_mor);

        let _gor = gor_tip_jet(c, subject).expect("gor-tip should succeed");
        let after_gor = cache_entries(c);
        assert_eq!(after_gor, after_mor);
    }

    #[test]
    fn jets_error_on_malformed_sample_shape() {
        let c = &mut init_context();
        let malformed_subject = T(&mut c.stack, &[D(0), D(42), D(0)]);

        assert!(dor_tip_jet(c, malformed_subject).is_err());
        assert!(gor_tip_jet(c, malformed_subject).is_err());
        assert!(mor_tip_jet(c, malformed_subject).is_err());
        assert!(gor_hip_jet(c, malformed_subject).is_err());
        assert!(mor_hip_jet(c, malformed_subject).is_err());
    }

    #[test]
    fn gor_hip_orders_digest_limbs_high_to_low() {
        let c = &mut init_context();
        let high = digest(&mut c.stack, [0, 0, 0, 0, 2]);
        let low = digest(&mut c.stack, [99, 99, 99, 99, 1]);

        assert!(
            cmp_with_jet(c, gor_hip_jet, high, low).expect("gor-hip should succeed"),
            "gor-hip must compare the most significant digest limb first"
        );
        assert!(
            !cmp_with_jet(c, gor_hip_jet, low, high).expect("gor-hip should succeed"),
            "gor-hip reverse order should be false"
        );
    }

    #[test]
    fn mor_hip_orders_digest_limbs_low_to_high() {
        let c = &mut init_context();
        let high = digest(&mut c.stack, [2, 0, 0, 0, 0]);
        let low = digest(&mut c.stack, [1, 99, 99, 99, 99]);

        assert!(
            cmp_with_jet(c, mor_hip_jet, high, low).expect("mor-hip should succeed"),
            "mor-hip must compare the reversed digest limb order"
        );
        assert!(
            !cmp_with_jet(c, mor_hip_jet, low, high).expect("mor-hip should succeed"),
            "mor-hip reverse order should be false"
        );
    }

    #[test]
    fn hip_comparators_accept_digest_lists() {
        let c = &mut init_context();
        let prefix = digest(&mut c.stack, [1, 2, 3, 4, 5]);
        let second_high = digest(&mut c.stack, [0, 0, 0, 0, 7]);
        let second_low = digest(&mut c.stack, [0, 0, 0, 0, 6]);
        let a = list(&mut c.stack, &[prefix, second_high]);
        let b = list(&mut c.stack, &[prefix, second_low]);

        assert!(
            cmp_with_jet(c, gor_hip_jet, a, b).expect("gor-hip should succeed"),
            "gor-hip should walk digest lists after an equal prefix"
        );
        assert!(
            cmp_with_jet(c, mor_hip_jet, a, b).expect("mor-hip should succeed"),
            "mor-hip should walk digest lists after an equal prefix"
        );
    }

    #[test]
    fn hip_comparators_match_independent_digest_oracle() {
        const DIGEST_CASES: [([u64; 5], [u64; 5]); 6] = [
            ([0, 0, 0, 0, 2], [99, 99, 99, 99, 1]),
            ([2, 0, 0, 0, 0], [1, 99, 99, 99, 99]),
            ([7, 7, 7, 7, 7], [7, 7, 7, 7, 7]),
            ([0, 1, 2, 3, 4], [0, 1, 2, 3, 5]),
            ([9, 1, 2, 3, 4], [8, 1, 2, 3, 4]),
            ([3, 4, 5, 6, 7], [3, 4, 5, 6, 6]),
        ];
        let c = &mut init_context();

        for (left, right) in DIGEST_CASES {
            let a = digest(&mut c.stack, left);
            let b = digest(&mut c.stack, right);
            assert_eq!(
                cmp_with_jet(c, gor_hip_jet, a, b).expect("gor-hip should succeed"),
                digest_list_order_oracle(&[left], &[right], &GOR_HIP_ORDER),
                "gor-hip oracle mismatch for {left:?} vs {right:?}"
            );
            assert_eq!(
                cmp_with_jet(c, mor_hip_jet, a, b).expect("mor-hip should succeed"),
                digest_list_order_oracle(&[left], &[right], &MOR_HIP_ORDER),
                "mor-hip oracle mismatch for {left:?} vs {right:?}"
            );
        }

        let list_cases = [
            (
                vec![[1, 2, 3, 4, 5], [0, 0, 0, 0, 9]],
                vec![[1, 2, 3, 4, 5], [0, 0, 0, 0, 8]],
            ),
            (
                vec![[1, 2, 3, 4, 5], [8, 8, 8, 8, 8]],
                vec![[1, 2, 3, 4, 5], [9, 9, 9, 9, 9]],
            ),
            (
                vec![[1, 2, 3, 4, 5], [9, 9, 9, 9, 9]],
                vec![[1, 2, 3, 4, 5], [8, 8, 8, 8, 8]],
            ),
            (
                vec![[4, 4, 4, 4, 4], [5, 5, 5, 5, 5]],
                vec![[4, 4, 4, 4, 4], [5, 5, 5, 5, 5]],
            ),
        ];

        for (left, right) in list_cases {
            let a = digest_list(&mut c.stack, &left);
            let b = digest_list(&mut c.stack, &right);
            assert_eq!(
                cmp_with_jet(c, gor_hip_jet, a, b).expect("gor-hip should succeed"),
                digest_list_order_oracle(&left, &right, &GOR_HIP_ORDER),
                "gor-hip list oracle mismatch for {left:?} vs {right:?}"
            );
            assert_eq!(
                cmp_with_jet(c, mor_hip_jet, a, b).expect("mor-hip should succeed"),
                digest_list_order_oracle(&left, &right, &MOR_HIP_ORDER),
                "mor-hip list oracle mismatch for {left:?} vs {right:?}"
            );
        }
    }

    #[test]
    fn hip_comparators_pin_empty_digest_list_boundaries() {
        let c = &mut init_context();
        let empty = D(0);
        let value = digest(&mut c.stack, [1, 2, 3, 4, 5]);

        assert!(!cmp_with_jet(c, gor_hip_jet, empty, value).expect("gor-hip should succeed"));
        assert!(cmp_with_jet(c, gor_hip_jet, value, empty).expect("gor-hip should succeed"));
        assert!(!cmp_with_jet(c, gor_hip_jet, empty, empty).expect("gor-hip should succeed"));
        assert!(!cmp_with_jet(c, mor_hip_jet, empty, value).expect("mor-hip should succeed"));
        assert!(cmp_with_jet(c, mor_hip_jet, value, empty).expect("mor-hip should succeed"));
        assert!(!cmp_with_jet(c, mor_hip_jet, empty, empty).expect("mor-hip should succeed"));
    }

    #[test]
    fn hip_jets_error_on_malformed_keys() {
        let c = &mut init_context();
        let good = digest(&mut c.stack, [1, 2, 3, 4, 5]);
        let malformed_atom = D(42);
        let malformed_middle = list(&mut c.stack, &[good, D(42)]);
        let improper_tail = T(&mut c.stack, &[good, D(7)]);
        let singleton = digest_list(&mut c.stack, &[[1, 2, 3, 4, 5]]);

        for bad in [malformed_atom, malformed_middle, improper_tail, singleton] {
            assert!(
                cmp_with_jet(c, gor_hip_jet, bad, good).is_err(),
                "gor-hip accepted malformed key {bad:?}"
            );
            assert!(
                cmp_with_jet(c, mor_hip_jet, good, bad).is_err(),
                "mor-hip accepted malformed key {bad:?}"
            );
        }
    }

    #[test]
    fn zh_conversion_jets_reject_malformed_z_trees() {
        let c = &mut init_context();
        let key = digest(&mut c.stack, [1, 2, 3, 4, 5]);
        let value = digest(&mut c.stack, [6, 7, 8, 9, 10]);
        let malformed_key = D(42);
        let malformed_map_entry = T(&mut c.stack, &[malformed_key, value]);
        let malformed_map = T(&mut c.stack, &[malformed_map_entry, D(0), D(0)]);
        let improper_map_entry = T(&mut c.stack, &[key, value]);
        let improper_map = T(&mut c.stack, &[improper_map_entry, D(0)]);
        let malformed_set = T(&mut c.stack, &[malformed_key, D(0), D(0)]);
        let improper_set = T(&mut c.stack, &[key, D(0)]);

        assert!(run_unary_jet(c, zh_molt_jet, D(42)).is_err());
        assert!(run_unary_jet(c, zh_molt_jet, malformed_map).is_err());
        assert!(run_unary_jet(c, zh_molt_jet, improper_map).is_err());
        assert!(run_unary_jet(c, zh_silt_jet, D(42)).is_err());
        assert!(run_unary_jet(c, zh_silt_jet, malformed_set).is_err());
        assert!(run_unary_jet(c, zh_silt_jet, improper_set).is_err());
    }

    #[test]
    fn zh_conversion_jets_reject_non_strict_h_order_trees() {
        let c = &mut init_context();
        let limbs = [1, 2, 3, 4, 5];
        let digest_key = digest(&mut c.stack, limbs);
        let singleton_list_key = digest_list(&mut c.stack, &[limbs]);

        let map_entry_a = T(&mut c.stack, &[digest_key, D(10)]);
        let map_entry_b = T(&mut c.stack, &[singleton_list_key, D(20)]);
        let map_child = T(&mut c.stack, &[map_entry_b, D(0), D(0)]);
        let map = T(&mut c.stack, &[map_entry_a, D(0), map_child]);

        let set_child = T(&mut c.stack, &[singleton_list_key, D(0), D(0)]);
        let set = T(&mut c.stack, &[digest_key, D(0), set_child]);

        assert!(run_unary_jet(c, zh_molt_jet, map).is_err());
        assert!(run_unary_jet(c, zh_silt_jet, set).is_err());
    }

    #[test]
    fn small_digest_fast_path_rejects_singleton_digest_list_keys() {
        let c = &mut init_context();
        let limbs = [1, 2, 3, 4, 5];
        let second = [6, 7, 8, 9, 10];
        let direct = digest(&mut c.stack, limbs);
        let singleton = digest_list(&mut c.stack, &[limbs]);
        let pair = digest_list(&mut c.stack, &[limbs, second]);
        let too_long = digest_list(&mut c.stack, &[limbs, second, [11, 12, 13, 14, 15]]);
        let improper = T(&mut c.stack, &[direct, D(7)]);
        let malformed_singleton = T(&mut c.stack, &[D(42), D(0)]);
        let pair_key = SmallHashedKey::pair(limbs, second);
        let space = c.stack.noun_space();

        assert_eq!(
            small_hashed_key_from_noun(direct, &space),
            Some(SmallHashedKey::single(limbs))
        );
        assert_eq!(small_hashed_key_from_noun(singleton, &space), None);
        assert_eq!(small_hashed_key_from_noun(pair, &space), Some(pair_key));
        assert_eq!(
            small_hashed_key_from_noun(D(0), &space),
            Some(SmallHashedKey::empty())
        );
        assert_eq!(small_hashed_key_from_noun(too_long, &space), None);
        assert_eq!(small_hashed_key_from_noun(improper, &space), None);
        assert_eq!(
            small_hashed_key_from_noun(malformed_singleton, &space),
            None
        );
    }

    #[test]
    fn zh_conversion_jets_match_slow_oracles_for_short_digest_lists() {
        let c = &mut init_context();
        let key_a = digest_list(&mut c.stack, &[[1, 0, 0, 0, 0], [1, 1, 0, 0, 0]]);
        let key_b = digest_list(&mut c.stack, &[[0, 2, 0, 0, 0], [0, 2, 1, 0, 0]]);
        let key_c = digest(&mut c.stack, [0, 0, 3, 0, 0]);
        let key_d = digest_list(&mut c.stack, &[[0, 0, 0, 4, 0], [0, 0, 0, 0, 5]]);
        let map = z_map_from_entries(
            &mut c.stack,
            &[(key_a, D(10)), (key_b, D(20)), (key_c, D(30)), (key_d, D(40))],
        );
        let entry_a = T(&mut c.stack, &[key_a, D(10)]);
        let entry_b = T(&mut c.stack, &[key_b, D(20)]);
        let entry_c = T(&mut c.stack, &[key_c, D(30)]);
        let entry_d = T(&mut c.stack, &[key_d, D(40)]);
        let h_entries = vec![
            HMapEntry {
                noun: entry_a,
                key: key_a,
            },
            HMapEntry {
                noun: entry_b,
                key: key_b,
            },
            HMapEntry {
                noun: entry_c,
                key: key_c,
            },
            HMapEntry {
                noun: entry_d,
                key: key_d,
            },
        ];
        let space = c.stack.noun_space();
        assert!(small_h_map_build_nodes(&h_entries, &space).is_some());

        let converted_map = run_unary_jet(c, zh_molt_jet, map).expect("zh-molt should convert");
        let expected_map = slow_z_map_to_h_map(&mut c.stack, map).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_map, expected_map);

        let set = z_set_from_items(&mut c.stack, &[key_a, key_b, key_c, key_d]);
        let set_items = vec![key_a, key_b, key_c, key_d];
        assert!(small_h_set_build_nodes(&set_items, &space).is_some());

        let converted_set = run_unary_jet(c, zh_silt_jet, set).expect("zh-silt should convert");
        let expected_set = slow_z_set_to_h_set(&mut c.stack, set).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_set, expected_set);

        let inner_key_a = digest_list(&mut c.stack, &[[4, 0, 0, 0, 0], [4, 1, 0, 0, 0]]);
        let inner_key_b = digest_list(&mut c.stack, &[[0, 5, 0, 0, 0], [0, 0, 6, 0, 0]]);
        let inner_map =
            z_map_from_entries(&mut c.stack, &[(inner_key_a, D(40)), (inner_key_b, D(50))]);
        let outer_key_a = digest_list(&mut c.stack, &[[0, 0, 6, 0, 0], [0, 0, 6, 1, 0]]);
        let outer_key_b = digest_list(&mut c.stack, &[[0, 0, 0, 7, 0], [0, 0, 0, 7, 1]]);
        let mip = z_map_from_entries(
            &mut c.stack,
            &[(outer_key_a, inner_map), (outer_key_b, inner_map)],
        );
        let converted_mip = run_unary_jet(c, zh_milt_jet, mip).expect("zh-milt should convert");
        let expected_mip = slow_z_mip_to_h_mip(&mut c.stack, mip).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_mip, expected_mip);

        let inner_set = z_set_from_items(&mut c.stack, &[inner_key_a, inner_key_b]);
        let jug = z_map_from_entries(
            &mut c.stack,
            &[(outer_key_a, inner_set), (outer_key_b, inner_set)],
        );
        let converted_jug = run_unary_jet(c, zh_jult_jet, jug).expect("zh-jult should convert");
        let expected_jug = slow_z_jug_to_h_jug(&mut c.stack, jug).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_jug, expected_jug);
    }

    #[test]
    fn zh_milt_jet_preserves_arbitrary_inner_values() {
        let c = &mut init_context();
        let outer_key = digest(&mut c.stack, [1, 1, 1, 1, 1]);
        let inner_key = digest(&mut c.stack, [2, 2, 2, 2, 2]);
        let value_tail = T(&mut c.stack, &[D(99), D(100)]);
        let arbitrary_value = T(&mut c.stack, &[D(42), value_tail]);
        let inner = z_map_from_entries(&mut c.stack, &[(inner_key, arbitrary_value)]);
        let mip = z_map_from_entries(&mut c.stack, &[(outer_key, inner)]);

        let converted = run_unary_jet(c, zh_milt_jet, mip).expect("zh-milt should convert");
        let expected = slow_z_mip_to_h_mip(&mut c.stack, mip).expect("oracle should convert");

        assert_noun_eq(&mut c.stack, converted, expected);
    }

    #[test]
    fn balance_parent_diff_matches_generic_for_put_update_delete() {
        let c = &mut init_context();
        let key_a = digest(&mut c.stack, [1, 0, 0, 0, 0]);
        let key_b = digest_list(&mut c.stack, &[[2, 0, 0, 0, 0], [3, 0, 0, 0, 0]]);
        let key_c = digest(&mut c.stack, [4, 0, 0, 0, 0]);
        let key_d = digest_list(&mut c.stack, &[[5, 0, 0, 0, 0], [6, 0, 0, 0, 0]]);
        let note_a = T(&mut c.stack, &[D(10), D(11)]);
        let note_b = T(&mut c.stack, &[D(20), D(21)]);
        let note_c = T(&mut c.stack, &[D(30), D(31)]);
        let note_b_updated = T(&mut c.stack, &[D(22), D(23)]);
        let note_d = T(&mut c.stack, &[D(40), D(41)]);
        let parent_z = z_map_from_entries(
            &mut c.stack,
            &[(key_a, note_a), (key_b, note_b), (key_c, note_c)],
        );
        let child_z = z_map_from_entries(
            &mut c.stack,
            &[(key_a, note_a), (key_b, note_b_updated), (key_d, note_d)],
        );
        let parent_h = slow_z_map_to_h_map(&mut c.stack, parent_z).expect("parent converts");
        let space = c.stack.noun_space();

        let derived = h_map_from_parent_z_diff(&mut c.stack, parent_z, parent_h, child_z, &space)
            .expect("diff works");
        let expected = slow_z_map_to_h_map(&mut c.stack, child_z).expect("child converts");

        assert_noun_eq(&mut c.stack, derived, expected);
    }

    #[test]
    fn balance_parent_diff_rejects_singleton_key_shape_change() {
        let c = &mut init_context();
        let key_direct = digest(&mut c.stack, [8, 0, 0, 0, 0]);
        let key_list = digest_list(&mut c.stack, &[[8, 0, 0, 0, 0]]);
        let note = T(&mut c.stack, &[D(80), D(81)]);
        let parent_z = z_map_from_entries(&mut c.stack, &[(key_direct, note)]);
        let child_z = z_map_from_entries(&mut c.stack, &[(key_list, note)]);
        let parent_h = slow_z_map_to_h_map(&mut c.stack, parent_z).expect("parent converts");
        let space = c.stack.noun_space();

        assert!(
            h_map_from_parent_z_diff(&mut c.stack, parent_z, parent_h, child_z, &space).is_err()
        );
        assert!(slow_z_map_to_h_map(&mut c.stack, child_z).is_err());
    }

    #[test]
    fn zh_balance_milt_jet_matches_generic_milt_for_parent_chain() {
        let c = &mut init_context();
        let parent_block = digest(&mut c.stack, [11, 0, 0, 0, 0]);
        let child_block = digest(&mut c.stack, [12, 0, 0, 0, 0]);
        let missing_genesis_parent = digest(&mut c.stack, [10, 0, 0, 0, 0]);
        let parent_page = local_page_v0(&mut c.stack, parent_block, missing_genesis_parent);
        let child_page = local_page_v0(&mut c.stack, child_block, parent_block);
        let blocks = z_map_from_entries(
            &mut c.stack,
            &[(parent_block, parent_page), (child_block, child_page)],
        );

        let key_a = digest(&mut c.stack, [1, 1, 0, 0, 0]);
        let key_b = digest_list(&mut c.stack, &[[2, 1, 0, 0, 0], [3, 1, 0, 0, 0]]);
        let key_c = digest(&mut c.stack, [4, 1, 0, 0, 0]);
        let note_a = T(&mut c.stack, &[D(100), D(101)]);
        let note_b = T(&mut c.stack, &[D(200), D(201)]);
        let note_c = T(&mut c.stack, &[D(300), D(301)]);
        let parent_balance = z_map_from_entries(&mut c.stack, &[(key_a, note_a), (key_b, note_b)]);
        let child_balance = z_map_from_entries(&mut c.stack, &[(key_b, note_b), (key_c, note_c)]);
        let balance = z_map_from_entries(
            &mut c.stack,
            &[(parent_block, parent_balance), (child_block, child_balance)],
        );

        let sample = T(&mut c.stack, &[blocks, balance]);
        let converted =
            run_unary_jet(c, zh_balance_milt_jet, sample).expect("zh-balmilt should convert");
        let expected = slow_z_mip_to_h_mip(&mut c.stack, balance).expect("generic oracle converts");

        assert_noun_eq(&mut c.stack, converted, expected);
    }

    #[test]
    fn zh_balance_milt_jet_falls_back_for_non_digest_outer_balance_keys() {
        let c = &mut init_context();
        let block = digest(&mut c.stack, [41, 0, 0, 0, 0]);
        let parent = digest(&mut c.stack, [40, 0, 0, 0, 0]);
        let page = local_page_v0(&mut c.stack, block, parent);
        let blocks = z_map_from_entries(&mut c.stack, &[(block, page)]);

        let outer_key = digest_list(&mut c.stack, &[[42, 0, 0, 0, 0], [43, 0, 0, 0, 0]]);
        let inner_key = digest(&mut c.stack, [44, 0, 0, 0, 0]);
        let inner_value = T(&mut c.stack, &[D(440), D(441)]);
        let inner = z_map_from_entries(&mut c.stack, &[(inner_key, inner_value)]);
        let balance = z_map_from_entries(&mut c.stack, &[(outer_key, inner)]);
        let sample = T(&mut c.stack, &[blocks, balance]);

        let converted =
            run_unary_jet(c, zh_balance_milt_jet, sample).expect("zh-balmilt should convert");
        let expected = slow_z_mip_to_h_mip(&mut c.stack, balance).expect("generic oracle converts");

        assert_noun_eq(&mut c.stack, converted, expected);
    }

    #[test]
    fn zh_balance_milt_jet_matches_generic_milt_for_parent_cycle() {
        let c = &mut init_context();
        let block_a = digest(&mut c.stack, [21, 0, 0, 0, 0]);
        let block_b = digest(&mut c.stack, [22, 0, 0, 0, 0]);
        let page_a = local_page_v0(&mut c.stack, block_a, block_b);
        let page_b = local_page_v0(&mut c.stack, block_b, block_a);
        let blocks = z_map_from_entries(&mut c.stack, &[(block_a, page_a), (block_b, page_b)]);

        let key_a = digest(&mut c.stack, [31, 0, 0, 0, 0]);
        let key_b = digest_list(&mut c.stack, &[[32, 0, 0, 0, 0], [33, 0, 0, 0, 0]]);
        let key_c = digest(&mut c.stack, [34, 0, 0, 0, 0]);
        let note_a = T(&mut c.stack, &[D(410), D(411)]);
        let note_b = T(&mut c.stack, &[D(420), D(421)]);
        let note_c = T(&mut c.stack, &[D(430), D(431)]);
        let balance_a = z_map_from_entries(&mut c.stack, &[(key_a, note_a), (key_b, note_b)]);
        let balance_b = z_map_from_entries(&mut c.stack, &[(key_b, note_b), (key_c, note_c)]);
        let balance =
            z_map_from_entries(&mut c.stack, &[(block_a, balance_a), (block_b, balance_b)]);

        let sample = T(&mut c.stack, &[blocks, balance]);
        let converted =
            run_unary_jet(c, zh_balance_milt_jet, sample).expect("zh-balmilt should convert");
        let expected = slow_z_mip_to_h_mip(&mut c.stack, balance).expect("generic oracle converts");

        assert_noun_eq(&mut c.stack, converted, expected);
    }

    #[test]
    fn zh_balance_milt_jet_matches_generic_milt_for_long_mixed_history() {
        let c = &mut init_context();
        let mut keys = Vec::new();
        for index in 0..18 {
            let limb = index as u64 + 1;
            let key = match index % 3 {
                0 => digest(&mut c.stack, [limb, 0, 0, 0, 0]),
                1 => digest_list(&mut c.stack, &[[0, limb, 0, 0, 0], [0, limb, 1, 0, 0]]),
                _ => digest_list(&mut c.stack, &[[0, 0, limb, 0, 0], [0, 0, 0, limb, 0]]),
            };
            keys.push(key);
        }

        let mut block_entries: Vec<(Noun, Noun)> = Vec::new();
        let mut balance_entries: Vec<(Noun, Noun)> = Vec::new();
        let mut live_values = vec![None; keys.len()];
        let mut parent = digest(&mut c.stack, [900, 0, 0, 0, 0]);

        for height in 0..48 {
            let block = digest(
                &mut c.stack,
                [1_000 + height as u64, height as u64, 0, 0, 0],
            );
            let page = if height % 5 == 0 {
                local_page_v1(&mut c.stack, block, parent)
            } else {
                local_page_v0(&mut c.stack, block, parent)
            };
            block_entries.push((block, page));

            let primary = height % keys.len();
            if height % 11 == 5 {
                live_values[primary] = None;
            } else {
                live_values[primary] = Some(T(
                    &mut c.stack,
                    &[D(10_000 + height as u64), D(primary as u64)],
                ));
            }

            let secondary = (height * 7 + 3) % keys.len();
            if height % 4 == 0 {
                live_values[secondary] = Some(T(
                    &mut c.stack,
                    &[D(20_000 + height as u64), D(secondary as u64)],
                ));
            }

            if live_values.iter().all(Option::is_none) {
                live_values[0] = Some(T(&mut c.stack, &[D(30_000 + height as u64), D(0)]));
            }

            let mut entries = Vec::new();
            for (key, value) in keys.iter().zip(live_values.iter()) {
                if let Some(value) = value {
                    entries.push((*key, *value));
                }
            }

            let balance = if height % 13 == 0 && !balance_entries.is_empty() {
                balance_entries
                    .last()
                    .expect("previous balance should exist")
                    .1
            } else {
                z_map_from_entries(&mut c.stack, &entries)
            };
            balance_entries.push((block, balance));
            parent = block;
        }

        let blocks = z_map_from_entries(&mut c.stack, &block_entries);
        let balance = z_map_from_entries(&mut c.stack, &balance_entries);
        let sample = T(&mut c.stack, &[blocks, balance]);

        let converted =
            run_unary_jet(c, zh_balance_milt_jet, sample).expect("zh-balmilt should convert");
        let expected = slow_z_mip_to_h_mip(&mut c.stack, balance).expect("generic oracle converts");

        assert_noun_eq(&mut c.stack, converted, expected);
    }

    #[test]
    fn quickcheck_parent_diff_matches_generic_for_generated_note_maps() {
        fn prop(input: NoteMapDiffInput) -> bool {
            let c = &mut init_context();
            let (parent_z, child_z) = generated_note_map_pair(&mut c.stack, &input);
            let parent_h = match slow_z_map_to_h_map(&mut c.stack, parent_z) {
                Ok(parent_h) => parent_h,
                Err(_) => return false,
            };
            let space = c.stack.noun_space();
            let derived =
                match h_map_from_parent_z_diff(&mut c.stack, parent_z, parent_h, child_z, &space) {
                    Ok(derived) => derived,
                    Err(_) => return false,
                };
            let expected = match slow_z_map_to_h_map(&mut c.stack, child_z) {
                Ok(expected) => expected,
                Err(_) => return false,
            };

            noun_eq(c, derived, expected)
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(NoteMapDiffInput) -> bool);
    }

    #[test]
    fn quickcheck_zh_balance_milt_matches_generic_for_generated_histories() {
        fn prop(input: BalanceHistoryInput) -> bool {
            let c = &mut init_context();
            let (blocks, balance, _block_ids) = generated_balance_history(&mut c.stack, &input);
            let sample = T(&mut c.stack, &[blocks, balance]);
            let converted = match run_unary_jet(c, zh_balance_milt_jet, sample) {
                Ok(converted) => converted,
                Err(_) => return false,
            };
            let expected = match slow_z_mip_to_h_mip(&mut c.stack, balance) {
                Ok(expected) => expected,
                Err(_) => return false,
            };

            noun_eq(c, converted, expected)
        }

        QuickCheck::new()
            .tests(128)
            .quickcheck(prop as fn(BalanceHistoryInput) -> bool);
    }

    #[test]
    fn quickcheck_zh_balance_milt_bad_block_hints_match_generic() {
        fn prop(input: BalanceHistoryInput, atom_hint: bool) -> bool {
            let c = &mut init_context();
            let (_blocks, balance, block_ids) = generated_balance_history(&mut c.stack, &input);
            let bad_blocks = if atom_hint {
                D(42)
            } else {
                let key = block_ids
                    .first()
                    .copied()
                    .unwrap_or_else(|| digest(&mut c.stack, [1; 5]));
                z_map_from_entries(&mut c.stack, &[(key, D(7))])
            };
            let sample = T(&mut c.stack, &[bad_blocks, balance]);
            let converted = match run_unary_jet(c, zh_balance_milt_jet, sample) {
                Ok(converted) => converted,
                Err(_) => return false,
            };
            let expected = match slow_z_mip_to_h_mip(&mut c.stack, balance) {
                Ok(expected) => expected,
                Err(_) => return false,
            };

            noun_eq(c, converted, expected)
        }

        QuickCheck::new()
            .tests(128)
            .quickcheck(prop as fn(BalanceHistoryInput, bool) -> bool);
    }

    #[test]
    fn zh_balance_milt_falls_back_for_large_rotation_region() {
        let c = &mut init_context();
        let parent_block = digest(&mut c.stack, [71, 0, 0, 0, 0]);
        let child_block = digest(&mut c.stack, [72, 0, 0, 0, 0]);
        let grandparent_block = digest(&mut c.stack, [70; 5]);
        let parent_page = local_page_v0(&mut c.stack, parent_block, grandparent_block);
        let child_page = local_page_v0(&mut c.stack, child_block, parent_block);
        let blocks = z_map_from_entries(
            &mut c.stack,
            &[(parent_block, parent_page), (child_block, child_page)],
        );
        let parent_balance = generated_dense_note_map(&mut c.stack, 0x7100, 270);
        let child_balance = generated_dense_note_map(&mut c.stack, 0x7200, 270);
        let balance = z_map_from_entries(
            &mut c.stack,
            &[(parent_block, parent_balance), (child_block, child_balance)],
        );
        let parent_h = slow_z_map_to_h_map(&mut c.stack, parent_balance).expect("parent converts");
        let space = c.stack.noun_space();

        assert!(
            h_map_from_parent_z_diff(
                &mut c.stack, parent_balance, parent_h, child_balance, &space,
            )
            .is_err(),
            "large unmatched rotation regions should use generic conversion"
        );

        let sample = T(&mut c.stack, &[blocks, balance]);
        let converted =
            run_unary_jet(c, zh_balance_milt_jet, sample).expect("zh-balmilt should convert");
        let expected = slow_z_mip_to_h_mip(&mut c.stack, balance).expect("generic oracle converts");

        assert_noun_eq(&mut c.stack, converted, expected);
    }

    #[test]
    fn balance_parent_diff_bails_on_singleton_digest_list_keys() {
        let c = &mut init_context();
        let limbs = [88, 0, 0, 0, 0];
        let direct = digest(&mut c.stack, limbs);
        let singleton = digest_list(&mut c.stack, &[limbs]);
        let parent_key = digest(&mut c.stack, [89, 0, 0, 0, 0]);
        let parent_z = z_map_from_entries(&mut c.stack, &[(parent_key, D(1))]);
        let child_z = z_map_from_entries(&mut c.stack, &[(direct, D(2)), (singleton, D(3))]);
        let mut actions: Vec<HMapDiffAction> = Vec::new();
        let space = c.stack.noun_space();

        assert!(
            collect_rotated_z_map_diff(&mut c.stack, parent_z, child_z, &space, &mut actions)
                .is_err(),
            "singleton digest-list keys should reject before diff optimization"
        );
    }

    #[test]
    fn local_page_parent_reads_v0_and_v1_storage_shapes() {
        let c = &mut init_context();
        let block = digest(&mut c.stack, [7, 7, 7, 7, 7]);
        let parent = digest(&mut c.stack, [8, 8, 8, 8, 8]);
        let v0 = local_page_v0(&mut c.stack, block, parent);
        let v1 = local_page_v1(&mut c.stack, block, parent);
        let space = c.stack.noun_space();

        let v0_parent = local_page_parent(v0, &space).expect("v0 parent should parse");
        let v1_parent = local_page_parent(v1, &space).expect("v1 parent should parse");

        assert_noun_eq(&mut c.stack, v0_parent, parent);
        assert_noun_eq(&mut c.stack, v1_parent, parent);
    }

    #[test]
    fn zh_nested_conversion_jets_validate_inner_container_keys() {
        let c = &mut init_context();
        let outer_key = digest(&mut c.stack, [1, 1, 1, 1, 1]);
        let malformed_map_entry = T(&mut c.stack, &[D(42), D(7)]);
        let bad_inner_map = T(&mut c.stack, &[malformed_map_entry, D(0), D(0)]);
        let bad_mip = z_map_from_entries(&mut c.stack, &[(outer_key, bad_inner_map)]);

        let jug_key = digest(&mut c.stack, [3, 3, 3, 3, 3]);
        let bad_set = T(&mut c.stack, &[D(42), D(0), D(0)]);
        let bad_jug = z_map_from_entries(&mut c.stack, &[(jug_key, bad_set)]);

        assert!(run_unary_jet(c, zh_milt_jet, bad_mip).is_err());
        assert!(run_unary_jet(c, zh_jult_jet, bad_jug).is_err());
    }

    #[test]
    fn zh_conversion_jets_match_slow_put_oracles_for_mixed_shapes() {
        let c = &mut init_context();
        let key_a = digest(&mut c.stack, [1, 0, 0, 0, 0]);
        let key_b = digest_list(&mut c.stack, &[[2, 0, 0, 0, 0], [3, 0, 0, 0, 0]]);
        let key_c = digest(&mut c.stack, [4, 0, 0, 0, 0]);
        let value_a = D(42);
        let value_b_tail = T(&mut c.stack, &[D(7), D(8)]);
        let value_b = T(&mut c.stack, &[D(6), value_b_tail]);
        let value_c = digest(&mut c.stack, [9, 0, 0, 0, 0]);
        let map = z_map_from_entries(
            &mut c.stack,
            &[(key_a, value_a), (key_b, value_b), (key_c, value_c)],
        );
        let converted_map = run_unary_jet(c, zh_molt_jet, map).expect("zh-molt should convert");
        let expected_map = slow_z_map_to_h_map(&mut c.stack, map).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_map, expected_map);

        let set = z_set_from_items(&mut c.stack, &[key_a, key_b, key_c]);
        let converted_set = run_unary_jet(c, zh_silt_jet, set).expect("zh-silt should convert");
        let expected_set = slow_z_set_to_h_set(&mut c.stack, set).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_set, expected_set);

        let inner_key_a = digest(&mut c.stack, [0, 1, 0, 0, 0]);
        let inner_key_b = digest_list(&mut c.stack, &[[0, 2, 0, 0, 0], [0, 3, 0, 0, 0]]);
        let first_value = T(&mut c.stack, &[D(1), D(2)]);
        let second_value = T(&mut c.stack, &[D(3), D(4)]);
        let replacement_value = T(&mut c.stack, &[D(5), D(6)]);
        let inner_map_a = z_map_from_entries(
            &mut c.stack,
            &[
                (inner_key_a, first_value),
                (inner_key_b, second_value),
                (inner_key_a, replacement_value),
            ],
        );
        let inner_map_b = z_map_from_entries(
            &mut c.stack,
            &[(inner_key_b, value_c), (inner_key_a, value_b)],
        );
        let outer_key_a = digest(&mut c.stack, [5, 0, 0, 0, 0]);
        let outer_key_b = digest(&mut c.stack, [6, 0, 0, 0, 0]);
        let mip = z_map_from_entries(
            &mut c.stack,
            &[(outer_key_a, inner_map_a), (outer_key_b, inner_map_b)],
        );
        let converted_mip = run_unary_jet(c, zh_milt_jet, mip).expect("zh-milt should convert");
        let expected_mip = slow_z_mip_to_h_mip(&mut c.stack, mip).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_mip, expected_mip);

        let inner_set_a = z_set_from_items(&mut c.stack, &[inner_key_a, inner_key_b]);
        let inner_set_b = z_set_from_items(&mut c.stack, &[inner_key_b, key_c]);
        let jug = z_map_from_entries(
            &mut c.stack,
            &[(outer_key_a, inner_set_a), (outer_key_b, inner_set_b)],
        );
        let converted_jug = run_unary_jet(c, zh_jult_jet, jug).expect("zh-jult should convert");
        let expected_jug = slow_z_jug_to_h_jug(&mut c.stack, jug).expect("oracle should convert");
        assert_noun_eq(&mut c.stack, converted_jug, expected_jug);
    }

    #[test]
    fn gor_tip_errors_on_non_decodable_tip_cache_entry() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(1), D(2)]);
        let b = T(&mut c.stack, &[D(3), D(4)]);
        inject_bad_cache_value(c, TIP_CACHE_TAG, a);

        let subject = jet_subject(&mut c.stack, a, b);
        assert!(gor_tip_jet(c, subject).is_err());
    }

    #[test]
    fn mor_tip_errors_on_non_decodable_double_tip_cache_entry() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(5), D(6)]);
        let b = T(&mut c.stack, &[D(7), D(8)]);
        inject_bad_cache_value(c, DOUBLE_TIP_CACHE_TAG, a);

        let subject = jet_subject(&mut c.stack, a, b);
        assert!(mor_tip_jet(c, subject).is_err());
    }

    #[test]
    fn gor_and_mor_error_on_non_u64_atom_inputs() {
        let c = &mut init_context();
        let huge_atom = A(&mut c.stack, &(UBig::from(1u128) << 64));
        let other = D(1);

        let subject = jet_subject(&mut c.stack, huge_atom, other);
        assert!(gor_tip_jet(c, subject).is_err());
        assert!(mor_tip_jet(c, subject).is_err());
    }

    #[test]
    fn quickcheck_order_laws_for_dor_gor_mor() {
        fn prop(a: BoundedNounInput, b: BoundedNounInput, c_input: BoundedNounInput) -> bool {
            let context = &mut init_context();
            let an = bounded_noun_from_input(&mut context.stack, &a);
            let bn = bounded_noun_from_input(&mut context.stack, &b);
            let cn = bounded_noun_from_input(&mut context.stack, &c_input);

            let dor_ab =
                cmp_with_jet(context, dor_tip_jet, an, bn).expect("dor comparison should succeed");
            let dor_ba =
                cmp_with_jet(context, dor_tip_jet, bn, an).expect("dor comparison should succeed");
            let dor_bc =
                cmp_with_jet(context, dor_tip_jet, bn, cn).expect("dor comparison should succeed");
            let dor_ac =
                cmp_with_jet(context, dor_tip_jet, an, cn).expect("dor comparison should succeed");

            if !dor_ab && !dor_ba {
                return false;
            }
            if dor_ab && dor_ba && !noun_eq(context, an, bn) {
                return false;
            }
            if dor_ab && dor_bc && !dor_ac {
                return false;
            }

            for jet in [gor_tip_jet, mor_tip_jet] {
                let ab = cmp_with_jet(context, jet, an, bn).expect("comparison should succeed");
                let ba = cmp_with_jet(context, jet, bn, an).expect("comparison should succeed");
                let bc = cmp_with_jet(context, jet, bn, cn).expect("comparison should succeed");
                let ac = cmp_with_jet(context, jet, an, cn).expect("comparison should succeed");

                if !ab && !ba {
                    return false; // totality
                }
                if ab && ba && !noun_eq(context, an, bn) {
                    return false; // antisymmetry
                }
                if ab && bc && !ac {
                    return false; // transitivity
                }
            }

            true
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(BoundedNounInput, BoundedNounInput, BoundedNounInput) -> bool);
    }

    #[test]
    fn quickcheck_cold_vs_warm_cache_equivalence() {
        fn prop(a: BoundedNounInput, b: BoundedNounInput) -> bool {
            let mut cold = init_context();
            let cold_a = bounded_noun_from_input(&mut cold.stack, &a);
            let cold_b = bounded_noun_from_input(&mut cold.stack, &b);

            let cold_dor =
                cmp_with_jet(&mut cold, dor_tip_jet, cold_a, cold_b).expect("dor should succeed");
            let cold_gor =
                cmp_with_jet(&mut cold, gor_tip_jet, cold_a, cold_b).expect("gor should succeed");
            let cold_mor =
                cmp_with_jet(&mut cold, mor_tip_jet, cold_a, cold_b).expect("mor should succeed");

            let mut warm = init_context();
            let warm_a = bounded_noun_from_input(&mut warm.stack, &a);
            let warm_b = bounded_noun_from_input(&mut warm.stack, &b);

            let warm_first_dor =
                cmp_with_jet(&mut warm, dor_tip_jet, warm_a, warm_b).expect("dor should succeed");
            let warm_first_gor =
                cmp_with_jet(&mut warm, gor_tip_jet, warm_a, warm_b).expect("gor should succeed");
            let warm_first_mor =
                cmp_with_jet(&mut warm, mor_tip_jet, warm_a, warm_b).expect("mor should succeed");

            let warm_second_dor =
                cmp_with_jet(&mut warm, dor_tip_jet, warm_a, warm_b).expect("dor should succeed");
            let warm_second_gor =
                cmp_with_jet(&mut warm, gor_tip_jet, warm_a, warm_b).expect("gor should succeed");
            let warm_second_mor =
                cmp_with_jet(&mut warm, mor_tip_jet, warm_a, warm_b).expect("mor should succeed");

            cold_dor == warm_first_dor
                && cold_gor == warm_first_gor
                && cold_mor == warm_first_mor
                && warm_first_dor == warm_second_dor
                && warm_first_gor == warm_second_gor
                && warm_first_mor == warm_second_mor
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(BoundedNounInput, BoundedNounInput) -> bool);
    }

    // The +gor-hip / +mor-hip total-order properties must hold over the entire
    // digest input space; the migration's +apt invariant on h-* containers is a
    // direct consequence. Random triples exercise both the single-digest fast
    // path and the digest-list lex path, including equal-prefix list ties.
    #[test]
    fn quickcheck_hip_total_order_laws() {
        fn prop(a: HipKeyInput, b: HipKeyInput, c_input: HipKeyInput) -> bool {
            let context = &mut init_context();
            let an = noun_for_hip_key(&mut context.stack, &a);
            let bn = noun_for_hip_key(&mut context.stack, &b);
            let cn = noun_for_hip_key(&mut context.stack, &c_input);

            for jet in [gor_hip_jet, mor_hip_jet] {
                let ab = cmp_with_jet(context, jet, an, bn).expect("hip jet should succeed");
                let ba = cmp_with_jet(context, jet, bn, an).expect("hip jet should succeed");
                let bc = cmp_with_jet(context, jet, bn, cn).expect("hip jet should succeed");
                let ac = cmp_with_jet(context, jet, an, cn).expect("hip jet should succeed");

                let eq_ab = noun_eq(context, an, bn);
                let eq_bc = noun_eq(context, bn, cn);

                // totality: gor(a, b) || gor(b, a) || a == b
                if !ab && !ba && !eq_ab {
                    return false;
                }
                // antisymmetry: not both ab and ba unless a == b
                if ab && ba && !eq_ab {
                    return false;
                }
                // transitivity through a "strict-less" lifting
                let strict_ab = ab && !eq_ab;
                let strict_bc = bc && !eq_bc;
                if strict_ab && strict_bc && !ac {
                    return false;
                }
            }

            // mor-hip relates to gor-hip by reversing each digest's limb order
            // (gor compares limbs 4..0, mor compares 0..4).
            let reversed_a = reverse_limbs_for_key(&a);
            let reversed_b = reverse_limbs_for_key(&b);
            let an_rev = noun_for_hip_key(&mut context.stack, &reversed_a);
            let bn_rev = noun_for_hip_key(&mut context.stack, &reversed_b);

            let mor_ab = cmp_with_jet(context, mor_hip_jet, an, bn).expect("mor-hip");
            let gor_rev =
                cmp_with_jet(context, gor_hip_jet, an_rev, bn_rev).expect("gor-hip reversed");
            if mor_ab != gor_rev {
                return false;
            }

            // gor-hip on digest lists must agree with an independent lex oracle.
            let limbs_a = limbs_for_key(&a);
            let limbs_b = limbs_for_key(&b);
            let oracle_gor = digest_list_order_oracle(&limbs_a, &limbs_b, &GOR_HIP_ORDER);
            let oracle_mor = digest_list_order_oracle(&limbs_a, &limbs_b, &MOR_HIP_ORDER);
            let jet_gor = cmp_with_jet(context, gor_hip_jet, an, bn).expect("gor-hip");
            let jet_mor = cmp_with_jet(context, mor_hip_jet, an, bn).expect("mor-hip");
            if oracle_gor != jet_gor {
                return false;
            }
            if oracle_mor != jet_mor {
                return false;
            }

            true
        }

        QuickCheck::new()
            .tests(1024)
            .quickcheck(prop as fn(HipKeyInput, HipKeyInput, HipKeyInput) -> bool);
    }

    // Adversarial fuzz for the h-zoon migration jets. Anywhere a noun crosses
    // from peer/disk into the migration we model the noun as malicious and
    // require either correct acceptance + round-trip identity, or clean
    // rejection via JetErr. No panic, no silent type confusion.
    //
    // The legitimate construction first feeds a small z-map / z-set built by
    // `z_map_put` / `z_set_put`; the mutated variants then derive adversarial
    // inputs by:
    //   - swapping a digest key for a non-digest atom (tag confusion)
    //   - truncating the digest list to 4 limbs (shape confusion)
    //   - injecting a %ztree marker (0x6565727a74) into an arm position
    //   - flipping the left/right children of an internal z-tree node so
    //     `apt` no longer holds
    //
    // Property:
    //   conversion(mutated)  is Ok  =>  round-trip back to z is identical
    //   conversion(mutated)  is Err =>  no panic, JetErr is observed
    #[test]
    fn quickcheck_zh_molt_rejects_mutated_z_maps() {
        fn prop(input: SoftFuzzInput) -> bool {
            let c = &mut init_context();
            let map = build_seed_z_map(&mut c.stack, &input);
            let mutated = mutate_z_tree(&mut c.stack, map, &input);

            // The jet must never panic on the mutated noun, but it may
            // return either Ok (when the mutation preserved the validity
            // constraints) or Err (when validity broke). On Ok we check
            // the inverse oracle agrees.
            match run_unary_jet(c, zh_molt_jet, mutated) {
                Ok(converted) => {
                    // If we got Ok, the noun is a well-formed z-map (the
                    // jet validates) and conversion must agree with the
                    // independent slow oracle.
                    match slow_z_map_to_h_map(&mut c.stack, mutated) {
                        Ok(expected) => noun_eq(c, converted, expected),
                        Err(_) => {
                            // jet accepted but oracle rejected — divergence
                            false
                        }
                    }
                }
                Err(_) => true,
            }
        }

        QuickCheck::new()
            .tests(2048)
            .quickcheck(prop as fn(SoftFuzzInput) -> bool);
    }

    #[test]
    fn quickcheck_zh_silt_rejects_mutated_z_sets() {
        fn prop(input: SoftFuzzInput) -> bool {
            let c = &mut init_context();
            let set = build_seed_z_set(&mut c.stack, &input);
            let mutated = mutate_z_tree(&mut c.stack, set, &input);

            match run_unary_jet(c, zh_silt_jet, mutated) {
                Ok(converted) => match slow_z_set_to_h_set(&mut c.stack, mutated) {
                    Ok(expected) => noun_eq(c, converted, expected),
                    Err(_) => false,
                },
                Err(_) => true,
            }
        }

        QuickCheck::new()
            .tests(2048)
            .quickcheck(prop as fn(SoftFuzzInput) -> bool);
    }

    // Marker non-confusion: %ztree (the atom 0x7a74726565) is not a valid
    // runtime leaf for any z-container. We verify the conversion jets reject
    // any tree that contains %ztree at any internal position, and that the
    // comparator jets reject %ztree-as-key likewise.
    #[test]
    fn ztree_marker_in_z_tree_is_rejected_by_conversion_jets() {
        let c = &mut init_context();
        let key = digest(&mut c.stack, [1, 2, 3, 4, 5]);
        let value = D(99);
        let entry = T(&mut c.stack, &[key, value]);
        let ztree_marker = D(0x6565727a74); // %ztree as a direct atom
        let with_marker_left = T(&mut c.stack, &[entry, ztree_marker, D(0)]);
        let with_marker_right = T(&mut c.stack, &[entry, D(0), ztree_marker]);
        let just_marker = ztree_marker;

        for bad in [with_marker_left, with_marker_right, just_marker] {
            assert!(
                run_unary_jet(c, zh_molt_jet, bad).is_err(),
                "zh-molt accepted %ztree marker as z-map"
            );
            // For zh-silt we need a singular entry shape, not a pair shape.
            let set_with_marker_left = T(&mut c.stack, &[key, ztree_marker, D(0)]);
            let set_with_marker_right = T(&mut c.stack, &[key, D(0), ztree_marker]);
            for set_bad in [set_with_marker_left, set_with_marker_right] {
                assert!(
                    run_unary_jet(c, zh_silt_jet, set_bad).is_err(),
                    "zh-silt accepted %ztree marker as z-set"
                );
            }
        }
    }

    // hset / hmap tag confusion: %hmap / %hset markers must not be accepted
    // by the z-side conversion jets either.
    #[test]
    fn h_markers_in_z_input_are_rejected_by_conversion_jets() {
        let c = &mut init_context();
        let hmap_marker = D(0x70616d68); // %hmap
        let hset_marker = D(0x74657368); // %hset
        let key = digest(&mut c.stack, [1, 2, 3, 4, 5]);
        let value = D(11);
        let entry = T(&mut c.stack, &[key, value]);

        // empty leaf where a z-tree expects ~ (i.e. D(0))
        assert!(run_unary_jet(c, zh_molt_jet, hmap_marker).is_err());
        assert!(run_unary_jet(c, zh_silt_jet, hset_marker).is_err());

        // empty leaf in a child position
        let with_hmap_l = T(&mut c.stack, &[entry, hmap_marker, D(0)]);
        let with_hset_r = T(&mut c.stack, &[key, D(0), hset_marker]);
        assert!(run_unary_jet(c, zh_molt_jet, with_hmap_l).is_err());
        assert!(run_unary_jet(c, zh_silt_jet, with_hset_r).is_err());
    }

    // Empty-list and equal-prefix edge cases are pinned points already
    // covered by `test-hip-ordering-prefix-and-empty-boundaries`. This block
    // confirms the same invariants survive random pairings drawn around the
    // empty-list and equal-prefix neighborhoods.
    #[test]
    fn quickcheck_hip_empty_and_prefix_boundaries() {
        fn prop(a: HipBoundaryInput, b: HipBoundaryInput) -> bool {
            let context = &mut init_context();
            let an = noun_for_boundary(&mut context.stack, &a);
            let bn = noun_for_boundary(&mut context.stack, &b);

            // Comparators must succeed on every valid boundary noun without panic.
            let _ = cmp_with_jet(context, gor_hip_jet, an, bn).expect("gor-hip boundary");
            let _ = cmp_with_jet(context, mor_hip_jet, an, bn).expect("mor-hip boundary");

            // The independent lex oracle must agree.
            let la = limbs_for_boundary(&a);
            let lb = limbs_for_boundary(&b);
            let jet_g = cmp_with_jet(context, gor_hip_jet, an, bn).expect("gor-hip");
            let jet_m = cmp_with_jet(context, mor_hip_jet, an, bn).expect("mor-hip");
            jet_g == digest_list_order_oracle(&la, &lb, &GOR_HIP_ORDER)
                && jet_m == digest_list_order_oracle(&la, &lb, &MOR_HIP_ORDER)
        }

        QuickCheck::new()
            .tests(512)
            .quickcheck(prop as fn(HipBoundaryInput, HipBoundaryInput) -> bool);
    }

    fn cache_entries(context: &Context) -> usize {
        context.cache.iter().map(|pairs| pairs.len()).sum()
    }

    fn inject_bad_cache_value(context: &mut Context, tag: u64, noun: Noun) {
        let mut key = T(&mut context.stack, &[D(tag), noun]);
        context.cache = context.cache.insert(&mut context.stack, &mut key, D(7));
    }

    fn cmp_with_jet(
        context: &mut Context,
        jet: fn(&mut Context, Noun) -> Result<Noun, JetErr>,
        a: Noun,
        b: Noun,
    ) -> Result<bool, JetErr> {
        let subject = jet_subject(&mut context.stack, a, b);
        let result = jet(context, subject)?;
        if unsafe { result.raw_equals(&YES) } {
            Ok(true)
        } else if unsafe { result.raw_equals(&NO) } {
            Ok(false)
        } else {
            panic!("comparison jet should return %.y/%.n, got: {result:?}");
        }
    }

    fn noun_eq(context: &mut Context, a: Noun, b: Noun) -> bool {
        let mut an = a;
        let mut bn = b;
        unsafe { unifying_equality(&mut context.stack, &mut an, &mut bn) }
    }

    fn jet_subject(stack: &mut nockvm::mem::NockStack, a: Noun, b: Noun) -> Noun {
        let sam = T(stack, &[a, b]);
        T(stack, &[D(0), sam, D(0)])
    }

    fn digest(stack: &mut NockStack, limbs: [u64; 5]) -> Noun {
        T(
            stack,
            &[
                D(direct_limb(limbs[0])),
                D(direct_limb(limbs[1])),
                D(direct_limb(limbs[2])),
                D(direct_limb(limbs[3])),
                D(direct_limb(limbs[4])),
            ],
        )
    }

    fn list(stack: &mut NockStack, items: &[Noun]) -> Noun {
        items
            .iter()
            .rev()
            .fold(D(0), |tail, item| T(stack, &[*item, tail]))
    }

    fn run_unary_jet(
        context: &mut Context,
        jet: fn(&mut Context, Noun) -> Result<Noun, JetErr>,
        sample: Noun,
    ) -> Result<Noun, JetErr> {
        let subject = unary_jet_subject(&mut context.stack, sample);
        jet(context, subject)
    }

    fn unary_jet_subject(stack: &mut NockStack, sample: Noun) -> Noun {
        T(stack, &[D(0), sample, D(0)])
    }

    fn z_map_from_entries(stack: &mut NockStack, entries: &[(Noun, Noun)]) -> Noun {
        let mut map = D(0);
        for (key, value) in entries {
            let mut key = *key;
            let mut value = *value;
            map = z_map_put(stack, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("z-map construction should succeed");
        }
        map
    }

    fn z_set_from_items(stack: &mut NockStack, items: &[Noun]) -> Noun {
        let mut set = D(0);
        for item in items {
            let mut item = *item;
            set = z_set_put(stack, &set, &mut item, &DefaultTipHasher)
                .expect("z-set construction should succeed");
        }
        set
    }

    fn slow_z_map_to_h_map(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
        let space = stack.noun_space();
        let mut entries = Vec::new();
        collect_z_map_entries(tree, &space, &mut entries)?;
        let mut map = h_map_empty();
        for (key, value) in entries {
            map = h_map_put(stack, map, key, value, &space)?;
        }
        Ok(map)
    }

    fn slow_z_set_to_h_set(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
        let space = stack.noun_space();
        let mut items = Vec::new();
        collect_z_set_items(tree, &space, &mut items)?;
        let mut set = h_set_empty();
        for item in items {
            set = h_set_put(stack, set, item, &space)?;
        }
        Ok(set)
    }

    fn slow_z_mip_to_h_mip(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
        let space = stack.noun_space();
        let mut entries = Vec::new();
        collect_z_map_entries(tree, &space, &mut entries)?;
        let mut map = h_map_empty();
        for (outer_key, inner_map) in entries {
            let converted_inner = slow_z_map_to_h_map(stack, inner_map)?;
            map = h_map_put(stack, map, outer_key, converted_inner, &space)?;
        }
        Ok(map)
    }

    fn slow_z_jug_to_h_jug(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
        let space = stack.noun_space();
        let mut entries = Vec::new();
        collect_z_map_entries(tree, &space, &mut entries)?;
        let mut map = h_map_empty();
        for (outer_key, inner_set) in entries {
            let converted_inner = slow_z_set_to_h_set(stack, inner_set)?;
            map = h_map_put(stack, map, outer_key, converted_inner, &space)?;
        }
        Ok(map)
    }

    fn collect_z_map_entries(
        tree: Noun,
        space: &NounSpace,
        entries: &mut Vec<(Noun, Noun)>,
    ) -> Result<(), JetErr> {
        if unsafe { tree.raw_equals(&D(0)) } {
            return Ok(());
        }
        if tree.is_atom() {
            return Err(BAIL_FAIL);
        }

        let [entry, left, right] = tree.uncell(space)?;
        let [key, value] = entry.uncell(space)?;
        entries.push((key, value));
        collect_z_map_entries(left, space, entries)?;
        collect_z_map_entries(right, space, entries)
    }

    fn collect_z_set_items(
        tree: Noun,
        space: &NounSpace,
        items: &mut Vec<Noun>,
    ) -> Result<(), JetErr> {
        if unsafe { tree.raw_equals(&D(0)) } {
            return Ok(());
        }
        if tree.is_atom() {
            return Err(BAIL_FAIL);
        }

        let [value, left, right] = tree.uncell(space)?;
        items.push(value);
        collect_z_set_items(left, space, items)?;
        collect_z_set_items(right, space, items)
    }

    fn assert_noun_eq(stack: &mut NockStack, mut left: Noun, mut right: Noun) {
        assert!(
            unsafe { stack.equals(&mut left, &mut right) },
            "nouns are not structurally equal: {left:?} != {right:?}"
        );
    }

    const GOR_HIP_ORDER: [usize; 5] = [4, 3, 2, 1, 0];
    const MOR_HIP_ORDER: [usize; 5] = [0, 1, 2, 3, 4];

    fn digest_list(stack: &mut NockStack, digests: &[[u64; 5]]) -> Noun {
        let items: Vec<Noun> = digests.iter().map(|limbs| digest(stack, *limbs)).collect();
        list(stack, &items)
    }

    fn local_page_v0(stack: &mut NockStack, block: Noun, parent: Noun) -> Noun {
        T(
            stack,
            &[block, D(0), parent, D(0), D(0), D(0), D(0), D(0), D(0), D(0), D(0)],
        )
    }

    fn local_page_v1(stack: &mut NockStack, block: Noun, parent: Noun) -> Noun {
        T(
            stack,
            &[D(1), block, D(0), parent, D(0), D(0), D(0), D(0), D(0), D(0), D(0), D(0)],
        )
    }

    fn digest_list_order_oracle(a: &[[u64; 5]], b: &[[u64; 5]], order: &[usize; 5]) -> bool {
        let mut index = 0;
        loop {
            match (a.get(index), b.get(index)) {
                (None, _) => return false,
                (Some(_), None) => return true,
                (Some(a_digest), Some(b_digest)) => {
                    if let Some(ordered) = digest_order_oracle(a_digest, b_digest, order) {
                        return ordered;
                    }
                }
            }
            index += 1;
        }
    }

    fn digest_order_oracle(a: &[u64; 5], b: &[u64; 5], order: &[usize; 5]) -> Option<bool> {
        for index in order {
            if a[*index] > b[*index] {
                return Some(true);
            }
            if a[*index] < b[*index] {
                return Some(false);
            }
        }
        None
    }

    #[derive(Clone, Debug)]
    struct BoundedNounInput {
        seed: u64,
        depth: u8,
    }

    impl Arbitrary for BoundedNounInput {
        fn arbitrary(g: &mut Gen) -> Self {
            Self {
                seed: u64::arbitrary(g),
                depth: (u8::arbitrary(g) % 6) + 1,
            }
        }
    }

    #[derive(Clone, Debug)]
    struct NoteMapDiffInput {
        seed: u64,
        notes: u8,
    }

    impl Arbitrary for NoteMapDiffInput {
        fn arbitrary(g: &mut Gen) -> Self {
            Self {
                seed: nonzero_seed(u64::arbitrary(g)),
                notes: (u8::arbitrary(g) % 32) + 1,
            }
        }
    }

    #[derive(Clone, Debug)]
    struct BalanceHistoryInput {
        seed: u64,
        blocks: u8,
        notes: u8,
    }

    impl Arbitrary for BalanceHistoryInput {
        fn arbitrary(g: &mut Gen) -> Self {
            Self {
                seed: nonzero_seed(u64::arbitrary(g)),
                blocks: (u8::arbitrary(g) % 48) + 1,
                notes: (u8::arbitrary(g) % 32) + 1,
            }
        }
    }

    // A `hashed` key for the +gor-hip / +mor-hip comparators.
    //
    // Generated keys must be either a single noun-digest:tip5 (a 5-tuple of
    // u63-bounded limbs) or a digest list with length 0 or at least 2. We bias the generator
    // toward shared prefixes so equal-prefix list comparisons stress the
    // "ties on prefix, decide on later limb" branch of the comparator.
    #[derive(Clone, Debug)]
    struct HipKeyInput {
        single: bool,
        digests: Vec<[u64; 5]>,
    }

    impl Arbitrary for HipKeyInput {
        fn arbitrary(g: &mut Gen) -> Self {
            let single = bool::arbitrary(g);
            let length = if single {
                1
            } else {
                (u8::arbitrary(g) % 3 + 2) as usize
            };

            // Keep limbs small to maximise equality-on-prefix collisions while
            // still spanning the comparator's interesting limb-difference cases.
            let bias = u8::arbitrary(g) % 8;
            let bound: u64 = if bias < 4 {
                4
            } else if bias < 7 {
                64
            } else {
                u64::MAX >> 1
            };

            let mut digests = Vec::with_capacity(length);
            for _ in 0..length {
                let mut limbs = [0u64; 5];
                for slot in limbs.iter_mut() {
                    *slot = (u64::arbitrary(g) & (u64::MAX >> 1)) % bound.max(1);
                }
                digests.push(limbs);
            }
            Self { single, digests }
        }
    }

    fn limbs_for_key(key: &HipKeyInput) -> Vec<[u64; 5]> {
        key.digests.clone()
    }

    fn noun_for_hip_key(stack: &mut NockStack, key: &HipKeyInput) -> Noun {
        if key.single {
            digest(stack, key.digests[0])
        } else {
            digest_list(stack, &key.digests)
        }
    }

    fn reverse_limbs_for_key(key: &HipKeyInput) -> HipKeyInput {
        let reversed = key
            .digests
            .iter()
            .map(|limbs| {
                let mut r = *limbs;
                r.reverse();
                r
            })
            .collect();
        HipKeyInput {
            single: key.single,
            digests: reversed,
        }
    }

    // Boundary input: includes the empty digest list (which the comparators
    // treat as the minimum) and short digest lists in its neighborhood.
    #[derive(Clone, Debug)]
    enum HipBoundaryInput {
        Empty,
        Single([u64; 5]),
        Pair([u64; 5], [u64; 5]),
    }

    impl Arbitrary for HipBoundaryInput {
        fn arbitrary(g: &mut Gen) -> Self {
            let limbs_for = |g: &mut Gen| {
                let bound = (u8::arbitrary(g) % 4 + 1) as u64;
                let mut limbs = [0u64; 5];
                for slot in limbs.iter_mut() {
                    *slot = u64::arbitrary(g) % bound;
                }
                limbs
            };
            match u8::arbitrary(g) % 3 {
                0 => HipBoundaryInput::Empty,
                1 => HipBoundaryInput::Single(limbs_for(g)),
                _ => HipBoundaryInput::Pair(limbs_for(g), limbs_for(g)),
            }
        }
    }

    fn noun_for_boundary(stack: &mut NockStack, input: &HipBoundaryInput) -> Noun {
        match input {
            HipBoundaryInput::Empty => D(0),
            HipBoundaryInput::Single(limbs) => digest(stack, *limbs),
            HipBoundaryInput::Pair(a, b) => digest_list(stack, &[*a, *b]),
        }
    }

    fn limbs_for_boundary(input: &HipBoundaryInput) -> Vec<[u64; 5]> {
        match input {
            HipBoundaryInput::Empty => Vec::new(),
            HipBoundaryInput::Single(limbs) => vec![*limbs],
            HipBoundaryInput::Pair(a, b) => vec![*a, *b],
        }
    }

    // Input drawing for adversarial soft-cast fuzz.
    //
    // `size` controls how many entries the seed z-* container has before
    // mutation. We keep it small (1..=4) because the mutation alone is the
    // adversary; what matters is breadth of mutation kinds and seed shape,
    // not container size.
    #[derive(Clone, Debug)]
    struct SoftFuzzInput {
        seed: u64,
        size: u8,
        mutation: u8,
    }

    impl Arbitrary for SoftFuzzInput {
        fn arbitrary(g: &mut Gen) -> Self {
            Self {
                seed: nonzero_seed(u64::arbitrary(g)),
                size: (u8::arbitrary(g) % 4) + 1,
                mutation: u8::arbitrary(g) % 8,
            }
        }
    }

    fn build_seed_z_map(stack: &mut NockStack, input: &SoftFuzzInput) -> Noun {
        let mut state = input.seed;
        let mut entries = Vec::new();
        for index in 0..(input.size as usize) {
            let limbs = [
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
            ];
            let key = if index % 2 == 0 {
                digest(stack, limbs)
            } else {
                let second = [
                    limbs[0].wrapping_add(1) & (u64::MAX >> 1),
                    limbs[1],
                    limbs[2],
                    limbs[3],
                    limbs[4],
                ];
                digest_list(stack, &[limbs, second])
            };
            entries.push((key, D((index as u64).wrapping_add(1))));
        }
        z_map_from_entries(stack, &entries)
    }

    fn build_seed_z_set(stack: &mut NockStack, input: &SoftFuzzInput) -> Noun {
        let mut state = input.seed;
        let mut items = Vec::new();
        for index in 0..(input.size as usize) {
            let limbs = [
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
                next_u64(&mut state) & (u64::MAX >> 1),
            ];
            let item = if index % 2 == 0 {
                digest(stack, limbs)
            } else {
                let second = [
                    limbs[0].wrapping_add(1) & (u64::MAX >> 1),
                    limbs[1],
                    limbs[2],
                    limbs[3],
                    limbs[4],
                ];
                digest_list(stack, &[limbs, second])
            };
            items.push(item);
        }
        z_set_from_items(stack, &items)
    }

    fn mutate_z_tree(stack: &mut NockStack, tree: Noun, input: &SoftFuzzInput) -> Noun {
        let space = stack.noun_space();
        if unsafe { tree.raw_equals(&D(0)) } {
            // mutate the empty tree by replacing the leaf with a marker
            return D(0x6565727a74); // %ztree
        }
        match input.mutation % 8 {
            0 => tree, // identity (control: must succeed)
            1 => {
                // Swap left/right at the root (likely breaks h-order
                // invariant; the jet must detect this).
                let Ok([entry, left, right]) = tree.uncell(&space) else {
                    return tree;
                };
                T(stack, &[entry, right, left])
            }
            2 => {
                // Replace the root's left child with a %ztree marker.
                let Ok([entry, _left, right]) = tree.uncell(&space) else {
                    return tree;
                };
                T(stack, &[entry, D(0x6565727a74), right])
            }
            3 => {
                // Replace the root's right child with a %hset marker.
                let Ok([entry, left, _right]) = tree.uncell(&space) else {
                    return tree;
                };
                T(stack, &[entry, left, D(0x74657368)])
            }
            4 => {
                // Replace the root entry's key with a non-digest atom (forces
                // the jet's small-key parser into the slow oracle path,
                // which then rejects).
                let Ok([entry, left, right]) = tree.uncell(&space) else {
                    return tree;
                };
                if let Ok([_key, value]) = entry.uncell(&space) {
                    let bad_key = D(42);
                    let new_entry = T(stack, &[bad_key, value]);
                    T(stack, &[new_entry, left, right])
                } else {
                    tree
                }
            }
            5 => {
                // Force the root entry to be a single-atom cell (improper
                // pair, breaks z-map shape).
                let Ok([entry, left, right]) = tree.uncell(&space) else {
                    return tree;
                };
                T(stack, &[entry, left, right]) // structural identity
                                                // (no-op when valid; for sets
                                                // adds zero coverage)
            }
            6 => {
                // Truncate a single-digest key to 4 limbs (improper pair).
                let Ok([entry, left, right]) = tree.uncell(&space) else {
                    return tree;
                };
                if let Ok([key, value_or_l_set]) = entry.uncell(&space) {
                    if let Ok([a, b, c_, d, _e]) = key.uncell_chain_5(&space) {
                        let bad_key = T(stack, &[a, b, c_, d]);
                        let new_entry = T(stack, &[bad_key, value_or_l_set]);
                        return T(stack, &[new_entry, left, right]);
                    }
                }
                tree
            }
            _ => {
                // Append spurious data after the right subtree (improper).
                let Ok([entry, left, right]) = tree.uncell(&space) else {
                    return tree;
                };
                let bad_tail = T(stack, &[right, D(0xdeadbeef)]);
                T(stack, &[entry, left, bad_tail])
            }
        }
    }

    // Helper to destructure a flat 5-tuple of atoms.
    trait UncellChain5 {
        fn uncell_chain_5(self, space: &NounSpace) -> Result<[Noun; 5], JetErr>;
    }

    impl UncellChain5 for Noun {
        fn uncell_chain_5(self, space: &NounSpace) -> Result<[Noun; 5], JetErr> {
            let [a, rest1] = self.uncell(space)?;
            let [b, rest2] = rest1.uncell(space)?;
            let [c_, rest3] = rest2.uncell(space)?;
            let [d, e] = rest3.uncell(space)?;
            Ok([a, b, c_, d, e])
        }
    }

    fn generated_note_map_pair(stack: &mut NockStack, input: &NoteMapDiffInput) -> (Noun, Noun) {
        let note_count = input.notes as usize;
        let mut parent_entries = Vec::new();
        let mut child_entries = Vec::new();
        let mut state = input.seed;

        for index in 0..note_count {
            let token = next_u64(&mut state);
            let parent_present = !token.is_multiple_of(5);
            let child_present = match token % 7 {
                0 => false,
                1..=3 => true,
                _ => parent_present,
            };
            let parent_variant = (token % 3) as u8;
            let parent_key = generated_note_key(stack, input.seed, index, parent_variant);
            let child_variant = if token.is_multiple_of(11) {
                ((parent_variant + 1) % 3) as u8
            } else {
                parent_variant
            };
            let child_key = generated_note_key(stack, input.seed, index, child_variant);
            let parent_value = generated_note_value(stack, input.seed, index, 0);
            let child_value = generated_note_value(stack, input.seed ^ token, index, 1);

            if parent_present {
                parent_entries.push((parent_key, parent_value));
            }
            if child_present {
                child_entries.push((child_key, child_value));
            }
        }

        (
            z_map_from_entries(stack, &parent_entries),
            z_map_from_entries(stack, &child_entries),
        )
    }

    fn generated_balance_history(
        stack: &mut NockStack,
        input: &BalanceHistoryInput,
    ) -> (Noun, Noun, Vec<Noun>) {
        let block_count = input.blocks as usize;
        let note_count = input.notes as usize;
        let mut state = input.seed;
        let mut block_ids = Vec::with_capacity(block_count);
        for height in 0..block_count {
            block_ids.push(generated_block_id(stack, input.seed, height));
        }

        let mut block_entries = Vec::with_capacity(block_count);
        let mut balance_entries = Vec::with_capacity(block_count);
        let mut live_values = vec![None; note_count];
        let mut last_balance = D(0);

        for height in 0..block_count {
            let block = block_ids[height];
            let parent = generated_parent_id(stack, input.seed, height, &block_ids, &mut state);
            let page = if next_u64(&mut state) & 1 == 0 {
                local_page_v0(stack, block, parent)
            } else {
                local_page_v1(stack, block, parent)
            };
            block_entries.push((block, page));

            let reuse_previous = height > 0 && next_u64(&mut state).is_multiple_of(13);
            let z_balance = if reuse_previous {
                last_balance
            } else {
                mutate_live_notes(stack, input.seed, height, &mut state, &mut live_values);
                let mut note_entries = Vec::new();
                for (index, value) in live_values.iter().enumerate() {
                    if let Some(value) = value {
                        let variant = ((input.seed >> (index % 8)) as u8)
                            .wrapping_add(height as u8)
                            .wrapping_add(index as u8);
                        let key = generated_note_key(stack, input.seed, index, variant);
                        note_entries.push((key, *value));
                    }
                }
                z_map_from_entries(stack, &note_entries)
            };
            balance_entries.push((block, z_balance));
            last_balance = z_balance;
        }

        (
            z_map_from_entries(stack, &block_entries),
            z_map_from_entries(stack, &balance_entries),
            block_ids,
        )
    }

    fn mutate_live_notes(
        stack: &mut NockStack,
        seed: u64,
        height: usize,
        state: &mut u64,
        live_values: &mut [Option<Noun>],
    ) {
        let mutations = (next_u64(state) % 3) + 1;
        for mutation in 0..mutations {
            let index = (next_u64(state) as usize) % live_values.len();
            if next_u64(state).is_multiple_of(7) {
                live_values[index] = None;
            } else {
                live_values[index] = Some(generated_note_value(
                    stack,
                    seed,
                    index,
                    height + mutation as usize,
                ));
            }
        }
        if live_values.iter().all(Option::is_none) {
            live_values[0] = Some(generated_note_value(stack, seed, 0, height));
        }
    }

    fn generated_parent_id(
        stack: &mut NockStack,
        seed: u64,
        height: usize,
        block_ids: &[Noun],
        state: &mut u64,
    ) -> Noun {
        if height == 0 || next_u64(state).is_multiple_of(17) {
            return digest(stack, [seed, height as u64, 0xdead, 0xbeef, 0]);
        }
        if height > 2 && next_u64(state).is_multiple_of(7) {
            block_ids[height - 2]
        } else {
            block_ids[height - 1]
        }
    }

    fn generated_dense_note_map(stack: &mut NockStack, seed: u64, count: usize) -> Noun {
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            let key = generated_note_key(stack, seed, index, 0);
            let value = generated_note_value(stack, seed, index, 0);
            entries.push((key, value));
        }
        z_map_from_entries(stack, &entries)
    }

    fn generated_block_id(stack: &mut NockStack, seed: u64, height: usize) -> Noun {
        digest(
            stack,
            [
                seed.wrapping_add(0x1000),
                height as u64,
                seed.rotate_left(11),
                height as u64 ^ 0x5555,
                seed.wrapping_mul(31).wrapping_add(height as u64),
            ],
        )
    }

    fn generated_note_key(stack: &mut NockStack, seed: u64, index: usize, variant: u8) -> Noun {
        let first = [
            seed.wrapping_add(index as u64).wrapping_add(1),
            seed.rotate_left(7).wrapping_add(index as u64 * 3),
            index as u64 ^ 0xa5a5,
            seed.rotate_right(9),
            seed.wrapping_mul(17).wrapping_add(index as u64),
        ];
        let second = [
            seed.wrapping_add(index as u64).wrapping_add(0x100),
            seed.rotate_left(13),
            index as u64 ^ 0x5a5a,
            seed.rotate_right(3).wrapping_add(index as u64),
            seed.wrapping_mul(19),
        ];
        let third = [
            seed.wrapping_add(index as u64).wrapping_add(0x200),
            seed.rotate_left(17),
            index as u64 ^ 0x3333,
            seed.rotate_right(5),
            seed.wrapping_mul(23).wrapping_add(index as u64),
        ];

        match variant % 4 {
            0 => digest(stack, first),
            1 => digest_list(stack, &[first, third]),
            2 => digest_list(stack, &[first, second]),
            _ => digest_list(stack, &[first, second, third]),
        }
    }

    fn generated_note_value(
        stack: &mut NockStack,
        seed: u64,
        index: usize,
        version: usize,
    ) -> Noun {
        T(
            stack,
            &[
                D(direct_limb(seed.wrapping_add(version as u64))),
                D(index as u64),
                D((version as u64) << 32 | index as u64),
            ],
        )
    }

    fn nonzero_seed(seed: u64) -> u64 {
        if seed == 0 {
            1
        } else {
            seed
        }
    }

    fn bounded_noun_from_input(stack: &mut NockStack, input: &BoundedNounInput) -> Noun {
        let mut state = if input.seed == 0 { 1 } else { input.seed };
        bounded_noun_from_state(stack, &mut state, input.depth)
    }

    fn bounded_noun_from_state(stack: &mut NockStack, state: &mut u64, depth: u8) -> Noun {
        let token = next_u64(state);
        let atom = D(token % 1_000_000_000);

        if depth == 0 || (token & 0b11) == 0 {
            atom
        } else {
            let left = bounded_noun_from_state(stack, state, depth - 1);
            let right = bounded_noun_from_state(stack, state, depth - 1);
            T(stack, &[left, right])
        }
    }

    fn next_u64(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    fn direct_limb(value: u64) -> u64 {
        value & (u64::MAX >> 1)
    }
}
