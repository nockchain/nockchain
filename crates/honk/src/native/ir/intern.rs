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
//!
//! NOTE: this operates on the Phase-1 boundary `Type` (native skeleton + carried
//! leaves). It already dedups the native skeleton — crucially the recursive
//! payload/cell/inner/subject chains where subject-deepening lives. Phase 2
//! nativizes the carried leaves (coil/treap/gene) so the deduplication reaches
//! inside cores/forks too; the table mechanism here is unchanged by that.
#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounSpace, D, T};

use super::leaf::Leaf;
use super::ty::{tas, Garb, Type};
use crate::errors::{CompilerError, Result};
use crate::native::noun::noun_pair;

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
            aura: Leaf::from_noun_raw(aura, space),
            bits: Leaf::from_noun_raw(bits, space),
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
            rest: Leaf::from_noun_raw(rest, space),
        }
    } else if tag.eq_bytes(b"face") {
        let (tool, inner) = pair(tail, space)?;
        Type::Face {
            tool: Leaf::from_noun_raw(tool, space),
            inner: intern_type_noun(table, memo, inner, space)?,
        }
    } else if tag.eq_bytes(b"hint") {
        let (head, payload) = pair(tail, space)?;
        Type::Hint {
            head: Leaf::from_noun_raw(head, space),
            payload: intern_type_noun(table, memo, payload, space)?,
        }
    } else if tag.eq_bytes(b"fork") {
        Type::Fork {
            set: Leaf::from_noun_raw(tail, space),
        }
    } else if tag.eq_bytes(b"hold") {
        let (subject, gene) = pair(tail, space)?;
        Type::Hold {
            subject: intern_type_noun(table, memo, subject, space)?,
            gene: Leaf::from_noun_raw(gene, space),
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
    to_noun_memo: HashMap<usize, Noun>, // was TO_NOUN_MEMO
    leaf_memo: HashMap<usize, Noun>,    // was LEAF_MEMO

    // --- boundary caches (key/value tuples copied verbatim) ---
    nest_cache: HashMap<(usize, usize, u8, u64), bool>, // NEST_CACHE
    #[allow(clippy::type_complexity)]
    core_mint_cache: HashMap<(usize, usize, u64, u8, u8, u64, u64, u64), (Rc<Type>, Noun)>, // CORE_MINT_CACHE
    #[allow(clippy::type_complexity)]
    mint_cache: HashMap<(usize, usize, u8, u64, u64, u64, u64), (Rc<Type>, Noun)>, // MINT_CACHE
    #[allow(clippy::type_complexity)]
    mull_cache: HashMap<(usize, usize, usize, u8, u64, u64, u64, u64), (Rc<Type>, Rc<Type>)>, // MULL_CACHE
    fuse_cache: HashMap<(usize, usize, u8, u64), Rc<Type>>, // FUSE_CACHE
    crop_cache: HashMap<(usize, usize, u8, u64), Rc<Type>>, // CROP_CACHE
    fish_cache: HashMap<(usize, u64, u8, u64), Noun>,       // FISH_CACHE

    // --- native_of content-keyed decode cache + fork cache ---
    native_of_mug_memo: HashMap<u64, Vec<Rc<Type>>>, // NATIVE_OF_MUG_MEMO
    fork_cache: HashMap<Vec<usize>, Rc<Type>>,       // FORK_CACHE

    // --- scope-precise fan key support (reachable %hold legs per type) ---
    // `legset_memo` maps an interned `Rc<Type>` pointer to the sorted-deduped set
    // of %hold leg-ids reachable from that type (`reachable_legs` in ut/mod.rs).
    // Sound because `intern_node` hash-conses (ptr == structural identity), so the
    // legset is a pure function of the pointer; computed bottom-up over the Rc DAG
    // and memoized so each distinct node is visited once (O(1) amortized).
    legset_memo: HashMap<usize, Rc<[u64]>>,
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
pub fn fork_cache_lookup(cx: &Context, key: &[usize]) -> Option<Rc<Type>> {
    cx.fork_cache.get(key).cloned()
}

/// Store a `cons_fork` result keyed by its canonical option-pointer key.
pub fn fork_cache_store(cx: &mut Context, key: Vec<usize>, fork: Rc<Type>) {
    cx.fork_cache.insert(key, fork);
}

/// Look up the memoized reachable-leg set for an interned type pointer.
/// `ptr` is `Rc::as_ptr(t) as usize`; the value is the sorted-deduped set of
/// %hold leg-ids reachable from `t`. See `reachable_legs` in ut/mod.rs.
pub fn legset_memo_lookup(cx: &Context, ptr: usize) -> Option<Rc<[u64]>> {
    cx.legset_memo.get(&ptr).cloned()
}

/// Store a memoized reachable-leg set for an interned type pointer.
pub fn legset_memo_store(cx: &mut Context, ptr: usize, legs: Rc<[u64]>) {
    cx.legset_memo.insert(ptr, legs);
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
) -> Option<(Rc<Type>, Noun)> {
    let key = (
        Rc::as_ptr(sut) as usize,
        Rc::as_ptr(gol) as usize,
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
    formula: Noun,
) {
    let key = (
        Rc::as_ptr(sut) as usize,
        Rc::as_ptr(gol) as usize,
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
) -> Option<(Rc<Type>, Noun)> {
    let key = (
        Rc::as_ptr(sut) as usize,
        Rc::as_ptr(gol) as usize,
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
    formula: Noun,
) {
    let key = (
        Rc::as_ptr(sut) as usize,
        Rc::as_ptr(gol) as usize,
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
        Rc::as_ptr(sut) as usize,
        Rc::as_ptr(gol) as usize,
        Rc::as_ptr(dox) as usize,
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
        Rc::as_ptr(sut) as usize,
        Rc::as_ptr(gol) as usize,
        Rc::as_ptr(dox) as usize,
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
    let key = (Rc::as_ptr(sut) as usize, Rc::as_ptr(ref_) as usize, vet, fan);
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
    let key = (Rc::as_ptr(sut) as usize, Rc::as_ptr(ref_) as usize, vet, fan);
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
    let key = (Rc::as_ptr(sut) as usize, Rc::as_ptr(ref_) as usize, vet, fan);
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
    let key = (Rc::as_ptr(sut) as usize, Rc::as_ptr(ref_) as usize, vet, fan);
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
    let key = (Rc::as_ptr(sut) as usize, Rc::as_ptr(ref_) as usize, vet, fan);
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
    let key = (Rc::as_ptr(sut) as usize, Rc::as_ptr(ref_) as usize, vet, fan);
    cx.crop_cache.insert(key, result);
}

/// Look up a native `fish` result by interned (sut) pointer + (axis, vet, fan).
/// Returns the cached NOCK FORMULA `Noun` directly.
pub fn fish_cache_lookup(cx: &Context, sut: &Rc<Type>, axis: u64, vet: u8, fan: u64) -> Option<Noun> {
    let key = (Rc::as_ptr(sut) as usize, axis, vet, fan);
    cx.fish_cache.get(&key).copied()
}

/// Store a native `fish` result (formula `Noun`) by interned (sut) pointer +
/// (axis, vet, fan).
pub fn fish_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    axis: u64,
    vet: u8,
    fan: u64,
    result: Noun,
) {
    let key = (Rc::as_ptr(sut) as usize, axis, vet, fan);
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
    let ptr = Rc::as_ptr(native) as usize;
    if let Some(noun) = cx.to_noun_memo.get(&ptr).copied() {
        return noun;
    }
    // Stack guard: this per-node recursion descends as deep as the type and runs
    // INSIDE other deep recursions (redo/repo, reached via `native_of_cached`'s
    // verify). Without growing the stack a deep recursive type overflows the guard
    // page (SIGBUS on macOS, no Rust panic). `maybe_grow` is a cheap pointer
    // compare when headroom remains; 64MB chunks dwarf any real type depth.
    let noun =
        stacker::maybe_grow(32 * 1024, 64 * 1024 * 1024, || live_to_noun_node(cx, native, dst));
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
        Type::Fork { set } => {
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
pub fn cons_core(cx: &mut Context, payload: Rc<Type>, garb: Garb, context: Rc<Type>, rest: Leaf) -> Rc<Type> {
    if matches!(&*payload, Type::Void) {
        return live_intern(cx, Type::Void);
    }
    live_intern(cx, Type::Core {
        payload,
        garb,
        context,
        rest,
    })
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

    /// Intern a type tree bottom-up, returning its canonical `Rc`.
    pub fn intern(&mut self, t: &Type) -> Rc<Type> {
        let node = match t {
            Type::Void => Type::Void,
            Type::Noun => Type::Noun,
            Type::Atom { aura, bits } => Type::Atom {
                aura: aura.clone(),
                bits: bits.clone(),
            },
            Type::Cell(h, tl) => Type::Cell(self.intern(h), self.intern(tl)),
            Type::Core {
                payload,
                garb,
                context,
                rest,
            } => Type::Core {
                payload: self.intern(payload),
                garb: garb.clone(),
                context: self.intern(context),
                rest: rest.clone(),
            },
            Type::Face { tool, inner } => Type::Face {
                tool: tool.clone(),
                inner: self.intern(inner),
            },
            Type::Hint { head, payload } => Type::Hint {
                head: head.clone(),
                payload: self.intern(payload),
            },
            Type::Fork { set } => Type::Fork { set: set.clone() },
            Type::Hold { subject, gene } => Type::Hold {
                subject: self.intern(subject),
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
        let rc = Rc::new(node);
        self.buckets.entry(h).or_default().push(Rc::clone(&rc));
        self.distinct += 1;
        rc
    }
}

/// Shallow structural hash: variant + children by canonical `Rc` pointer + leaf
/// content. Valid only when children are already interned (bottom-up).
fn node_hash(t: &Type) -> u64 {
    let mut h = DefaultHasher::new();
    std::mem::discriminant(t).hash(&mut h);
    let p = |rc: &Rc<Type>, h: &mut DefaultHasher| (Rc::as_ptr(rc) as usize).hash(h);
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
        Type::Fork { set } => set.hash(&mut h),
        Type::Hold { subject, gene } => {
            p(subject, &mut h);
            gene.hash(&mut h);
        }
    }
    h.finish()
}

/// Shallow structural equality (children by canonical `Rc` identity).
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
        (Face { tool: t1, inner: i1 }, Face { tool: t2, inner: i2 }) => {
            t1 == t2 && Rc::ptr_eq(i1, i2)
        }
        (Hint { head: h1, payload: p1 }, Hint { head: h2, payload: p2 }) => {
            h1 == h2 && Rc::ptr_eq(p1, p2)
        }
        (Fork { set: s1 }, Fork { set: s2 }) => s1 == s2,
        (Hold { subject: s1, gene: g1 }, Hold { subject: s2, gene: g2 }) => {
            Rc::ptr_eq(s1, s2) && g1 == g2
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
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
        let r1 = tab.intern(&atom());
        let r2 = tab.intern(&atom()); // distinct allocation, equal structure
        assert!(Rc::ptr_eq(&r1, &r2), "equal atoms intern to one Rc");
        assert_eq!(tab.distinct, 1);
        assert_eq!(tab.hits, 1);

        // Equal cells over equal children also collapse.
        let c1 = Type::Cell(Rc::new(atom()), Rc::new(atom()));
        let c2 = Type::Cell(Rc::new(atom()), Rc::new(atom()));
        let rc1 = tab.intern(&c1);
        let rc2 = tab.intern(&c2);
        assert!(Rc::ptr_eq(&rc1, &rc2), "equal cells intern to one Rc");
        assert_eq!(tab.distinct, 2, "only the atom and the cell are distinct");
    }

    // The subject-deepening fix in miniature: a fully-duplicated balanced cell
    // tree of depth D has 2^(D+1)-1 structural nodes but only D+1 distinct after
    // hash-consing — O(2^n) → O(n).
    #[test]
    fn hash_consing_collapses_duplicated_structure() {
        fn build(depth: u32) -> Type {
            if depth == 0 {
                atom()
            } else {
                Type::Cell(Rc::new(build(depth - 1)), Rc::new(build(depth - 1)))
            }
        }
        let depth = 12;
        let mut tab = TypeTable::new();
        let _root = tab.intern(&build(depth));
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
}
