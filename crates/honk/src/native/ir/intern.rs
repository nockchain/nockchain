//! Hash-cons table for [`super::ty::Type`].
//!
//! A [`TypeTable`] owns boxed slots for one compiler context. Equal nodes map to
//! one dense [`super::ty::TypeId`] and one stable [`super::ty::TypeRef`], so the
//! repeated subjects produced while minting share their representation. Nodes
//! are interned bottom-up; child IDs make shallow hashing and exact comparison
//! constant-time with respect to descendant depth. Exact Hoon nouns retained by
//! leaves and forks remain the serialization witnesses at noun boundaries.

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
use crate::native::identity::*;
use crate::native::noun::{noun_eq, noun_pair};
use crate::native::ut::keys::*;

/// Decode a type noun into native IR and intern it in one pass. The
/// pointer-identity `memo` persists across calls, so each noun node is walked at
/// most once per compile. Structurally equal but pointer-distinct subtrees (the
/// duplicated subjects of subject deepening) collapse to one canonical handle.
fn intern_type_noun(
    table: &mut TypeTable,
    memo: &mut InternMemo,
    noun: Noun,
    space: &NounSpace,
) -> Result<Rc<Type>> {
    // `%void` and `%noun` are direct atom cords and skip the memo.
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
    // The identity word is a memo key only and is never dereferenced.
    let raw_addr = NounIdentity::of(noun);
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
// Per-compile hash-consing table.
//
// Every native type is interned into one table owned by the compile's
// `Context`, so structurally equal types share one canonical handle.
// ---------------------------------------------------------------------------

/// Decode memo for `intern_type_noun` and `native_of`, from source noun
/// identity to the canonical interned type. The compile slab never reclaims
/// memory, so a source address is never reused within a compile and an entry
/// never goes stale. Entries accumulate for the life of the `Context`.
struct InternMemo {
    map: HashMap<NounIdentity, Rc<Type>>,
}

impl InternMemo {
    fn new() -> Self {
        InternMemo {
            map: HashMap::new(),
        }
    }

    #[inline]
    fn get(&self, raw: &NounIdentity) -> Option<&Rc<Type>> {
        self.map.get(raw)
    }

    #[inline]
    fn insert(&mut self, raw: NounIdentity, rc: Rc<Type>) {
        self.map.insert(raw, rc);
    }
}

struct LiveIntern {
    table: TypeTable,
    memo: InternMemo,
}

impl LiveIntern {
    fn new() -> Self {
        LiveIntern {
            table: TypeTable::new(),
            memo: InternMemo::new(),
        }
    }
}

/// Per-compile native IR state, owned by `Ut` as its `cx` field.
///
/// Holds the hash-consing table, the encode memos, and the native boundary
/// caches. Each `Ut` gets a fresh `Context`, so compiles never share cache
/// entries. Dropping the owning `Ut` drops this context and every handle into
/// it together; the table is never reset while `TypeRef` handles may be live.
pub struct Context {
    live: LiveIntern,

    // Encode memos.
    to_noun_memo: HashMap<TypeId, Noun>,
    leaf_memo: HashMap<JamIdentity, Noun>,

    // Boundary caches keyed by canonical type IDs.
    nest_cache: HashMap<TypeBinaryKey<TypeId>, bool>,
    /// Entries carry the tomes map and prefix the key's `TomesSignature` was
    /// hashed from; the lookup's caller checks them before trusting a hit.
    core_mint_cache: HashMap<CoreMintKey, CoreMintEntry>,
    mint_cache: HashMap<MintKey<TypeId>, (Rc<Type>, FormulaId)>,
    mull_cache: HashMap<MullKey, (Rc<Type>, Rc<Type>)>,
    fuse_cache: HashMap<TypeBinaryKey<TypeId>, Rc<Type>>,
    crop_cache: HashMap<TypeBinaryKey<TypeId>, Rc<Type>>,
    fish_cache: HashMap<FishKey, FormulaId>,

    // Content-keyed `native_of` decode cache; mug buckets are compared exactly.
    native_of_mug_memo: HashMap<NounMug, Vec<Rc<Type>>>,

    // Sorted, deduplicated hold legs reachable from each canonical type.
    // Computed bottom-up over the arena DAG once per distinct TypeId.
    legset_memo: HashMap<TypeId, SharedRc<[FanLegId]>>,
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
            legset_memo: HashMap::new(),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
fn canonical_id(ty: &Rc<Type>) -> TypeId {
    ty.arena_id()
}

/// Look up the sorted, deduplicated hold legs reachable from a canonical type.
pub fn legset_memo_lookup(cx: &Context, id: TypeId) -> Option<SharedRc<[FanLegId]>> {
    cx.legset_memo.get(&id).cloned()
}

/// Store the reachable hold legs for a canonical type.
pub fn legset_memo_store(cx: &mut Context, id: TypeId, legs: SharedRc<[FanLegId]>) {
    cx.legset_memo.insert(id, legs);
}

/// Content-keyed `native_of` fast path: returns the types recorded for a noun
/// mug. Callers compare each candidate exactly.
pub fn native_of_mug_candidates(cx: &Context, mug: NounMug) -> Vec<Rc<Type>> {
    cx.native_of_mug_memo.get(&mug).cloned().unwrap_or_default()
}

/// Record a decoded `(mug -> Rc)` association for the content-keyed `native_of`
/// cache. Idempotent per `Rc` within a bucket.
pub fn native_of_mug_insert(cx: &mut Context, mug: NounMug, rc: Rc<Type>) {
    let bucket = cx.native_of_mug_memo.entry(mug).or_default();
    if !bucket.iter().any(|existing| Rc::ptr_eq(existing, &rc)) {
        bucket.push(rc);
    }
}

/// A cached `core_mint` result with the tomes map and prefix it was minted
/// from, which the 31-bit mug in `TomesSignature` alone does not pin down.
#[derive(Clone)]
pub struct CoreMintEntry {
    pub core_type: Rc<Type>,
    pub formula: FormulaId,
    pub tomes_map: Noun,
    pub prefix: Option<String>,
}

/// Look up a native `core_mint` result by canonical (sut, gol) IDs plus the
/// arm-map signature, vet, poly, fan, arm epoch, and placeholder signature.
#[allow(clippy::too_many_arguments)]
pub fn core_mint_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    tomes_sig: TomesSignature,
    vet: VetMode,
    poly: PolyKey,
    fan: FanContextId,
    arm_epoch: ArmEpoch,
    placeholder: PlaceholderSignature,
) -> Option<CoreMintEntry> {
    let key = CoreMintKey {
        subject: canonical_id(sut),
        goal: canonical_id(gol),
        tomes: tomes_sig,
        vet,
        poly,
        fan,
        arm_epoch,
        placeholder,
    };
    cx.core_mint_cache.get(&key).cloned()
}

/// Store a native `core_mint` result by canonical (sut, gol) IDs + semantic key.
#[allow(clippy::too_many_arguments)]
pub fn core_mint_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    tomes_sig: TomesSignature,
    vet: VetMode,
    poly: PolyKey,
    fan: FanContextId,
    arm_epoch: ArmEpoch,
    placeholder: PlaceholderSignature,
    entry: CoreMintEntry,
) {
    let key = CoreMintKey {
        subject: canonical_id(sut),
        goal: canonical_id(gol),
        tomes: tomes_sig,
        vet,
        poly,
        fan,
        arm_epoch,
        placeholder,
    };
    cx.core_mint_cache.insert(key, entry);
}

/// Look up a native `mint` result by canonical (sut, gol) IDs plus vet, the
/// gene signature, fan, arm epoch, and placeholder signature.
#[allow(clippy::too_many_arguments)]
pub fn mint_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    vet: VetMode,
    gen_sig: HoonSignature,
    fan: FanContextId,
    arm_epoch: ArmEpoch,
    placeholder: PlaceholderSignature,
) -> Option<(Rc<Type>, FormulaId)> {
    let key = MintKey {
        subject: canonical_id(sut),
        goal: canonical_id(gol),
        vet,
        gene: gen_sig,
        fan,
        arm_epoch,
        placeholder,
    };
    cx.mint_cache.get(&key).cloned()
}

/// Store a native `mint` result by canonical (sut, gol) IDs + semantic key.
#[allow(clippy::too_many_arguments)]
pub fn mint_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    vet: VetMode,
    gen_sig: HoonSignature,
    fan: FanContextId,
    arm_epoch: ArmEpoch,
    placeholder: PlaceholderSignature,
    ty: Rc<Type>,
    formula: FormulaId,
) {
    let key = MintKey {
        subject: canonical_id(sut),
        goal: canonical_id(gol),
        vet,
        gene: gen_sig,
        fan,
        arm_epoch,
        placeholder,
    };
    cx.mint_cache.insert(key, (ty, formula));
}

/// Look up a native `mull` result by canonical (sut, gol, dox) IDs plus vet,
/// the gene signature, fan, arm epoch, and placeholder signature.
#[allow(clippy::too_many_arguments)]
pub fn mull_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    dox: &Rc<Type>,
    vet: VetMode,
    gen_sig: HoonSignature,
    fan: FanContextId,
    arm_epoch: ArmEpoch,
    placeholder: PlaceholderSignature,
) -> Option<(Rc<Type>, Rc<Type>)> {
    let key = MullKey {
        subject: canonical_id(sut),
        goal: canonical_id(gol),
        secondary_subject: canonical_id(dox),
        vet,
        gene: gen_sig,
        fan,
        arm_epoch,
        placeholder,
    };
    cx.mull_cache.get(&key).cloned()
}

/// Store a native `mull` result by canonical (sut, gol, dox) IDs + semantic key.
#[allow(clippy::too_many_arguments)]
pub fn mull_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    gol: &Rc<Type>,
    dox: &Rc<Type>,
    vet: VetMode,
    gen_sig: HoonSignature,
    fan: FanContextId,
    arm_epoch: ArmEpoch,
    placeholder: PlaceholderSignature,
    p_ty: Rc<Type>,
    q_ty: Rc<Type>,
) {
    let key = MullKey {
        subject: canonical_id(sut),
        goal: canonical_id(gol),
        secondary_subject: canonical_id(dox),
        vet,
        gene: gen_sig,
        fan,
        arm_epoch,
        placeholder,
    };
    cx.mull_cache.insert(key, (p_ty, q_ty));
}

/// Look up a native `nest` result by canonical (sut, ref) IDs + context.
pub fn nest_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: VetMode,
    fan: FanContextId,
) -> Option<bool> {
    let key = TypeBinaryKey {
        subject: canonical_id(sut),
        reference: canonical_id(ref_),
        vet,
        fan,
    };
    cx.nest_cache.get(&key).copied()
}

/// Store a native `nest` result by canonical (sut, ref) IDs + context.
pub fn nest_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: VetMode,
    fan: FanContextId,
    result: bool,
) {
    let key = TypeBinaryKey {
        subject: canonical_id(sut),
        reference: canonical_id(ref_),
        vet,
        fan,
    };
    cx.nest_cache.insert(key, result);
}

/// Look up a native `fuse` result by canonical (sut, ref) IDs + (vet, fan).
pub fn fuse_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: VetMode,
    fan: FanContextId,
) -> Option<Rc<Type>> {
    let key = TypeBinaryKey {
        subject: canonical_id(sut),
        reference: canonical_id(ref_),
        vet,
        fan,
    };
    cx.fuse_cache.get(&key).cloned()
}

/// Store a native `fuse` result by canonical (sut, ref) IDs + (vet, fan).
pub fn fuse_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: VetMode,
    fan: FanContextId,
    result: Rc<Type>,
) {
    let key = TypeBinaryKey {
        subject: canonical_id(sut),
        reference: canonical_id(ref_),
        vet,
        fan,
    };
    cx.fuse_cache.insert(key, result);
}

/// Look up a native `crop` result by canonical (sut, ref) IDs + (vet, fan).
pub fn crop_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: VetMode,
    fan: FanContextId,
) -> Option<Rc<Type>> {
    let key = TypeBinaryKey {
        subject: canonical_id(sut),
        reference: canonical_id(ref_),
        vet,
        fan,
    };
    cx.crop_cache.get(&key).cloned()
}

/// Store a native `crop` result by canonical (sut, ref) IDs + (vet, fan).
pub fn crop_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    ref_: &Rc<Type>,
    vet: VetMode,
    fan: FanContextId,
    result: Rc<Type>,
) {
    let key = TypeBinaryKey {
        subject: canonical_id(sut),
        reference: canonical_id(ref_),
        vet,
        fan,
    };
    cx.crop_cache.insert(key, result);
}

/// Look up a native `fish` result by canonical subject ID + (axis, vet, fan).
pub fn fish_cache_lookup(
    cx: &Context,
    sut: &Rc<Type>,
    axis: &BigUint,
    vet: VetMode,
    fan: FanContextId,
) -> Option<FormulaId> {
    cx.fish_cache
        .get(&FishKey {
            subject: canonical_id(sut),
            axis: axis.clone(),
            vet,
            fan,
        })
        .copied()
}

/// Store a native `fish` result by canonical subject ID + (axis, vet, fan).
pub fn fish_cache_store(
    cx: &mut Context,
    sut: &Rc<Type>,
    axis: &BigUint,
    vet: VetMode,
    fan: FanContextId,
    result: FormulaId,
) {
    let key = FishKey {
        subject: canonical_id(sut),
        axis: axis.clone(),
        vet,
        fan,
    };
    cx.fish_cache.insert(key, result);
}

#[cfg(test)]
static LIVE_ENABLED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Whether `HONK_NATIVE_TYPES` is set. Test constructors then cross-check each
/// native type against the noun they build.
#[cfg(test)]
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

/// Memoized `Type::to_noun`: lower a canonical native type to a noun, caching by
/// type ID so repeated lowerings of large deepened types are O(1). IDs are unique
/// within a `Context`, and callers always lower into the one compile slab, so a
/// cached noun stays valid for the life of the `Context`.
pub fn live_to_noun(cx: &mut Context, native: &Rc<Type>, dst: &mut NounSlab) -> Noun {
    let ptr = canonical_id(native);
    if let Some(noun) = cx.to_noun_memo.get(&ptr).copied() {
        return noun;
    }
    // This recursion descends as deep as the type and runs inside other deep
    // recursions (redo/repo, via `native_of_cached`'s verify). Without growing the
    // stack, a deep recursive type overflows the guard page (SIGBUS on macOS, not a
    // Rust panic). `maybe_grow` is a pointer compare when headroom remains.
    let noun = stacker::maybe_grow(32 * 1024, 64 * 1024 * 1024, || {
        live_to_noun_node(cx, native, dst)
    });
    // The noun already lives in the compile slab `dst`, so the memo stores it as is.
    cx.to_noun_memo.insert(ptr, noun);
    noun
}

/// One node of `live_to_noun`'s recursion, split out so `maybe_grow` wraps each
/// level.
fn live_to_noun_node(cx: &mut Context, native: &Rc<Type>, dst: &mut NounSlab) -> Noun {
    // Children go through the memoized `live_to_noun`/`live_leaf_to_noun` rather
    // than the recursive `Type::to_noun`, which rebuilds the whole subtree. A
    // `%core`'s `context` is often the whole stdlib subject, shared by many
    // ancestors; the memo lowers each distinct node and cues each jammed leaf once
    // per compile. Node shapes match `Type::to_noun`, the slab-agnostic reference.
    match &**native {
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
    }
}

/// Intern one native node in the compile's type table. Children must already be
/// canonical handles from the same table, because `intern_shallow` hashes and
/// compares them by identity.
pub fn live_intern(cx: &mut Context, node: Type) -> Rc<Type> {
    cx.live.table.intern_shallow(node)
}

/// Collapse-aware native `%cell`: `cell(void,_)` and `cell(_,void)` -> void.
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
/// The coil is carried decomposed, with `context` (the deepening subject) as a
/// shared canonical type.
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

/// Decode `noun` to its canonical native type through the shared memoized walk,
/// so each noun node is walked at most once per compile.
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

/// Memoized leaf lowering: lower a carried `Leaf` (core coil rest, fork set, hold
/// gene, atom aura/bits, face tool, hint head) to a noun for the noun-based leaf
/// helpers, caching `Jammed` leaves by `Arc` pointer so repeated lowerings are O(1).
pub fn live_leaf_to_noun(cx: &mut Context, leaf: &Leaf, dst: &mut NounSlab) -> Noun {
    match leaf {
        Leaf::Direct(_) => leaf.to_noun(dst),
        // A raw leaf's noun already lives in the compile slab, so it needs no copy
        // or cue.
        Leaf::Noun(n, _) => *n,
        Leaf::Jammed(arc, _) => {
            let ptr = JamIdentity(std::sync::Arc::as_ptr(arc) as *const u8 as usize);
            if let Some(noun) = cx.leaf_memo.get(&ptr).copied() {
                return noun;
            }
            // Cued into the compile slab once and reused for the rest of the compile.
            let noun = leaf.to_noun(dst);
            cx.leaf_memo.insert(ptr, noun);
            noun
        }
    }
}

/// Test oracle: panic unless `to_noun(native)` jams identically to `noun`.
#[cfg(test)]
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
    buckets: HashMap<NodeHash, Vec<Rc<Type>>>,
    /// Source-noun identity to its canonical carried leaf.
    live_leaves_by_raw: HashMap<NounIdentity, Leaf>,
    /// Mug buckets for the one exact comparison needed to canonicalize a new
    /// source identity. The full noun comparison protects against mug
    /// collisions; equal leaves then share one raw noun.
    live_leaves_by_mug: HashMap<NounMug, Vec<Leaf>>,
    /// Stable ownership for canonical nodes. Handles point into these boxes and
    /// therefore clone without touching a reference count. The boxes are
    /// required: growing the outer vector must not move slots while `TypeRef`
    /// handles point at them.
    #[allow(clippy::vec_box)]
    slots: Vec<Box<TypeSlot<Type>>>,
    /// Total node constructions seen by `intern_node` (the unshared structural size).
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
        let raw = NounIdentity::of(noun);
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

    /// Intern a single node whose children are already canonical. O(1) amortized.
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

/// Shallow structural hash: variant + children by canonical type ID + leaf
/// content. Valid only when children are already interned (bottom-up). A fork's
/// adaptive `options`/`options_seen` state is excluded: both are caches of the
/// exact `set` witness, and interior mutation must never change a key after
/// insertion into `TypeTable`.
fn node_hash(t: &Type) -> NodeHash {
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
    NodeHash(h.finish())
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

    // A fully duplicated balanced cell tree of depth D has 2^(D+1)-1 structural
    // nodes but only D+1 distinct nodes after hash-consing.
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
