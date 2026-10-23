use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::Arc;

use nockapp::Noun;
use nockvm::noun::{NounAllocator, NounSpace};
use num_bigint::BigUint;

use super::keys::*;
use crate::errors::Result;
use crate::native::identity::*;
use crate::native::ir::formula_dag::FormulaId;
use crate::native::ir::semi_dag::SemiId;
use crate::native::ir::ty::{Type as NTy, TypeId, TypeRef as NRc};
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

#[derive(Clone)]
pub struct BranSemiCacheEntry {
    // Native re-key (Phase-2 tail): the bran subject + the active hold scope are
    // interned native types, matched by `NRc::ptr_eq`. The output is a compact
    // identity in the compile-local native seminoun arena.
    pub sut: NRc<NTy>,
    pub seen_holds: Vec<NRc<NTy>>,
    pub semi: SemiId,
}

pub struct BoundaryMemoSet {
    pub mint: BucketMemo<MintKey<NounMug>, MintCacheEntry>,
    pub redo: BucketMemo<TypeBinaryKey<NounMug>, UnaryTypeBoundaryEntry>,
    pub rest: BucketMemo<RestKey, RestCacheEntry>,
    pub nest: BucketMemo<TypeBinaryKey<NounMug>, NestCacheEntry>,
}

impl Default for BoundaryMemoSet {
    fn default() -> Self {
        Self {
            mint: Default::default(),
            redo: Default::default(),
            rest: Default::default(),
            nest: Default::default(),
        }
    }
}

impl BoundaryMemoSet {
    /// Drop noun-boundary results; future calls recompute them in their context.
    pub fn clear(&mut self) {
        self.mint.clear();
        self.redo.clear();
        self.rest.clear();
        self.nest.clear();
    }
}

pub struct HoldMemoSet {
    pub hold_type_raw: RawMemoMap<HoldKey<NounIdentity>, Noun>,
    pub hold_type: BucketMemo<HoldKey<NounMug>, HoldTypeCacheEntry>,
}

impl Default for HoldMemoSet {
    fn default() -> Self {
        Self {
            hold_type_raw: Default::default(),
            hold_type: Default::default(),
        }
    }
}

impl HoldMemoSet {
    /// Drop every memoized hold repo/type result (see `BoundaryMemoSet::clear`).
    pub fn clear(&mut self) {
        self.hold_type_raw.clear();
        self.hold_type.clear();
    }
}

#[derive(Default)]
pub struct StructNounSet {
    pub buckets: HashMap<NounMug, Vec<Noun>>,
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
    pub buckets: HashMap<(NounMug, NounMug), Vec<(Noun, Noun)>>,
    pub raw_pairs: FastHashSet<(NounIdentity, NounIdentity)>,
    pub pair_count: usize,
    pub signature_sum: u64,
    pub signature_xor: u64,
}

#[derive(Clone)]
pub struct HoldRepoFanContextBucket {
    pub key: (NounMug, NounMug),
    pub pairs: Vec<(Noun, Noun)>,
}

#[derive(Clone)]
pub struct HoldRepoFanContextSnapshot {
    pub buckets: Vec<HoldRepoFanContextBucket>,
    pub pair_count: usize,
}

impl StructNounPairSet {
    #[inline]
    fn pair_signature_component(key: (NounMug, NounMug)) -> u64 {
        let mut hasher = FastHasher::default();
        hasher.write_u32(key.0 .0);
        hasher.write_u32(key.1 .0);
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
        let raw_key = (NounIdentity::of(sut), NounIdentity::of(ref_));
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
        let raw_key = (NounIdentity::of(sut), NounIdentity::of(ref_));
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
                            .remove(&(NounIdentity::of(prior_sut), NounIdentity::of(prior_ref)));
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
    pub raw_ids: HashMap<NounIdentity, NestNounId>,
    pub mug_ids: FastHashMap<NounMug, Vec<(Noun, NestNounId)>>,
    pub next_id: NestNounId,
}

impl NestTypeInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id_for(&mut self, ut: &mut Ut, noun: Noun) -> Result<NestNounId> {
        let raw = NounIdentity::of(noun);
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
        self.next_id = NestNounId(
            self.next_id
                .0
                .checked_add(1)
                .expect("nest type interner exhausted u64 ids"),
        );
        self.raw_ids.insert(raw, id);
        self.mug_ids.entry(mug).or_default().push((noun, id));
        Ok(id)
    }
}

/// Ordered recursion guard whose ID domain cannot change after construction.
/// ```compile_fail
/// use honk::native::identity::NestNounId;
/// use honk::native::ir::ty::TypeId;
/// use honk::native::ut::types::NestSeenSet;
/// let mut native = NestSeenSet::<TypeId>::new();
/// native.insert_id(NestNounId::default());
/// ```
#[derive(Clone)]
pub struct NestSeenSet<I = TypeId> {
    pub ids: Vec<I>,
}

impl<I: Copy + Ord> NestSeenSet<I> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    pub fn snapshot(&self) -> Vec<I> {
        self.ids.clone()
    }

    /// Membership in this set's ID domain (native TypeId by default).
    pub fn contains_id(&self, id: I) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    pub fn insert_id(&mut self, id: I) -> bool {
        match self.ids.binary_search(&id) {
            Ok(_) => false,
            Err(idx) => {
                self.ids.insert(idx, id);
                true
            }
        }
    }

    pub fn remove_id(&mut self, id: I) -> bool {
        match self.ids.binary_search(&id) {
            Ok(idx) => {
                self.ids.remove(idx);
                true
            }
            Err(_) => false,
        }
    }
}

impl NestSeenSet<NestNounId> {
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

impl<I> Default for NestSeenSet<I> {
    fn default() -> Self {
        Self { ids: Vec::new() }
    }
}

pub type NestTypeSet = NestSeenSet;

#[derive(Clone)]
pub struct NestPairSet<I = TypeId> {
    pub ids: Vec<(I, I)>,
}

impl<I: Copy + Ord> NestPairSet<I> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<(I, I)> {
        self.ids.clone()
    }

    /// Membership in this set's ID domain (native TypeId by default).
    pub fn contains_id(&self, sut_id: I, ref_id: I) -> bool {
        self.ids.binary_search(&(sut_id, ref_id)).is_ok()
    }

    pub fn insert_id(&mut self, sut_id: I, ref_id: I) -> bool {
        let key = (sut_id, ref_id);
        match self.ids.binary_search(&key) {
            Ok(_) => false,
            Err(idx) => {
                self.ids.insert(idx, key);
                true
            }
        }
    }

    pub fn remove_id(&mut self, sut_id: I, ref_id: I) -> bool {
        let key = (sut_id, ref_id);
        match self.ids.binary_search(&key) {
            Ok(idx) => {
                self.ids.remove(idx);
                true
            }
            Err(_) => false,
        }
    }
}

impl NestPairSet<NestNounId> {
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

impl<I> Default for NestPairSet<I> {
    fn default() -> Self {
        Self { ids: Vec::new() }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NestMemoKey {
    pub sut_id: TypeId,
    pub ref_id: TypeId,
    pub seg: Vec<TypeId>,
    pub reg: Vec<TypeId>,
    pub gil: Vec<(TypeId, TypeId)>,
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

#[derive(Clone, Copy, Debug)]
pub struct MintCacheEntry {
    pub sut: Noun,
    pub gol: Noun,
    pub gen: Noun,
    pub ty: Noun,
    pub formula: Noun,
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
