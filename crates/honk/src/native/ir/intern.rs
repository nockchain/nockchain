//! Hash-cons intern table for [`super::ty::Type`] (plan §3.3) — the keystone of
//! the memory win.
//!
//! Returns the canonical `Rc<Type>` for a structurally-equal type, so equal
//! subtrees — including pointer-distinct ones produced by minting — collapse to
//! ONE shared `Rc`. This is what fixes subject-deepening: the repeated embedded
//! subjects become a single shared node instead of O(N²) duplicated structure.
//!
//! Interning is BOTTOM-UP: children are interned first, so a node's hash/eq use
//! the children's canonical `Rc` IDENTITY (`Rc::ptr_eq`) plus its own leaf
//! content — O(1) per node (no deep recursion in the compare), O(total) overall.
//! Fork members are canonical child pointers too; their exact Hoon treap remains
//! separately attached as the serialization witness and fork-node key.
//!
//! NOTE: this operates on the Phase-1 boundary `Type` (native skeleton + carried
//! leaves). It already dedups the native skeleton — crucially the recursive
//! payload/cell/inner/subject/fork chains where subject-deepening lives. Later
//! phases nativize the remaining coil/gene leaves; the table is unchanged.
#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc as SharedRc;

use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounSpace, D, T};
use num_bigint::BigUint;

use super::formula_dag::FormulaId;
use super::leaf::Leaf;
use super::ty::{tas, BoundaryType, Garb, Type, TypeId, TypeRef as Rc, TypeSlot};
use crate::errors::{CompilerError, Result};
use crate::native::noun::{noun_eq, noun_pair};

/// Decode a type noun into the native IR AND intern it in one O(n) pass, using a
/// persistent pointer-identity `memo` so the noun DAG (and anything carried over
/// from a prior call) is walked at most once. Structurally-equal-but-pointer-
/// distinct subtrees — the duplicated embedded subjects of subject-deepening —
/// are collapsed by the table to one shared `Rc`. This is the O(n) construction
/// primitive the native-mint port is built on (the combinator O(n²) trap was
/// re-parsing per call with no shared memo; this shares one).
fn intern_type_noun(
    table: &mut TypeTable,
    memo: &mut InternMemo,
    noun: Noun,
    space: &NounSpace,
) -> Result<Rc<Type>> {
    // %void / %noun are bare atom cords — directs, no memo needed.
    if let Ok(atom) = noun.in_space(space).as_atom() {
        if atom.eq_bytes(b"void") {
            return Ok(table.intern_shallow(Type::Void));
        }
        if atom.eq_bytes(b"noun") {
            return Ok(table.intern_shallow(Type::Noun));
        }
        return Err(CompilerError::Decode(
            "native type IR: unknown atom type tag".into(),
        ));
    }
    // SAFETY: `noun` is a live, in-`space` slab noun; `as_raw` only reads its
    // identity word (used purely as a memo key, never dereferenced).
    let raw_addr = unsafe { noun.as_raw() };
    if let Some(rc) = memo.get(&raw_addr) {
        return Ok(Rc::clone(rc));
    }
    let (tag, tail) = pair(noun, space)?;
    let tag = tag
        .in_space(space)
        .as_atom()
        .map_err(|_| CompilerError::Decode("native type IR: type tag not atom".into()))?;
    let node = if tag.eq_bytes(b"atom") {
        let (aura, bits) = pair(tail, space)?;
        Type::Atom {
            aura: table.intern_live_leaf(aura, space),
            bits: table.intern_live_leaf(bits, space),
        }
    } else if tag.eq_bytes(b"cell") {
        let (h, t) = pair(tail, space)?;
        Type::Cell(
            intern_type_noun(table, memo, h, space)?,
            intern_type_noun(table, memo, t, space)?,
        )
    } else if tag.eq_bytes(b"core") {
        let (payload, coil) = pair(tail, space)?;
        // coil = [garb [context rest]]
        let (garb, coil_tail) = pair(coil, space)?;
        let (context, rest) = pair(coil_tail, space)?;
        Type::Core {
            payload: intern_type_noun(table, memo, payload, space)?,
            garb: Garb::from_noun(garb, space)?,
            context: intern_type_noun(table, memo, context, space)?,
            rest: table.intern_live_leaf(rest, space),
        }
    } else if tag.eq_bytes(b"face") {
        let (tool, inner) = pair(tail, space)?;
        Type::Face {
            tool: table.intern_live_leaf(tool, space),
            inner: intern_type_noun(table, memo, inner, space)?,
        }
    } else if tag.eq_bytes(b"hint") {
        let (head, payload) = pair(tail, space)?;
        Type::Hint {
            head: table.intern_live_leaf(head, space),
            payload: intern_type_noun(table, memo, payload, space)?,
        }
    } else if tag.eq_bytes(b"fork") {
        Type::Fork {
            set: table.intern_live_leaf(tail, space),
            options: Default::default(),
            options_seen: Default::default(),
        }
    } else if tag.eq_bytes(b"hold") {
        let (subject, gene) = pair(tail, space)?;
        Type::Hold {
            subject: intern_type_noun(table, memo, subject, space)?,
            gene: table.intern_live_leaf(gene, space),
        }
    } else {
        return Err(CompilerError::Decode(
            "native type IR: unknown type tag".into(),
        ));
    };
    let interned = table.intern_shallow(node);
    memo.insert(raw_addr, Rc::clone(&interned));
    Ok(interned)
}

fn pair(n: Noun, space: &NounSpace) -> Result<(Noun, Noun)> {
    noun_pair(n, space).map_err(|_| CompilerError::Decode("native type IR: bad cell".into()))
}

// ---------------------------------------------------------------------------
// Live native-mint construction-port harness (flag-gated by HONK_NATIVE_TYPES).
//
// This is the first real step of the construction port: as `mint` builds each
// core's type noun, we build the corresponding interned native type into one
// PERSISTENT table (shared pointer-memo across the whole compile). It runs
// alongside the noun path (which stays the live oracle), so it is additive and
// safe, and it measures the thing the whole migration turns on: how much the
// intern table collapses the mint-time type duplication (subject-deepening).
//
// Single-thread, single-compile harness — call `live_reset` at compile start.
// ---------------------------------------------------------------------------

/// The `intern_type_noun`/`native_of` decode memo: source-noun-address ->
/// canonical interned `Rc<Type>`. Its KEYS are slab addresses; with the frame
/// arena retired the compile slab never reclaims a frame, so an address is never
/// recycled within a compile and an entry can never go stale (its source noun
/// stays live for the whole compile). The memo therefore just accumulates for the
/// life of the `Context` (one compile) — a plain map, no frame-scoped eviction.
struct InternMemo {
    map: HashMap<u64, Rc<Type>>,
}

impl InternMemo {
    fn new() -> Self {
        InternMemo {
            map: HashMap::new(),
        }
    }

    #[inline]
    fn get(&self, raw: &u64) -> Option<&Rc<Type>> {
        self.map.get(raw)
    }

    #[inline]
    fn insert(&mut self, raw: u64, rc: Rc<Type>) {
        self.map.insert(raw, rc);
    }
}

struct LiveIntern {
    table: TypeTable,
    memo: InternMemo,
    cores: u64,
    next_report: u64,
}

impl LiveIntern {
    fn new() -> Self {
        LiveIntern {
            table: TypeTable::new(),
            memo: InternMemo::new(),
            cores: 0,
            next_report: 100_000,
        }
    }
}

/// Owned per-compile native-IR state.
///
/// Consolidates every per-compile native-IR cache that used to be a module
/// thread-local (the hash-cons core `live`, the encode memos, the boundary
/// caches, and the content-keyed decode / fork caches) into one struct OWNED by
/// `Ut` (as the `cx` field). The surface free functions in this module take
/// `&mut Context` / `&Context`; a fresh `Ut` gets a fresh `Context`, which gives
/// each compile an isolated cache universe (replacing the old per-compile
/// `live_reset`).
///
/// `new`/`reset` start every field empty (and `live` is eagerly constructed,
/// replacing the former thread-local's lazy `Option<LiveIntern>` +
/// `get_or_insert_with`).
pub struct Context {
    // --- LIVE (hash-cons core): was `static LIVE: RefCell<Option<LiveIntern>>` ---
    // `LiveIntern` bundles `table: TypeTable`, `memo: InternMemo`, `cores`,
    // `next_report`. It was `Option` only so a thread-local could lazily create it
    // via `get_or_insert_with(LiveIntern::new)`; an owned `Context` creates it
    // eagerly in `new`, so the `Option` is unnecessary.
    live: LiveIntern,

    // --- encode memos ---
    to_noun_memo: HashMap<u32, Noun>, // was TO_NOUN_MEMO
    leaf_memo: HashMap<usize, Noun>,  // was LEAF_MEMO

    // --- boundary caches (key/value tuples copied verbatim) ---
    nest_cache: HashMap<(u32, u32, u8, u64), bool>, // NEST_CACHE
    #[allow(clippy::type_complexity)]
    core_mint_cache: HashMap<(u32, u32, u64, u8, u8, u64, u64, u64), (Rc<Type>, FormulaId)>, // CORE_MINT_CACHE
    #[allow(clippy::type_complexity)]
    mint_cache: HashMap<(u32, u32, u8, u64, u64, u64, u64), (Rc<Type>, FormulaId)>, // MINT_CACHE
    #[allow(clippy::type_complexity)]
    mull_cache: HashMap<(u32, u32, u32, u8, u64, u64, u64, u64), (Rc<Type>, Rc<Type>)>, // MULL_CACHE
    fuse_cache: HashMap<(u32, u32, u8, u64), Rc<Type>>, // FUSE_CACHE
    crop_cache: HashMap<(u32, u32, u8, u64), Rc<Type>>, // CROP_CACHE
    fish_cache: HashMap<(u32, BigUint, u8, u64), FormulaId>, // FISH_CACHE

    // --- native_of content-keyed decode cache + fork cache ---
    native_of_mug_memo: HashMap<u64, Vec<Rc<Type>>>, // NATIVE_OF_MUG_MEMO
    fork_cache: HashMap<Vec<u32>, Rc<Type>>,         // FORK_CACHE

    // --- scope-precise fan key support (reachable %hold legs per type) ---
    // `legset_memo` maps an interned `Rc<Type>` pointer to the sorted-deduped set
    // of %hold leg-ids reachable from that type (`reachable_legs` in ut/mod.rs).
    // Sound because `intern_node` hash-conses (ptr == structural identity), so the
    // legset is a pure function of the pointer; computed bottom-up over the Rc DAG
    // and memoized so each distinct node is visited once (O(1) amortized).
    legset_memo: HashMap<u32, SharedRc<[u64]>>,
}

impl Context {
    pub fn new() -> Self {
        Context {
            live: LiveIntern::new(),
            to_noun_memo: HashMap::new(),
            leaf_memo: HashMap::new(),
            nest_cache: HashMap::new(),
            core_mint_cache: HashMap::new(),
            mint_cache: HashMap::new(),
            mull_cache: HashMap::new(),
            fuse_cache: HashMap::new(),
            crop_cache: HashMap::new(),
            fish_cache: HashMap::new(),
            native_of_mug_memo: HashMap::new(),
            fork_cache: HashMap::new(),
            legset_memo: HashMap::new(),
        }
    }

    /// Re-init every field (mirrors the `live_reset` body exactly). Provided for a
    /// later `live_reset()` -> `self.cx.reset()` swap even though the chosen wiring
    /// constructs a fresh `Context` per compile.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        *self = Context::new();
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
fn canonical_id(ty: &Rc<Type>) -> u32 {
    ty.arena_id().0
}

static LIVE_ENABLED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Look up a memoized `cons_fork` result by its canonical option-pointer key.
///
/// The `cons_fork` memo (was `FORK_CACHE`): the canonical (sorted, deduped)
/// option `Rc` pointers -> the resulting fork `Rc`. A fork is mug-ordered and
/// set-valued, so it is fully determined by the SET of its option types; since
/// options are interned (canonical), their pointer set is the exact key. In the
/// recursive-type elaboration the same forks recur constantly, and each
/// `cons_fork` miss costs a full mug-treap rebuild (fork_from_options:
/// set_put_mug/slab_mug) PLUS a decode+jam of the treap leaf (native_of). This
/// memo collapses the recurrence to O(1). Byte-exact: returns the SAME interned
/// `Rc` the rebuild would. Cleared with the table by `Context::reset`.
pub fn fork_cache_lookup(cx: &Context, key: &[u32]) -> Option<Rc<Type>> {
    cx.fork_cache.get(key).cloned()
}

/// Store a `cons_fork` result keyed by its canonical option-pointer key.
pub fn fork_cache_store(cx: &mut Context, key: Vec<u32>, fork: Rc<Type>) {
    cx.fork_cache.insert(key, fork);
}

/// Look up the memoized reachable-leg set for an interned type pointer.
/// `ptr` is `canonical_id(t)`; the value is the sorted-deduped set of
/// %hold leg-ids reachable from `t`. See `reachable_legs` in ut/mod.rs.
pub fn legset_memo_lookup(cx: &Context, id: u32) -> Option<SharedRc<[u64]>> {
    cx.legset_memo.get(&id).cloned()
}

/// Store a memoized reachable-leg set for an interned type pointer.
pub fn legset_memo_store(cx: &mut Context, id: u32, legs: SharedRc<[u64]>) {
    cx.legset_memo.insert(id, legs);
}

/// Content-keyed `native_of` fast path (see `Context::native_of_mug_memo`).
/// Returns the memoized candidate `Rc`s for a noun mug (a tiny bucket;
/// collisions are rare).
pub fn native_of_mug_candidates(cx: &Context, mug: u64) -> Vec<Rc<Type>> {
    cx.native_of_mug_memo.get(&mug).cloned().unwrap_or_default()
}

/// Record a decoded `(mug -> Rc)` association for the content-keyed `native_of`
/// cache. Idempotent per `Rc` within a bucket.
pub fn native_of_mug_insert(cx: &mut Context, mug: u64, rc: Rc<Type>) {
    let bucket = cx.native_of_mug_memo.entry(mug).or_default();
    if !bucket.iter().any(|existing| Rc::ptr_eq(existing, &rc)) {
        bucket.push(rc);
    }
}

/// Look up a native `core_mint` result by interned (sut, gol) pointers + the
/// preserved semantic key fields (tomes_sig, vet, poly, fan, arm_epoch,
/// placeholder). Returns the native (core type, formula) directly — no `native_of`.
#[allow(clippy::too_many_arguments)]
pub fn core_mint_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    tomes_sig: u64,
    vet: u8,
    poly: u8,
    fan: u64,
    arm_epoch: u64,
    placeholder: u64,
) -> Option<(Rc<Type>, FormulaId)> {
    let key = (
        canonical_id(sut),
        canonical_id(gol),
        tomes_sig,
        vet,
        poly,
        fan,
        arm_epoch,
        placeholder,
    );
    cx.core_mint_cache.get(&key).cloned()
}

/// Store a native `core_mint` result by interned (sut, gol) pointers + semantic key.
#[allow(clippy::too_many_arguments)]
pub fn core_mint_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    tomes_sig: u64,
    vet: u8,
    poly: u8,
    fan: u64,
    arm_epoch: u64,
    placeholder: u64,
    core_type: Rc<Type>,
    formula: FormulaId,
) {
    let key = (
        canonical_id(sut),
        canonical_id(gol),
        tomes_sig,
        vet,
        poly,
        fan,
        arm_epoch,
        placeholder,
    );
    cx.core_mint_cache.insert(key, (core_type, formula));
}

/// Look up a native `mint` result by interned (sut, gol) pointers + the preserved
/// semantic key fields (vet, gen_sig, fan, arm_epoch, placeholder). Returns the
/// native (type, formula) directly — no `native_of`.
#[allow(clippy::too_many_arguments)]
pub fn mint_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    vet: u8,
    gen_sig: u64,
    fan: u64,
    arm_epoch: u64,
    placeholder: u64,
) -> Option<(Rc<Type>, FormulaId)> {
    let key = (
        canonical_id(sut),
        canonical_id(gol),
        vet,
        gen_sig,
        fan,
        arm_epoch,
        placeholder,
    );
    cx.mint_cache.get(&key).cloned()
}

/// Store a native `mint` result by interned (sut, gol) pointers + semantic key.
#[allow(clippy::too_many_arguments)]
pub fn mint_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    vet: u8,
    gen_sig: u64,
    fan: u64,
    arm_epoch: u64,
    placeholder: u64,
    ty: Rc<Type>,
    formula: FormulaId,
) {
    let key = (
        canonical_id(sut),
        canonical_id(gol),
        vet,
        gen_sig,
        fan,
        arm_epoch,
        placeholder,
    );
    cx.mint_cache.insert(key, (ty, formula));
}

/// Look up a native `mull` result by interned (sut, gol, dox) pointers + the
/// preserved semantic key fields (vet, gen_sig, fan, arm_epoch, placeholder).
/// Returns the native (p type, q type) directly — no `native_of`.
#[allow(clippy::too_many_arguments)]
pub fn mull_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    dox: &Rc<Type>,
    vet: u8,
    gen_sig: u64,
    fan: u64,
    arm_epoch: u64,
    placeholder: u64,
) -> Option<(Rc<Type>, Rc<Type>)> {
    let key = (
        canonical_id(sut),
        canonical_id(gol),
        canonical_id(dox),
        vet,
        gen_sig,
        fan,
        arm_epoch,
        placeholder,
    );
    cx.mull_cache.get(&key).cloned()
}

/// Store a native `mull` result by interned (sut, gol, dox) pointers + semantic key.
#[allow(clippy::too_many_arguments)]
pub fn mull_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    dox: &Rc<Type>,
    vet: u8,
    gen_sig: u64,
    fan: u64,
    arm_epoch: u64,
    placeholder: u64,
    p_ty: Rc<Type>,
    q_ty: Rc<Type>,
) {
    let key = (
        canonical_id(sut),
        canonical_id(gol),
        canonical_id(dox),
        vet,
        gen_sig,
        fan,
        arm_epoch,
        placeholder,
    );
    cx.mull_cache.insert(key, (p_ty, q_ty));
}

/// Look up a native `nest` result by interned (sut, ref) pointers + context.
pub fn nest_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: u8,
    fan: u64,
) -> Option<bool> {
    let key = (canonical_id(sut), canonical_id(ref_), vet, fan);
    cx.nest_cache.get(&key).copied()
}

/// Store a native `nest` result by interned (sut, ref) pointers + context.
pub fn nest_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: u8,
    fan: u64,
    result: bool,
) {
    let key = (canonical_id(sut), canonical_id(ref_), vet, fan);
    cx.nest_cache.insert(key, result);
}

/// Look up a native `fuse` result by interned (sut, ref) pointers + (vet, fan).
/// Returns the native result type directly — no `native_of`.
pub fn fuse_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: u8,
    fan: u64,
) -> Option<Rc<Type>> {
    let key = (canonical_id(sut), canonical_id(ref_), vet, fan);
    cx.fuse_cache.get(&key).cloned()
}

/// Store a native `fuse` result by interned (sut, ref) pointers + (vet, fan).
pub fn fuse_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: u8,
    fan: u64,
    result: Rc<Type>,
) {
    let key = (canonical_id(sut), canonical_id(ref_), vet, fan);
    cx.fuse_cache.insert(key, result);
}

/// Look up a native `crop` result by interned (sut, ref) pointers + (vet, fan).
/// Returns the native result type directly — no `native_of`.
pub fn crop_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: u8,
    fan: u64,
) -> Option<Rc<Type>> {
    let key = (canonical_id(sut), canonical_id(ref_), vet, fan);
    cx.crop_cache.get(&key).cloned()
}

/// Store a native `crop` result by interned (sut, ref) pointers + (vet, fan).
pub fn crop_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: u8,
    fan: u64,
    result: Rc<Type>,
) {
    let key = (canonical_id(sut), canonical_id(ref_), vet, fan);
    cx.crop_cache.insert(key, result);
}

/// Look up a native `fish` result by interned (sut) pointer + (axis, vet, fan).
/// Returns the cached canonical formula ID directly.
pub fn fish_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    axis: &BigUint,
    vet: u8,
    fan: u64,
) -> Option<FormulaId> {
    cx.fish_cache
        .get(&(canonical_id(sut), axis.clone(), vet, fan))
        .copied()
}

/// Store a native `fish` result by interned (sut) pointer +
/// (axis, vet, fan).
pub fn fish_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    axis: &BigUint,
    vet: u8,
    fan: u64,
    result: FormulaId,
) {
    let key = (canonical_id(sut), axis.clone(), vet, fan);
    cx.fish_cache.insert(key, result);
}

/// Whether the live native-type harness is on (`HONK_NATIVE_TYPES`), cached.
pub fn live_enabled() -> bool {
    use std::sync::atomic::Ordering;
    match LIVE_ENABLED.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let on = std::env::var_os("HONK_NATIVE_TYPES").is_some();
            LIVE_ENABLED.store(if on { 1 } else { 2 }, Ordering::Relaxed);
            on
        }
    }
}

/// Memoized `Type::to_noun` for the flip bridges: lower a canonical native type to
/// a noun, caching by interned `Rc` pointer so repeated lowerings (the hot
/// `*_noun` bridges on big deepened types) are O(1) instead of O(type). SOUND
/// because flip natives are interned (the table holds them for the whole compile,
/// so the address is stable + not reused) and the bridges always lower into the
/// one compile slab; reset per compile via `live_reset`.
pub fn live_to_noun(cx: &mut Context, native: &Rc<Type>, dst: &mut NounSlab) -> Noun {
    let ptr = canonical_id(native);
    if let Some(noun) = cx.to_noun_memo.get(&ptr).copied() {
        return noun;
    }
    // Stack guard: this per-node recursion descends as deep as the type and runs
    // INSIDE other deep recursions (redo/repo, reached via `native_of_cached`'s
    // verify). Without growing the stack a deep recursive type overflows the guard
    // page (SIGBUS on macOS, no Rust panic). `maybe_grow` is a cheap pointer
    // compare when headroom remains; 64MB chunks dwarf any real type depth.
    let noun = stacker::maybe_grow(32 * 1024, 64 * 1024 * 1024, || {
        live_to_noun_node(cx, native, dst)
    });
    // The node-built noun is already resident in the compile slab `dst` (the frame
    // arena was retired, so there is no base region to relocate to). The memo keeps
    // it live for the whole compile.
    cx.to_noun_memo.insert(ptr, noun);
    noun
}

/// One node of `live_to_noun`'s memoized recursion (split out so the `maybe_grow`
/// stack guard wraps each level). See `live_to_noun`.
fn live_to_noun_node(cx: &mut Context, native: &Rc<Type>, dst: &mut NounSlab) -> Noun {
    // PERF (RT-05): lower the IMMEDIATE children through `live_to_noun` /
    // `live_leaf_to_noun` (both per-pointer memoized) rather than the bare
    // recursive `Type::to_noun`, which re-materializes the WHOLE subtree fresh.
    // Recursive type children (a `%core`'s shared `context` = the deepening
    // subject, often the entire stdlib subject) are one canonical `Rc` shared by
    // many ancestors; the bare walk re-lowered — and re-cued the Jammed battery
    // `rest`/`garb`/`gene` leaves — once per ancestor, so a handful of mints over
    // a big subject cost seconds. Routing children through the memo lowers each
    // distinct node (and each Jammed leaf) exactly once per compile; jam output is
    // structure-only so byte-exactness is unchanged. Mirrors `Type::to_noun`'s
    // node shapes exactly (which stays the pure, slab-agnostic oracle form).
    let noun = match &**native {
        Type::Void => D(tas("void")),
        Type::Noun => D(tas("noun")),
        Type::Atom { aura, bits } => {
            let a = live_leaf_to_noun(&mut *cx, aura, dst);
            let b = live_leaf_to_noun(&mut *cx, bits, dst);
            T(dst, &[D(tas("atom")), a, b])
        }
        Type::Cell(h, t) => {
            let hn = live_to_noun(&mut *cx, h, dst);
            let tn = live_to_noun(&mut *cx, t, dst);
            T(dst, &[D(tas("cell")), hn, tn])
        }
        Type::Core {
            payload,
            garb,
            context,
            rest,
        } => {
            let p = live_to_noun(&mut *cx, payload, dst);
            let g = garb.to_noun(dst);
            let ctx = live_to_noun(&mut *cx, context, dst);
            let r = live_leaf_to_noun(&mut *cx, rest, dst);
            let tail = T(dst, &[ctx, r]);
            let coil = T(dst, &[g, tail]);
            T(dst, &[D(tas("core")), p, coil])
        }
        Type::Face { tool, inner } => {
            let tl = live_leaf_to_noun(&mut *cx, tool, dst);
            let inn = live_to_noun(&mut *cx, inner, dst);
            T(dst, &[D(tas("face")), tl, inn])
        }
        Type::Hint { head, payload } => {
            let h = live_leaf_to_noun(&mut *cx, head, dst);
            let p = live_to_noun(&mut *cx, payload, dst);
            T(dst, &[D(tas("hint")), h, p])
        }
        Type::Fork { set, .. } => {
            let s = live_leaf_to_noun(&mut *cx, set, dst);
            T(dst, &[D(tas("fork")), s])
        }
        Type::Hold { subject, gene } => {
            let s = live_to_noun(&mut *cx, subject, dst);
            let g = live_leaf_to_noun(&mut *cx, gene, dst);
            T(dst, &[D(tas("hold")), s, g])
        }
    };
    // The built node cell lives in the compile slab `dst`; children are already
    // resident there via their own `live_to_noun`/`live_leaf_to_noun`. The caller
    // (`live_to_noun`) does the memo insert that keeps it live for the compile.
    noun
}

/// Intern a single native node through the owned per-compile table (`cx.live`) —
/// the ONE canonical pointer-identity universe shared by all native-shadow
/// construction (so `intern_shallow`'s children-by-`Rc`-pointer hashing stays
/// valid). Children must already be canonical `Rc<Type>` from this same table.
/// Works regardless of `HONK_NATIVE_TYPES` (the flag only gates the measurement
/// hook); it is only ever CALLED on the native-shadow path.
pub fn live_intern(cx: &mut Context, node: Type) -> Rc<Type> {
    cx.live.table.intern_shallow(node)
}

/// Native-only cell constructor (collapse-aware): `cell(void,_)`/`cell(_,void)` ->
/// void, else `Cell`. For flipped producers that hold native children directly.
pub fn cons_cell(cx: &mut Context, head: Rc<Type>, tail: Rc<Type>) -> Rc<Type> {
    if matches!(&*head, Type::Void) || matches!(&*tail, Type::Void) {
        return live_intern(cx, Type::Void);
    }
    live_intern(cx, Type::Cell(head, tail))
}

/// Native `%void` / `%noun`.
pub fn cons_void(cx: &mut Context) -> Rc<Type> {
    live_intern(cx, Type::Void)
}
pub fn cons_noun(cx: &mut Context) -> Rc<Type> {
    live_intern(cx, Type::Noun)
}

/// Collapse-aware native `%core`: `core(void,_)` -> void (mirrors ty_core_n).
/// The coil is carried decomposed: tiny `garb`/bounded `rest` as leaves and the
/// `context` (deepening subject) as a SHARED native `Rc<Type>`.
pub fn cons_core(
    cx: &mut Context,
    payload: Rc<Type>,
    garb: Garb,
    context: Rc<Type>,
    rest: Leaf,
) -> Rc<Type> {
    if matches!(&*payload, Type::Void) {
        return live_intern(cx, Type::Void);
    }
    live_intern(
        cx,
        Type::Core {
            payload,
            garb,
            context,
            rest,
        },
    )
}

/// Collapse-aware native `%face`: `face(_,void)` -> void (mirrors ty_face_tool_n).
pub fn cons_face(cx: &mut Context, tool: Leaf, inner: Rc<Type>) -> Rc<Type> {
    if matches!(&*inner, Type::Void) {
        return live_intern(cx, Type::Void);
    }
    live_intern(cx, Type::Face { tool, inner })
}

/// Collapse-aware native `%hint`: `hint(_,void)` -> void, `hint(_,noun)` -> noun
/// (mirrors ty_hint_n).
pub fn cons_hint(cx: &mut Context, head: Leaf, payload: Rc<Type>) -> Rc<Type> {
    match &*payload {
        Type::Void => live_intern(cx, Type::Void),
        Type::Noun => live_intern(cx, Type::Noun),
        _ => live_intern(cx, Type::Hint { head, payload }),
    }
}

/// The O(n) fallback for a not-yet-threaded child: decode `noun` to its canonical
/// native `Rc<Type>` via the shared memoized walk. One shared `(table, memo)`
/// means each noun node is walked at most once per compile.
pub fn native_of(cx: &mut Context, noun: Noun, space: &NounSpace) -> Result<Rc<Type>> {
    intern_type_noun(&mut cx.live.table, &mut cx.live.memo, noun, space)
}

/// Canonicalize a carried live noun before embedding it in a native type node.
/// Pointer-distinct but structurally equal leaves pay one exact comparison on
/// first sight and thereafter share the same raw noun, making hot type-table
/// equality checks pointer-fast.
pub fn live_leaf_from_noun(cx: &mut Context, noun: Noun, space: &NounSpace) -> Leaf {
    cx.live.table.intern_live_leaf(noun, space)
}

/// Memoized leaf lowering for the flipped consumers: lower a carried `Leaf`
/// (core coil, fork set, hold gene, atom aura/bits, face tool, hint head) to a
/// noun for the still-noun leaf helpers (coil_parts/fork_set_options/garb_*/fitz),
/// caching `Jammed` leaves by their `Arc` pointer so repeated lowerings on the hot
/// recursive paths are O(1). Reset per compile via `live_reset`.
pub fn live_leaf_to_noun(cx: &mut Context, leaf: &Leaf, dst: &mut NounSlab) -> Noun {
    match leaf {
        Leaf::Direct(_) => leaf.to_noun(dst),
        // The cue elimination: a raw leaf's noun already lives in THIS compile's
        // `dst` slab (built there during minting, kept live for the whole compile),
        // so return it as-is — no copy, no cue.
        Leaf::Noun(n, _) => *n,
        Leaf::Jammed(arc, _) => {
            let ptr = std::sync::Arc::as_ptr(arc) as *const u8 as usize;
            if let Some(noun) = cx.leaf_memo.get(&ptr).copied() {
                return noun;
            }
            // Cued into the compile slab `dst` and kept live for the whole compile
            // by the memo (the frame arena was retired, so there is no base region
            // to relocate to).
            let noun = leaf.to_noun(dst);
            cx.leaf_memo.insert(ptr, noun);
            noun
        }
    }
}

/// Live byte-exact oracle: panic unless `to_noun(native)` jams identically to the
/// `noun` it shadows. The per-node validation for the construction port.
pub fn assert_native_eq(noun: Noun, native: &Rc<Type>, space: &NounSpace) {
    let mut a: NounSlab = NounSlab::new();
    a.copy_into(noun, space);
    let ja = a.jam();
    let mut b: NounSlab = NounSlab::new();
    let rebuilt = native.to_noun(&mut b);
    b.set_root(rebuilt);
    let jb = b.jam();
    assert!(
        ja == jb,
        "native shadow mismatch: to_noun(native)={} bytes != noun={} bytes",
        jb.len(),
        ja.len()
    );
}

#[derive(Default)]
pub struct TypeTable {
    buckets: HashMap<u64, Vec<Rc<Type>>>,
    /// Source-noun identity to its canonical carried leaf.
    live_leaves_by_raw: HashMap<u64, Leaf>,
    /// Mug buckets for the one exact comparison needed to canonicalize a new
    /// source identity. The full noun comparison protects against mug
    /// collisions; equal leaves then share one raw noun.
    live_leaves_by_mug: HashMap<u32, Vec<Leaf>>,
    /// Stable ownership for canonical nodes. Handles point into these boxes and
    /// therefore clone without touching a reference count.
    slots: Vec<Box<TypeSlot<Type>>>,
    /// Total node-constructions seen by `intern` (the un-shared structural size).
    pub interned_calls: u64,
    /// Distinct canonical nodes retained (the hash-consed size).
    pub distinct: u64,
    /// Dedup hits (a structurally-equal node already existed).
    pub hits: u64,
}

impl TypeTable {
    pub fn new() -> Self {
        Self::default()
    }

    fn intern_live_leaf(&mut self, noun: Noun, space: &NounSpace) -> Leaf {
        if noun.is_direct() {
            return Leaf::from_noun_raw(noun, space);
        }
        let raw = unsafe { noun.as_raw() };
        if let Some(canonical) = self.live_leaves_by_raw.get(&raw) {
            return canonical.clone();
        }
        let leaf = Leaf::from_noun_raw(noun, space);
        let Leaf::Noun(_, mug) = &leaf else {
            return leaf;
        };

        let canonical = self.live_leaves_by_mug.get(mug).and_then(|bucket| {
            bucket
                .iter()
                .find(|candidate| {
                    let Leaf::Noun(existing, _) = candidate else {
                        unreachable!("live leaf mug buckets contain only raw nouns")
                    };
                    noun_eq(*existing, noun, space)
                        .expect("live native-type leaves must be valid slab nouns")
                })
                .cloned()
        });
        if let Some(canonical) = canonical {
            self.live_leaves_by_raw.insert(raw, canonical.clone());
            return canonical;
        }

        self.live_leaves_by_mug
            .entry(*mug)
            .or_default()
            .push(leaf.clone());
        self.live_leaves_by_raw.insert(raw, leaf.clone());
        leaf
    }

    /// Intern the independently-owned boundary decoder tree. This path only
    /// serves the optional public stats oracle; live compilation constructs
    /// canonical arena nodes directly through `intern_shallow`.
    pub(super) fn intern_boundary(&mut self, t: &BoundaryType) -> Rc<Type> {
        let node = match t {
            BoundaryType::Void => Type::Void,
            BoundaryType::Noun => Type::Noun,
            BoundaryType::Atom { aura, bits } => Type::Atom {
                aura: aura.clone(),
                bits: bits.clone(),
            },
            BoundaryType::Cell(head, tail) => {
                Type::Cell(self.intern_boundary(head), self.intern_boundary(tail))
            }
            BoundaryType::Core {
                payload,
                garb,
                context,
                rest,
            } => Type::Core {
                payload: self.intern_boundary(payload),
                garb: garb.clone(),
                context: self.intern_boundary(context),
                rest: rest.clone(),
            },
            BoundaryType::Face { tool, inner } => Type::Face {
                tool: tool.clone(),
                inner: self.intern_boundary(inner),
            },
            BoundaryType::Hint { head, payload } => Type::Hint {
                head: head.clone(),
                payload: self.intern_boundary(payload),
            },
            BoundaryType::Fork { set, options } => {
                let native_options = std::cell::OnceCell::new();
                native_options
                    .set(
                        options
                            .iter()
                            .map(|option| self.intern_boundary(option))
                            .collect(),
                    )
                    .expect("fresh fork options cell");
                Type::Fork {
                    set: set.clone(),
                    options: native_options,
                    options_seen: std::cell::Cell::new(true),
                }
            }
            BoundaryType::Hold { subject, gene } => Type::Hold {
                subject: self.intern_boundary(subject),
                gene: gene.clone(),
            },
        };
        self.intern_node(node)
    }

    /// Intern a single node whose children are ALREADY canonical (interned).
    /// O(1) amortized. Used by the memoized decode-and-intern walk
    /// ([`intern_type_noun`]) which interns bottom-up itself.
    pub fn intern_shallow(&mut self, node: Type) -> Rc<Type> {
        self.intern_node(node)
    }

    fn intern_node(&mut self, node: Type) -> Rc<Type> {
        self.interned_calls += 1;
        let h = node_hash(&node);
        if let Some(bucket) = self.buckets.get(&h) {
            for existing in bucket {
                if node_eq(existing, &node) {
                    self.hits += 1;
                    return Rc::clone(existing);
                }
            }
        }
        let id = TypeId(
            u32::try_from(self.slots.len())
                .expect("one compiler context cannot contain more than u32::MAX type nodes"),
        );
        let slot = Box::new(TypeSlot::new(id, node));
        let rc = Rc::from_arena_slot(&slot);
        self.slots.push(slot);
        self.buckets.entry(h).or_default().push(Rc::clone(&rc));
        self.distinct += 1;
        rc
    }
}

/// Shallow structural hash: variant + children by canonical `Rc` pointer + leaf
/// content. Valid only when children are already interned (bottom-up). A fork's
/// adaptive `options`/`options_seen` state is deliberately excluded: both are
/// pure caches of the exact `set` witness, and interior mutation must never
/// change a key after insertion into `TypeTable`.
fn node_hash(t: &Type) -> u64 {
    let mut h = DefaultHasher::new();
    std::mem::discriminant(t).hash(&mut h);
    let p = |rc: &Rc<Type>, h: &mut DefaultHasher| (canonical_id(rc)).hash(h);
    match t {
        Type::Void | Type::Noun => {}
        Type::Atom { aura, bits } => {
            aura.hash(&mut h);
            bits.hash(&mut h);
        }
        Type::Cell(a, b) => {
            p(a, &mut h);
            p(b, &mut h);
        }
        Type::Core {
            payload,
            garb,
            context,
            rest,
        } => {
            p(payload, &mut h);
            garb.hash(&mut h);
            p(context, &mut h);
            rest.hash(&mut h);
        }
        Type::Face { tool, inner } => {
            tool.hash(&mut h);
            p(inner, &mut h);
        }
        Type::Hint { head, payload } => {
            head.hash(&mut h);
            p(payload, &mut h);
        }
        Type::Fork { set, .. } => set.hash(&mut h),
        Type::Hold { subject, gene } => {
            p(subject, &mut h);
            gene.hash(&mut h);
        }
    }
    h.finish()
}

/// Shallow structural equality (children by canonical `Rc` identity). As with
/// `node_hash`, fork equality is defined by the exact set witness rather than its
/// lazily populated native-edge cache.
fn node_eq(a: &Type, b: &Type) -> bool {
    use Type::*;
    match (a, b) {
        (Void, Void) | (Noun, Noun) => true,
        (Atom { aura: a1, bits: b1 }, Atom { aura: a2, bits: b2 }) => a1 == a2 && b1 == b2,
        (Cell(h1, t1), Cell(h2, t2)) => Rc::ptr_eq(h1, h2) && Rc::ptr_eq(t1, t2),
        (
            Core {
                payload: p1,
                garb: g1,
                context: ctx1,
                rest: r1,
            },
            Core {
                payload: p2,
                garb: g2,
                context: ctx2,
                rest: r2,
            },
        ) => Rc::ptr_eq(p1, p2) && g1 == g2 && Rc::ptr_eq(ctx1, ctx2) && r1 == r2,
        (
            Face {
                tool: t1,
                inner: i1,
            },
            Face {
                tool: t2,
                inner: i2,
            },
        ) => t1 == t2 && Rc::ptr_eq(i1, i2),
        (
            Hint {
                head: h1,
                payload: p1,
            },
            Hint {
                head: h2,
                payload: p2,
            },
        ) => h1 == h2 && Rc::ptr_eq(p1, p2),
        (Fork { set: s1, .. }, Fork { set: s2, .. }) => s1 == s2,
        (
            Hold {
                subject: s1,
                gene: g1,
            },
            Hold {
                subject: s2,
                gene: g2,
            },
        ) => Rc::ptr_eq(s1, s2) && g1 == g2,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use nockvm::noun::NounAllocator;

    use super::*;
    use crate::native::ir::leaf::Leaf;

    fn atom() -> Type {
        Type::Atom {
            aura: Leaf::Direct(100),
            bits: Leaf::Direct(0),
        }
    }

    #[test]
    fn dedups_structurally_equal_to_one_rc() {
        let mut tab = TypeTable::new();
        let r1 = tab.intern_shallow(atom());
        let r2 = tab.intern_shallow(atom());
        assert!(Rc::ptr_eq(&r1, &r2), "equal atoms intern to one Rc");
        assert_eq!(tab.distinct, 1);
        assert_eq!(tab.hits, 1);

        // Equal cells over equal children also collapse.
        let rc1 = tab.intern_shallow(Type::Cell(r1, r1));
        let rc2 = tab.intern_shallow(Type::Cell(r2, r2));
        assert!(Rc::ptr_eq(&rc1, &rc2), "equal cells intern to one Rc");
        assert_eq!(tab.distinct, 2, "only the atom and the cell are distinct");
    }

    #[test]
    fn live_leaf_interning_canonicalizes_equal_distinct_nouns() {
        let mut slab: NounSlab = NounSlab::new();
        let first = T(&mut slab, &[D(1), D(2)]);
        let second = T(&mut slab, &[D(1), D(2)]);
        assert!(!unsafe { first.raw_equals(&second) });
        let space = slab.noun_space();
        let mut tab = TypeTable::new();

        let first_leaf = tab.intern_live_leaf(first, &space);
        let second_leaf = tab.intern_live_leaf(second, &space);
        let (Leaf::Noun(first_noun, _), Leaf::Noun(second_noun, _)) = (&first_leaf, &second_leaf)
        else {
            panic!("cell leaves must remain raw nouns")
        };

        assert!(unsafe { first_noun.raw_equals(second_noun) });
        assert_eq!(tab.live_leaves_by_raw.len(), 2);
        assert_eq!(
            tab.live_leaves_by_mug.values().map(Vec::len).sum::<usize>(),
            1
        );
    }

    #[test]
    fn arena_handles_are_one_word_copies_with_dense_ids() {
        assert_eq!(
            std::mem::size_of::<Rc<Type>>(),
            std::mem::size_of::<usize>()
        );
        let mut tab = TypeTable::new();
        let atom = tab.intern_shallow(atom());
        let noun = tab.intern_shallow(Type::Noun);
        let atom_copy = atom;
        assert!(Rc::ptr_eq(&atom, &atom_copy));
        assert_eq!(atom.arena_id(), TypeId(0));
        assert_eq!(noun.arena_id(), TypeId(1));
    }

    // The subject-deepening fix in miniature: a fully-duplicated balanced cell
    // tree of depth D has 2^(D+1)-1 structural nodes but only D+1 distinct after
    // hash-consing — O(2^n) → O(n).
    #[test]
    fn hash_consing_collapses_duplicated_structure() {
        fn build(tab: &mut TypeTable, depth: u32) -> Rc<Type> {
            if depth == 0 {
                tab.intern_shallow(atom())
            } else {
                let head = build(tab, depth - 1);
                let tail = build(tab, depth - 1);
                tab.intern_shallow(Type::Cell(head, tail))
            }
        }
        let depth = 12;
        let mut tab = TypeTable::new();
        let _root = build(&mut tab, depth);
        assert_eq!(
            tab.distinct as u32,
            depth + 1,
            "duplicated tree collapses to O(depth) distinct nodes"
        );
        assert_eq!(
            tab.interned_calls,
            (1u64 << (depth + 1)) - 1,
            "the full duplicated tree was walked"
        );
    }

    #[test]
    fn fork_edge_materialization_does_not_change_intern_identity() {
        let empty_edges = std::cell::OnceCell::new();
        let fork = Type::Fork {
            set: Leaf::Direct(123),
            options: empty_edges,
            options_seen: Default::default(),
        };
        let mut tab = TypeTable::new();
        let canonical = tab.intern_shallow(fork);
        let hash_before = node_hash(&canonical);

        let Type::Fork { options, .. } = &*canonical else {
            unreachable!()
        };
        options
            .set(vec![tab.intern_shallow(atom())])
            .expect("first fork-edge materialization");
        assert_eq!(
            hash_before,
            node_hash(&canonical),
            "interior edge caching must not mutate the interner key"
        );

        let duplicate = Type::Fork {
            set: Leaf::Direct(123),
            options: std::cell::OnceCell::new(),
            options_seen: Default::default(),
        };
        let reinterned = tab.intern_shallow(duplicate);
        assert!(
            Rc::ptr_eq(&canonical, &reinterned),
            "the exact set witness remains fork identity after edge caching"
        );
    }
}
