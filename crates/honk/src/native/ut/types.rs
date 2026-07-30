use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::Arc;

use hatch::ast::hoon::WingType;
use nockapp::Noun;
use nockvm::noun::{NounAllocator, NounSpace};
use num_bigint::BigUint;

use crate::errors::Result;
use crate::native::ir::formula_dag::FormulaId;
use crate::native::ir::ty::{Type as NTy, TypeRef as NRc};
use crate::native::ut::{noun_eq, Ut};

// Compiler inputs are not attacker-controlled; prefer a fast, non-cryptographic hasher for
// hot-path internal caches (notably `find`/`cool` on large molds like hoon-138).
pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
pub type FastHashSet<K> = HashSet<K, BuildHasherDefault<FastHasher>>;

pub struct RawMemoMap<K, V> {
    pub values: FastHashMap<K, V>,
    pub order: VecDeque<K>,
}

impl<K, V> Default for RawMemoMap<K, V> {
    fn default() -> Self {
        Self {
            values: Default::default(),
            order: VecDeque::new(),
        }
    }
}

impl<K, V> RawMemoMap<K, V>
where
    K: Eq + Hash + Copy,
    V: Clone,
{
    pub fn get(&self, key: &K) -> Option<V> {
        self.values.get(key).cloned()
    }

    pub fn insert_with_limit(&mut self, key: K, value: V, key_limit: usize) {
        if !self.values.contains_key(&key) {
            self.order.push_back(key);
            if self.order.len() > key_limit {
                if let Some(evict) = self.order.pop_front() {
                    self.values.remove(&evict);
                }
            }
        }
        self.values.insert(key, value);
    }

    pub fn clear(&mut self) {
        self.values.clear();
        self.order.clear();
    }
}

pub struct RawMemoSet<K> {
    pub values: FastHashSet<K>,
    pub order: VecDeque<K>,
}

impl<K> Default for RawMemoSet<K> {
    fn default() -> Self {
        Self {
            values: Default::default(),
            order: VecDeque::new(),
        }
    }
}

impl<K> RawMemoSet<K>
where
    K: Eq + Hash + Copy,
{
    pub fn contains(&self, key: &K) -> bool {
        self.values.contains(key)
    }

    pub fn insert_with_limit(&mut self, key: K, key_limit: usize) {
        if self.values.insert(key) {
            self.order.push_back(key);
            if self.order.len() > key_limit {
                if let Some(evict) = self.order.pop_front() {
                    self.values.remove(&evict);
                }
            }
        }
    }
}

pub struct BucketMemo<K, E> {
    pub buckets: FastHashMap<K, VecDeque<E>>,
    pub order: VecDeque<K>,
}

impl<K, E> Default for BucketMemo<K, E> {
    fn default() -> Self {
        Self {
            buckets: Default::default(),
            order: VecDeque::new(),
        }
    }
}

impl<K, E> BucketMemo<K, E>
where
    K: Eq + Hash + Copy,
{
    pub fn get(&self, key: &K) -> Option<&VecDeque<E>> {
        self.buckets.get(key)
    }

    pub fn ensure_key(&mut self, key: K, key_limit: usize) -> &mut VecDeque<E> {
        if !self.buckets.contains_key(&key) {
            self.order.push_back(key);
            if self.order.len() > key_limit {
                if let Some(evict) = self.order.pop_front() {
                    self.buckets.remove(&evict);
                }
            }
        }
        self.buckets.entry(key).or_default()
    }

    pub fn clear(&mut self) {
        self.buckets.clear();
        self.order.clear();
    }
}

pub type CoreMintBoundaryKey = (u32, u32, u32, u8, u8, u64, u64, u64);
// Native re-key (Phase-2 tail): the bran/seminoun subject is the DEEPENING type,
// so the first field is the interned native `Rc` pointer (`NRc::as_ptr as u64`)
// instead of a noun mug — lowering the deepening subject just to mug it was the
// O(N^2) cost this nativization kills.
pub type BranSemiMemoKey = (u64, u8, u64, u64, u64, usize);
// Boundary-cache keys carry the active semantic/memo context (fan_context_key
// for %rest/%hold scope, and for mint/mull the arm_epoch/placeholder context)
// in addition to (mug, …, vet). These collapse to 0 in the steady state, so
// adding them only forces a fresh (correct) recompute when the cached type
// operation's result actually depends on context the mugs don't capture —
// closing the same roswell-class stale-hit bug the miss memo had. See the
// cache-context construction sites in ut/mod.rs.
pub type MintBoundaryKey = (u32, u32, u8, u64, u64, u64, u64);
pub type MullBoundaryKey = (u32, u32, u32, u8, u64, u64, u64, u64);
pub type RedoBoundaryKey = (u32, u32, u8, u64);
pub type RestBoundaryKey = (u32, u32, u8, u64);
pub type NestBoundaryRawKey = (u64, u64, u64);
pub type NestBoundaryKey = (u32, u32, u8, u64);
pub type TypeBinaryBoundaryKey = (u32, u32, u8, u64);
pub type CoolMemoKey = (u32, u32, u8, u64, u64);
pub type ChipMemoKey = (u32, u8, u8, u8, u8, u64, u64, u64, u64);
pub type WingAxisMemoKey = (u32, u64, u64);
pub type LookMemoKey = (u64, u64);
pub type HoldTypeRawMemoKey = (u64, u64);
pub type HoldTypeMemoKey = (u32, u32);
pub type HoldRepoRawMemoKey = (u64, u64, u64);
pub type HoldRepoCoreRawMemoKey = (u64, u64, u64, u64, u64, u64);
pub type HoldRepoCoreMemoKey = (u32, u32, u32, u32, u32, u64);

#[derive(Clone)]
pub struct BranSemiCacheEntry {
    // Native re-key (Phase-2 tail): the bran subject + the active hold scope are
    // interned native types, matched by `NRc::ptr_eq`. The `semi` output stays a
    // seminoun (the semi_* algebra is noun-based).
    pub sut: NRc<NTy>,
    pub seen_holds: Vec<NRc<NTy>>,
    pub semi: Noun,
}

pub struct BoundaryMemoSet {
    pub core_mint: BucketMemo<CoreMintBoundaryKey, CoreMintCacheEntry>,
    pub mint: BucketMemo<MintBoundaryKey, MintCacheEntry>,
    pub mull: BucketMemo<MullBoundaryKey, MullCacheEntry>,
    pub redo: BucketMemo<RedoBoundaryKey, UnaryTypeBoundaryEntry>,
    pub rest: BucketMemo<RestBoundaryKey, RestCacheEntry>,
    pub nest_raw: RawMemoMap<NestBoundaryRawKey, bool>,
    pub nest: BucketMemo<NestBoundaryKey, NestCacheEntry>,
    pub crop: BucketMemo<TypeBinaryBoundaryKey, UnaryTypeBoundaryEntry>,
    pub fuse: BucketMemo<TypeBinaryBoundaryKey, UnaryTypeBoundaryEntry>,
}

impl Default for BoundaryMemoSet {
    fn default() -> Self {
        Self {
            core_mint: Default::default(),
            mint: Default::default(),
            mull: Default::default(),
            redo: Default::default(),
            rest: Default::default(),
            nest_raw: Default::default(),
            nest: Default::default(),
            crop: Default::default(),
            fuse: Default::default(),
        }
    }
}

impl BoundaryMemoSet {
    /// Drop every memoized type-operation result. These are pure functions of
    /// their (already context-keyed) inputs, so a cleared entry just forces a
    /// correct recompute. Used at frame-arena reclamation to evict entries whose
    /// minted type/formula values lived in the reclaimed per-arm scratch.
    pub fn clear(&mut self) {
        self.core_mint.clear();
        self.mint.clear();
        self.mull.clear();
        self.redo.clear();
        self.rest.clear();
        self.nest_raw.clear();
        self.nest.clear();
        self.crop.clear();
        self.fuse.clear();
    }
}

pub struct LookupMemoSet {
    // ATOMIC FLIP (C6+C9): the find/find_raw/strict_term_port[_raw] caches were
    // DORMANT (only `.clear()`ed, never read/written on the live path) and they
    // embedded `Port` (whose shape changed to carry native types) — deleted here
    // rather than re-keyed. The surviving entries do not embed `Port`; they remain
    // dormant noun-keyed caches (re-key on native identity at C-final if revived).
    pub strict_term_core_parts_raw: RawMemoMap<(u64, u64), Option<(Noun, Noun, Noun, Noun)>>,
    pub cool: BucketMemo<CoolMemoKey, (Noun, Noun, WingType, Noun)>,
    pub chip: BucketMemo<ChipMemoKey, (Noun, Noun)>,
    pub wing_axis: BucketMemo<WingAxisMemoKey, (Noun, WingType, u64)>,
    pub look: RawMemoMap<LookMemoKey, Option<(u64, Noun)>>,
    pub loot: RawMemoMap<LookMemoKey, Option<(u64, Noun)>>,
}

impl Default for LookupMemoSet {
    fn default() -> Self {
        Self {
            strict_term_core_parts_raw: Default::default(),
            cool: Default::default(),
            chip: Default::default(),
            wing_axis: Default::default(),
            look: Default::default(),
            loot: Default::default(),
        }
    }
}

impl LookupMemoSet {
    /// Drop every memoized find/lookup result (see `BoundaryMemoSet::clear`).
    pub fn clear(&mut self) {
        self.strict_term_core_parts_raw.clear();
        self.cool.clear();
        self.chip.clear();
        self.wing_axis.clear();
        self.look.clear();
        self.loot.clear();
    }
}

pub struct HoldMemoSet {
    pub repo_raw: RawMemoMap<u64, Noun>,
    pub hold_type_raw: RawMemoMap<HoldTypeRawMemoKey, Noun>,
    pub hold_type: BucketMemo<HoldTypeMemoKey, HoldTypeCacheEntry>,
    pub hold_repo_raw: RawMemoMap<HoldRepoRawMemoKey, Noun>,
    pub hold_repo_core_raw: RawMemoMap<HoldRepoCoreRawMemoKey, Noun>,
    pub hold_repo_core: BucketMemo<HoldRepoCoreMemoKey, HoldRepoCoreCacheEntry>,
}

impl Default for HoldMemoSet {
    fn default() -> Self {
        Self {
            repo_raw: Default::default(),
            hold_type_raw: Default::default(),
            hold_type: Default::default(),
            hold_repo_raw: Default::default(),
            hold_repo_core_raw: Default::default(),
            hold_repo_core: Default::default(),
        }
    }
}

impl HoldMemoSet {
    /// Drop every memoized hold repo/type result (see `BoundaryMemoSet::clear`).
    pub fn clear(&mut self) {
        self.repo_raw.clear();
        self.hold_type_raw.clear();
        self.hold_type.clear();
        self.hold_repo_raw.clear();
        self.hold_repo_core_raw.clear();
        self.hold_repo_core.clear();
    }
}

#[derive(Default)]
pub struct StructNounSet {
    pub buckets: HashMap<u32, Vec<Noun>>,
}

impl StructNounSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, ut: &mut Ut, noun: Noun) -> Result<bool> {
        let space = ut.slab.noun_space();
        let mug = ut.noun_mug_cached(noun);
        let Some(bucket) = self.buckets.get(&mug) else {
            return Ok(false);
        };
        for prior in bucket {
            if unsafe { prior.raw_equals(&noun) } || noun_eq(*prior, noun, &space)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn insert(&mut self, ut: &mut Ut, noun: Noun) -> Result<bool> {
        let space = ut.slab.noun_space();
        let mug = ut.noun_mug_cached(noun);
        if let Some(bucket) = self.buckets.get(&mug) {
            for prior in bucket {
                if unsafe { prior.raw_equals(&noun) } || noun_eq(*prior, noun, &space)? {
                    return Ok(false);
                }
            }
        }
        self.buckets.entry(mug).or_default().push(noun);
        Ok(true)
    }

    pub fn remove(&mut self, ut: &mut Ut, noun: Noun) -> Result<bool> {
        let space = ut.slab.noun_space();
        let mug = ut.noun_mug_cached(noun);
        let mut removed = false;
        let mut remove_bucket = false;
        if let Some(bucket) = self.buckets.get_mut(&mug) {
            for idx in 0..bucket.len() {
                let prior = bucket[idx];
                if unsafe { prior.raw_equals(&noun) } || noun_eq(prior, noun, &space)? {
                    bucket.swap_remove(idx);
                    removed = true;
                    remove_bucket = bucket.is_empty();
                    break;
                }
            }
        }
        if remove_bucket {
            self.buckets.remove(&mug);
        }
        Ok(removed)
    }
}

#[derive(Default)]
pub struct StructNounPairSet {
    pub buckets: HashMap<(u32, u32), Vec<(Noun, Noun)>>,
    pub raw_pairs: FastHashSet<(u64, u64)>,
    pub pair_count: usize,
    pub signature_sum: u64,
    pub signature_xor: u64,
}

#[derive(Clone)]
pub struct HoldRepoFanContextBucket {
    pub key: (u32, u32),
    pub pairs: Vec<(Noun, Noun)>,
}

#[derive(Clone)]
pub struct HoldRepoFanContextSnapshot {
    pub buckets: Vec<HoldRepoFanContextBucket>,
    pub pair_count: usize,
}

impl StructNounPairSet {
    #[inline]
    fn pair_signature_component(key: (u32, u32)) -> u64 {
        let mut hasher = FastHasher::default();
        hasher.write_u32(key.0);
        hasher.write_u32(key.1);
        hasher.finish()
    }

    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pair_count == 0
    }

    #[inline]
    pub fn cache_signature(&self) -> u64 {
        if self.pair_count == 0 {
            return 0;
        }
        self.signature_sum
            ^ self.signature_xor.rotate_left(17)
            ^ (self.pair_count as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (self.buckets.len() as u64).rotate_left(41)
    }

    pub fn insert(&mut self, ut: &mut Ut, sut: Noun, ref_: Noun) -> Result<bool> {
        let raw_key = (unsafe { sut.as_raw() }, unsafe { ref_.as_raw() });
        if self.raw_pairs.contains(&raw_key) {
            return Ok(false);
        }
        let space = ut.slab.noun_space();
        let key = (ut.noun_mug_cached(sut), ut.noun_mug_cached(ref_));
        if let Some(bucket) = self.buckets.get(&key) {
            for (prior_sut, prior_ref) in bucket {
                if (unsafe { prior_sut.raw_equals(&sut) } || noun_eq(*prior_sut, sut, &space)?)
                    && (unsafe { prior_ref.raw_equals(&ref_) }
                        || noun_eq(*prior_ref, ref_, &space)?)
                {
                    return Ok(false);
                }
            }
        }
        self.buckets.entry(key).or_default().push((sut, ref_));
        self.raw_pairs.insert(raw_key);
        let component = Self::pair_signature_component(key);
        self.pair_count = self.pair_count.saturating_add(1);
        self.signature_sum = self.signature_sum.wrapping_add(component);
        self.signature_xor ^= component;
        Ok(true)
    }

    pub fn snapshot(&self) -> HoldRepoFanContextSnapshot {
        let mut buckets: Vec<HoldRepoFanContextBucket> = self
            .buckets
            .iter()
            .map(|(key, pairs)| HoldRepoFanContextBucket {
                key: *key,
                pairs: pairs.clone(),
            })
            .collect();
        buckets.sort_unstable_by_key(|bucket| bucket.key);
        HoldRepoFanContextSnapshot {
            buckets,
            pair_count: self.pair_count,
        }
    }

    pub fn matches_snapshot(
        &self,
        snapshot: &HoldRepoFanContextSnapshot,
        space: &NounSpace,
    ) -> Result<bool> {
        if self.buckets.len() != snapshot.buckets.len() {
            return Ok(false);
        }
        if self.pair_count != snapshot.pair_count {
            return Ok(false);
        }
        for (key, current_pairs) in self.buckets.iter() {
            let Ok(snapshot_idx) = snapshot
                .buckets
                .binary_search_by_key(key, |bucket| bucket.key)
            else {
                return Ok(false);
            };
            let snapshot_pairs = &snapshot.buckets[snapshot_idx].pairs;
            if current_pairs.len() != snapshot_pairs.len() {
                return Ok(false);
            }
            let mut matched = vec![false; snapshot_pairs.len()];
            'current: for (current_sut, current_ref) in current_pairs.iter() {
                for (idx, (snapshot_sut, snapshot_ref)) in snapshot_pairs.iter().enumerate() {
                    if matched[idx] {
                        continue;
                    }
                    let sut_match = unsafe { current_sut.raw_equals(snapshot_sut) }
                        || noun_eq(*current_sut, *snapshot_sut, space)?;
                    if !sut_match {
                        continue;
                    }
                    let ref_match = unsafe { current_ref.raw_equals(snapshot_ref) }
                        || noun_eq(*current_ref, *snapshot_ref, space)?;
                    if ref_match {
                        matched[idx] = true;
                        continue 'current;
                    }
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn remove(&mut self, ut: &mut Ut, sut: Noun, ref_: Noun) -> Result<bool> {
        let raw_key = (unsafe { sut.as_raw() }, unsafe { ref_.as_raw() });
        let space = ut.slab.noun_space();
        let key = (ut.noun_mug_cached(sut), ut.noun_mug_cached(ref_));
        let mut removed = false;
        let mut remove_bucket = false;
        if let Some(bucket) = self.buckets.get_mut(&key) {
            if self.raw_pairs.contains(&raw_key) {
                for idx in 0..bucket.len() {
                    let (prior_sut, prior_ref) = bucket[idx];
                    if unsafe { prior_sut.raw_equals(&sut) }
                        && unsafe { prior_ref.raw_equals(&ref_) }
                    {
                        bucket.swap_remove(idx);
                        self.raw_pairs.remove(&raw_key);
                        removed = true;
                        remove_bucket = bucket.is_empty();
                        let component = Self::pair_signature_component(key);
                        self.pair_count = self.pair_count.saturating_sub(1);
                        self.signature_sum = self.signature_sum.wrapping_sub(component);
                        self.signature_xor ^= component;
                        break;
                    }
                }
            }
            if !removed {
                for idx in 0..bucket.len() {
                    let (prior_sut, prior_ref) = bucket[idx];
                    if (unsafe { prior_sut.raw_equals(&sut) } || noun_eq(prior_sut, sut, &space)?)
                        && (unsafe { prior_ref.raw_equals(&ref_) }
                            || noun_eq(prior_ref, ref_, &space)?)
                    {
                        bucket.swap_remove(idx);
                        self.raw_pairs
                            .remove(&(unsafe { prior_sut.as_raw() }, unsafe {
                                prior_ref.as_raw()
                            }));
                        removed = true;
                        remove_bucket = bucket.is_empty();
                        let component = Self::pair_signature_component(key);
                        self.pair_count = self.pair_count.saturating_sub(1);
                        self.signature_sum = self.signature_sum.wrapping_sub(component);
                        self.signature_xor ^= component;
                        break;
                    }
                }
            }
        }
        if remove_bucket {
            self.buckets.remove(&key);
        }
        Ok(removed)
    }
}

#[derive(Default)]
pub struct NestTypeInterner {
    pub raw_ids: HashMap<u64, u64>,
    pub mug_ids: FastHashMap<u32, Vec<(Noun, u64)>>,
    pub next_id: u64,
}

impl NestTypeInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id_for(&mut self, ut: &mut Ut, noun: Noun) -> Result<u64> {
        let raw = unsafe { noun.as_raw() };
        if let Some(id) = self.raw_ids.get(&raw) {
            return Ok(*id);
        }

        let space = ut.slab.noun_space();
        let mug = ut.noun_mug_cached(noun);
        if let Some(bucket) = self.mug_ids.get(&mug) {
            for (prior, id) in bucket {
                if unsafe { prior.raw_equals(&noun) } || noun_eq(*prior, noun, &space)? {
                    self.raw_ids.insert(raw, *id);
                    return Ok(*id);
                }
            }
        }

        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("nest type interner exhausted u64 ids");
        self.raw_ids.insert(raw, id);
        self.mug_ids.entry(mug).or_default().push((noun, id));
        Ok(id)
    }
}

#[derive(Default, Clone)]
pub struct NestSeenSet {
    pub ids: Vec<u64>,
}

impl NestSeenSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    pub fn snapshot(&self) -> Vec<u64> {
        self.ids.clone()
    }

    /// Native-id variants (C8): the interned `Rc` pointer is a canonical type id,
    /// so the seen-hold set keys directly on it instead of the noun interner.
    pub fn contains_id(&self, id: u64) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    pub fn insert_id(&mut self, id: u64) -> bool {
        match self.ids.binary_search(&id) {
            Ok(_) => false,
            Err(idx) => {
                self.ids.insert(idx, id);
                true
            }
        }
    }

    pub fn remove_id(&mut self, id: u64) -> bool {
        match self.ids.binary_search(&id) {
            Ok(idx) => {
                self.ids.remove(idx);
                true
            }
            Err(_) => false,
        }
    }

    pub fn contains(
        &mut self,
        ut: &mut Ut,
        interner: &mut NestTypeInterner,
        noun: Noun,
    ) -> Result<bool> {
        let id = interner.id_for(ut, noun)?;
        Ok(self.ids.binary_search(&id).is_ok())
    }

    pub fn insert(
        &mut self,
        ut: &mut Ut,
        interner: &mut NestTypeInterner,
        noun: Noun,
    ) -> Result<bool> {
        let id = interner.id_for(ut, noun)?;
        match self.ids.binary_search(&id) {
            Ok(_) => Ok(false),
            Err(idx) => {
                self.ids.insert(idx, id);
                Ok(true)
            }
        }
    }

    pub fn remove(
        &mut self,
        ut: &mut Ut,
        interner: &mut NestTypeInterner,
        noun: Noun,
    ) -> Result<bool> {
        let id = interner.id_for(ut, noun)?;
        match self.ids.binary_search(&id) {
            Ok(idx) => {
                self.ids.remove(idx);
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }
}

pub type NestTypeSet = NestSeenSet;

#[derive(Default, Clone)]
pub struct NestPairSet {
    pub ids: Vec<(u64, u64)>,
}

impl NestPairSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<(u64, u64)> {
        self.ids.clone()
    }

    /// Native-id variants (C8): key directly on interned `Rc`-pointer ids.
    pub fn contains_id(&self, sut_id: u64, ref_id: u64) -> bool {
        self.ids.binary_search(&(sut_id, ref_id)).is_ok()
    }

    pub fn insert_id(&mut self, sut_id: u64, ref_id: u64) -> bool {
        let key = (sut_id, ref_id);
        match self.ids.binary_search(&key) {
            Ok(_) => false,
            Err(idx) => {
                self.ids.insert(idx, key);
                true
            }
        }
    }

    pub fn remove_id(&mut self, sut_id: u64, ref_id: u64) -> bool {
        let key = (sut_id, ref_id);
        match self.ids.binary_search(&key) {
            Ok(idx) => {
                self.ids.remove(idx);
                true
            }
            Err(_) => false,
        }
    }

    pub fn contains(
        &mut self,
        ut: &mut Ut,
        interner: &mut NestTypeInterner,
        sut: Noun,
        ref_: Noun,
    ) -> Result<bool> {
        let sut_id = interner.id_for(ut, sut)?;
        let ref_id = interner.id_for(ut, ref_)?;
        Ok(self.ids.binary_search(&(sut_id, ref_id)).is_ok())
    }

    pub fn insert(
        &mut self,
        ut: &mut Ut,
        interner: &mut NestTypeInterner,
        sut: Noun,
        ref_: Noun,
    ) -> Result<bool> {
        let sut_id = interner.id_for(ut, sut)?;
        let ref_id = interner.id_for(ut, ref_)?;
        let key = (sut_id, ref_id);
        match self.ids.binary_search(&key) {
            Ok(_) => Ok(false),
            Err(idx) => {
                self.ids.insert(idx, key);
                Ok(true)
            }
        }
    }

    pub fn remove(
        &mut self,
        ut: &mut Ut,
        interner: &mut NestTypeInterner,
        sut: Noun,
        ref_: Noun,
    ) -> Result<bool> {
        let sut_id = interner.id_for(ut, sut)?;
        let ref_id = interner.id_for(ut, ref_)?;
        let key = (sut_id, ref_id);
        match self.ids.binary_search(&key) {
            Ok(idx) => {
                self.ids.remove(idx);
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NestMemoKey {
    pub sut_id: u64,
    pub ref_id: u64,
    pub seg: Vec<u64>,
    pub reg: Vec<u64>,
    pub gil: Vec<(u64, u64)>,
}

pub struct FastHasher {
    pub state: u64,
}

impl FastHasher {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    #[inline]
    pub fn mix_u64(&mut self, value: u64) {
        self.state ^= value;
        self.state = self.state.wrapping_mul(Self::PRIME);
    }
}

impl Default for FastHasher {
    #[inline]
    fn default() -> Self {
        Self {
            state: Self::OFFSET,
        }
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.state
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.mix_u64(u64::from(*byte));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.mix_u64(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.mix_u64(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.mix_u64(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.mix_u64(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.mix_u64(i as u64);
    }

    #[inline]
    fn write_i8(&mut self, i: i8) {
        self.mix_u64(i as u64);
    }

    #[inline]
    fn write_i16(&mut self, i: i16) {
        self.mix_u64(i as u64);
    }

    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.mix_u64(i as u64);
    }

    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.mix_u64(i as u64);
    }

    #[inline]
    fn write_isize(&mut self, i: isize) {
        self.mix_u64(i as u64);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Read,
    Rite,
    Both,
    Free,
}

#[derive(Clone, Copy, Debug)]
pub enum MuskOutput {
    Stop,
    Wait,
    Done(Noun),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Poly {
    Wet,
    Dry,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Vair {
    Gold,
    Iron,
    Lead,
    Zinc,
}

// ATOMIC FLIP (C6+C9): the wing-navigation Port/Palo/Opal/Pony carriers now hold
// NATIVE type values (`NRc<NTy>`). The FORMULA slot (Synthetic.formula) and the
// arm-spec foot (`Opal::Arm.arms[].1`) stay `Noun` — those are nock formulas /
// hoon arm-specs, not types.
#[derive(Clone, Debug)]
pub enum Port {
    Palo(Palo),
    Synthetic { typ: NRc<NTy>, formula: FormulaId },
}

#[derive(Clone, Debug)]
pub struct Palo {
    pub vein: Vec<Option<BigUint>>,
    pub opal: Opal,
}

#[derive(Clone, Debug)]
pub enum Opal {
    Leg(NRc<NTy>),
    Arm {
        axis: BigUint,
        arms: Vec<(NRc<NTy>, Noun)>,
    },
}

#[derive(Clone, Debug)]
pub enum Pony {
    Void,
    Palo(Palo),
    Unmatched(u64),
    Synthetic { typ: NRc<NTy>, formula: FormulaId },
}

#[derive(Clone, Debug)]
pub struct CoreMintCacheEntry {
    pub sut: Noun,
    pub gol: Noun,
    pub tomes_map: Noun,
    pub prefix: Option<String>,
    pub poly: Poly,
    pub vet: bool,
    pub core_type: Noun,
    pub formula: Noun,
}

#[derive(Clone, Copy, Debug)]
pub struct MintCacheEntry {
    pub sut: Noun,
    pub gol: Noun,
    pub gen: Noun,
    pub ty: Noun,
    pub formula: Noun,
}

#[derive(Clone, Copy, Debug)]
pub struct MullCacheEntry {
    pub sut: Noun,
    pub gol: Noun,
    pub dox: Noun,
    pub gen: Noun,
    pub p_ty: Noun,
    pub q_ty: Noun,
}

#[derive(Clone, Copy, Debug)]
pub struct UnaryTypeBoundaryEntry {
    pub sut: Noun,
    pub ref_: Noun,
    pub result: Noun,
}

#[derive(Clone, Copy, Debug)]
pub struct RestCacheEntry {
    pub sut: Noun,
    pub legs: Noun,
    pub result: Noun,
}

#[derive(Clone, Copy, Debug)]
pub struct NestCacheEntry {
    pub sut: Noun,
    pub ref_: Noun,
    pub result: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct HoldTypeCacheEntry {
    pub inner: Noun,
    pub hoon: Noun,
    pub hold: Noun,
}

#[derive(Clone, Copy, Debug)]
pub struct HoldRepoCoreCacheEntry {
    pub hoon: Noun,
    pub payload: Noun,
    pub garb: Noun,
    pub context: Noun,
    pub tomes: Noun,
    pub result: Noun,
}

#[derive(Clone, Debug)]
pub struct LazyResolverArmEntry {
    pub arm_name: Arc<str>,
    pub hoon_noun: Noun,
}

#[derive(Clone, Debug)]
pub struct LazyResolverContext {
    // ATOMIC FLIP perf: the lazy core is the NATIVE deepening core (the interned
    // Rc threaded from mint_core). It is heap-resident (not slab) so it needs no
    // relocation, and it shares pointer identity with the in-progress entries
    // pushed during the same core's arm builds.
    pub core_type: NRc<NTy>,
    pub poly: Poly,
    pub arms_by_axis: HashMap<BigUint, LazyResolverArmEntry>,
    pub cached_formula_by_axis: HashMap<BigUint, FormulaId>,
    pub in_progress_axes: HashSet<BigUint>,
}

#[derive(Clone, Debug)]
pub struct ArmInProgressEntry {
    pub key: Arc<str>,
    // ATOMIC FLIP perf: the in-progress core is the NATIVE deepening core (the
    // interned Rc threaded down from mint_core), not a re-lifted noun. Cross-arm
    // cycle detection compares interned Rc identity (see
    // arm_goal_for_hoon_in_progress) instead of noun structural equality.
    pub core: NRc<NTy>,
    pub hoon: Noun,
    pub goal: NRc<NTy>,
    pub vet: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ArmSplitTotals {
    pub calls: u64,
    pub total_us: u128,
    pub play_us: u128,
    pub mint_us: u128,
    pub prune_us: u128,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MintTslsTotals {
    pub calls: u64,
    pub total_us: u128,
    pub p_mint_us: u128,
    pub q_mint_us: u128,
    pub comb_us: u128,
}
