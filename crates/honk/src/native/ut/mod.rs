#![allow(clippy::items_after_test_module)]

use std::cmp;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hasher;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::rc::Rc as SharedRc;
use std::sync::Arc;

use hatch::ast::hoon::{
    Alas, Axis as AstAxis, BaseType, Beer, Block, Chum, Coil, Cord, FaceType, Garb, Gate, Hoon,
    Knot, Limb, Mane, Manx, Marl, Mart, Marx, Nock, NockHint, Note, NounExpr, ParsedAtom, Path,
    Pint, Poly as AstPoly, SemiNounExpr, Skin, Spec, Spot, Stencil, TermOrPair, TermOrTune, Tome,
    Tuna, TunaTail, Tune, Type, Tyre, Vair as AstVair, WingType, Woof, ZpwtArg,
};
use hatch::utils::{
    chum_to_nounexpr, example, factory, grip, hoon_to_noun, hoon_to_noun_with_cache, noun_to_hoon,
    open, reek, string_to_atom,
};
use nockapp::noun::slab::NounSlab;
use nockapp::noun::NounAllocatorExt;
use nockapp::utils::{create_context, NOCK_STACK_SIZE_MEDIUM};
use nockvm::ext::{AtomExt, NounExt};
use nockvm::interpreter::{interpret, Context as NockContext};
use nockvm::jets::cold::Cold;
use nockvm::jets::math::util::lth_b;
use nockvm::jets::warm::Warm;
use nockvm::jets::JetDispatchMode;
use nockvm::mem::{AllocationError, NockStack};
use nockvm::mug::{get_mug, set_mug};
use nockvm::noun::{Atom, AtomHandle, Noun, NounAllocator, NounSpace, D, T};
use num_bigint::BigUint;
use smallvec::SmallVec;

use self::keys::*;
use crate::errors::{CompilerError, CompilerErrorLocation, CompilerErrorMetadata, Result};
use crate::native::hot::native_hot_state;
use crate::native::identity::*;
use crate::native::ir::formula_dag::{FormulaArena, FormulaId};
use crate::native::ir::semi_dag::{SemiArena, SemiId, SemiNode};
use crate::native::ir::ty::TypeId;
use crate::native::ir::value_dag::{ValueArena, ValueId};
use crate::native::noun::{
    atom_to_string, noun_expr_to_noun, opt_from_noun, opt_to_noun, parsed_atom_to_noun, tag,
    term_to_noun, vec_to_list,
};
#[cfg(test)]
use crate::native::noun::{noun_eq_direct, noun_pair};

mod find;
mod fire;
pub(crate) mod keys;
mod repo;
#[cfg(test)]
pub mod test;
pub mod types;
mod wet;
pub use types::*;

#[derive(Clone, Copy)]
struct HoldRepoFanLegIdEntry {
    id: FanLegId,
    inner: Noun,
    hoon: Noun,
}

#[derive(Clone, Copy)]
struct HoldRepoFanHoldIdEntry {
    hold: Noun,
    id: FanLegId,
}

const SEMI_TAG_FULL: u64 = 1_819_047_270; // %full
const SEMI_TAG_HALF: u64 = 1_718_378_856; // %half
const SEMI_TAG_LAZY: u64 = 2_038_063_468; // %lazy

struct MuskRuntime {
    context: NockContext,
    cold_state: Option<&'static [u8]>,
    // Dynamic subject/formula states on the current partial-evaluation stack.
    araw_active: Vec<ArawKey>,
    // Copied interpreter-side cores keyed by the source core raw noun. These nouns live on the
    // runtime's long-lived eval stack, outside per-mack call frames, so repeated `^~` folds share
    // copied batteries/context instead of copying the same core tree for every arm invocation.
    mack_core_cache_raw: FastHashMap<NounIdentity, Noun>,
    mack_core_cache_context: Option<EvalFrameId>,
    // Complete seminoun data is canonicalized by `ValueArena`, so equivalent
    // cores share one identity even when their slab addresses differ.
    mack_cache_by_value: FastHashMap<MackKey, Option<Noun>>,
}

impl MuskRuntime {
    fn new() -> Self {
        Self {
            context: create_musk_eval_context(),
            cold_state: None,
            araw_active: Default::default(),
            mack_core_cache_raw: Default::default(),
            mack_core_cache_context: None,
            mack_cache_by_value: Default::default(),
        }
    }

    fn with_cold_state(raw: &'static [u8], label: &str) -> Result<Self> {
        let mut runtime = Self::new();
        install_musk_cold_state(&mut runtime.context, raw, label)?;
        runtime.cold_state = Some(raw);
        Ok(runtime)
    }

    fn clear_context_dependent_caches(&mut self) {
        self.mack_core_cache_raw.clear();
        self.mack_core_cache_context = None;
    }
}

/// Dense, scope-local identity for a parsed Hoon node.
///
/// IDs are valid only while the outermost `mint`/`play` call that registered
/// the borrowed AST is active. Compiler sidecars use this index instead of
/// independently hashing the same AST address for every property.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
struct HoonId(u32);

#[derive(Default)]
struct HoonArenaEntry {
    source: Option<NonNull<Hoon>>,
    signature: Option<HoonSignature>,
    hot_children: Option<[HoonId; 2]>,
    noun: Option<Noun>,
    // `Some(None)` is a cached negative result: canonical `open` returned the
    // node unchanged. `None` means the node has not been opened yet.
    opened: Option<Option<Arc<Hoon>>>,
}

#[derive(Default)]
struct HoonArena {
    by_ptr: FastHashMap<HoonIdentity, HoonId>,
    entries: Vec<HoonArenaEntry>,
}

struct HoonArenaBuildNode {
    ptr: HoonIdentity,
    signature: HoonSignature,
    hot_children: Option<[HoonIdentity; 2]>,
}

impl HoonArena {
    fn clear(&mut self) {
        self.by_ptr.clear();
        self.entries.clear();
    }

    /// Register nodes, draining the build vector so its capacity returns to
    /// the caller's scratch pool.
    fn register(&mut self, nodes: &mut Vec<HoonArenaBuildNode>) {
        self.clear();
        self.entries.reserve(nodes.len());
        self.by_ptr.reserve(nodes.len());
        for node in nodes.iter() {
            let id = HoonId(
                u32::try_from(self.entries.len())
                    .expect("one Hoon compiler scope cannot contain more than u32::MAX nodes"),
            );
            self.by_ptr.insert(node.ptr, id);
            self.entries.push(HoonArenaEntry {
                source: NonNull::new(node.ptr.0 as *mut Hoon),
                signature: Some(node.signature),
                hot_children: None,
                noun: None,
                opened: None,
            });
        }
        for (index, node) in nodes.drain(..).enumerate() {
            self.entries[index].hot_children = node
                .hot_children
                .map(|[head, tail]| [self.by_ptr[&head], self.by_ptr[&tail]]);
        }
    }

    fn register_unsigned_root(&mut self, ptr: HoonIdentity) {
        self.clear();
        self.by_ptr.insert(ptr, HoonId(0));
        self.entries.push(HoonArenaEntry {
            source: NonNull::new(ptr.0 as *mut Hoon),
            ..HoonArenaEntry::default()
        });
    }

    #[inline]
    fn id_for(&self, hoon: &Hoon) -> Option<HoonId> {
        self.by_ptr.get(&(HoonIdentity::of(hoon))).copied()
    }

    #[inline]
    fn entry(&self, id: HoonId) -> &HoonArenaEntry {
        &self.entries[id.0 as usize]
    }

    #[inline]
    fn entry_mut(&mut self, id: HoonId) -> &mut HoonArenaEntry {
        &mut self.entries[id.0 as usize]
    }

    #[inline]
    fn source_ptr(&self, id: HoonId) -> *const Hoon {
        self.entry(id)
            .source
            .expect("active HoonId must have a source node")
            .as_ptr()
    }

    #[inline]
    fn child(&self, id: HoonId, index: usize) -> HoonId {
        self.entry(id)
            .hot_children
            .expect("arena child lookup is only valid for a hot binary gene")[index]
    }

    #[inline]
    #[cfg(test)]
    fn child_count(&self, id: HoonId) -> usize {
        self.entry(id).hot_children.map_or(0, |_| 2)
    }
}

struct HoonAstScope<'ut, 'slab, 'root> {
    ut: &'ut mut Ut<'slab>,
    pushed: bool,
    root: HoonId,
    // Keeps every raw source pointer in `hoon_arena` bounded by the borrow of
    // the root whose descendants were registered.
    _root: PhantomData<&'root Hoon>,
}

impl<'ut, 'slab, 'root> Deref for HoonAstScope<'ut, 'slab, 'root> {
    type Target = Ut<'slab>;

    fn deref(&self) -> &Self::Target {
        self.ut
    }
}

impl DerefMut for HoonAstScope<'_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ut
    }
}

impl Drop for HoonAstScope<'_, '_, '_> {
    fn drop(&mut self) {
        self.ut.leave_hoon_ast_scope(self.pushed);
    }
}

pub struct Ut<'a> {
    pub slab: &'a mut NounSlab,
    // Canonical Nock formula graph. Formula-producing paths build `FormulaId`s;
    // nouns are materialized only at explicit semantic boundaries and at the
    // public output boundary.
    formula_arena: FormulaArena,
    // Canonical structural identities for complete values and abstract
    // seminoun states used by Musk's native evaluator.
    value_arena: ValueArena,
    semi_arena: SemiArena,
    // Per-compile native-IR state: intern table, decode/encode memos, and
    // boundary caches. It is disjoint from `slab`, so the IR free functions can
    // borrow `&mut self.cx` and `self.slab` together. It lives as long as the `Ut`.
    cx: Context,
    // Compiler context falls into three groups:
    // - semantic context: `vet` plus active `%rest`/`fan` scope
    // - memo context: recursion-sensitive arm epoch and placeholder signature
    // - recursion guards: in-progress arm state and wet `rib`
    pub vet: bool,
    dbug_locations: Vec<CompilerErrorLocation>,
    // Cache keys take semantic and memo state from the context-key helpers
    // (`cache_context_key` etc.) instead of assembling context tuples by hand.
    // The recursion and in-progress guards below are not caches; they limit memo
    // reuse and feed the memo-context helpers where a cache needs them.
    pub arm_in_progress: HashSet<(Arc<str>, TypeId)>,
    pub arm_goal_in_progress: Vec<ArmInProgressEntry>,
    pub arm_placeholder_play_in_progress: HashSet<NounIdentity>,
    pub arm_epoch: ArmEpoch,
    pub lazy_resolver_next_id: LazyResolverId,
    pub lazy_resolvers: HashMap<LazyResolverId, LazyResolverContext>,
    // Canonical lazy-core identity: maps a recursive core's structural key
    // (subject type ID, tome signature, poly) to one resolver ID, so structurally
    // equal lazy cores intern to one type and identity-keyed recursion cuts
    // converge. Lives as long as `lazy_resolvers`, for the whole compile.
    pub lazy_resolver_canonical_ids: HashMap<LazyCoreKey, LazyResolverId>,
    // Exact AST recovery from structurally equal hoon nouns. The honk binary
    // enables it for prelude builds; the normal compile path keeps it off.
    pub exact_hoon_ast_lookup_enabled: bool,
    pub hoon_identity_cache_raw: HashMap<NounIdentity, Noun>,
    pub hoon_identity_cache_order: VecDeque<NounIdentity>,
    pub hoon_cache_raw: HashMap<NounIdentity, Arc<Hoon>>,
    pub hoon_cache_raw_order: VecDeque<NounIdentity>,
    pub hoon_cache_struct: FastHashMap<NounMug, VecDeque<(Noun, Arc<Hoon>)>>,
    pub hoon_cache_struct_order: VecDeque<NounMug>,
    pub hoon_identity_cache_struct: FastHashMap<NounMug, VecDeque<Noun>>,
    pub hoon_identity_cache_struct_order: VecDeque<NounMug>,
    // Bounded decode cache for `%hold` gene nouns that are discovered from type data rather than
    // from parser-owned AST pointers.
    pub decoded_hold_hoon_cache_raw: HashMap<NounIdentity, Arc<Hoon>>,
    pub decoded_hold_hoon_cache_order: VecDeque<NounIdentity>,
    pub decoded_hold_hoon_ptr_cache: HashMap<HoonIdentity, (Option<NounIdentity>, Noun)>,
    // Arena IR for the borrowed top-level Hoon tree. Address lookup happens
    // once at a compiler boundary; signatures, canonical nouns, and opened
    // forms are then dense HoonId-indexed properties. Generated lowering
    // temporaries get a nested arena for the duration of their shorter borrow.
    hoon_ast_scope_depth: usize,
    hoon_arena: HoonArena,
    // Compiler-generated lowerings have shorter borrows than their parsed
    // parent. They receive their own dense arena and suspend the parent here;
    // LIFO scope guards restore the previous graph without pointer probing.
    hoon_arena_stack: Vec<HoonArena>,
    // Retired arenas and signature scratch buffers. Each scope entry reuses
    // their grown capacity; a scope opens for every distinct AST root, including
    // each compiler-generated lowering, so regrowing empty maps would be hot.
    hoon_arena_pool: Vec<HoonArena>,
    sig_scratch_pool: Vec<(
        FastHashMap<HoonIdentity, HoonSignature>,
        Vec<HoonArenaBuildNode>,
    )>,
    pub hoon_ast_ptr_cache: HashMap<HoonIdentity, (Option<NounIdentity>, Noun)>,
    pub hoon_ast_ptr_cache_order: VecDeque<HoonIdentity>,
    pub hold_memo: HoldMemoSet,
    // Canonical `++fire` wet-arm validation tracks `[sut dox gen]` in `rib`
    // to avoid recursive re-entry during `mull` checks. `sut` and `dox` are
    // hash-consed native types, so the rib compares them by identity; `gen` is a
    // canonicalized hoon noun, kept both as the noun and by raw address.
    fire_wet_rib: Vec<(NRc<NTy>, NRc<NTy>, Noun)>,
    fire_wet_rib_raw: FastHashSet<WetRibKey>,
    // Dynamic `%rest` / `%hold` fan scope. This is part of the semantic execution context.
    // Canonical `++rest` tracks active loop legs in `fan`; here each structural `[inner hoon]`
    // pair is interned to a stable `leg_id`, and the active scope is keyed by that set.
    hold_repo_fan_leg_ids: FastHashMap<HoldKey<NounMug>, Vec<HoldRepoFanLegIdEntry>>,
    hold_repo_fan_leg_raw_ids: FastHashMap<HoldKey<NounIdentity>, FanLegId>,
    hold_repo_fan_leg_id_by_hold_raw: FastHashMap<NounIdentity, FanLegId>,
    hold_repo_fan_leg_id_by_hold_raw_order: VecDeque<NounIdentity>,
    hold_repo_fan_leg_id_by_hold_mug: FastHashMap<NounMug, VecDeque<HoldRepoFanHoldIdEntry>>,
    hold_repo_fan_leg_id_by_hold_mug_order: VecDeque<NounMug>,
    hold_repo_fan_active_leg_ids: Vec<FanLegId>,
    hold_repo_fan_signature_sum: u64,
    hold_repo_fan_signature_xor: u64,
    hold_repo_fan_context_by_signature:
        FastHashMap<SetSignature, Vec<(Vec<FanLegId>, FanContextId)>>,
    pub hold_repo_fan_context_id: FanContextId,
    pub hold_repo_fan_context_next_id: FanContextId,
    hold_repo_fan_leg_next_id: FanLegId,
    // Leg ID per `%hold` type ID, so `reachable_legs` finds a hold's leg in O(1)
    // amortized; the noun-path leg intern runs once per distinct hold. Kept for
    // the whole compile, since leg IDs are compile-stable.
    hold_repo_fan_leg_id_by_ptr: FastHashMap<TypeId, FanLegId>,
    // Interned IDs for scoped fan subsets (active legs ∩ reachable legs),
    // bucketed by (sum, xor, len) like `hold_repo_fan_context_by_signature`.
    hold_repo_fan_subset_by_signature:
        FastHashMap<SetSignature, Vec<(Vec<FanLegId>, FanContextId)>>,
    // Noun-keyed caches at Vere-style `+ut` boundaries (`mint`, `nest`, `redo`,
    // `rest`) and related memo tables.
    pub boundary_memo: BoundaryMemoSet,
    pub bran_semi_memo: BucketMemo<BranSemiKey, BranSemiCacheEntry>,
    pub spec_example_cache: HashMap<SpecSignature, VecDeque<(Spec, Arc<Hoon>)>>,
    pub spec_example_cache_order: VecDeque<SpecSignature>,
    pub spec_factory_open_cache: HashMap<SpecSignature, VecDeque<(Spec, Arc<Hoon>)>>,
    pub spec_factory_open_cache_order: VecDeque<SpecSignature>,
    pub burp_type_cache: HashMap<NounIdentity, Noun>,
    musk: MuskRuntime,
    /// Cross-call `miss` memo, enabled only for the prelude mint and retained
    /// for a single semantic epoch: `(vet, active fan, arm epoch, placeholder
    /// context)`. `miss` reaches `repo`/`rest`/`redo`, whose state evolves
    /// during a build, so a verdict memoized under one hold-expansion state can
    /// flip under another (e.g. ++dish's `~|` vase constant kept a `%hint`
    /// fork member that hoonc resolves away). Clearing on any epoch change keeps
    /// within-arm reuse and never reuses a verdict across states.
    miss_memo_persist: Option<(CacheContextKey, FastHashMap<MissKey, bool>)>,
    // `^~` fold outcomes keyed by the canonical (bran, formula) pair. Folding is a pure
    // function of these two values: arm resolution through the persistent
    // lazy resolvers does not change over time, so both successes and failures
    // are safe to reuse for the lifetime of the `Ut`.
    ktsg_fold_cache: FastHashMap<FoldKey, Option<Noun>>,
    semi_root_blocked_set: Option<Noun>,
    semi_full_blocked_interned: Option<Noun>,
    // Cache for `hatch::utils::open()`. Keyed by `&Hoon` pointer, guarded by a structural
    // signature to avoid incorrect hits when the allocator reuses freed AST node addresses.
    //
    // Value is `None` when `open(gen)` returns `gen` unchanged, avoiding storing duplicate clones.
    pub open_cache: HashMap<HoonIdentity, (HoonSignature, Option<Arc<Hoon>>)>,
    pub open_cache_order: VecDeque<HoonIdentity>,
    pub arm_key_term_cache: HashMap<NounIdentity, Arc<str>>,
    pub arm_key_term_cache_order: VecDeque<NounIdentity>,
    #[cfg(test)]
    pub skin_match_static_calls: usize,
    #[cfg(test)]
    pub stack_guard_calls: usize,
}

pub struct Sig64 {
    state: u64,
    include_dbug_spot: bool,
    // A Hoon child contributes its completed structural digest to its parent.
    // Besides avoiding quadratic subtree rescans, this records the digest for
    // every native AST node reached through Spec/Tome/etc. in one traversal.
    hoon_signatures: FastHashMap<HoonIdentity, HoonSignature>,
    // Completed nodes are emitted in post-order. Only the two canonical binary
    // forms that recurse directly by ID carry child edges; every other form
    // enters through the arena boundary without paying generic edge costs.
    hoon_nodes: Vec<HoonArenaBuildNode>,
}

impl Sig64 {
    // Rolling signature used only as a cache guard against pointer reuse. It
    // must be stable for a given value within one process but need not be
    // cryptographic, so it mixes one splitmix64 round per 8-byte word. Sub-word
    // tails also mix their length as a second word, which keeps distinct call
    // sequences distinct without per-byte work.
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

    fn new_with_dbug_spots(include_dbug_spot: bool) -> Self {
        Self {
            state: Self::OFFSET,
            include_dbug_spot,
            hoon_signatures: Default::default(),
            hoon_nodes: Vec::new(),
        }
    }

    fn finish(self) -> u64 {
        self.state
    }

    #[inline]
    fn mix(state: u64, value: u64) -> u64 {
        let mut mixed = state ^ value;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    #[inline]
    fn write_byte(&mut self, byte: u8) {
        self.state = Self::mix(self.state, u64::from(byte));
    }

    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let word = u64::from_le_bytes(chunk.try_into().expect("chunks_exact yields 8 bytes"));
            self.state = Self::mix(self.state, word);
        }
        let tail = chunks.remainder();
        if !tail.is_empty() {
            let mut word = [0u8; 8];
            word[..tail.len()].copy_from_slice(tail);
            self.state = Self::mix(self.state, u64::from_le_bytes(word));
            self.state = Self::mix(self.state, tail.len() as u64);
        }
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.state = Self::mix(self.state, value);
    }

    #[inline]
    fn write_u128(&mut self, value: u128) {
        self.state = Self::mix(self.state, value as u64);
        self.state = Self::mix(self.state, (value >> 64) as u64);
    }

    #[inline]
    fn write_axis(&mut self, axis: &AstAxis) {
        let bytes = axis.as_biguint().to_bytes_le();
        self.write_u64(bytes.len() as u64);
        self.write_bytes(&bytes);
    }

    #[inline]
    fn write_str(&mut self, value: &str) {
        self.write_u64(value.len() as u64);
        self.write_bytes(value.as_bytes());
    }

    fn write_path(&mut self, path: &Path) {
        self.write_u64(path.len() as u64);
        for knot in path {
            self.write_str(knot);
        }
    }

    fn write_pint(&mut self, pint: &Pint) {
        self.write_u64(pint.p.0);
        self.write_u64(pint.p.1);
        self.write_u64(pint.q.0);
        self.write_u64(pint.q.1);
    }

    fn write_spot(&mut self, spot: &Spot) {
        self.write_path(&spot.p);
        self.write_pint(&spot.q);
    }

    fn hoon_signature_spot_sensitive(hoon: &Hoon) -> Option<HoonSignature> {
        let mut sig = Self::new_with_dbug_spots(true);
        sig.write_hoon(hoon)?;
        sig.hoon_signatures.get(&(HoonIdentity::of(hoon))).copied()
    }

    /// Spot-sensitive signatures for `hoon` and every descendant, computed in
    /// scratch storage the caller lends. The tables always come back, filled
    /// with build nodes on success. Scope entry runs once per distinct AST root,
    /// so rebuilding these tables from empty would be measurable.
    fn hoon_signatures_spot_sensitive_pooled(
        hoon: &Hoon,
        mut signatures: FastHashMap<HoonIdentity, HoonSignature>,
        mut nodes: Vec<HoonArenaBuildNode>,
    ) -> (
        Option<HoonSignature>,
        FastHashMap<HoonIdentity, HoonSignature>,
        Vec<HoonArenaBuildNode>,
    ) {
        signatures.clear();
        nodes.clear();
        let mut sig = Self {
            state: Self::OFFSET,
            include_dbug_spot: true,
            hoon_signatures: signatures,
            hoon_nodes: nodes,
        };
        let root = sig
            .write_hoon(hoon)
            .and_then(|_| sig.hoon_signatures.get(&(HoonIdentity::of(hoon))).copied());
        (root, sig.hoon_signatures, sig.hoon_nodes)
    }

    fn spec_signature_spot_sensitive(spec: &Spec) -> Option<SpecSignature> {
        let mut sig = Self::new_with_dbug_spots(true);
        sig.write_spec(spec)?;
        Some(SpecSignature(sig.finish()))
    }

    fn write_parsed_atom(&mut self, atom: &ParsedAtom) {
        match atom {
            ParsedAtom::Small(value) => {
                self.write_byte(0x01);
                self.write_u128(*value);
            }
            ParsedAtom::Big(value) => {
                self.write_byte(0x02);
                let bytes = value.to_bytes_le();
                self.write_u64(bytes.len() as u64);
                self.write_bytes(&bytes);
            }
        }
    }

    fn write_noun_expr(&mut self, expr: &NounExpr) -> Option<()> {
        match expr {
            NounExpr::ParsedAtom(atom) => {
                self.write_byte(0x01);
                self.write_parsed_atom(atom);
            }
            NounExpr::Cell(head, tail) => {
                self.write_byte(0x02);
                self.write_noun_expr(head)?;
                self.write_noun_expr(tail)?;
            }
        }
        Some(())
    }

    fn write_base_type(&mut self, bt: &BaseType) {
        match bt {
            BaseType::NounExpr => self.write_byte(0x01),
            BaseType::Cell => self.write_byte(0x02),
            BaseType::Flag => self.write_byte(0x03),
            BaseType::Null => self.write_byte(0x04),
            BaseType::Void => self.write_byte(0x05),
            BaseType::Atom(aura) => {
                self.write_byte(0x06);
                self.write_str(aura);
            }
        }
    }

    fn write_wing(&mut self, wing: &WingType) {
        // Use the existing compact wing signature to keep hashing cheap.
        self.write_u64(Ut::<'_>::wing_signature(wing));
    }

    fn write_note(&mut self, note: &Note) -> Option<()> {
        match note {
            Note::Help(help) => {
                self.write_byte(0x03);
                self.write_noun_expr(help)?;
            }
            Note::Know(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
            Note::Made(name, wings) => {
                self.write_byte(0x02);
                self.write_str(name);
                match wings {
                    None => self.write_byte(0x00),
                    Some(wings) => {
                        self.write_byte(0x01);
                        self.write_u64(wings.len() as u64);
                        for wing in wings {
                            self.write_wing(wing);
                        }
                    }
                }
            }
        }
        Some(())
    }

    fn write_skin(&mut self, skin: &Skin) -> Option<()> {
        match skin {
            Skin::Term(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
            Skin::Base(bt) => {
                self.write_byte(0x02);
                self.write_base_type(bt);
            }
            Skin::Cell(head, tail) => {
                self.write_byte(0x03);
                self.write_skin(head)?;
                self.write_skin(tail)?;
            }
            Skin::Dbug(spot, inner) => {
                self.write_byte(0x04);
                if self.include_dbug_spot {
                    self.write_spot(spot);
                }
                self.write_skin(inner)?;
            }
            Skin::Help(help, inner) => {
                self.write_byte(0x09);
                self.write_noun_expr(help)?;
                self.write_skin(inner)?;
            }
            Skin::Leaf(tag, atom) => {
                self.write_byte(0x05);
                self.write_str(tag);
                self.write_parsed_atom(atom);
            }
            Skin::Name(name, inner) => {
                self.write_byte(0x06);
                self.write_str(name);
                self.write_skin(inner)?;
            }
            Skin::Over(wing, inner) => {
                self.write_byte(0x07);
                self.write_wing(wing);
                self.write_skin(inner)?;
            }
            Skin::Spec(spec, inner) => {
                self.write_byte(0x08);
                self.write_spec(spec)?;
                self.write_skin(inner)?;
            }
            Skin::Wash(axis) => {
                self.write_byte(0x0a);
                self.write_u64(*axis);
            }
        }
        Some(())
    }

    fn write_spec(&mut self, spec: &Spec) -> Option<()> {
        match spec {
            Spec::Base(bt) => {
                self.write_byte(0x01);
                self.write_base_type(bt);
            }
            Spec::Dbug(spot, inner) => {
                self.write_byte(0x02);
                if self.include_dbug_spot {
                    self.write_spot(spot);
                }
                self.write_spec(inner)?;
            }
            Spec::Gist(help, inner) => {
                self.write_byte(0x1e);
                self.write_noun_expr(help)?;
                self.write_spec(inner)?;
            }
            Spec::Leaf(tag, atom) => {
                self.write_byte(0x03);
                self.write_str(tag);
                self.write_parsed_atom(atom);
            }
            Spec::Like(wing, wings) => {
                self.write_byte(0x04);
                self.write_wing(wing);
                self.write_u64(wings.len() as u64);
                for item in wings {
                    self.write_wing(item);
                }
            }
            Spec::Loop(name) => {
                self.write_byte(0x05);
                self.write_str(name);
            }
            Spec::Made((name, args), inner) => {
                self.write_byte(0x06);
                self.write_str(name);
                self.write_u64(args.len() as u64);
                for arg in args {
                    self.write_str(arg);
                }
                self.write_spec(inner)?;
            }
            Spec::Make(gen, args) => {
                self.write_byte(0x07);
                self.write_hoon(gen)?;
                self.write_u64(args.len() as u64);
                for arg in args {
                    self.write_spec(arg)?;
                }
            }
            Spec::Name(name, inner) => {
                self.write_byte(0x08);
                self.write_str(name);
                self.write_spec(inner)?;
            }
            Spec::Over(wing, inner) => {
                self.write_byte(0x09);
                self.write_wing(wing);
                self.write_spec(inner)?;
            }
            Spec::BucGar(a, b) => {
                self.write_byte(0x0a);
                self.write_spec(a)?;
                self.write_spec(b)?;
            }
            Spec::BucBuc(a, map) => {
                self.write_byte(0x0b);
                self.write_spec(a)?;
                self.write_u64(map.len() as u64);
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (key, value) in entries {
                    self.write_str(key);
                    self.write_spec(value)?;
                }
            }
            Spec::BucBar(a, gen) => {
                self.write_byte(0x0c);
                self.write_spec(a)?;
                self.write_hoon(gen)?;
            }
            Spec::BucCab(gen) => {
                self.write_byte(0x0d);
                self.write_hoon(gen)?;
            }
            Spec::BucCol(head, tail) => {
                self.write_byte(0x0e);
                self.write_spec(head)?;
                self.write_u64(tail.len() as u64);
                for item in tail {
                    self.write_spec(item)?;
                }
            }
            Spec::BucCen(head, tail) => {
                self.write_byte(0x0f);
                self.write_spec(head)?;
                self.write_u64(tail.len() as u64);
                for item in tail {
                    self.write_spec(item)?;
                }
            }
            Spec::BucDot(head, map) => {
                self.write_byte(0x10);
                self.write_spec(head)?;
                self.write_u64(map.len() as u64);
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (key, value) in entries {
                    self.write_str(key);
                    self.write_spec(value)?;
                }
            }
            Spec::BucGal(a, b) => {
                self.write_byte(0x11);
                self.write_spec(a)?;
                self.write_spec(b)?;
            }
            Spec::BucHep(a, b) => {
                self.write_byte(0x12);
                self.write_spec(a)?;
                self.write_spec(b)?;
            }
            Spec::BucKet(a, b) => {
                self.write_byte(0x13);
                self.write_spec(a)?;
                self.write_spec(b)?;
            }
            Spec::BucLus(name, inner) => {
                self.write_byte(0x14);
                self.write_str(name);
                self.write_spec(inner)?;
            }
            Spec::BucFas(head, map) => {
                self.write_byte(0x15);
                self.write_spec(head)?;
                self.write_u64(map.len() as u64);
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (key, value) in entries {
                    self.write_str(key);
                    self.write_spec(value)?;
                }
            }
            Spec::BucMic(gen) => {
                self.write_byte(0x16);
                self.write_hoon(gen)?;
            }
            Spec::BucPam(head, gen) => {
                self.write_byte(0x17);
                self.write_spec(head)?;
                self.write_hoon(gen)?;
            }
            Spec::BucSig(gen, inner) => {
                self.write_byte(0x18);
                self.write_hoon(gen)?;
                self.write_spec(inner)?;
            }
            Spec::BucTic(head, map) => {
                self.write_byte(0x19);
                self.write_spec(head)?;
                self.write_u64(map.len() as u64);
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (key, value) in entries {
                    self.write_str(key);
                    self.write_spec(value)?;
                }
            }
            Spec::BucTis(skin, inner) => {
                self.write_byte(0x1a);
                self.write_skin(skin)?;
                self.write_spec(inner)?;
            }
            Spec::BucPat(a, b) => {
                self.write_byte(0x1b);
                self.write_spec(a)?;
                self.write_spec(b)?;
            }
            Spec::BucWut(head, tail) => {
                self.write_byte(0x1c);
                self.write_spec(head)?;
                self.write_u64(tail.len() as u64);
                for item in tail {
                    self.write_spec(item)?;
                }
            }
            Spec::BucZap(head, map) => {
                self.write_byte(0x1d);
                self.write_spec(head)?;
                self.write_u64(map.len() as u64);
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (key, value) in entries {
                    self.write_str(key);
                    self.write_spec(value)?;
                }
            }
        }
        Some(())
    }

    fn write_chum(&mut self, chum: &Chum) -> Option<()> {
        match chum {
            Chum::Lef(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
            Chum::StdKel(a, b) => {
                self.write_byte(0x02);
                self.write_str(a);
                self.write_parsed_atom(b);
            }
            Chum::VenProKel(a, b, c) => {
                self.write_byte(0x03);
                self.write_str(a);
                self.write_str(b);
                self.write_parsed_atom(c);
            }
            Chum::VenProVerKel(a, b, c, d) => {
                self.write_byte(0x04);
                self.write_str(a);
                self.write_str(b);
                self.write_parsed_atom(c);
                self.write_parsed_atom(d);
            }
        }
        Some(())
    }

    fn write_term_or_pair(&mut self, term_or_pair: &TermOrPair) -> Option<()> {
        match term_or_pair {
            TermOrPair::Term(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
            TermOrPair::Pair(name, hoon) => {
                self.write_byte(0x02);
                self.write_str(name);
                self.write_hoon(hoon)?;
            }
        }
        Some(())
    }

    fn write_tune(&mut self, tune: &Tune) -> Option<()> {
        let (map, list) = tune;
        self.write_u64(map.len() as u64);
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        for (key, value) in entries {
            self.write_str(key);
            match value {
                None => self.write_byte(0x00),
                Some(inner) => {
                    self.write_byte(0x01);
                    self.write_hoon(inner)?;
                }
            }
        }
        self.write_u64(list.len() as u64);
        for item in list {
            self.write_hoon(item)?;
        }
        Some(())
    }

    fn write_term_or_tune(&mut self, term_or_tune: &TermOrTune) -> Option<()> {
        match term_or_tune {
            TermOrTune::Term(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
            TermOrTune::Tune(tune) => {
                self.write_byte(0x02);
                self.write_tune(tune)?;
            }
        }
        Some(())
    }

    fn write_tome(&mut self, tome: &Tome) -> Option<()> {
        let (what, map) = tome;
        match what {
            None => self.write_byte(0x00),
            Some(what) => {
                self.write_byte(0x01);
                self.write_noun_expr(what)?;
            }
        }
        self.write_u64(map.len() as u64);
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        for (key, value) in entries {
            self.write_str(key);
            self.write_hoon(value)?;
        }
        Some(())
    }

    fn write_tyre(&mut self, tyre: &Tyre) -> Option<()> {
        self.write_u64(tyre.len() as u64);
        for (name, hoon) in tyre {
            self.write_str(name);
            self.write_hoon(hoon)?;
        }
        Some(())
    }

    fn write_woof(&mut self, woof: &Woof) -> Option<()> {
        match woof {
            Woof::ParsedAtom(atom) => {
                self.write_byte(0x01);
                self.write_parsed_atom(atom);
            }
            Woof::Hoon(hoon) => {
                self.write_byte(0x02);
                self.write_hoon(hoon)?;
            }
        }
        Some(())
    }

    fn write_beer(&mut self, beer: &Beer) -> Option<()> {
        match beer {
            Beer::Char(cord) => {
                self.write_byte(0x01);
                self.write_str(cord);
            }
            Beer::Hoon(hoon) => {
                self.write_byte(0x02);
                self.write_hoon(hoon)?;
            }
        }
        Some(())
    }

    fn write_mane(&mut self, mane: &Mane) {
        match mane {
            Mane::Tag(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
            Mane::TagSpace(a, b) => {
                self.write_byte(0x02);
                self.write_str(a);
                self.write_str(b);
            }
        }
    }

    fn write_mart(&mut self, mart: &Mart) -> Option<()> {
        self.write_u64(mart.len() as u64);
        for (mane, beers) in mart {
            self.write_mane(mane);
            self.write_u64(beers.len() as u64);
            for beer in beers {
                self.write_beer(beer)?;
            }
        }
        Some(())
    }

    fn write_marx(&mut self, marx: &Marx) -> Option<()> {
        self.write_mane(&marx.n);
        self.write_mart(&marx.a)?;
        Some(())
    }

    fn write_tuna_tail(&mut self, tail: &TunaTail) -> Option<()> {
        match tail {
            TunaTail::Tape(hoon) => {
                self.write_byte(0x01);
                self.write_hoon(hoon)?;
            }
            TunaTail::Manx(hoon) => {
                self.write_byte(0x02);
                self.write_hoon(hoon)?;
            }
            TunaTail::Marl(hoon) => {
                self.write_byte(0x03);
                self.write_hoon(hoon)?;
            }
            TunaTail::Call(hoon) => {
                self.write_byte(0x04);
                self.write_hoon(hoon)?;
            }
        }
        Some(())
    }

    fn write_tuna(&mut self, tuna: &Tuna) -> Option<()> {
        match tuna {
            Tuna::Manx(manx) => {
                self.write_byte(0x01);
                self.write_manx(manx)?;
            }
            Tuna::TunaTail(tail) => {
                self.write_byte(0x02);
                self.write_tuna_tail(tail)?;
            }
        }
        Some(())
    }

    fn write_marl(&mut self, marl: &Marl) -> Option<()> {
        self.write_u64(marl.len() as u64);
        for tuna in marl {
            self.write_tuna(tuna)?;
        }
        Some(())
    }

    fn write_manx(&mut self, manx: &Manx) -> Option<()> {
        self.write_marx(&manx.g)?;
        self.write_marl(&manx.c)?;
        Some(())
    }

    fn write_poly(&mut self, poly: &AstPoly) {
        match poly {
            AstPoly::Wet => self.write_byte(0x01),
            AstPoly::Dry => self.write_byte(0x02),
        }
    }

    fn write_vair(&mut self, vair: &AstVair) {
        match vair {
            AstVair::Gold => self.write_byte(0x01),
            AstVair::Iron => self.write_byte(0x02),
            AstVair::Lead => self.write_byte(0x03),
            AstVair::Zinc => self.write_byte(0x04),
        }
    }

    fn write_garb(&mut self, garb: &Garb) -> Option<()> {
        match &garb.name {
            None => self.write_byte(0x00),
            Some(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
        }
        self.write_poly(&garb.poly);
        self.write_vair(&garb.vair);
        Some(())
    }

    fn write_stencil(&mut self, stencil: &Stencil) -> Option<()> {
        match stencil {
            Stencil::Half { left, rite } => {
                self.write_byte(0x01);
                self.write_stencil(left)?;
                self.write_stencil(rite)?;
            }
            Stencil::Full { blocks } => {
                self.write_byte(0x02);
                self.write_u64(blocks.len() as u64);
                for block in blocks {
                    self.write_u64(block.len() as u64);
                    for path in block {
                        self.write_u64(path.len() as u64);
                        for knot in path {
                            self.write_str(knot);
                        }
                    }
                }
            }
            Stencil::Lazy { fragment, resolve } => {
                self.write_byte(0x03);
                self.write_axis(fragment);
                let (a, b) = resolve;
                self.write_spec(a)?;
                self.write_spec(b)?;
            }
        }
        Some(())
    }

    fn write_semi_noun_expr(&mut self, expr: &SemiNounExpr) -> Option<()> {
        let (stencil, noun) = expr;
        self.write_stencil(stencil)?;
        self.write_noun_expr(noun)?;
        Some(())
    }

    fn write_coil(&mut self, coil: &Coil) -> Option<()> {
        self.write_garb(&coil.p)?;
        self.write_type(&coil.q)?;
        let (semi, map) = &coil.r;
        self.write_semi_noun_expr(semi)?;
        self.write_u64(map.len() as u64);
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        for (key, value) in entries {
            self.write_str(key);
            self.write_tome(value)?;
        }
        Some(())
    }

    fn write_face_type(&mut self, face: &FaceType) -> Option<()> {
        match face {
            FaceType::Term(name) => {
                self.write_byte(0x01);
                self.write_str(name);
            }
            FaceType::Tune(tune) => {
                self.write_byte(0x02);
                self.write_tune(tune)?;
            }
        }
        Some(())
    }

    fn write_type(&mut self, typ: &Type) -> Option<()> {
        match typ {
            Type::NounExpr => self.write_byte(0x01),
            Type::Void => self.write_byte(0x02),
            Type::ParsedAtom(aura, value) => {
                self.write_byte(0x03);
                self.write_str(aura);
                match value {
                    None => self.write_byte(0x00),
                    Some(value) => {
                        self.write_byte(0x01);
                        self.write_u64(*value);
                    }
                }
            }
            Type::Cell(head, tail) => {
                self.write_byte(0x04);
                self.write_type(head)?;
                self.write_type(tail)?;
            }
            Type::Core(payload, coil) => {
                self.write_byte(0x05);
                self.write_type(payload)?;
                self.write_coil(coil)?;
            }
            Type::Face(face, inner) => {
                self.write_byte(0x06);
                self.write_face_type(face)?;
                self.write_type(inner)?;
            }
            Type::Fork(options) => {
                self.write_byte(0x07);
                self.write_u64(options.len() as u64);
                for option in options {
                    self.write_type(option)?;
                }
            }
            Type::Hint((inner, note), tail) => {
                self.write_byte(0x08);
                self.write_type(inner)?;
                self.write_note(note)?;
                self.write_type(tail)?;
            }
            Type::Hold(inner, hoon) => {
                self.write_byte(0x09);
                self.write_type(inner)?;
                self.write_hoon(hoon)?;
            }
        }
        Some(())
    }

    fn write_nock_hint(&mut self, hint: &NockHint) -> Option<()> {
        match hint {
            NockHint::ParsedAtom(atom) => {
                self.write_byte(0x01);
                self.write_u64(*atom);
            }
            NockHint::Pair(axis, nock) => {
                self.write_byte(0x02);
                self.write_u64(*axis);
                self.write_nock(nock)?;
            }
        }
        Some(())
    }

    fn write_nock(&mut self, nock: &Nock) -> Option<()> {
        match nock {
            Nock::Pair(a, b) => {
                self.write_byte(0x01);
                self.write_nock(a)?;
                self.write_nock(b)?;
            }
            Nock::Const(noun) => {
                self.write_byte(0x02);
                self.write_noun_expr(noun)?;
            }
            Nock::Compose(a, b) => {
                self.write_byte(0x03);
                self.write_nock(a)?;
                self.write_nock(b)?;
            }
            Nock::CellTest(a) => {
                self.write_byte(0x04);
                self.write_nock(a)?;
            }
            Nock::Increment(a) => {
                self.write_byte(0x05);
                self.write_nock(a)?;
            }
            Nock::Equality(a, b) => {
                self.write_byte(0x06);
                self.write_nock(a)?;
                self.write_nock(b)?;
            }
            Nock::IfThenElse(a, b, c) => {
                self.write_byte(0x07);
                self.write_nock(a)?;
                self.write_nock(b)?;
                self.write_nock(c)?;
            }
            Nock::SerialCompose(a, b) => {
                self.write_byte(0x08);
                self.write_nock(a)?;
                self.write_nock(b)?;
            }
            Nock::PushSubject(a, b) => {
                self.write_byte(0x09);
                self.write_nock(a)?;
                self.write_nock(b)?;
            }
            Nock::SelectArm(axis, nock) => {
                self.write_byte(0x0a);
                self.write_axis(axis);
                self.write_nock(nock)?;
            }
            Nock::Edit((axis, nock), rest) => {
                self.write_byte(0x0b);
                self.write_axis(axis);
                self.write_nock(nock)?;
                self.write_nock(rest)?;
            }
            Nock::Hint(hint, nock) => {
                self.write_byte(0x0c);
                self.write_nock_hint(hint)?;
                self.write_nock(nock)?;
            }
            Nock::GrabData(a, b) => {
                self.write_byte(0x0d);
                self.write_nock(a)?;
                self.write_nock(b)?;
            }
            Nock::AxisSelect(axis) => {
                self.write_byte(0x0e);
                self.write_axis(axis);
            }
        }
        Some(())
    }

    fn write_zpwt_arg(&mut self, arg: &ZpwtArg) {
        match arg {
            ZpwtArg::ParsedAtom(atom) => {
                self.write_byte(0x01);
                self.write_str(atom);
            }
            ZpwtArg::Pair(a, b) => {
                self.write_byte(0x02);
                self.write_str(a);
                self.write_str(b);
            }
        }
    }

    fn write_hoon(&mut self, hoon: &Hoon) -> Option<()> {
        let ptr = HoonIdentity::of(hoon);
        if let Some(signature) = self.hoon_signatures.get(&ptr).copied() {
            self.write_byte(0xff);
            self.write_u64(signature.0);
            return Some(());
        }

        // Hash this node independently, then feed its digest into the enclosing
        // Hoon/helper node. This makes the signature compositional: registering
        // a root computes each descendant once rather than hashing the
        // same suffix again at every recursive mint/play/mull boundary.
        let parent_state = std::mem::replace(&mut self.state, Self::OFFSET);
        match hoon {
            Hoon::Pair(a, b) => {
                self.write_byte(0x01);
                self.write_hoon(a)?;
                self.write_hoon(b)?;
            }
            Hoon::ZapZap => self.write_byte(0x02),
            Hoon::Axis(axis) => {
                self.write_byte(0x03);
                self.write_axis(axis);
            }
            Hoon::Base(bt) => {
                self.write_byte(0x04);
                self.write_base_type(bt);
            }
            Hoon::Bust(bt) => {
                self.write_byte(0x05);
                self.write_base_type(bt);
            }
            Hoon::Dbug(spot, inner) => {
                self.write_byte(0x06);
                if self.include_dbug_spot {
                    self.write_spot(spot);
                }
                self.write_hoon(inner)?;
            }
            Hoon::Eror(msg) => {
                self.write_byte(0x07);
                self.write_str(msg);
            }
            Hoon::Hand(typ, nock) => {
                self.write_byte(0x08);
                self.write_type(typ)?;
                self.write_nock(nock)?;
            }
            Hoon::Note(note, inner) => {
                self.write_byte(0x09);
                self.write_note(note)?;
                self.write_hoon(inner)?;
            }
            Hoon::Fits(p, wing) => {
                self.write_byte(0x0a);
                self.write_hoon(p)?;
                self.write_wing(wing);
            }
            Hoon::Knit(woofs) => {
                self.write_byte(0x0b);
                self.write_u64(woofs.len() as u64);
                for woof in woofs {
                    self.write_woof(woof)?;
                }
            }
            Hoon::Leaf(tag, atom) => {
                self.write_byte(0x0c);
                self.write_str(tag);
                self.write_parsed_atom(atom);
            }
            Hoon::Limb(name) => {
                self.write_byte(0x0d);
                self.write_str(name);
            }
            Hoon::Lost(inner) => {
                self.write_byte(0x0e);
                self.write_hoon(inner)?;
            }
            Hoon::Rock(tag, expr) => {
                self.write_byte(0x0f);
                self.write_str(tag);
                self.write_noun_expr(expr)?;
            }
            Hoon::Sand(tag, expr) => {
                self.write_byte(0x10);
                self.write_str(tag);
                self.write_noun_expr(expr)?;
            }
            Hoon::Tell(list) => {
                self.write_byte(0x11);
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::Tune(term_or_tune) => {
                self.write_byte(0x12);
                self.write_term_or_tune(term_or_tune)?;
            }
            Hoon::Wing(wing) => {
                self.write_byte(0x13);
                self.write_wing(wing);
            }
            Hoon::Yell(list) => {
                self.write_byte(0x14);
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::Xray(manx) => {
                self.write_byte(0x15);
                self.write_manx(manx)?;
            }
            Hoon::BarBuc(sample, body) => {
                self.write_byte(0x16);
                self.write_u64(sample.len() as u64);
                for term in sample {
                    self.write_str(term);
                }
                self.write_spec(body)?;
            }
            Hoon::BarCab(spec, alas, arms) => {
                self.write_byte(0x17);
                self.write_spec(spec)?;
                self.write_u64(alas.len() as u64);
                for (name, hoon) in alas {
                    self.write_str(name);
                    self.write_hoon(hoon)?;
                }
                self.write_u64(arms.len() as u64);
                let mut entries: Vec<_> = arms.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (name, tome) in entries {
                    self.write_str(name);
                    self.write_tome(tome)?;
                }
            }
            Hoon::BarCol(p, q) => {
                self.write_byte(0x18);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::BarCen(prefix, arms) => {
                self.write_byte(0x19);
                match prefix {
                    None => self.write_byte(0x00),
                    Some(name) => {
                        self.write_byte(0x01);
                        self.write_str(name);
                    }
                }
                self.write_u64(arms.len() as u64);
                let mut entries: Vec<_> = arms.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (name, tome) in entries {
                    self.write_str(name);
                    self.write_tome(tome)?;
                }
            }
            Hoon::BarDot(p) => {
                self.write_byte(0x1a);
                self.write_hoon(p)?;
            }
            Hoon::BarKet(p, arms) => {
                self.write_byte(0x1b);
                self.write_hoon(p)?;
                self.write_u64(arms.len() as u64);
                let mut entries: Vec<_> = arms.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (name, tome) in entries {
                    self.write_str(name);
                    self.write_tome(tome)?;
                }
            }
            Hoon::BarHep(p) => {
                self.write_byte(0x1c);
                self.write_hoon(p)?;
            }
            Hoon::BarSig(spec, q) => {
                self.write_byte(0x1d);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::BarTar(spec, q) => {
                self.write_byte(0x1e);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::BarTis(spec, q) => {
                self.write_byte(0x1f);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::BarPat(prefix, arms) => {
                self.write_byte(0x20);
                match prefix {
                    None => self.write_byte(0x00),
                    Some(name) => {
                        self.write_byte(0x01);
                        self.write_str(name);
                    }
                }
                self.write_u64(arms.len() as u64);
                let mut entries: Vec<_> = arms.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                for (name, tome) in entries {
                    self.write_str(name);
                    self.write_tome(tome)?;
                }
            }
            Hoon::BarWut(p) => {
                self.write_byte(0x21);
                self.write_hoon(p)?;
            }
            Hoon::ColCab(p, q) => {
                self.write_byte(0x22);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::ColKet(p, q, r, s) => {
                self.write_byte(0x23);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
                self.write_hoon(s)?;
            }
            Hoon::ColHep(p, q) => {
                self.write_byte(0x24);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::ColLus(p, q, r) => {
                self.write_byte(0x25);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::ColSig(items) => {
                self.write_byte(0x26);
                self.write_u64(items.len() as u64);
                for item in items {
                    self.write_hoon(item)?;
                }
            }
            Hoon::ColTar(items) => {
                self.write_byte(0x27);
                self.write_u64(items.len() as u64);
                for item in items {
                    self.write_hoon(item)?;
                }
            }
            Hoon::CenCab(wing, pairs) => {
                self.write_byte(0x28);
                self.write_wing(wing);
                self.write_u64(pairs.len() as u64);
                for (w, h) in pairs {
                    self.write_wing(w);
                    self.write_hoon(h)?;
                }
            }
            Hoon::CenDot(p, q) => {
                self.write_byte(0x29);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::CenHep(p, q) => {
                self.write_byte(0x2a);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::CenCol(p, list) => {
                self.write_byte(0x2b);
                self.write_hoon(p)?;
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::CenTar(wing, p, pairs) => {
                self.write_byte(0x2c);
                self.write_wing(wing);
                self.write_hoon(p)?;
                self.write_u64(pairs.len() as u64);
                for (w, h) in pairs {
                    self.write_wing(w);
                    self.write_hoon(h)?;
                }
            }
            Hoon::CenKet(p, q, r, s) => {
                self.write_byte(0x2d);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
                self.write_hoon(s)?;
            }
            Hoon::CenLus(p, q, r) => {
                self.write_byte(0x2e);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::CenSig(wing, p, list) => {
                self.write_byte(0x2f);
                self.write_wing(wing);
                self.write_hoon(p)?;
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::CenTis(wing, pairs) => {
                self.write_byte(0x30);
                self.write_wing(wing);
                self.write_u64(pairs.len() as u64);
                for (w, h) in pairs {
                    self.write_wing(w);
                    self.write_hoon(h)?;
                }
            }
            Hoon::DotKet(spec, q) => {
                self.write_byte(0x31);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::DotLus(p) => {
                self.write_byte(0x32);
                self.write_hoon(p)?;
            }
            Hoon::DotTar(p, q) => {
                self.write_byte(0x33);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::DotTis(p, q) => {
                self.write_byte(0x34);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::DotWut(p) => {
                self.write_byte(0x35);
                self.write_hoon(p)?;
            }
            Hoon::KetBar(p) => {
                self.write_byte(0x36);
                self.write_hoon(p)?;
            }
            Hoon::KetDot(p, q) => {
                self.write_byte(0x37);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::KetLus(p, q) => {
                self.write_byte(0x38);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::KetHep(spec, q) => {
                self.write_byte(0x39);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::KetPam(p) => {
                self.write_byte(0x3a);
                self.write_hoon(p)?;
            }
            Hoon::KetSig(p) => {
                self.write_byte(0x3b);
                self.write_hoon(p)?;
            }
            Hoon::KetTis(skin, p) => {
                self.write_byte(0x3c);
                self.write_skin(skin)?;
                self.write_hoon(p)?;
            }
            Hoon::KetWut(p) => {
                self.write_byte(0x3d);
                self.write_hoon(p)?;
            }
            Hoon::KetTar(spec) => {
                self.write_byte(0x3e);
                self.write_spec(spec)?;
            }
            Hoon::KetCol(spec) => {
                self.write_byte(0x3f);
                self.write_spec(spec)?;
            }
            Hoon::SigBar(p, q) => {
                self.write_byte(0x40);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::SigCab(p, q) => {
                self.write_byte(0x41);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::SigCen(chum, p, tyre, q) => {
                self.write_byte(0x42);
                self.write_chum(chum)?;
                self.write_hoon(p)?;
                self.write_tyre(tyre)?;
                self.write_hoon(q)?;
            }
            Hoon::SigFas(chum, q) => {
                self.write_byte(0x43);
                self.write_chum(chum)?;
                self.write_hoon(q)?;
            }
            Hoon::SigGal(term_or_pair, q) => {
                self.write_byte(0x44);
                self.write_term_or_pair(term_or_pair)?;
                self.write_hoon(q)?;
            }
            Hoon::SigGar(term_or_pair, q) => {
                self.write_byte(0x45);
                self.write_term_or_pair(term_or_pair)?;
                self.write_hoon(q)?;
            }
            Hoon::SigBuc(term, q) => {
                self.write_byte(0x46);
                self.write_str(term);
                self.write_hoon(q)?;
            }
            Hoon::SigLus(a, q) => {
                self.write_byte(0x47);
                self.write_u64(*a);
                self.write_hoon(q)?;
            }
            Hoon::SigPam(a, p, q) => {
                self.write_byte(0x48);
                self.write_u64(*a);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::SigTis(p, q) => {
                self.write_byte(0x49);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::SigWut(a, p, q, r) => {
                self.write_byte(0x4a);
                self.write_u64(*a);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::SigZap(p, q) => {
                self.write_byte(0x4b);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::MicTis(marl) => {
                self.write_byte(0x4c);
                self.write_marl(marl)?;
            }
            Hoon::MicCol(p, list) => {
                self.write_byte(0x4d);
                self.write_hoon(p)?;
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::MicFas(p) => {
                self.write_byte(0x4e);
                self.write_hoon(p)?;
            }
            Hoon::MicGal(spec, q, r, s) => {
                self.write_byte(0x4f);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
                self.write_hoon(s)?;
            }
            Hoon::MicSig(p, list) => {
                self.write_byte(0x50);
                self.write_hoon(p)?;
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::MicMic(spec, q) => {
                self.write_byte(0x51);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::TisBar(spec, q) => {
                self.write_byte(0x52);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::TisCol(pairs, q) => {
                self.write_byte(0x53);
                self.write_u64(pairs.len() as u64);
                for (wing, hoon) in pairs {
                    self.write_wing(wing);
                    self.write_hoon(hoon)?;
                }
                self.write_hoon(q)?;
            }
            Hoon::TisFas(skin, p, q) => {
                self.write_byte(0x54);
                self.write_skin(skin)?;
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisMic(skin, p, q) => {
                self.write_byte(0x55);
                self.write_skin(skin)?;
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisDot(wing, p, q) => {
                self.write_byte(0x56);
                self.write_wing(wing);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisWut(wing, p, q, r) => {
                self.write_byte(0x57);
                self.write_wing(wing);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::TisGal(p, q) => {
                self.write_byte(0x58);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisHep(p, q) => {
                self.write_byte(0x59);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisGar(p, q) => {
                self.write_byte(0x5a);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisKet(skin, wing, p, q) => {
                self.write_byte(0x5b);
                self.write_skin(skin)?;
                self.write_wing(wing);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisLus(p, q) => {
                self.write_byte(0x5c);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisSig(list) => {
                self.write_byte(0x5d);
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::TisTar((term, spec_opt), p, q) => {
                self.write_byte(0x5e);
                self.write_str(term);
                match spec_opt {
                    None => self.write_byte(0x00),
                    Some(spec) => {
                        self.write_byte(0x01);
                        self.write_spec(spec)?;
                    }
                }
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::TisCom(p, q) => {
                self.write_byte(0x5f);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::WutBar(list) => {
                self.write_byte(0x60);
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::WutHep(wing, pairs) => {
                self.write_byte(0x61);
                self.write_wing(wing);
                self.write_u64(pairs.len() as u64);
                for (spec, hoon) in pairs {
                    self.write_spec(spec)?;
                    self.write_hoon(hoon)?;
                }
            }
            Hoon::WutCol(p, q, r) => {
                self.write_byte(0x62);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::WutDot(p, q, r) => {
                self.write_byte(0x63);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::WutKet(wing, p, q) => {
                self.write_byte(0x64);
                self.write_wing(wing);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::WutGal(p, q) => {
                self.write_byte(0x65);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::WutGar(p, q) => {
                self.write_byte(0x66);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::WutLus(wing, p, pairs) => {
                self.write_byte(0x67);
                self.write_wing(wing);
                self.write_hoon(p)?;
                self.write_u64(pairs.len() as u64);
                for (spec, hoon) in pairs {
                    self.write_spec(spec)?;
                    self.write_hoon(hoon)?;
                }
            }
            Hoon::WutPam(list) => {
                self.write_byte(0x68);
                self.write_u64(list.len() as u64);
                for item in list {
                    self.write_hoon(item)?;
                }
            }
            Hoon::WutPat(wing, q, r) => {
                self.write_byte(0x69);
                self.write_wing(wing);
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::WutSig(wing, q, r) => {
                self.write_byte(0x6a);
                self.write_wing(wing);
                self.write_hoon(q)?;
                self.write_hoon(r)?;
            }
            Hoon::WutHax(skin, wing) => {
                self.write_byte(0x6b);
                self.write_skin(skin)?;
                self.write_wing(wing);
            }
            Hoon::WutTis(spec, wing) => {
                self.write_byte(0x6c);
                self.write_spec(spec)?;
                self.write_wing(wing);
            }
            Hoon::WutZap(p) => {
                self.write_byte(0x6d);
                self.write_hoon(p)?;
            }
            Hoon::ZapCom(p, q) => {
                self.write_byte(0x6e);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::ZapGar(p) => {
                self.write_byte(0x6f);
                self.write_hoon(p)?;
            }
            Hoon::ZapGal(spec, q) => {
                self.write_byte(0x70);
                self.write_spec(spec)?;
                self.write_hoon(q)?;
            }
            Hoon::ZapMic(p, q) => {
                self.write_byte(0x71);
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::ZapTis(p) => {
                self.write_byte(0x72);
                self.write_hoon(p)?;
            }
            Hoon::ZapPat(wings, p, q) => {
                self.write_byte(0x73);
                self.write_u64(wings.len() as u64);
                for wing in wings {
                    self.write_wing(wing);
                }
                self.write_hoon(p)?;
                self.write_hoon(q)?;
            }
            Hoon::ZapWut(arg, q) => {
                self.write_byte(0x74);
                self.write_zpwt_arg(arg);
                self.write_hoon(q)?;
            }
        }
        let signature = HoonSignature(self.state);
        let hot_children = match hoon {
            Hoon::Pair(head, tail) | Hoon::TisGar(head, tail) => {
                Some([HoonIdentity::of(head.as_ref()), HoonIdentity::of(tail.as_ref())])
            }
            _ => None,
        };
        self.hoon_signatures.insert(ptr, signature);
        self.hoon_nodes.push(HoonArenaBuildNode {
            ptr,
            signature,
            hot_children,
        });
        self.state = parent_state;
        self.write_byte(0xff);
        self.write_u64(signature.0);
        Some(())
    }
}

impl<'a> Ut<'a> {
    pub fn new(slab: &'a mut NounSlab) -> Self {
        // Every `Ut` gets a fresh `Context`. Its intern table and
        // `live_to_noun`/`live_leaf_to_noun` memos hold nouns bound to this
        // compile's slab, and the next compile can reuse a freed slab's address,
        // so a shared context would return stale types or dangling nouns.
        Self {
            slab,
            formula_arena: FormulaArena::new(),
            value_arena: ValueArena::new(),
            semi_arena: SemiArena::new(),
            cx: Context::new(),
            vet: true,
            dbug_locations: Vec::new(),
            arm_in_progress: HashSet::new(),
            arm_goal_in_progress: Vec::new(),
            arm_placeholder_play_in_progress: HashSet::new(),
            arm_epoch: ArmEpoch(0),
            lazy_resolver_next_id: LazyResolverId(1),
            lazy_resolvers: HashMap::new(),
            lazy_resolver_canonical_ids: HashMap::new(),
            exact_hoon_ast_lookup_enabled: false,
            hoon_identity_cache_raw: HashMap::new(),
            hoon_identity_cache_order: VecDeque::new(),
            hoon_cache_raw: HashMap::new(),
            hoon_cache_raw_order: VecDeque::new(),
            hoon_cache_struct: Default::default(),
            hoon_cache_struct_order: VecDeque::new(),
            hoon_identity_cache_struct: Default::default(),
            hoon_identity_cache_struct_order: VecDeque::new(),
            decoded_hold_hoon_cache_raw: HashMap::new(),
            decoded_hold_hoon_cache_order: VecDeque::new(),
            decoded_hold_hoon_ptr_cache: HashMap::new(),
            hoon_ast_scope_depth: 0,
            hoon_arena: Default::default(),
            hoon_arena_stack: Vec::new(),
            hoon_arena_pool: Vec::new(),
            sig_scratch_pool: Vec::new(),
            hoon_ast_ptr_cache: HashMap::new(),
            hoon_ast_ptr_cache_order: VecDeque::new(),
            hold_memo: Default::default(),
            fire_wet_rib: Vec::new(),
            fire_wet_rib_raw: Default::default(),
            hold_repo_fan_leg_ids: Default::default(),
            hold_repo_fan_leg_raw_ids: Default::default(),
            hold_repo_fan_leg_id_by_hold_raw: Default::default(),
            hold_repo_fan_leg_id_by_hold_raw_order: VecDeque::new(),
            hold_repo_fan_leg_id_by_hold_mug: Default::default(),
            hold_repo_fan_leg_id_by_hold_mug_order: VecDeque::new(),
            hold_repo_fan_active_leg_ids: Vec::new(),
            hold_repo_fan_signature_sum: 0,
            hold_repo_fan_signature_xor: 0,
            hold_repo_fan_context_by_signature: Default::default(),
            hold_repo_fan_context_id: FanContextId(0),
            hold_repo_fan_context_next_id: FanContextId(1),
            hold_repo_fan_leg_next_id: FanLegId(1),
            hold_repo_fan_leg_id_by_ptr: Default::default(),
            hold_repo_fan_subset_by_signature: Default::default(),
            boundary_memo: Default::default(),
            bran_semi_memo: Default::default(),
            spec_example_cache: HashMap::new(),
            spec_example_cache_order: VecDeque::new(),
            spec_factory_open_cache: HashMap::new(),
            spec_factory_open_cache_order: VecDeque::new(),
            burp_type_cache: HashMap::new(),
            musk: MuskRuntime::new(),
            miss_memo_persist: None,
            ktsg_fold_cache: Default::default(),
            semi_root_blocked_set: None,
            semi_full_blocked_interned: None,
            open_cache: HashMap::new(),
            open_cache_order: VecDeque::new(),
            arm_key_term_cache: HashMap::new(),
            arm_key_term_cache_order: VecDeque::new(),
            #[cfg(test)]
            skin_match_static_calls: 0,
            #[cfg(test)]
            stack_guard_calls: 0,
        }
    }

    #[inline]
    fn formula_import(&mut self, noun: Noun) -> Result<FormulaId> {
        let space = self.slab.noun_space();
        self.formula_arena.import(noun, &space)
    }

    #[inline]
    fn formula_materialize(&mut self, formula: FormulaId) -> Noun {
        self.formula_arena.materialize(formula, self.slab)
    }

    #[inline]
    fn formula_slot_u64(&mut self, axis: u64) -> FormulaId {
        self.formula_arena.slot_u64(axis)
    }

    #[inline]
    fn formula_slot(&mut self, axis: BigUint) -> FormulaId {
        self.formula_arena.slot(axis)
    }

    #[inline]
    fn formula_quote(&mut self, noun: Noun) -> FormulaId {
        let space = self.slab.noun_space();
        self.formula_arena.quote(noun, &space)
    }

    #[inline]
    fn formula_op(&mut self, code: NockOpcode, args: &[FormulaId]) -> FormulaId {
        self.formula_arena.op(code, args)
    }

    #[inline]
    fn formula_cons(&mut self, head: FormulaId, tail: FormulaId) -> FormulaId {
        self.formula_arena.cons(self.slab, head, tail)
    }

    #[inline]
    fn formula_comb(&mut self, left: FormulaId, right: FormulaId) -> FormulaId {
        self.formula_arena.comb(left, right)
    }

    #[inline]
    fn formula_cond(&mut self, test: FormulaId, yes: FormulaId, no: FormulaId) -> FormulaId {
        self.formula_arena.cond(test, yes, no)
    }

    #[inline]
    fn formula_flip(&mut self, formula: FormulaId) -> FormulaId {
        self.formula_arena.flip(formula)
    }

    #[inline]
    fn formula_flan(&mut self, left: FormulaId, right: FormulaId) -> FormulaId {
        self.formula_arena.flan(left, right)
    }

    #[inline]
    fn formula_flor(&mut self, left: FormulaId, right: FormulaId) -> FormulaId {
        self.formula_arena.flor(left, right)
    }

    fn clear_musk_context_dependent_caches(&mut self) {
        self.musk.clear_context_dependent_caches();
    }

    /// Upper bound on the cell-level eval-stack copy cache. The cache maps each
    /// cell copied onto the eval stack by its *source* slab address, so shared
    /// subtrees within and across `^~` folds are copied once. Uncapped, a
    /// hoon-138 self-mint grows it to about 59M entries (about 1.6 GB): each new
    /// core copies about 150K cells at fresh addresses that are rarely queried
    /// again. Clearing it does not change fold results: the copies stay live on
    /// the long-lived eval stack, and a later copy of the same structure is
    /// re-made rather than shared.
    const MUSK_CORE_CACHE_CAP: usize = 4_000_000;

    fn ensure_musk_mack_core_cache_context(&mut self, context: &NockContext) {
        let context_id = EvalFrameId(context.stack.frame_identity());
        if self.musk.mack_core_cache_context != Some(context_id) {
            self.musk.mack_core_cache_raw.clear();
            self.musk.mack_core_cache_context = Some(context_id);
        } else if self.musk.mack_core_cache_raw.len() > Self::MUSK_CORE_CACHE_CAP {
            // Bound RSS by dropping entries from earlier folds; the current fold
            // re-warms what it needs. On the hoon-138 self-mint, the 4M cap cuts
            // peak RSS from about 13.0 GB to 9.0 GB and also lowers CPU time
            // (45.9 s vs 47.6 s): entries past the cap are mostly stale, and the
            // smaller map has better cache locality.
            self.musk.mack_core_cache_raw.clear();
        }
    }

    unsafe fn musk_mack_cached_core_in_context(
        &mut self,
        context: &mut NockContext,
        core: Noun,
        core_space: &NounSpace,
    ) -> Option<Noun> {
        self.ensure_musk_mack_core_cache_context(context);
        let raw = NounIdentity::of(core);
        if let Some(cached) = self.musk.mack_core_cache_raw.get(&raw).copied() {
            return Some(cached);
        }
        let stack_checkpoint = context.stack.checkpoint();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Fill the slab-side mug cache for the whole core before copying:
            // the copy below propagates cached mugs onto the eval-stack nouns,
            // which lets cold-state registration inside `interpret` short-circuit
            // its unifying-equality checks on mug mismatch instead of walking
            // whole battery structures per call.
            self.noun_mug_cached(core);
            self.copy_into_eval_stack_shared(context, core, core_space)
        }));
        match outcome {
            Ok(cached) => {
                self.musk.mack_core_cache_raw.insert(raw, cached);
                Some(cached)
            }
            Err(payload) => {
                // Recursive copying populates the cache as it goes. Roll back
                // its allocations and discard every pointer that may now refer
                // into the released stack region before declining this fold.
                unsafe { context.stack.restore_checkpoint(&stack_checkpoint) };
                self.musk.clear_context_dependent_caches();
                if payload.is::<AllocationError>() {
                    None
                } else {
                    std::panic::resume_unwind(payload)
                }
            }
        }
    }

    /// Copy a slab noun onto the musk eval stack with structural sharing:
    /// every cell (and indirect atom) copied is remembered in
    /// `musk.mack_core_cache_raw`, so the large shared parts of mack call
    /// cores (batteries, the context chain) are copied once per eval context.
    /// Later calls of the same gate copy only the fresh spine and sample. The
    /// copies are made outside the per-call interpreter snapshot, so they stay
    /// live on the eval stack for the lifetime of the context.
    unsafe fn copy_into_eval_stack_shared(
        &mut self,
        context: &mut NockContext,
        noun: Noun,
        space: &NounSpace,
    ) -> Noun {
        let Ok(cell) = noun.in_space(space).as_cell() else {
            if noun.is_direct() {
                return noun;
            }
            let raw = NounIdentity::of(noun);
            if let Some(cached) = self.musk.mack_core_cache_raw.get(&raw).copied() {
                return cached;
            }
            let copied = context.stack.copy_into(noun, space);
            if let Some(mug) = get_mug(noun, space) {
                if let Ok(mut allocated) = copied.as_allocated() {
                    context.stack.with_fast_noun_space(|eval_space| {
                        set_mug(&mut allocated, mug, eval_space)
                    });
                }
            }
            self.musk.mack_core_cache_raw.insert(raw, copied);
            return copied;
        };
        let raw = NounIdentity::of(noun);
        if let Some(cached) = self.musk.mack_core_cache_raw.get(&raw).copied() {
            return cached;
        }
        let head = self.copy_into_eval_stack_shared(context, cell.head().noun(), space);
        let tail = self.copy_into_eval_stack_shared(context, cell.tail().noun(), space);
        let copied = T(&mut context.stack, &[head, tail]);
        if let Some(mug) = get_mug(noun, space) {
            if let Ok(mut allocated) = copied.as_allocated() {
                context
                    .stack
                    .with_fast_noun_space(|eval_space| set_mug(&mut allocated, mug, eval_space));
            }
        }
        self.musk.mack_core_cache_raw.insert(raw, copied);
        copied
    }

    pub fn load_musk_cold_state(&mut self, raw: &'static [u8], label: &str) -> Result<()> {
        self.clear_musk_context_dependent_caches();
        self.musk = MuskRuntime::with_cold_state(raw, label)?;
        Ok(())
    }

    pub fn set_vet(&mut self, vet: bool) {
        self.vet = vet;
    }

    /// Run `f` with `vet` forced off, restoring the previous value afterward
    /// on both `Ok` and `Err`. A panic unwinding through here leaves `vet` off;
    /// honk discards the `Ut` after any caught compile panic.
    fn with_vet_off<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let prev = std::mem::replace(&mut self.vet, false);
        let result = f(self);
        self.vet = prev;
        result
    }

    pub fn clear_build_memos(&mut self) {
        self.clear_build_transients();
        self.arm_epoch = ArmEpoch(0);
        self.hold_memo = Default::default();
        self.boundary_memo = Default::default();
        self.spec_example_cache.clear();
        self.spec_example_cache_order.clear();
        self.spec_factory_open_cache.clear();
        self.spec_factory_open_cache_order.clear();
        self.burp_type_cache.clear();
        self.open_cache.clear();
        self.open_cache_order.clear();
        self.arm_key_term_cache.clear();
        self.arm_key_term_cache_order.clear();
    }

    pub fn clear_build_transients(&mut self) {
        self.arm_in_progress.clear();
        self.arm_goal_in_progress.clear();
        self.arm_placeholder_play_in_progress.clear();
        // Lazy resolver contexts are not transient: types cached across
        // build entries (mint/rest/redo caches) embed `[%lazy 1 id]` battery
        // semis, and hoon-138's equivalent resolver is a pure gate inside
        // the seminoun that remains callable forever. Clearing the registry
        // or reusing ids would dangle or cross-wire those references.
        self.fire_wet_rib.clear();
        self.fire_wet_rib_raw.clear();
        self.hold_repo_fan_active_leg_ids.clear();
        self.hold_repo_fan_signature_sum = 0;
        self.hold_repo_fan_signature_xor = 0;
        self.hold_repo_fan_context_id = FanContextId(0);
        self.hold_repo_fan_subset_by_signature.clear();
        self.bran_semi_memo = Default::default();
    }

    #[cfg(test)]
    const NEST_MUG_BUCKET_LIMIT: usize = 32;
    #[cfg(test)]
    const NEST_MUG_KEY_LIMIT: usize = 65_536;
    const HOON_CACHE_RAW_KEY_LIMIT: usize = 16_384;
    const BURP_TYPE_CACHE_LIMIT: usize = 65_536;
    const HOON_CACHE_STRUCT_KEY_LIMIT: usize = 16_384;
    const HOON_CACHE_STRUCT_BUCKET_LIMIT: usize = 8;
    const SPEC_CACHE_KEY_LIMIT: usize = 16_384;
    const SPEC_CACHE_BUCKET_LIMIT: usize = 8;
    #[cfg(test)]
    const MINT_CACHE_BUCKET_LIMIT: usize = 4;
    #[cfg(test)]
    const MINT_CACHE_KEY_LIMIT: usize = 16_384;
    const REDO_CACHE_BUCKET_LIMIT: usize = 8;
    const REDO_CACHE_KEY_LIMIT: usize = 32_768;
    const REST_CACHE_BUCKET_LIMIT: usize = 8;
    const REST_CACHE_KEY_LIMIT: usize = 32_768;
    const BRAN_SEMI_CACHE_KEY_LIMIT: usize = 65_536;
    const BRAN_SEMI_CACHE_BUCKET_LIMIT: usize = 8;
    const HOLD_TYPE_CACHE_BUCKET_LIMIT: usize = 8;
    const HOLD_TYPE_CACHE_KEY_LIMIT: usize = 65_536;
    const HOLD_TYPE_CACHE_RAW_KEY_LIMIT: usize = 65_536;
    const HOLD_REPO_FAN_LEG_HOLD_RAW_KEY_LIMIT: usize = 65_536;
    const HOLD_REPO_FAN_LEG_HOLD_MUG_KEY_LIMIT: usize = 65_536;
    const HOLD_REPO_FAN_LEG_HOLD_MUG_BUCKET_LIMIT: usize = 8;

    fn hold_repo_fan_context_key(&self) -> FanContextId {
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            FanContextId(0)
        } else {
            self.hold_repo_fan_context_id
        }
    }

    fn hold_repo_fan_leg_signature_component(leg_id: FanLegId) -> u64 {
        let mut hasher = FastHasher::default();
        hasher.mix_u64(leg_id.0);
        hasher.finish()
    }

    fn semantic_context_key(&self) -> SemanticContextKey {
        SemanticContextKey {
            vet_key: VetMode(self.vet),
            fan_context_key: self.hold_repo_fan_context_key(),
        }
    }

    fn hold_repo_fan_leg_lookup_id(&mut self, inner: Noun, hoon: Noun) -> Result<Option<FanLegId>> {
        let raw_key = HoldKey {
            subject: NounIdentity::of(inner),
            gene: NounIdentity::of(hoon),
        };
        if let Some(id) = self.hold_repo_fan_leg_raw_ids.get(&raw_key).copied() {
            return Ok(Some(id));
        }
        let key = HoldKey {
            subject: self.noun_mug_cached(inner),
            gene: self.noun_mug_cached(hoon),
        };
        let Some(entries) = self.hold_repo_fan_leg_ids.get(&key) else {
            return Ok(None);
        };
        let inner_raw = NounIdentity::of(inner);
        let hoon_raw = NounIdentity::of(hoon);
        for entry in entries.iter().rev() {
            let inner_match = unsafe { entry.inner.raw_equals(&inner) }
                || NounIdentity::of(entry.inner) == inner_raw
                || noun_eq(entry.inner, inner, &self.slab.noun_space())?;
            if !inner_match {
                continue;
            }
            let hoon_match = unsafe { entry.hoon.raw_equals(&hoon) }
                || NounIdentity::of(entry.hoon) == hoon_raw
                || noun_eq(entry.hoon, hoon, &self.slab.noun_space())?;
            if hoon_match {
                self.hold_repo_fan_leg_raw_ids.insert(raw_key, entry.id);
                return Ok(Some(entry.id));
            }
        }
        Ok(None)
    }

    fn hold_repo_fan_leg_intern_id(&mut self, inner: Noun, hoon: Noun) -> Result<FanLegId> {
        if let Some(id) = self.hold_repo_fan_leg_lookup_id(inner, hoon)? {
            return Ok(id);
        }

        let key = HoldKey {
            subject: self.noun_mug_cached(inner),
            gene: self.noun_mug_cached(hoon),
        };
        let id = self.hold_repo_fan_leg_next_id.max(FanLegId(1));
        self.hold_repo_fan_leg_next_id = FanLegId(self.hold_repo_fan_leg_next_id.0.wrapping_add(1));
        if self.hold_repo_fan_leg_next_id == FanLegId(0) {
            self.hold_repo_fan_leg_next_id = FanLegId(1);
        }
        self.hold_repo_fan_leg_raw_ids.insert(
            HoldKey {
                subject: NounIdentity::of(inner),
                gene: NounIdentity::of(hoon),
            },
            id,
        );
        // The mug store is authoritative for the whole compile. `inner` and
        // `hoon` live in the compile slab, so later `noun_eq` lookups stay valid
        // without relocation.
        self.hold_repo_fan_leg_ids
            .entry(key)
            .or_default()
            .push(HoldRepoFanLegIdEntry { id, inner, hoon });
        Ok(id)
    }

    fn hold_repo_fan_leg_id_by_hold_raw_store(&mut self, hold_raw: NounIdentity, leg_id: FanLegId) {
        if !self
            .hold_repo_fan_leg_id_by_hold_raw
            .contains_key(&hold_raw)
        {
            self.hold_repo_fan_leg_id_by_hold_raw_order
                .push_back(hold_raw);
            if self.hold_repo_fan_leg_id_by_hold_raw_order.len()
                > Self::HOLD_REPO_FAN_LEG_HOLD_RAW_KEY_LIMIT
            {
                if let Some(evict) = self.hold_repo_fan_leg_id_by_hold_raw_order.pop_front() {
                    self.hold_repo_fan_leg_id_by_hold_raw.remove(&evict);
                }
            }
        }
        self.hold_repo_fan_leg_id_by_hold_raw
            .insert(hold_raw, leg_id);
    }

    fn hold_repo_fan_leg_id_by_hold_mug_lookup(&mut self, hold: Noun) -> Result<Option<FanLegId>> {
        let hold_mug = self.noun_mug_cached(hold);
        let Some(entries) = self.hold_repo_fan_leg_id_by_hold_mug.get(&hold_mug) else {
            return Ok(None);
        };
        let hold_raw = NounIdentity::of(hold);
        let mut matched = None;
        for entry in entries.iter().rev() {
            let hold_match = unsafe { entry.hold.raw_equals(&hold) }
                || NounIdentity::of(entry.hold) == hold_raw
                || noun_eq(entry.hold, hold, &self.slab.noun_space())?;
            if hold_match {
                matched = Some(entry.id);
                break;
            }
        }
        if let Some(id) = matched {
            self.hold_repo_fan_leg_id_by_hold_raw_store(hold_raw, id);
            return Ok(Some(id));
        }
        Ok(None)
    }

    fn hold_repo_fan_leg_id_by_hold_mug_store(
        &mut self,
        hold: Noun,
        leg_id: FanLegId,
    ) -> Result<()> {
        let hold_mug = self.noun_mug_cached(hold);
        if !self
            .hold_repo_fan_leg_id_by_hold_mug
            .contains_key(&hold_mug)
        {
            self.hold_repo_fan_leg_id_by_hold_mug_order
                .push_back(hold_mug);
            if self.hold_repo_fan_leg_id_by_hold_mug_order.len()
                > Self::HOLD_REPO_FAN_LEG_HOLD_MUG_KEY_LIMIT
            {
                if let Some(evict) = self.hold_repo_fan_leg_id_by_hold_mug_order.pop_front() {
                    self.hold_repo_fan_leg_id_by_hold_mug.remove(&evict);
                }
            }
        }
        let bucket = self
            .hold_repo_fan_leg_id_by_hold_mug
            .entry(hold_mug)
            .or_default();
        let hold_raw = NounIdentity::of(hold);
        for entry in bucket.iter() {
            let hold_match = unsafe { entry.hold.raw_equals(&hold) }
                || NounIdentity::of(entry.hold) == hold_raw
                || noun_eq(entry.hold, hold, &self.slab.noun_space())?;
            if hold_match {
                return Ok(());
            }
        }
        if bucket.len() >= Self::HOLD_REPO_FAN_LEG_HOLD_MUG_BUCKET_LIMIT {
            bucket.pop_front();
        }
        // `hold` lives in the compile slab, so later cross-arm `noun_eq`
        // lookups stay valid without relocation.
        bucket.push_back(HoldRepoFanHoldIdEntry { hold, id: leg_id });
        Ok(())
    }

    fn hold_repo_fan_leg_id_for_hold_type(
        &mut self,
        hold: Noun,
        inner: Noun,
        hoon: Noun,
    ) -> Result<FanLegId> {
        let hold_raw = NounIdentity::of(hold);
        if let Some(id) = self
            .hold_repo_fan_leg_id_by_hold_raw
            .get(&hold_raw)
            .copied()
        {
            return Ok(id);
        }
        if let Some(id) = self.hold_repo_fan_leg_id_by_hold_mug_lookup(hold)? {
            return Ok(id);
        }
        let leg_id = self.hold_repo_fan_leg_intern_id(inner, hoon)?;
        self.hold_repo_fan_leg_id_by_hold_raw_store(hold_raw, leg_id);
        self.hold_repo_fan_leg_id_by_hold_mug_store(hold, leg_id)?;
        Ok(leg_id)
    }

    /// Leg ID for a native `%hold` type, memoized by type ID. The first lookup
    /// per hold goes through the noun-path intern
    /// (`hold_repo_fan_leg_id_for_hold_type`), so it returns the same ID
    /// `redo_subject_hold_in_fan` would; later lookups are O(1).
    fn hold_repo_fan_leg_id_for_hold_native(&mut self, hold: &NRc<NTy>) -> Result<FanLegId> {
        let ptr = native_type_id(hold);
        if let Some(id) = self.hold_repo_fan_leg_id_by_ptr.get(&ptr).copied() {
            return Ok(id);
        }
        let NTy::Hold { subject, gene } = &**hold else {
            return Err(CompilerError::Noun(
                "scoped fan: leg-id of non-hold".to_string(),
            ));
        };
        let subject = subject.clone();
        let gene = gene.clone();
        let inner = live_to_noun(&mut self.cx, &subject, self.slab);
        let hoon = live_leaf_to_noun(&mut self.cx, &gene, self.slab);
        let hold_noun = live_to_noun(&mut self.cx, hold, self.slab);
        let leg_id = self.hold_repo_fan_leg_id_for_hold_type(hold_noun, inner, hoon)?;
        self.hold_repo_fan_leg_id_by_ptr.insert(ptr, leg_id);
        Ok(leg_id)
    }

    /// The sorted, deduplicated `%hold` leg IDs reachable from `t`, memoized by
    /// canonical type ID. Types are hash-consed, so the ID is structural
    /// identity. The type DAG is acyclic (a node's hash depends on its interned
    /// children), so each distinct node is visited at most once. A `Fork` unions
    /// its options' legsets.
    fn reachable_legs(&mut self, t: &NRc<NTy>) -> Result<SharedRc<[FanLegId]>> {
        let id = t.arena_id();
        if let Some(legs) = legset_memo_lookup(&self.cx, id) {
            return Ok(legs);
        }
        let legs = self.with_stack_guard(|ut| ut.reachable_legs_node(t))?;
        legset_memo_store(&mut self.cx, id, legs.clone());
        Ok(legs)
    }

    /// One node of `reachable_legs`' memoized recursion; `reachable_legs` wraps
    /// each level in `with_stack_guard` for deep DAGs.
    fn reachable_legs_node(&mut self, t: &NRc<NTy>) -> Result<SharedRc<[FanLegId]>> {
        let legs: Vec<FanLegId> = match &**t {
            NTy::Void | NTy::Noun | NTy::Atom { .. } => Vec::new(),
            NTy::Cell(h, tl) => {
                let h = h.clone();
                let tl = tl.clone();
                let lh = self.reachable_legs(&h)?;
                let lt = self.reachable_legs(&tl)?;
                Self::merge_sorted_legs(&lh, &lt)
            }
            NTy::Core {
                payload, context, ..
            } => {
                let payload = payload.clone();
                let context = context.clone();
                let lp = self.reachable_legs(&payload)?;
                let lc = self.reachable_legs(&context)?;
                Self::merge_sorted_legs(&lp, &lc)
            }
            NTy::Face { inner, .. } => {
                let inner = inner.clone();
                return self.reachable_legs(&inner);
            }
            NTy::Hint { payload, .. } => {
                let payload = payload.clone();
                return self.reachable_legs(&payload);
            }
            NTy::Fork { .. } => {
                // Fork options are native DAG children; union their legsets.
                let options = self.fork_options_native(t)?;
                let mut acc: Vec<FanLegId> = Vec::new();
                for opt in options {
                    let lo = self.reachable_legs(&opt)?;
                    acc = Self::merge_sorted_legs(&acc, &lo);
                }
                acc
            }
            NTy::Hold { subject, .. } => {
                // legset(Hold) = {leg_id(self)} ∪ legset(subject), without the
                // repo expansion: the expansion's holds are reached at the next
                // descent level, which keys on its own scope. This matches
                // `redo_subject_hold_in_fan`, which tests only leg_id(self).
                let subject = subject.clone();
                let self_leg = self.hold_repo_fan_leg_id_for_hold_native(t)?;
                let ls = self.reachable_legs(&subject)?;
                Self::merge_sorted_legs(&ls, std::slice::from_ref(&self_leg))
            }
        };
        Ok(SharedRc::from(legs.into_boxed_slice()))
    }

    /// Union of two sorted, deduplicated leg-ID slices, by linear merge.
    fn merge_sorted_legs(a: &[FanLegId], b: &[FanLegId]) -> Vec<FanLegId> {
        if a.is_empty() {
            return b.to_vec();
        }
        if b.is_empty() {
            return a.to_vec();
        }
        let mut out = Vec::with_capacity(a.len() + b.len());
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => {
                    out.push(a[i]);
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    out.push(b[j]);
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    out.push(a[i]);
                    i += 1;
                    j += 1;
                }
            }
        }
        out.extend_from_slice(&a[i..]);
        out.extend_from_slice(&b[j..]);
        out
    }

    /// Intersection of two sorted, deduplicated leg-ID slices.
    fn intersect_sorted_legs(a: &[FanLegId], b: &[FanLegId]) -> Vec<FanLegId> {
        if a.is_empty() || b.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    out.push(a[i]);
                    i += 1;
                    j += 1;
                }
            }
        }
        out
    }

    /// Map a sorted, deduplicated leg-ID subset to a stable ID. Buckets by
    /// (sum, xor, len) like `refresh_hold_repo_fan_context_id`, so equal subsets
    /// get one ID. IDs come from `hold_repo_fan_context_next_id`, the same space
    /// as whole-fan context IDs. The empty subset maps to 0, the empty-fan key.
    fn intern_fan_subset_id(&mut self, subset: &[FanLegId]) -> FanContextId {
        if subset.is_empty() {
            return FanContextId(0);
        }
        let mut sum: u64 = 0;
        let mut xor: u64 = 0;
        for leg_id in subset {
            let component = Self::hold_repo_fan_leg_signature_component(*leg_id);
            sum = sum.wrapping_add(component);
            xor ^= component;
        }
        let key = SetSignature {
            sum,
            xor,
            len: subset.len(),
        };
        if let Some(entries) = self.hold_repo_fan_subset_by_signature.get(&key) {
            for (legs, id) in entries.iter().rev() {
                if legs.as_slice() == subset {
                    return *id;
                }
            }
        }
        let id = self.hold_repo_fan_context_next_id.max(FanContextId(1));
        self.hold_repo_fan_context_next_id =
            FanContextId(self.hold_repo_fan_context_next_id.0.wrapping_add(1));
        if self.hold_repo_fan_context_next_id == FanContextId(0) {
            self.hold_repo_fan_context_next_id = FanContextId(1);
        }
        self.hold_repo_fan_subset_by_signature
            .entry(key)
            .or_default()
            .push((subset.to_vec(), id));
        id
    }

    /// Scoped fan keys are always on. They leave kernel output unchanged and
    /// keep the hoon-138 self-mint bounded (about 13 GB and 46 s; whole-fan keys
    /// grew to 38-46 GB without finishing). The cache helpers and the partition
    /// tests share this one switch.
    fn scoped_fan_enabled() -> bool {
        true
    }

    /// Fan key for a descent on `scope` (the deepening subject). Intersects the
    /// active leg set with `reachable_legs(scope)`: a leg not reachable from
    /// `scope` is never tested while resolving it, so it cannot change the
    /// result. Returns 0, the empty-fan key, when the intersection is empty
    /// (about 97% of resolutions). With scoping off, returns the whole-fan key.
    fn fan_context_key_scoped(&mut self, scope: &NRc<NTy>) -> Result<FanContextId> {
        if !Self::scoped_fan_enabled() {
            return Ok(self.hold_repo_fan_context_key());
        }
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            return Ok(FanContextId(0));
        }
        let legs = self.reachable_legs(scope)?;
        let inter = Self::intersect_sorted_legs(&self.hold_repo_fan_active_leg_ids, &legs);
        if inter.is_empty() {
            return Ok(FanContextId(0));
        }
        Ok(self.intern_fan_subset_id(&inter))
    }

    /// Fan key over the union of two scopes' reachable legs, for ops like `mull`
    /// that can consult the fan from either the `sut` or the `dox` descent. Any
    /// leg that could change the result is reachable from one of the two scopes.
    fn fan_context_key_scoped_pair(&mut self, a: &NRc<NTy>, b: &NRc<NTy>) -> Result<FanContextId> {
        if !Self::scoped_fan_enabled() {
            return Ok(self.hold_repo_fan_context_key());
        }
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            return Ok(FanContextId(0));
        }
        let la = self.reachable_legs(a)?;
        let lb = self.reachable_legs(b)?;
        let legs = Self::merge_sorted_legs(&la, &lb);
        let inter = Self::intersect_sorted_legs(&self.hold_repo_fan_active_leg_ids, &legs);
        if inter.is_empty() {
            return Ok(FanContextId(0));
        }
        Ok(self.intern_fan_subset_id(&inter))
    }

    /// `fan_context_key_scoped` for noun-keyed caches (`redo`, `rest`). Decodes
    /// the subject with `native_of_cached` only when scoping is on and the
    /// active set is non-empty.
    fn fan_context_key_scoped_noun(&mut self, sut: Noun) -> Result<FanContextId> {
        if !Self::scoped_fan_enabled() {
            return Ok(self.hold_repo_fan_context_key());
        }
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            return Ok(FanContextId(0));
        }
        let sut_native = self.native_of_cached(sut)?;
        self.fan_context_key_scoped(&sut_native)
    }

    fn refresh_hold_repo_fan_context_id(&mut self) {
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            self.hold_repo_fan_context_id = FanContextId(0);
            return;
        }

        let key = SetSignature {
            sum: self.hold_repo_fan_signature_sum,
            xor: self.hold_repo_fan_signature_xor,
            len: self.hold_repo_fan_active_leg_ids.len(),
        };
        if let Some(entries) = self.hold_repo_fan_context_by_signature.get(&key) {
            for (legs, id) in entries.iter().rev() {
                if *legs == self.hold_repo_fan_active_leg_ids {
                    self.hold_repo_fan_context_id = *id;
                    return;
                }
            }
        }

        let id = self.hold_repo_fan_context_next_id.max(FanContextId(1));
        self.hold_repo_fan_context_next_id =
            FanContextId(self.hold_repo_fan_context_next_id.0.wrapping_add(1));
        if self.hold_repo_fan_context_next_id == FanContextId(0) {
            self.hold_repo_fan_context_next_id = FanContextId(1);
        }
        self.hold_repo_fan_context_by_signature
            .entry(key)
            .or_default()
            .push((self.hold_repo_fan_active_leg_ids.clone(), id));
        self.hold_repo_fan_context_id = id;
    }

    fn hold_repo_fan_activate_leg_id(&mut self, leg_id: FanLegId) -> bool {
        match self.hold_repo_fan_active_leg_ids.binary_search(&leg_id) {
            Ok(_) => false,
            Err(idx) => {
                self.hold_repo_fan_active_leg_ids.insert(idx, leg_id);
                let component = Self::hold_repo_fan_leg_signature_component(leg_id);
                self.hold_repo_fan_signature_sum =
                    self.hold_repo_fan_signature_sum.wrapping_add(component);
                self.hold_repo_fan_signature_xor ^= component;
                self.refresh_hold_repo_fan_context_id();
                true
            }
        }
    }

    fn hold_repo_fan_deactivate_leg_id(&mut self, leg_id: FanLegId) {
        if let Ok(idx) = self.hold_repo_fan_active_leg_ids.binary_search(&leg_id) {
            self.hold_repo_fan_active_leg_ids.remove(idx);
            let component = Self::hold_repo_fan_leg_signature_component(leg_id);
            self.hold_repo_fan_signature_sum =
                self.hold_repo_fan_signature_sum.wrapping_sub(component);
            self.hold_repo_fan_signature_xor ^= component;
        } else {
            debug_assert!(false, "active rest leg id should exist on scoped exit");
        }
        self.refresh_hold_repo_fan_context_id();
    }

    fn wing_signature(wing: &[Limb]) -> u64 {
        // FNV-1a-like rolling signature; exact wing equality is verified on lookup.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for limb in wing {
            match limb {
                Limb::Term(name) => {
                    hash ^= 0x01;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                    for byte in name.as_bytes() {
                        hash ^= u64::from(*byte);
                        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                    }
                }
                Limb::Axis(axis) => {
                    hash ^= 0x02;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                    for byte in axis.as_biguint().to_bytes_le() {
                        hash ^= u64::from(byte);
                        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                    }
                }
                Limb::Parent(axis, name) => {
                    hash ^= 0x03;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                    hash ^= *axis;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                    match name {
                        Some(name) => {
                            hash ^= 0x11;
                            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                            for byte in name.as_bytes() {
                                hash ^= u64::from(*byte);
                                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                            }
                        }
                        None => {
                            hash ^= 0x10;
                            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                        }
                    }
                }
            }
        }
        hash
    }

    fn enter_hoon_ast_scope(&mut self, gen: &Hoon) -> (bool, HoonId) {
        let (pushed, root) = match self.hoon_arena.id_for(gen) {
            Some(root) => (false, root),
            None => {
                let mut next = self.hoon_arena_pool.pop().unwrap_or_default();
                let (signatures, nodes) = self.sig_scratch_pool.pop().unwrap_or_default();
                let (root_signature, signatures, mut nodes) =
                    Sig64::hoon_signatures_spot_sensitive_pooled(gen, signatures, nodes);
                if root_signature.is_some() {
                    next.register(&mut nodes);
                } else {
                    next.register_unsigned_root(Self::hoon_ast_ptr_key(gen));
                }
                self.sig_scratch_pool.push((signatures, nodes));
                let root = next
                    .id_for(gen)
                    .expect("new Hoon arena must contain its root");
                let previous = std::mem::replace(&mut self.hoon_arena, next);
                self.hoon_arena_stack.push(previous);
                (true, root)
            }
        };
        self.hoon_ast_scope_depth += 1;
        (pushed, root)
    }

    fn hoon_ast_scope<'ut, 'root>(&'ut mut self, gen: &'root Hoon) -> HoonAstScope<'ut, 'a, 'root> {
        let (pushed, root) = self.enter_hoon_ast_scope(gen);
        HoonAstScope {
            ut: self,
            pushed,
            root,
            _root: PhantomData,
        }
    }

    fn leave_hoon_ast_scope(&mut self, pushed: bool) {
        self.hoon_ast_scope_depth -= 1;
        if pushed {
            // The borrowed root may die or its address may be reused after this
            // call, so drop its graph before restoring the longer-lived parent.
            // `clear()` releases every entry; the pool keeps the capacity.
            let parent = self
                .hoon_arena_stack
                .pop()
                .expect("every pushed Hoon arena must have a suspended parent");
            let mut finished = std::mem::replace(&mut self.hoon_arena, parent);
            finished.clear();
            self.hoon_arena_pool.push(finished);
        }
    }

    fn mint_cache_signature_for(
        &mut self,
        gen: &Hoon,
        id: Option<HoonId>,
    ) -> Option<HoonSignature> {
        // `mint` returns formulas with `%spot` hints, so cache keys must include debug spots.
        if let Some(id) = id.or_else(|| self.hoon_arena.id_for(gen)) {
            return self.hoon_arena.entry(id).signature;
        }
        Sig64::hoon_signature_spot_sensitive(gen)
    }

    #[inline]
    fn mint_cache_signature_id(&self, id: HoonId) -> Option<HoonSignature> {
        self.hoon_arena.entry(id).signature
    }

    fn mint_cache_signature(&mut self, gen: &Hoon) -> Option<HoonSignature> {
        self.mint_cache_signature_for(gen, None)
    }

    fn strip_dbug_wrapper_noun(mut hoon_noun: Noun, space: &NounSpace) -> Noun {
        for _ in 0..64 {
            let Ok(cell) = hoon_noun.in_space(space).as_cell() else {
                break;
            };
            let Ok(tag_atom) = cell.head().as_atom() else {
                break;
            };
            let Ok(tag) = atom_to_string(tag_atom) else {
                break;
            };
            if tag != "dbug" {
                break;
            }
            let Ok(tail) = cell.tail().as_cell() else {
                break;
            };
            hoon_noun = tail.tail().noun();
        }
        hoon_noun
    }

    fn strip_spec_gist_wrapper_noun(mut spec_noun: Noun, space: &NounSpace) -> Noun {
        for _ in 0..64 {
            let Ok(cell) = spec_noun.in_space(space).as_cell() else {
                break;
            };
            let Ok(tag_atom) = cell.head().as_atom() else {
                break;
            };
            let Ok(tag) = atom_to_string(tag_atom) else {
                break;
            };
            if tag != "gist" {
                break;
            }
            let Ok(tail) = cell.tail().as_cell() else {
                break;
            };
            spec_noun = tail.tail().noun();
        }
        spec_noun
    }

    fn canonicalize_nonsemantic_hoon_noun(&mut self, hoon_noun: Noun) -> Noun {
        let space = self.slab.noun_space();
        let stripped = Self::strip_dbug_wrapper_noun(hoon_noun, &space);
        if let Ok(cell) = stripped.in_space(&space).as_cell() {
            if let Ok(tag_atom) = cell.head().as_atom() {
                if let Ok(tag) = atom_to_string(tag_atom) {
                    if tag == "ktcl" {
                        let head = cell.head().noun();
                        let spec = cell.tail().noun();
                        let stripped_spec = Self::strip_spec_gist_wrapper_noun(spec, &space);
                        if unsafe { stripped_spec.raw_equals(&spec) } {
                            stripped
                        } else {
                            T(self.slab, &[head, stripped_spec])
                        }
                    } else {
                        stripped
                    }
                } else {
                    stripped
                }
            } else {
                stripped
            }
        } else {
            stripped
        }
    }

    fn hoon_noun_tag(noun: Noun, space: &NounSpace) -> Option<String> {
        let cell = noun.in_space(space).as_cell().ok()?;
        let atom = cell.head().as_atom().ok()?;
        atom_to_string(atom).ok()
    }

    fn lower_coltar(items: &[Hoon]) -> Hoon {
        match items.split_first() {
            None => Hoon::ZapZap,
            Some((head, [])) => head.clone(),
            Some(_) => {
                let mut rev_items = items.iter().rev();
                let Some(last) = rev_items.next() else {
                    return Hoon::ZapZap;
                };
                let mut acc = last.clone();
                for item in rev_items {
                    acc = Hoon::Pair(Box::new(item.clone()), Box::new(acc));
                }
                acc
            }
        }
    }

    fn lower_bardot(p: &Hoon) -> Hoon {
        let mut inner_arms = HashMap::new();
        inner_arms.insert("$".to_string(), p.clone());
        let mut tomes = HashMap::new();
        tomes.insert("$".to_string(), (None, inner_arms));
        Hoon::BarCen(None, tomes)
    }

    fn lower_barcol(p: &Hoon, q: &Hoon) -> Hoon {
        Hoon::TisLus(
            Box::new(p.clone()),
            Box::new(Hoon::BarDot(Box::new(q.clone()))),
        )
    }

    fn lower_bartar(spec: &Spec, q: &Hoon) -> Hoon {
        let mut arms = HashMap::new();
        arms.insert("$".to_string(), q.clone());
        let mut tomes = HashMap::new();
        tomes.insert("$".to_string(), (None, arms));
        Hoon::TisLus(
            Box::new(Hoon::KetTar(Box::new(spec.clone()))),
            Box::new(Hoon::BarPat(None, tomes)),
        )
    }

    fn lower_barhep(p: &Hoon) -> Hoon {
        Hoon::TisGal(
            Box::new(Hoon::Limb("$".to_string())),
            Box::new(Hoon::BarDot(Box::new(p.clone()))),
        )
    }

    fn lower_brtis(spec: &Spec, q: &Hoon) -> Hoon {
        // hoon-138 `++open` %brts (hoon-138.hoon:8425): `|=` lowers to `|_`
        // with the spec (including any %gist annotations) preserved in place.
        let mut arms = HashMap::new();
        arms.insert("$".to_string(), q.clone());
        let mut tomes = HashMap::new();
        tomes.insert("$".to_string(), (None, arms));
        Hoon::BarCab(Box::new(spec.clone()), vec![], tomes)
    }

    fn lower_colhep(p: &Hoon, q: &Hoon) -> Hoon {
        Hoon::Pair(Box::new(p.clone()), Box::new(q.clone()))
    }

    fn lower_tisdot(wing: &WingType, p: &Hoon, q: &Hoon) -> Hoon {
        Hoon::TisGar(
            Box::new(Hoon::CenCab(
                vec![Limb::Axis((1u64).into())],
                vec![(wing.clone(), p.clone())],
            )),
            Box::new(q.clone()),
        )
    }

    fn lower_censig(wing: &WingType, p: &Hoon, hoons: &[Hoon]) -> Result<Hoon> {
        // Canonical hoon-138 open() lowering:
        //   [%cnsg p q r] => [%cntr p q (compiled r with axe walk starting at 6)]
        // where each item gets wing [[%| 0 ~] [%& axe] ~], with recursive axe progression.
        let mut compiled = Vec::with_capacity(hoons.len());
        let mut axe = BigUint::from(6u32);
        for (idx, hoon) in hoons.iter().enumerate() {
            let is_last = idx + 1 == hoons.len();
            let wing_axe = if is_last {
                axe.clone()
            } else {
                peg_axis_big(axe.clone(), 2)?
            };
            let target_wing = vec![Limb::Parent(0, None), Limb::Axis(wing_axe.into())];
            compiled.push((target_wing, hoon.clone()));
            if !is_last {
                axe = peg_axis_big(axe, 3)?;
            }
        }
        Ok(Hoon::CenTar(wing.clone(), Box::new(p.clone()), compiled))
    }

    fn lower_centar(wing: &WingType, p: &Hoon, pairs: &[(WingType, Hoon)]) -> Hoon {
        // Canonical parser/open lowering:
        //   [%cntr p q r]
        //   ?~(r [%tsgr q [%wing p]]
        //      [%tsls q [%cnts [weld(p ~[%& 2]) (turn r |=([a=wing b=hoon] [a [%tsgr [%$ 3] b]]))]]])
        if pairs.is_empty() {
            return Hoon::TisGar(Box::new(p.clone()), Box::new(Hoon::Wing(wing.clone())));
        }

        let mut extended_wing = wing.clone();
        extended_wing.push(Limb::Axis((2u64).into()));
        let wrapped_pairs = pairs
            .iter()
            .map(|(pair_wing, pair_hoon)| {
                (
                    pair_wing.clone(),
                    Hoon::TisGar(
                        Box::new(Hoon::Axis((3u64).into())),
                        Box::new(pair_hoon.clone()),
                    ),
                )
            })
            .collect::<Vec<_>>();

        Hoon::TisLus(
            Box::new(p.clone()),
            Box::new(Hoon::CenTis(extended_wing, wrapped_pairs)),
        )
    }

    fn lower_kettis(skin: &Skin, p: &Hoon) -> Hoon {
        // Canonical parser/open lowering:
        //   [%ktts *] => grip(p.gen, q.gen, ~)
        grip(skin.clone(), p.clone(), Vec::new())
    }

    fn lower_cencol(p: &Hoon, hoons: &[Hoon]) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%cncl *] => [%cnsg [%$ ~] p.gen q.gen]
        Hoon::CenSig(
            vec![Limb::Term("$".to_string())],
            Box::new(p.clone()),
            hoons.to_vec(),
        )
    }

    fn lower_cenhep(p: &Hoon, q: &Hoon) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%cnhp *] => [%cncl p.gen q.gen ~]
        Hoon::CenCol(Box::new(p.clone()), vec![q.clone()])
    }

    fn lower_ktdt(p: &Hoon, q: &Hoon) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%ktdt p q] => [%ktls [%cncl p q ~] q]
        Hoon::KetLus(
            Box::new(Hoon::CenCol(Box::new(p.clone()), vec![q.clone()])),
            Box::new(q.clone()),
        )
    }

    fn lower_cenlus(p: &Hoon, q: &Hoon, r: &Hoon) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%cnls *] => [%cncl p.gen q.gen r.gen ~]
        // and `%cncl` lowers to `%cnsg [%$ ~] p.gen q.gen`.
        // In parser open() this is represented as `CenCol(p, [q, r])`.
        Hoon::CenCol(Box::new(p.clone()), vec![q.clone(), r.clone()])
    }

    fn lower_wtpm(list: &[Hoon]) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%wtpm *]
        // |-
        // ?~(p.gen [%rock %f 0] [%wtcl i.p.gen $(p.gen t.p.gen) [%rock %f 1]])
        match list {
            [] => Hoon::Rock("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
            [head, tail @ ..] => Hoon::WutCol(
                Box::new(head.clone()),
                Box::new(Self::lower_wtpm(tail)),
                Box::new(Hoon::Rock(
                    "f".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                )),
            ),
        }
    }

    fn lower_wtbr(list: &[Hoon]) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%wtbr *]
        // |-
        // ?~(p.gen [%rock %f 1] [%wtcl i.p.gen [%rock %f 0] $(p.gen t.p.gen)])
        match list {
            [] => Hoon::Rock("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1))),
            [head, tail @ ..] => Hoon::WutCol(
                Box::new(head.clone()),
                Box::new(Hoon::Rock(
                    "f".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                )),
                Box::new(Self::lower_wtbr(tail)),
            ),
        }
    }

    fn lower_sigbar(p: &Hoon, q: &Hoon) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%sgbr p=hoon q=hoon]
        // => [%sggr [%mean ?^((feck p) [%rock %tas u.fek]
        //                 [%brdt [%cncl [%limb %cain] [%zpgr [%tsgr [%$ 3] p]] ~]])] q]
        fn feck_tas(gen: &Hoon) -> Option<ParsedAtom> {
            match gen {
                Hoon::Sand(term, noun) if term == "tas" => match noun {
                    NounExpr::ParsedAtom(atom) => Some(atom.clone()),
                    NounExpr::Cell(_, _) => None,
                },
                Hoon::Dbug(_spot, expr) => feck_tas(expr),
                _ => None,
            }
        }

        let fek = match feck_tas(p) {
            Some(atom) => Hoon::Rock("tas".to_string(), NounExpr::ParsedAtom(atom)),
            None => Hoon::BarDot(Box::new(Hoon::CenCol(
                Box::new(Hoon::Limb("cain".to_string())),
                vec![Hoon::ZapGar(Box::new(Hoon::TisGar(
                    Box::new(Hoon::Axis((3u64).into())),
                    Box::new(p.clone()),
                )))],
            ))),
        };

        Hoon::SigGar(
            TermOrPair::Pair("mean".to_string(), Box::new(fek)),
            Box::new(q.clone()),
        )
    }

    fn lower_sigcab(p: &Hoon, q: &Hoon) -> Hoon {
        // Canonical hoon-138 open() lowering:
        //   [%sgcb *] => [%sggr [%mean [%brdt p.gen]] q.gen]
        Hoon::SigGar(
            TermOrPair::Pair(
                "mean".to_string(),
                Box::new(Hoon::BarDot(Box::new(p.clone()))),
            ),
            Box::new(q.clone()),
        )
    }

    fn lower_siglus(a: u64, q: &Hoon) -> Hoon {
        Hoon::SigGar(
            TermOrPair::Pair(
                "memo".to_string(),
                Box::new(Hoon::Rock(
                    "$".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(a.into())),
                )),
            ),
            Box::new(q.clone()),
        )
    }

    fn lower_miccol(p: &Hoon, hoons: &[Hoon]) -> Hoon {
        fn loop_yex(yex: &[Hoon]) -> Hoon {
            match yex {
                [] => Hoon::Eror("open-mccl-empty".to_string()),
                // hoon-138 `%mccl` open lowering terminal case:
                //   [* ~]  [%tsgr [%$ 3] i.yex]
                [h] => Hoon::TisGar(Box::new(Hoon::Axis((3u64).into())), Box::new(h.clone())),
                [h, t @ ..] => Hoon::CenCol(
                    Box::new(Hoon::Axis((2u64).into())),
                    vec![
                        Hoon::TisGar(Box::new(Hoon::Axis((3u64).into())), Box::new(h.clone())),
                        loop_yex(t),
                    ],
                ),
            }
        }

        match hoons {
            [] => Hoon::ZapZap,
            [h] => h.clone(),
            _ => Hoon::TisLus(Box::new(p.clone()), Box::new(loop_yex(hoons))),
        }
    }

    fn lower_collus(p: &Hoon, q: &Hoon, r: &Hoon) -> Hoon {
        Hoon::Pair(
            Box::new(p.clone()),
            Box::new(Hoon::Pair(Box::new(q.clone()), Box::new(r.clone()))),
        )
    }

    fn lower_siggal(term_or_pair: &TermOrPair, q: &Hoon) -> Hoon {
        Hoon::TisGal(
            Box::new(Hoon::SigGar(
                term_or_pair.clone(),
                Box::new(Hoon::Axis((1u64).into())),
            )),
            Box::new(q.clone()),
        )
    }

    fn lower_sigfas(chum: &Chum, q: &Hoon) -> Hoon {
        Hoon::SigCen(
            chum.clone(),
            Box::new(Hoon::Axis((7u64).into())),
            Vec::new(),
            Box::new(q.clone()),
        )
    }

    /// Desugar `~%` (SigCen) to match hoon-138's `++open`:
    /// `[%sgcn p=chum q=parent r=tyre s=body]` →
    /// `[%sggl [%fast [%clls chum [%zpts parent] [%clsg tyre]]] body]`
    fn lower_sigcen(chum: &Chum, p: &Hoon, tyre: &[(String, Hoon)], q: &Hoon) -> Hoon {
        let clsg_vec = tyre
            .iter()
            .map(|(name, gen)| {
                Hoon::Pair(
                    Box::new(Hoon::Rock(
                        "$".to_string(),
                        NounExpr::ParsedAtom(string_to_atom(name.clone())),
                    )),
                    Box::new(Hoon::ZapTis(Box::new(gen.clone()))),
                )
            })
            .collect::<Vec<_>>();
        let clls = Hoon::ColLus(
            Box::new(Hoon::Rock("$".to_string(), chum_to_nounexpr(chum.clone()))),
            Box::new(Hoon::ZapTis(Box::new(p.clone()))),
            Box::new(Hoon::ColSig(clsg_vec)),
        );
        Hoon::SigGal(
            TermOrPair::Pair("fast".to_string(), Box::new(clls)),
            Box::new(q.clone()),
        )
    }

    /// Desugar `;~` (MicSig) to match hoon-138's `++open`.
    fn lower_micsig(p: &Hoon, list: &[Hoon]) -> Hoon {
        let Some(last) = list.last() else {
            return Hoon::Eror("open-mcsg".to_string());
        };
        let mut acc = Hoon::TisGar(
            Box::new(Hoon::Limb("v".to_string())),
            Box::new(last.clone()),
        );
        for item in list[..list.len() - 1].iter().rev() {
            let a_bind = Hoon::KetTis(Skin::Term("a".to_string()), Box::new(acc));
            let b_bind = Hoon::KetTis(
                Skin::Term("b".to_string()),
                Box::new(Hoon::TisGar(
                    Box::new(Hoon::Limb("v".to_string())),
                    Box::new(item.clone()),
                )),
            );
            let wing_parent_axis6 = vec![Limb::Parent(0, None), Limb::Axis((6u64).into())];
            let c_bind = Hoon::KetTis(
                Skin::Term("c".to_string()),
                Box::new(Hoon::TisGal(
                    Box::new(Hoon::Wing(wing_parent_axis6.clone())),
                    Box::new(Hoon::Limb("b".to_string())),
                )),
            );
            let tsgr_v_p = Hoon::TisGar(Box::new(Hoon::Limb("v".to_string())), Box::new(p.clone()));
            let cncl_b_c = Hoon::CenCol(
                Box::new(Hoon::Limb("b".to_string())),
                vec![Hoon::Limb("c".to_string())],
            );
            let cnts = Hoon::CenTis(
                vec![Limb::Term("a".to_string())],
                vec![(wing_parent_axis6, Hoon::Limb("c".to_string()))],
            );
            let cnls = Hoon::CenLus(Box::new(tsgr_v_p), Box::new(cncl_b_c), Box::new(cnts));
            acc = Hoon::TisLus(
                Box::new(a_bind),
                Box::new(Hoon::TisLus(
                    Box::new(b_bind),
                    Box::new(Hoon::TisLus(
                        Box::new(c_bind),
                        Box::new(Hoon::BarDot(Box::new(cnls))),
                    )),
                )),
            );
        }
        Hoon::TisGar(
            Box::new(Hoon::KetTis(
                Skin::Term("v".to_string()),
                Box::new(Hoon::Axis((1u64).into())),
            )),
            Box::new(acc),
        )
    }

    fn prefix_signature(prefix: Option<&str>) -> PrefixSignature {
        let mut hash: u32 = 0x811c_9dc5;
        match prefix {
            Some(term) => {
                hash ^= 1;
                hash = hash.wrapping_mul(0x0100_0193);
                for byte in term.as_bytes() {
                    hash ^= u32::from(*byte);
                    hash = hash.wrapping_mul(0x0100_0193);
                }
            }
            None => {
                hash ^= 0;
                hash = hash.wrapping_mul(0x0100_0193);
            }
        }
        PrefixSignature(hash)
    }

    /// Native `core_mint` cache. Subject and goal are keyed by canonical type
    /// ID, so the subject is never lowered to a noun here. The key also carries
    /// `tomes_sig` = mug(tomes_map) ^ prefix_signature(prefix) (`tomes_map` comes
    /// from the AST, so a mug suffices), vet, poly, the fan scope, the arm epoch,
    /// and the placeholder signature.
    fn core_mint_cache_lookup(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        tomes_map: Noun,
        prefix: &Option<String>,
        poly: Poly,
    ) -> Result<Option<(NRc<NTy>, FormulaId)>> {
        let context = self.cache_context_key();
        // `mint_core` validates the produced core against `gol` through
        // `nice`/`nest`, so `%hold` fan reachability is a property of the
        // subject-goal pair, not just the deepening subject.
        let fan = self.fan_context_key_scoped_pair(sut, gol)?;
        let tomes_sig = TomesSignature(u64::from(
            self.noun_mug_cached(tomes_map).0 ^ Self::prefix_signature(prefix.as_deref()).0,
        ));
        let poly_key = PolyKey::from(poly);
        Ok(native_core_mint_cache_lookup(
            &self.cx, sut, gol, tomes_sig, context.semantic.vet_key, poly_key, fan,
            context.memo.arm_epoch_key, context.memo.placeholder_context_key,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn core_mint_cache_store(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        tomes_map: Noun,
        prefix: &Option<String>,
        poly: Poly,
        core_type: NRc<NTy>,
        formula: FormulaId,
    ) -> Result<()> {
        let context = self.cache_context_key();
        let fan = self.fan_context_key_scoped_pair(sut, gol)?;
        let tomes_sig = TomesSignature(u64::from(
            self.noun_mug_cached(tomes_map).0 ^ Self::prefix_signature(prefix.as_deref()).0,
        ));
        let poly_key = PolyKey::from(poly);
        native_core_mint_cache_store(
            &mut self.cx, sut, gol, tomes_sig, context.semantic.vet_key, poly_key, fan,
            context.memo.arm_epoch_key, context.memo.placeholder_context_key, core_type, formula,
        );
        Ok(())
    }

    fn arm_placeholder_context_signature(&self) -> PlaceholderSignature {
        if self.arm_placeholder_play_in_progress.is_empty() {
            return PlaceholderSignature(0);
        }
        let mut raws: Vec<NounIdentity> = self
            .arm_placeholder_play_in_progress
            .iter()
            .copied()
            .collect();
        raws.sort_unstable();
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for raw in raws {
            hash ^= raw.0;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        PlaceholderSignature(
            hash ^ (self.arm_placeholder_play_in_progress.len() as u64)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15),
        )
    }

    fn arm_cache_epoch_key(&self) -> ArmEpoch {
        // `arm_epoch` bumps on every arm enter and exit, so keying caches on it directly would
        // defeat cross-arm memoization. The key keeps the epoch while recursion or placeholder
        // state is live and collapses to 0 when all such state is empty.
        if self.arm_in_progress.is_empty()
            && self.arm_goal_in_progress.is_empty()
            && self.arm_placeholder_play_in_progress.is_empty()
        {
            ArmEpoch(0)
        } else {
            self.arm_epoch
        }
    }

    fn memo_context_key(&self) -> MemoContextKey {
        MemoContextKey {
            arm_epoch_key: self.arm_cache_epoch_key(),
            placeholder_context_key: self.arm_placeholder_context_signature(),
        }
    }

    fn cache_context_key(&self) -> CacheContextKey {
        CacheContextKey {
            semantic: self.semantic_context_key(),
            memo: self.memo_context_key(),
        }
    }

    #[cfg(test)]
    fn mint_cache_key(&mut self, sut: Noun, gol: Noun, gen_sig: HoonSignature) -> MintKey<NounMug> {
        // As with the core_mint cache, mint's result depends on the active fan
        // scope (%hold/%rest legs) and on in-progress recursive-arm state, so the
        // key carries fan, arm epoch, and placeholder; all three are 0 in the
        // steady state.
        let context = self.cache_context_key();
        MintKey {
            subject: self.noun_mug_cached(sut),
            goal: self.noun_mug_cached(gol),
            vet: context.semantic.vet_key,
            gene: gen_sig,
            fan: context.semantic.fan_context_key,
            arm_epoch: context.memo.arm_epoch_key,
            placeholder: context.memo.placeholder_context_key,
        }
    }

    /// Native `mint` cache. Subject and goal are keyed by canonical type ID;
    /// the key also carries vet, the gene signature, the fan scope, the arm
    /// epoch, and the placeholder signature.
    fn mint_cache_lookup(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        gen_sig: HoonSignature,
    ) -> Result<Option<(NRc<NTy>, FormulaId)>> {
        let context = self.cache_context_key();
        // `mint` cache hits bypass the fresh `nice(sut, gol, typ)` check, so the
        // fan key is scoped on `%hold` legs reachable from the goal as well as the
        // subject. A cached success is reused only in an equivalent fan context.
        let fan = self.fan_context_key_scoped_pair(sut, gol)?;
        Ok(native_mint_cache_lookup(
            &self.cx, sut, gol, context.semantic.vet_key, gen_sig, fan, context.memo.arm_epoch_key,
            context.memo.placeholder_context_key,
        ))
    }

    fn mint_cache_store(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        gen_sig: HoonSignature,
        ty: NRc<NTy>,
        formula: FormulaId,
    ) -> Result<()> {
        let context = self.cache_context_key();
        let fan = self.fan_context_key_scoped_pair(sut, gol)?;
        native_mint_cache_store(
            &mut self.cx, sut, gol, context.semantic.vet_key, gen_sig, fan,
            context.memo.arm_epoch_key, context.memo.placeholder_context_key, ty, formula,
        );
        Ok(())
    }

    #[cfg(test)]
    fn mint_boundary_lookup_exact(
        &mut self,
        sut: Noun,
        gol: Noun,
        gen: Noun,
    ) -> Result<Option<(Noun, Noun)>> {
        let key = self.mint_cache_key(
            sut,
            gol,
            HoonSignature(u64::from(self.noun_mug_cached(gen).0)),
        );
        let Some(entries) = self.boundary_memo.mint.get(&key) else {
            return Ok(None);
        };
        let sut_raw = NounIdentity::of(sut);
        let gol_raw = NounIdentity::of(gol);
        for entry in entries.iter().rev() {
            let sut_match = NounIdentity::of(entry.sut) == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let gol_match = NounIdentity::of(entry.gol) == gol_raw
                || noun_eq(entry.gol, gol, &self.slab.noun_space())?;
            let gen_match = unsafe { entry.gen.raw_equals(&gen) }
                || noun_eq(entry.gen, gen, &self.slab.noun_space())?;
            if sut_match && gol_match && gen_match {
                return Ok(Some((entry.ty, entry.formula)));
            }
        }
        Ok(None)
    }

    #[cfg(test)]
    fn mint_boundary_store_exact(
        &mut self,
        sut: Noun,
        gol: Noun,
        gen: Noun,
        ty: Noun,
        formula: Noun,
    ) -> Result<()> {
        let key = self.mint_cache_key(
            sut,
            gol,
            HoonSignature(u64::from(self.noun_mug_cached(gen).0)),
        );
        let bucket = self
            .boundary_memo
            .mint
            .ensure_key(key, Self::MINT_CACHE_KEY_LIMIT);
        let sut_raw = NounIdentity::of(sut);
        let gol_raw = NounIdentity::of(gol);
        for entry in bucket.iter() {
            let sut_match = NounIdentity::of(entry.sut) == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let gol_match = NounIdentity::of(entry.gol) == gol_raw
                || noun_eq(entry.gol, gol, &self.slab.noun_space())?;
            let gen_match = unsafe { entry.gen.raw_equals(&gen) }
                || noun_eq(entry.gen, gen, &self.slab.noun_space())?;
            if sut_match && gol_match && gen_match {
                return Ok(());
            }
        }
        if bucket.len() >= Self::MINT_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(MintCacheEntry {
            sut,
            gol,
            gen,
            ty,
            formula,
        });
        Ok(())
    }

    /// Native `mull` cache. Subject, goal, and `dox` are keyed by canonical type
    /// ID; the key also carries vet, the gene signature, the fan scope, the arm
    /// epoch, and the placeholder signature.
    fn mull_cache_lookup(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        dox: &NRc<NTy>,
        gen_sig: HoonSignature,
    ) -> Result<Option<(NRc<NTy>, NRc<NTy>)>> {
        let context = self.cache_context_key();
        // mull checks both `sut` and `dox`, and either one's %hold descent can
        // consult the active fan, so the fan key is scoped on the union of their
        // reachable legs.
        let fan = self.fan_context_key_scoped_pair(sut, dox)?;
        Ok(native_mull_cache_lookup(
            &self.cx, sut, gol, dox, context.semantic.vet_key, gen_sig, fan,
            context.memo.arm_epoch_key, context.memo.placeholder_context_key,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn mull_cache_store(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        dox: &NRc<NTy>,
        gen_sig: HoonSignature,
        p_ty: NRc<NTy>,
        q_ty: NRc<NTy>,
    ) -> Result<()> {
        let context = self.cache_context_key();
        let fan = self.fan_context_key_scoped_pair(sut, dox)?;
        native_mull_cache_store(
            &mut self.cx, sut, gol, dox, context.semantic.vet_key, gen_sig, fan,
            context.memo.arm_epoch_key, context.memo.placeholder_context_key, p_ty, q_ty,
        );
        Ok(())
    }

    /// Native `crop` cache, keyed by the canonical IDs of subject and reference
    /// plus vet and the fan scope. Stores the native result type.
    fn crop_boundary_lookup(
        &mut self,
        sut: &NRc<NTy>,
        ref_: &NRc<NTy>,
    ) -> Result<Option<NRc<NTy>>> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        Ok(native_crop_cache_lookup(
            &self.cx, sut, ref_, semantic.vet_key, fan,
        ))
    }

    fn crop_boundary_store(
        &mut self,
        sut: &NRc<NTy>,
        ref_: &NRc<NTy>,
        result: NRc<NTy>,
    ) -> Result<()> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        native_crop_cache_store(&mut self.cx, sut, ref_, semantic.vet_key, fan, result);
        Ok(())
    }

    /// Native `fuse` cache, keyed by the canonical IDs of subject and reference
    /// plus vet and the fan scope.
    fn fuse_boundary_lookup(
        &mut self,
        sut: &NRc<NTy>,
        ref_: &NRc<NTy>,
    ) -> Result<Option<NRc<NTy>>> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        Ok(native_fuse_cache_lookup(
            &self.cx, sut, ref_, semantic.vet_key, fan,
        ))
    }

    fn fuse_boundary_store(
        &mut self,
        sut: &NRc<NTy>,
        ref_: &NRc<NTy>,
        result: NRc<NTy>,
    ) -> Result<()> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        native_fuse_cache_store(&mut self.cx, sut, ref_, semantic.vet_key, fan, result);
        Ok(())
    }

    pub(super) fn redo_boundary_lookup(&mut self, sut: Noun, ref_: Noun) -> Result<Option<Noun>> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped_noun(sut)?;
        let key = TypeBinaryKey {
            subject: self.noun_mug_cached(sut),
            reference: self.noun_mug_cached(ref_),
            vet: semantic.vet_key,
            fan,
        };
        let Some(bucket) = self.boundary_memo.redo.get(&key) else {
            return Ok(None);
        };
        let sut_raw = NounIdentity::of(sut);
        let ref_raw = NounIdentity::of(ref_);
        for entry in bucket.iter().rev() {
            let sut_match = NounIdentity::of(entry.sut) == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let ref_match = NounIdentity::of(entry.ref_) == ref_raw
                || noun_eq(entry.ref_, ref_, &self.slab.noun_space())?;
            if sut_match && ref_match {
                return Ok(Some(entry.result));
            }
        }
        Ok(None)
    }

    pub(super) fn redo_boundary_store(
        &mut self,
        sut: Noun,
        ref_: Noun,
        result: Noun,
    ) -> Result<()> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped_noun(sut)?;
        let key = TypeBinaryKey {
            subject: self.noun_mug_cached(sut),
            reference: self.noun_mug_cached(ref_),
            vet: semantic.vet_key,
            fan,
        };
        let bucket = self
            .boundary_memo
            .redo
            .ensure_key(key, Self::REDO_CACHE_KEY_LIMIT);
        let sut_raw = NounIdentity::of(sut);
        let ref_raw = NounIdentity::of(ref_);
        for entry in bucket.iter() {
            let sut_match = NounIdentity::of(entry.sut) == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let ref_match = NounIdentity::of(entry.ref_) == ref_raw
                || noun_eq(entry.ref_, ref_, &self.slab.noun_space())?;
            if sut_match && ref_match {
                return Ok(());
            }
        }
        if bucket.len() >= Self::REDO_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(UnaryTypeBoundaryEntry { sut, ref_, result });
        Ok(())
    }

    pub(super) fn rest_legs_noun(&mut self, legs: &[(Noun, Noun)]) -> Noun {
        let mut items = Vec::with_capacity(legs.len());
        for (inner, hoon) in legs {
            items.push(T(self.slab, &[*inner, *hoon]));
        }
        vec_to_list(self.slab, items)
    }

    pub(super) fn rest_boundary_lookup(&mut self, sut: Noun, legs: Noun) -> Result<Option<Noun>> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped_noun(sut)?;
        let key = RestKey {
            subject: self.noun_mug_cached(sut),
            legs: self.noun_mug_cached(legs),
            vet: semantic.vet_key,
            fan,
        };
        let Some(bucket) = self.boundary_memo.rest.get(&key) else {
            return Ok(None);
        };
        let sut_raw = NounIdentity::of(sut);
        let legs_raw = NounIdentity::of(legs);
        for entry in bucket.iter().rev() {
            let sut_match = NounIdentity::of(entry.sut) == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let legs_match = NounIdentity::of(entry.legs) == legs_raw
                || noun_eq(entry.legs, legs, &self.slab.noun_space())?;
            if sut_match && legs_match {
                return Ok(Some(entry.result));
            }
        }
        Ok(None)
    }

    pub(super) fn rest_boundary_store(
        &mut self,
        sut: Noun,
        legs: Noun,
        result: Noun,
    ) -> Result<()> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped_noun(sut)?;
        let key = RestKey {
            subject: self.noun_mug_cached(sut),
            legs: self.noun_mug_cached(legs),
            vet: semantic.vet_key,
            fan,
        };
        let bucket = self
            .boundary_memo
            .rest
            .ensure_key(key, Self::REST_CACHE_KEY_LIMIT);
        let sut_raw = NounIdentity::of(sut);
        let legs_raw = NounIdentity::of(legs);
        for entry in bucket.iter() {
            let sut_match = NounIdentity::of(entry.sut) == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let legs_match = NounIdentity::of(entry.legs) == legs_raw
                || noun_eq(entry.legs, legs, &self.slab.noun_space())?;
            if sut_match && legs_match {
                return Ok(());
            }
        }
        if bucket.len() >= Self::REST_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(RestCacheEntry { sut, legs, result });
        Ok(())
    }

    /// Native `fish` cache, keyed by the subject's canonical ID plus axis, vet,
    /// and the fan scope. Stores the canonical formula ID.
    fn fish_boundary_lookup(
        &mut self,
        sut: &NRc<NTy>,
        axis: &BigUint,
    ) -> Result<Option<FormulaId>> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        Ok(native_fish_cache_lookup(
            &self.cx, sut, axis, semantic.vet_key, fan,
        ))
    }

    fn fish_boundary_store(
        &mut self,
        sut: &NRc<NTy>,
        axis: &BigUint,
        result: FormulaId,
    ) -> Result<()> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        native_fish_cache_store(&mut self.cx, sut, axis, semantic.vet_key, fan, result);
        Ok(())
    }

    fn noun_mug_cached(&self, noun: Noun) -> NounMug {
        // Prefer the mug cached on allocated nouns. This avoids building an ever-growing Rust
        // HashMap while compiling large inputs (hoon-138).
        let space = self.slab.noun_space();
        NounMug(get_mug(noun, &space).unwrap_or_else(|| slab_mug(noun, &space)))
    }

    #[cfg(test)]
    fn nest_mug_lookup(&mut self, sut: Noun, ref_: Noun) -> Result<Option<bool>> {
        let semantic = self.semantic_context_key();
        let key = TypeBinaryKey {
            subject: self.noun_mug_cached(sut),
            reference: self.noun_mug_cached(ref_),
            vet: semantic.vet_key,
            fan: semantic.fan_context_key,
        };
        let Some(entries) = self.boundary_memo.nest.get(&key) else {
            return Ok(None);
        };
        for entry in entries.iter().rev() {
            if noun_eq(entry.sut, sut, &self.slab.noun_space())?
                && noun_eq(entry.ref_, ref_, &self.slab.noun_space())?
            {
                return Ok(Some(entry.result));
            }
        }
        Ok(None)
    }

    #[cfg(test)]
    fn nest_mug_register(&mut self, sut: Noun, ref_: Noun, result: bool) {
        let semantic = self.semantic_context_key();
        let key = TypeBinaryKey {
            subject: self.noun_mug_cached(sut),
            reference: self.noun_mug_cached(ref_),
            vet: semantic.vet_key,
            fan: semantic.fan_context_key,
        };
        let bucket = self
            .boundary_memo
            .nest
            .ensure_key(key, Self::NEST_MUG_KEY_LIMIT);
        if bucket.len() >= Self::NEST_MUG_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(NestCacheEntry { sut, ref_, result });
    }

    pub(crate) fn mint(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        gen: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let mut scope = self.hoon_ast_scope(gen);
        let root = scope.root;
        let result = scope.mint_arena(sut, gol, root);
        match result {
            Ok(value) => Ok(value),
            Err(err) => Err(scope.decorate_error(err)),
        }
    }

    fn mint_arena(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        id: HoonId,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let source = self.hoon_arena.source_ptr(id);
        // SAFETY: the outermost mint/play scope holds the caller's root borrow
        // until `leave_hoon_ast_scope`; registration visits only descendants of
        // that root, and nested calls cannot clear the arena. The pointer is
        // read-only, and the arena never mutates the borrowed AST.
        let gen = unsafe { &*source };
        self.mint_inner(sut, gol, gen, id)
    }

    fn mint_arena_child(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        parent: HoonId,
        index: usize,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let child = self.hoon_arena.child(parent, index);
        match self.mint_arena(sut, gol, child) {
            Ok(value) => Ok(value),
            Err(err) => Err(self.decorate_error(err)),
        }
    }

    /// Public noun boundary: lift sut/gol, run native mint, and materialize the
    /// type and formula once for external consumers. The `self.mint` call is the
    /// native mint; calling `mint_noun` here would recurse forever.
    pub fn mint_noun(&mut self, sut: Noun, gol: Noun, gen: &Hoon) -> Result<(Noun, Noun)> {
        let space = self.slab.noun_space();
        let sut_n = native_of(&mut self.cx, sut, &space)?;
        let gol_n = native_of(&mut self.cx, gol, &space)?;
        let (ty, formula) = self.mint(sut_n, gol_n, gen)?;
        let ty_noun = live_to_noun(&mut self.cx, &ty, self.slab);
        let formula = self.formula_materialize(formula);
        Ok((ty_noun, formula))
    }

    fn mint_inner(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        gen: &Hoon,
        gen_id: HoonId,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let cache_sig = self.mint_cache_signature_id(gen_id);
        if let Some(gen_sig) = cache_sig {
            if let Some(cached) = self.mint_cache_lookup(&sut, &gol, gen_sig)? {
                return Ok(cached);
            }
        }

        // hoon-138 `++mint` short-circuit: if the subject is void (except direct `dbug`),
        // allow only `%lost`/`%zpzp` under `vet`, and return `[void [0 0]]`.
        let is_dbug = match gen {
            Hoon::Dbug(_, _) => true,
            _ => false,
        };
        let is_allowed_void = match gen {
            Hoon::Lost(_) | Hoon::ZapZap => true,
            _ => false,
        };
        if matches!(&*sut, NTy::Void) && !is_dbug {
            if self.vet && !is_allowed_void {
                return Err(CompilerError::Noun("mint-vain".to_string()));
            }
            return Ok((cons_void(&mut self.cx), self.formula_slot_u64(0)));
        }

        // Keep (sut, gol) for the cache store; the match below moves them into
        // the per-arm recursion. Cloning an `Rc` only bumps a refcount.
        let cache_sut = sut.clone();
        let cache_gol = gol.clone();
        let result = match gen {
            Hoon::Pair(_, _) => {
                let gol_head = cons_noun(&mut self.cx);
                let (t_head, f_head) = self.mint_arena_child(sut.clone(), gol_head, gen_id, 0)?;
                let gol_tail = cons_noun(&mut self.cx);
                let (t_tail, f_tail) = self.mint_arena_child(sut.clone(), gol_tail, gen_id, 1)?;
                let ty = cons_cell(&mut self.cx, t_head, t_tail);
                let ty = self.nice(sut, gol, ty)?;
                let formula = self.formula_cons(f_head, f_tail);
                Ok((ty, formula))
            }
            Hoon::Rock(_, expr) | Hoon::Sand(_, expr) => {
                let ty = self.play(sut.clone(), gen)?;
                let ty = self.nice(sut, gol, ty)?;
                let value = noun_expr_to_noun(self.slab, expr);
                let formula = self.formula_quote(value);
                Ok((ty, formula))
            }
            Hoon::ZapZap | Hoon::Eror(_) => {
                let ty = cons_void(&mut self.cx);
                let ty = self.nice(sut, gol, ty)?;
                let formula = self.formula_slot_u64(0);
                Ok((ty, formula))
            }
            Hoon::Dbug(spot, inner) => self.mint_dbug(sut, gol, spot, inner),
            Hoon::Note(note, inner) => self.mint_note(sut, gol, note, inner),
            Hoon::Lost(_) => self.mint_lost(sut, gol),
            Hoon::TisGar(_, _) => self.mint_tsgr_arena(sut, gol, gen_id),
            Hoon::TisGal(_, _) | Hoon::TisHep(_, _) | Hoon::TisLus(_, _) => {
                self.mint_opened(sut, gol, gen)
            }
            Hoon::WutCol(p, q, r) => self.mint_wtcl(sut, gol, p, q, r),
            Hoon::WutDot(p, q, r) => self.mint_wtcl(sut, gol, p, r, q),
            Hoon::TisCom(p, q) => self.mint_tscm(sut, gol, p, q),
            Hoon::WutPam(list) => self.mint_wtpm(sut, gol, list),
            Hoon::WutBar(list) => self.mint_wtbr(sut, gol, list),
            Hoon::WutPat(wing, q, r) => self.mint_wtpt(sut, gol, wing, q, r),
            Hoon::WutSig(wing, q, r) => self.mint_wtsg(sut, gol, wing, q, r),
            Hoon::WutZap(p) => self.mint_wtzp(sut, gol, p),
            Hoon::WutKet(wing, q, r) => self.mint_wtkt(sut, gol, wing, q, r),
            Hoon::WutGal(p, q) => self.mint_wtgl(sut, gol, p, q),
            Hoon::WutGar(p, q) => self.mint_wtgr(sut, gol, p, q),
            Hoon::Fits(p, wing) => self.mint_fits(sut, gol, p, wing),
            Hoon::WutHax(skin, wing) => self.mint_wthx(sut, gol, skin, wing),
            Hoon::WutTis(spec, wing) => self.mint_wtts(sut, gol, spec, wing),
            Hoon::WutLus(wing, q, list) => self.mint_wtls(sut, gol, wing, q, list),
            Hoon::WutHep(wing, list) => self.mint_wthp(sut, gol, wing, list),
            Hoon::KetDot(p, q) => {
                let lowered = Self::lower_ktdt(p, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::KetLus(p, q) => self.mint_ktsl(sut, gol, p, q),
            Hoon::KetBar(p) => self.mint_ketvar(sut, gol, p, Vair::Iron),
            Hoon::KetPam(p) => self.mint_ketvar(sut, gol, p, Vair::Zinc),
            Hoon::KetHep(spec, q) => self.mint_kthp(sut, gol, spec, q),
            Hoon::KetTar(spec) => self.mint_kttr(sut, gol, spec),
            Hoon::KetCol(spec) => self.mint_ktcl(sut, gol, spec),
            Hoon::KetTis(skin, p) => {
                let lowered = Self::lower_kettis(skin, p);
                self.mint(sut, gol, &lowered)
            }
            Hoon::KetSig(p) => self.mint_ktsg(sut, gol, p),
            Hoon::KetWut(p) => self.mint_ketvar(sut, gol, p, Vair::Lead),
            Hoon::DotKet(spec, q) => self.mint_dtkt(sut, gol, spec, q),
            Hoon::DotLus(p) => self.mint_dtls(sut, gol, p),
            Hoon::DotTar(p, q) => self.mint_dttr(sut, gol, p, q),
            Hoon::DotTis(p, q) => self.mint_dtts(sut, gol, p, q),
            Hoon::DotWut(p) => self.mint_dtwt(sut, gol, p),
            Hoon::TisBar(spec, q) => self.mint_tsbr(sut, gol, spec, q),
            Hoon::TisFas(skin, p, q) => self.mint_tsfs(sut, gol, skin, p, q),
            Hoon::TisMic(skin, p, q) => self.mint_tsmc(sut, gol, skin, p, q),
            Hoon::TisDot(wing, p, q) => {
                let lowered = Self::lower_tisdot(wing, p, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::Wing(wing) => self.mint_wing(sut, gol, wing),
            Hoon::CenHep(p, q) => {
                let lowered = Self::lower_cenhep(p, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::CenCol(p, hoons) => {
                let lowered = Self::lower_cencol(p, hoons);
                self.mint(sut, gol, &lowered)
            }
            Hoon::CenTis(wing, pairs) => self.emin(sut, gol, wing, pairs),
            Hoon::CenTar(wing, p, pairs) => {
                let lowered = Self::lower_centar(wing, p, pairs);
                self.mint(sut, gol, &lowered)
            }
            Hoon::CenLus(p, q, r) => {
                let lowered = Self::lower_cenlus(p, q, r);
                self.mint(sut, gol, &lowered)
            }
            Hoon::CenSig(wing, p, hoons) => {
                let lowered = Self::lower_censig(wing, p, hoons)?;
                self.mint(sut, gol, &lowered)
            }
            Hoon::Limb(name) => self.mint_limb(sut, gol, name),
            Hoon::Hand(typ, nock) => self.mint_hand(typ, nock),
            Hoon::Tune(tune) => self.mint_tune(sut, gol, tune),
            Hoon::SigGar(hint, q) => self.mint_siggar(sut, gol, hint, q),
            Hoon::BarTis(spec, q) => self.mint_brtis(sut, gol, spec.as_ref(), q),
            Hoon::BarCab(spec, alas, tomes) => self.mint_brcb(sut, gol, spec.as_ref(), alas, tomes),
            Hoon::BarCol(p, q) => {
                let lowered = Self::lower_barcol(p, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::BarDot(p) => {
                let lowered = Self::lower_bardot(p);
                self.mint(sut, gol, &lowered)
            }
            Hoon::BarHep(p) => {
                let lowered = Self::lower_barhep(p);
                self.mint(sut, gol, &lowered)
            }
            Hoon::BarTar(spec, q) => {
                let lowered = Self::lower_bartar(spec, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::SigBar(p, q) => {
                let lowered = Self::lower_sigbar(p, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::SigCab(p, q) => {
                let lowered = Self::lower_sigcab(p, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::MicSig(p, list) => {
                let lowered = Self::lower_micsig(p, list);
                self.mint(sut, gol, &lowered)
            }
            Hoon::MicCol(p, hoons) => {
                let lowered = Self::lower_miccol(p, hoons);
                self.mint(sut, gol, &lowered)
            }
            Hoon::ColTar(items) => {
                let lowered = Self::lower_coltar(items);
                self.mint(sut, gol, &lowered)
            }
            Hoon::ColHep(p, q) => {
                let lowered = Self::lower_colhep(p, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::ColLus(p, q, r) => {
                let lowered = Self::lower_collus(p, q, r);
                self.mint(sut, gol, &lowered)
            }
            Hoon::ColSig(items) => self.mint_colsig(sut, gol, items),
            Hoon::SigCen(chum, p, tyre, q) => {
                let lowered = Self::lower_sigcen(chum, p, tyre, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::SigFas(chum, q) => {
                let lowered = Self::lower_sigfas(chum, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::SigGal(term_or_pair, q) => {
                let lowered = Self::lower_siggal(term_or_pair, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::SigLus(a, q) => {
                let lowered = Self::lower_siglus(*a, q);
                self.mint(sut, gol, &lowered)
            }
            Hoon::SigZap(p, q) => self.mint_sigzap(sut, gol, p, q),
            Hoon::ZapCom(p, q) => self.mint_zpcom(sut, gol, p, q),
            Hoon::ZapMic(p, q) => self.mint_zpmc(sut, gol, p, q),
            Hoon::ZapGal(spec, q) => self.mint_zpgl(sut, gol, spec, q),
            Hoon::ZapTis(p) => self.mint_zpts(sut, gol, p),
            Hoon::ZapPat(wings, q, r) => self.mint_zppt(sut, gol, wings, q, r),
            Hoon::TisSig(list) => match list.as_slice() {
                [] => {
                    let ty = cons_void(&mut self.cx);
                    let formula = self.formula_slot_u64(0);
                    Ok((ty, formula))
                }
                [single] => self.mint(sut, gol, single),
                _ => {
                    let mut iter = list.iter().rev();
                    let mut acc = iter.next().cloned().ok_or_else(|| {
                        CompilerError::UnsupportedExpr("empty TisSig".to_string())
                    })?;
                    for item in iter {
                        acc = Hoon::TisGar(Box::new(item.clone()), Box::new(acc));
                    }
                    self.mint(sut, gol, &acc)
                }
            },
            Hoon::Axis(axis) => {
                let ty = self.peek(sut.clone(), Way::Free, axis.as_biguint().clone())?;
                let ty = self.nice(sut, gol, ty)?;
                let formula = self.formula_slot(axis.as_biguint().clone());
                Ok((ty, formula))
            }
            Hoon::BarCen(prefix, tomes) => {
                self.mine(sut, gol, Vair::Gold, prefix.as_deref(), Poly::Dry, tomes)
            }
            Hoon::BarPat(prefix, tomes) => {
                self.mine(sut, gol, Vair::Gold, prefix.as_deref(), Poly::Wet, tomes)
            }
            _ => self.mint_opened(sut, gol, gen),
        }?;

        if let Some(gen_sig) = cache_sig {
            self.mint_cache_store(&cache_sut, &cache_gol, gen_sig, result.0.clone(), result.1)?;
        }

        Ok(result)
    }

    pub fn slot_axis<A: Into<BigUint>>(noun: Noun, axis: A, space: &NounSpace) -> Option<Noun> {
        let mut axis = axis.into();
        if axis == BigUint::from(0u8) {
            return None;
        }
        let mut node = noun;
        let mut steps = Vec::new();
        while axis > BigUint::from(1u8) {
            steps.push(if (&axis & BigUint::from(1u8)) == BigUint::from(0u8) {
                0
            } else {
                1
            });
            axis >>= 1;
        }
        while let Some(step) = steps.pop() {
            let cell = node.in_space(space).as_cell().ok()?;
            node = if step == 0 {
                cell.head().noun()
            } else {
                cell.tail().noun()
            };
        }
        Some(node)
    }

    fn decorate_error(&self, err: CompilerError) -> CompilerError {
        match self.dbug_locations.last().cloned() {
            Some(location) => {
                err.with_metadata(CompilerErrorMetadata::default().with_location(location))
            }
            None => err,
        }
    }

    fn location_from_spot(spot: &Spot) -> CompilerErrorLocation {
        let file = Self::dbug_path(&spot.p);
        let (start_line, start_col) = spot.q.p;
        let (end_line, end_col) = spot.q.q;
        CompilerErrorLocation {
            file: Some(file),
            start_line: Some(start_line),
            start_col: Some(start_col),
            end_line: Some(end_line),
            end_col: Some(end_col),
            ..CompilerErrorLocation::default()
        }
    }

    fn dbug_path(path: &[String]) -> String {
        if path.is_empty() {
            return "?".to_string();
        }

        // getcwd is a syscall and this runs once per `%dbug` node; honk never
        // chdirs, so resolve the cwd's components once per process.
        static CWD_COMPONENTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        let cwd_components = CWD_COMPONENTS.get_or_init(|| {
            std::env::current_dir()
                .map(|cwd| {
                    cwd.components()
                        .filter_map(|component| match component {
                            std::path::Component::Normal(segment) => {
                                Some(segment.to_string_lossy().into_owned())
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        if !cwd_components.is_empty()
            && path.len() > cwd_components.len()
            && path.starts_with(cwd_components.as_slice())
        {
            return path[cwd_components.len()..].join("/");
        }

        path.join("/")
    }

    pub(crate) fn play(&mut self, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        let mut scope = self.hoon_ast_scope(gen);
        // hoon-138 `++play` runs with vet disabled for the whole evaluation;
        // `with_vet_off` restores it afterward.
        let root = scope.root;
        scope.with_vet_off(|ut| ut.play_arena(sut, root))
    }

    fn play_arena(&mut self, sut: NRc<NTy>, id: HoonId) -> Result<NRc<NTy>> {
        let source = self.hoon_arena.source_ptr(id);
        // SAFETY: identical scope invariant to `mint_arena`; this is an
        // immutable descendant pointer registered under the active root borrow.
        let gen = unsafe { &*source };
        self.play_inner(sut, gen, id)
    }

    fn play_arena_child(
        &mut self,
        sut: NRc<NTy>,
        parent: HoonId,
        index: usize,
    ) -> Result<NRc<NTy>> {
        let child = self.hoon_arena.child(parent, index);
        self.play_arena(sut, child)
    }

    /// Noun-in, noun-out `play` for callers that hold a noun subject (the binary's
    /// prelude seed, the `%spec` skin test): lifts `sut` with `native_of`, runs
    /// native play, and lowers the result to a noun.
    pub fn play_noun(&mut self, sut: Noun, gen: &Hoon) -> Result<Noun> {
        let sut = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let r = self.play(sut, gen)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    fn play_inner(&mut self, sut: NRc<NTy>, gen: &Hoon, gen_id: HoonId) -> Result<NRc<NTy>> {
        let result = {
            match gen {
                Hoon::Pair(_, _) => {
                    let head = self.play_arena_child(sut.clone(), gen_id, 0)?;
                    let tail = self.play_arena_child(sut, gen_id, 1)?;
                    Ok(cons_cell(&mut self.cx, head, tail))
                }
                Hoon::Rock(aura, expr) => Ok(self.play_rock(aura, expr)),
                Hoon::Sand(aura, expr) => self.play_sand(aura, expr),
                Hoon::ZapZap | Hoon::Eror(_) => Ok(cons_void(&mut self.cx)),
                Hoon::Dbug(_, inner) => self.play_dbug(sut, inner),
                Hoon::Note(note, inner) => self.play_note(sut, note, inner),
                Hoon::Lost(_) => Ok(cons_void(&mut self.cx)),
                Hoon::TisGar(_, _) => {
                    let next = self.play_arena_child(sut, gen_id, 0)?;
                    self.play_arena_child(next, gen_id, 1)
                }
                Hoon::TisGal(_, _) | Hoon::TisHep(_, _) | Hoon::TisLus(_, _) => {
                    self.play_opened(sut, gen)
                }
                Hoon::WutCol(p, q, r) => self.play_wtcl(sut, p, q, r),
                Hoon::WutDot(p, q, r) => self.play_wtcl(sut, p, r, q),
                Hoon::TisCom(p, q) => {
                    let busked = self.busk(sut, p);
                    self.play(busked, q)
                }
                Hoon::WutPam(list) => {
                    let lowered = Self::lower_wtpm(list);
                    self.play(sut, &lowered)
                }
                Hoon::WutBar(list) => {
                    let lowered = Self::lower_wtbr(list);
                    self.play(sut, &lowered)
                }
                Hoon::WutPat(wing, q, r) => {
                    let expanded = expand_wutpat(wing, q, r);
                    self.play(sut, &expanded)
                }
                Hoon::WutSig(wing, q, r) => {
                    let expanded = expand_wutsig(wing, q, r);
                    self.play(sut, &expanded)
                }
                Hoon::WutZap(_p) => Ok(ty_bool_n(&mut self.cx, self.slab).1),
                Hoon::WutKet(wing, q, r) => {
                    let test = Hoon::WutTis(
                        Box::new(Spec::Base(BaseType::Atom("$".to_string()))),
                        wing.clone(),
                    );
                    // Match hoon-138 open() lowering: wtkt -> wtcl(wtts atom p, r, q)
                    let expanded =
                        Hoon::WutCol(Box::new(test), Box::new(*r.clone()), Box::new(*q.clone()));
                    self.play(sut, &expanded)
                }
                Hoon::WutGal(p, q) => {
                    // Match hoon-138 open() lowering: wtgl -> wtcl(p, zpzp, q)
                    let expanded = Hoon::WutCol(
                        Box::new(*p.clone()),
                        Box::new(Hoon::ZapZap),
                        Box::new(*q.clone()),
                    );
                    self.play(sut, &expanded)
                }
                Hoon::WutGar(p, q) => {
                    // Match hoon-138 open() lowering: wtgr -> wtcl(p, q, zpzp)
                    let expanded = Hoon::WutCol(
                        Box::new(*p.clone()),
                        Box::new(*q.clone()),
                        Box::new(Hoon::ZapZap),
                    );
                    self.play(sut, &expanded)
                }
                Hoon::Fits(_p, _wing) => Ok(ty_bool_n(&mut self.cx, self.slab).1),
                Hoon::WutHax(_skin, _wing) => Ok(ty_bool_n(&mut self.cx, self.slab).1),
                Hoon::WutTis(spec, wing) => self.play_wtts(sut, spec, wing),
                Hoon::WutLus(wing, q, list) => {
                    let expanded = expand_wutlus(wing, q, list);
                    self.play(sut, &expanded)
                }
                Hoon::WutHep(wing, list) => {
                    let expanded = expand_wuthep(wing, list);
                    self.play(sut, &expanded)
                }
                Hoon::KetDot(p, q) => {
                    let lowered = Self::lower_ktdt(p, q);
                    self.play(sut, &lowered)
                }
                Hoon::KetLus(p, _q) => self.play(sut, p),
                Hoon::KetBar(p) => self.play_ketvar(sut, p, Vair::Iron),
                Hoon::KetPam(p) => self.play_ketvar(sut, p, Vair::Zinc),
                Hoon::KetHep(spec, q) => self.play_kthp(sut, spec, q),
                Hoon::KetTar(spec) => self.play_kttr(sut, spec),
                Hoon::KetCol(spec) => self.play_ktcl(sut, spec),
                Hoon::KetTis(skin, p) => {
                    let lowered = Self::lower_kettis(skin, p);
                    self.play(sut, &lowered)
                }
                Hoon::KetSig(p) => self.play(sut, p),
                Hoon::KetWut(p) => self.play_ketvar(sut, p, Vair::Lead),
                Hoon::DotKet(spec, _q) => {
                    let example = spec_example(spec);
                    self.play(sut, &example)
                }
                Hoon::DotLus(p) => self.play_dtls(sut, p),
                Hoon::DotTar(_p, _q) => Ok(cons_noun(&mut self.cx)),
                Hoon::DotTis(_p, _q) => Ok(ty_bool_n(&mut self.cx, self.slab).1),
                Hoon::DotWut(_p) => Ok(ty_bool_n(&mut self.cx, self.slab).1),
                Hoon::TisBar(spec, q) => {
                    let example = self.spec_example_cached(spec);
                    let expanded =
                        Hoon::TisLus(Box::new(example.as_ref().clone()), Box::new(*q.clone()));
                    self.play(sut, &expanded)
                }
                Hoon::TisFas(skin, p, q) => {
                    let expanded = Hoon::TisLus(
                        Box::new(Hoon::KetTis(skin.clone(), p.clone())),
                        Box::new(*q.clone()),
                    );
                    self.play(sut, &expanded)
                }
                Hoon::TisMic(skin, p, q) => {
                    let expanded = Hoon::TisFas(skin.clone(), q.clone(), p.clone());
                    self.play(sut, &expanded)
                }
                Hoon::TisDot(wing, p, q) => {
                    let lowered = Self::lower_tisdot(wing, p, q);
                    self.play(sut, &lowered)
                }
                Hoon::Wing(wing) => self.play_wing(sut, wing),
                Hoon::CenHep(p, q) => {
                    let lowered = Self::lower_cenhep(p, q);
                    self.play(sut, &lowered)
                }
                Hoon::CenCol(p, hoons) => {
                    let lowered = Self::lower_cencol(p, hoons);
                    self.play(sut, &lowered)
                }
                Hoon::CenTis(wing, pairs) => self.epla(sut, wing, pairs),
                Hoon::CenTar(wing, p, pairs) => {
                    let lowered = Self::lower_centar(wing, p, pairs);
                    self.play(sut, &lowered)
                }
                Hoon::CenLus(p, q, r) => {
                    let lowered = Self::lower_cenlus(p, q, r);
                    self.play(sut, &lowered)
                }
                Hoon::CenSig(wing, p, hoons) => {
                    let lowered = Self::lower_censig(wing, p, hoons)?;
                    self.play(sut, &lowered)
                }
                Hoon::Limb(name) => self.play_limb(sut, name),
                Hoon::Hand(typ, _nock) => {
                    let n = type_to_noun(self.slab, typ)?;
                    native_of(&mut self.cx, n, &self.slab.noun_space())
                }
                Hoon::Tune(tune) => self.play_tune(sut, tune),
                Hoon::SigGar(_hint, q) => self.play(sut, q),
                Hoon::BarTis(spec, q) => self.play_brtis(sut, spec.as_ref(), q),
                Hoon::BarCab(spec, alas, tomes) => self.play_brcb(sut, spec.as_ref(), alas, tomes),
                Hoon::BarCol(p, q) => {
                    let lowered = Self::lower_barcol(p, q);
                    self.play(sut, &lowered)
                }
                Hoon::BarDot(p) => {
                    let lowered = Self::lower_bardot(p);
                    self.play(sut, &lowered)
                }
                Hoon::BarHep(p) => {
                    let lowered = Self::lower_barhep(p);
                    self.play(sut, &lowered)
                }
                Hoon::BarTar(spec, q) => {
                    let lowered = Self::lower_bartar(spec, q);
                    self.play(sut, &lowered)
                }
                Hoon::SigBar(p, q) => {
                    let lowered = Self::lower_sigbar(p, q);
                    self.play(sut, &lowered)
                }
                Hoon::SigCab(p, q) => {
                    let lowered = Self::lower_sigcab(p, q);
                    self.play(sut, &lowered)
                }
                Hoon::MicSig(p, list) => {
                    let lowered = Self::lower_micsig(p, list);
                    self.play(sut, &lowered)
                }
                Hoon::MicCol(p, hoons) => {
                    let lowered = Self::lower_miccol(p, hoons);
                    self.play(sut, &lowered)
                }
                Hoon::ColTar(items) => {
                    let lowered = Self::lower_coltar(items);
                    self.play(sut, &lowered)
                }
                Hoon::ColHep(p, q) => {
                    let lowered = Self::lower_colhep(p, q);
                    self.play(sut, &lowered)
                }
                Hoon::ColLus(p, q, r) => {
                    let lowered = Self::lower_collus(p, q, r);
                    self.play(sut, &lowered)
                }
                Hoon::ColSig(items) => self.play_colsig(sut, items),
                Hoon::SigCen(chum, p, tyre, q) => {
                    let lowered = Self::lower_sigcen(chum, p, tyre, q);
                    self.play(sut, &lowered)
                }
                Hoon::SigFas(chum, q) => {
                    let lowered = Self::lower_sigfas(chum, q);
                    self.play(sut, &lowered)
                }
                Hoon::SigGal(term_or_pair, q) => {
                    let lowered = Self::lower_siggal(term_or_pair, q);
                    self.play(sut, &lowered)
                }
                Hoon::SigLus(a, q) => {
                    let lowered = Self::lower_siglus(*a, q);
                    self.play(sut, &lowered)
                }
                Hoon::SigZap(p, q) => {
                    let _ = self.play(sut.clone(), p)?;
                    self.play(sut, q)
                }
                Hoon::ZapCom(p, _q) => self.play(sut, p),
                Hoon::ZapMic(p, q) => {
                    let pt = self.play(sut.clone(), p)?;
                    let qt = self.play(sut, q)?;
                    Ok(cons_cell(&mut self.cx, pt, qt))
                }
                Hoon::ZapGal(spec, _q) => {
                    let example = spec_example(spec);
                    self.play(sut, &example)
                }
                Hoon::ZapTis(_p) => Ok(cons_noun(&mut self.cx)),
                Hoon::ZapPat(wings, q, r) => {
                    if self.feel(sut.clone(), wings)? {
                        self.play(sut, q)
                    } else {
                        self.play(sut, r)
                    }
                }
                Hoon::TisSig(list) => match list.as_slice() {
                    [] => Ok(cons_void(&mut self.cx)),
                    [single] => self.play(sut, single),
                    _ => {
                        let mut iter = list.iter().rev();
                        let mut acc = iter.next().cloned().ok_or_else(|| {
                            CompilerError::UnsupportedExpr("empty TisSig".to_string())
                        })?;
                        for item in iter {
                            acc = Hoon::TisGar(Box::new(item.clone()), Box::new(acc));
                        }
                        self.play(sut, &acc)
                    }
                },
                Hoon::Axis(axis) => self.peek(sut, Way::Free, axis.as_biguint().clone()),
                Hoon::BarCen(prefix, tomes) => self.play_core(sut, prefix, tomes, Poly::Dry),
                Hoon::BarPat(prefix, tomes) => self.play_core(sut, prefix, tomes, Poly::Wet),
                _ => self.play_opened(sut, gen),
            }
        };
        result
    }

    fn mint_tsgr_arena(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        id: HoonId,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let goal = cons_noun(&mut self.cx);
        let (p_ty, p_formula) = self.mint_arena_child(sut, goal, id, 0)?;
        let (q_ty, q_formula) = self.mint_arena_child(p_ty, gol, id, 1)?;
        let formula = self.formula_comb(p_formula, q_formula);
        Ok((q_ty, formula))
    }

    fn mint_brtis(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let lowered = Self::lower_brtis(spec, q);
        self.mint(sut, gol, &lowered)
    }

    fn mint_brcb(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        alas: &Alas,
        tomes: &HashMap<String, Tome>,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let transformed = if alas.is_empty() {
            tomes.clone()
        } else {
            self.barcab_apply_alas(alas, tomes)
        };
        let lowered = Hoon::TisLus(
            Box::new(Hoon::KetTar(Box::new(spec.clone()))),
            Box::new(Hoon::BarCen(None, transformed)),
        );
        self.mint(sut, gol, &lowered)
    }

    fn mint_colsig(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        items: &[Hoon],
    ) -> Result<(NRc<NTy>, FormulaId)> {
        match items {
            [] => {
                let zero = D(0);
                let ty = ty_atom_n(&mut self.cx, self.slab, "n", Some(zero)).1;
                let ty = self.nice(sut, gol, ty)?;
                let formula = self.formula_quote(zero);
                Ok((ty, formula))
            }
            [head, tail @ ..] => {
                let goal = cons_noun(&mut self.cx);
                let (head_ty, head_formula) = self.mint(sut.clone(), goal.clone(), head)?;
                let (tail_ty, tail_formula) = self.mint_colsig(sut.clone(), goal, tail)?;
                let ty = cons_cell(&mut self.cx, head_ty, tail_ty);
                let ty = self.nice(sut, gol, ty)?;
                let formula = self.formula_cons(head_formula, tail_formula);
                Ok((ty, formula))
            }
        }
    }

    fn play_colsig(&mut self, sut: NRc<NTy>, items: &[Hoon]) -> Result<NRc<NTy>> {
        match items {
            [] => Ok(ty_atom_n(&mut self.cx, self.slab, "n", Some(D(0))).1),
            [head, tail @ ..] => {
                let head_ty = self.play(sut.clone(), head)?;
                let tail_ty = self.play_colsig(sut, tail)?;
                Ok(cons_cell(&mut self.cx, head_ty, tail_ty))
            }
        }
    }

    fn barcab_apply_alas(
        &mut self,
        alas: &Alas,
        tomes: &HashMap<String, Tome>,
    ) -> HashMap<String, Tome> {
        let mut transformed: HashMap<String, Tome> = HashMap::with_capacity(tomes.len());
        for (term, (what, arms_map)) in tomes.iter() {
            let mut wrapped_map: HashMap<String, Hoon> = HashMap::with_capacity(arms_map.len());
            for (face, expr) in arms_map.iter() {
                // `+*` aliases nest the original arm body under `TisTar`. Hold genes can
                // point at that nested body, so cache its AST here.
                self.cache_hoon_ast_for_node(expr);
                let mut body = expr.clone();
                for (alas_face, alas_init) in alas.iter().rev() {
                    body = Hoon::TisTar(
                        (alas_face.clone(), None),
                        Box::new(alas_init.clone()),
                        Box::new(body),
                    );
                }
                wrapped_map.insert(face.clone(), body);
            }
            transformed.insert(term.clone(), (what.clone(), wrapped_map));
        }
        transformed
    }

    fn play_brtis(&mut self, sut: NRc<NTy>, spec: &Spec, q: &Hoon) -> Result<NRc<NTy>> {
        let lowered = Self::lower_brtis(spec, q);
        self.play(sut, &lowered)
    }

    fn play_brcb(
        &mut self,
        sut: NRc<NTy>,
        spec: &Spec,
        alas: &Alas,
        tomes: &HashMap<String, Tome>,
    ) -> Result<NRc<NTy>> {
        let transformed = if alas.is_empty() {
            tomes.clone()
        } else {
            self.barcab_apply_alas(alas, tomes)
        };
        let lowered = Hoon::TisLus(
            Box::new(Hoon::KetTar(Box::new(spec.clone()))),
            Box::new(Hoon::BarCen(None, transformed)),
        );
        self.play(sut, &lowered)
    }

    fn play_wtcl(&mut self, sut: NRc<NTy>, p: &Hoon, q: &Hoon, r: &Hoon) -> Result<NRc<NTy>> {
        let fex = self.gain(sut.clone(), p)?;
        let wux = self.lose(sut, p)?;
        let mut options = Vec::with_capacity(2);
        if !matches!(&*fex, NTy::Void) {
            options.push(self.play(fex, q)?);
        }
        if !matches!(&*wux, NTy::Void) {
            options.push(self.play(wux, r)?);
        }
        self.cons_fork(options)
    }

    // `?:` if-then-else (hoon-138 `++mint` `%wtcl`).
    fn mint_wtcl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
        r: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let (_cond_ty, cond_formula) = self.mint(sut.clone(), bool_ty, p)?;
        let fex = self.gain(sut.clone(), p)?;
        let wux = self.lose(sut.clone(), p)?;
        let fex_void = matches!(&*fex, NTy::Void);
        let wux_void = matches!(&*wux, NTy::Void);
        let (ned, duy) = if fex_void && wux_void {
            // Canonical hoon-138 `%wtcl`: [%void %void] => [ned=0 duy=[%0 0]].
            (false, self.formula_slot_u64(0))
        } else if fex_void {
            (true, self.formula_quote(D(1)))
        } else if wux_void {
            (true, self.formula_quote(D(0)))
        } else {
            (false, cond_formula)
        };
        let (q_ty, q_formula) = self.mint(fex, gol.clone(), q)?;
        let (r_ty, r_formula) = self.mint(wux, gol, r)?;
        let fol = self.formula_cond(duy, q_formula, r_formula);
        // `cons_fork` keeps members mug-ordered to match hoon-138's treap.
        let ty = self.cons_fork(vec![q_ty, r_ty])?;
        let formula = if ned {
            let toss_tag = term_to_noun(self.slab, "toss");
            let cond_noun = self.formula_materialize(cond_formula);
            let toss = T(self.slab, &[toss_tag, cond_noun]);
            let space = self.slab.noun_space();
            self.formula_arena.hint(toss, fol, &space)
        } else {
            fol
        };
        Ok((ty, formula))
    }

    fn mint_tscm(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let busked = self.busk(sut, p);
        self.mint(busked, gol, q)
    }

    fn busk(&mut self, sut: NRc<NTy>, gen: &Hoon) -> NRc<NTy> {
        // hoon-138 `++busk`: [%face [~ [gen ~]] sut]. The new %face shares `sut`'s `Rc`.
        let gen_noun = self.hoon_noun_for_node(gen);
        self.cache_hoon_ast_for_node(gen);
        let pair = T(self.slab, &[gen_noun, D(0)]);
        let tool = T(self.slab, &[D(0), pair]);
        let tool_leaf = live_leaf_from_noun(&mut self.cx, tool, &self.slab.noun_space());
        cons_face(&mut self.cx, tool_leaf, sut)
    }

    fn mint_wtpm(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        list: &[Hoon],
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let lowered = Self::lower_wtpm(list);
        self.mint(sut, gol, &lowered)
    }

    fn mint_wtbr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        list: &[Hoon],
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let lowered = Self::lower_wtbr(list);
        self.mint(sut, gol, &lowered)
    }

    fn mint_wtpt(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        q: &Hoon,
        r: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let expanded = expand_wutpat(wing, q, r);
        self.mint(sut, gol, &expanded)
    }

    fn mint_wtsg(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        q: &Hoon,
        r: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let expanded = expand_wutsig(wing, q, r);
        self.mint(sut, gol, &expanded)
    }

    fn mint_wtgl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Match hoon-138 open() lowering: wtgl -> wtcl(p, zpzp, q)
        let expanded = Hoon::WutCol(
            Box::new((*p).clone()),
            Box::new(Hoon::ZapZap),
            Box::new((*q).clone()),
        );
        self.mint(sut, gol, &expanded)
    }

    fn mint_wtgr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Match hoon-138 open() lowering: wtgr -> wtcl(p, q, zpzp)
        let expanded = Hoon::WutCol(
            Box::new((*p).clone()),
            Box::new((*q).clone()),
            Box::new(Hoon::ZapZap),
        );
        self.mint(sut, gol, &expanded)
    }

    fn mint_fits(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        wing: &WingType,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let ref_type = self.play(sut.clone(), p)?;
        let port = self.find(sut.clone(), Way::Read, wing)?;
        let formula = match &port {
            // Match hoon-138 `%fits`: when `find` returns a leg, fish directly at that axis
            // rather than composing through `%7`.
            Port::Palo(palo) if matches!(palo.opal, Opal::Leg(_)) => {
                let axis = tend_big(&palo.vein)?;
                self.type_test_formula_on_axis(ref_type.clone(), axis)?
            }
            _ => {
                let (_ty, base_formula) = self.fine(&port)?;
                let test = self.type_test_formula_on_axis(ref_type.clone(), 1u64)?;
                // hoon-138 emits an explicit `%7` in this branch.
                self.formula_op(NockOpcode::COMPOSE, &[base_formula, test])
            }
        };
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let ty = self.nice(sut, gol, bool_ty)?;
        Ok((ty, formula))
    }

    fn mint_wthx(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        skin: &Skin,
        wing: &WingType,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // The skin matchers take noun types, so the hit type and subject are lowered.
        let (hit_ty, axis) = self.fend(sut.clone(), Way::Read, wing)?;
        let hit_ty = live_to_noun(&mut self.cx, &hit_ty, self.slab);
        let static_match = self.skin_match_static(hit_ty, skin)?;
        let formula = if let Some(matches) = static_match {
            self.formula_quote(D(if matches { 0 } else { 1 }))
        } else {
            let sut_noun = live_to_noun(&mut self.cx, &sut, self.slab);
            self.skin_test_formula(sut_noun, axis, skin)?
        };
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let ty = self.nice(sut, gol, bool_ty)?;
        Ok((ty, formula))
    }

    fn base_match_static(&mut self, ty: Noun, base: &BaseType) -> Result<Option<bool>> {
        match base {
            BaseType::NounExpr => Ok(Some(true)),
            BaseType::Void => Ok(Some(false)),
            BaseType::Cell => {
                let head = Skin::Base(BaseType::NounExpr);
                let tail = Skin::Base(BaseType::NounExpr);
                self.cell_skin_match_static(ty, &head, &tail)
            }
            BaseType::Flag => {
                let bool_ty = ty_bool(self.slab);
                if self.nest_noun(bool_ty, ty)? {
                    return Ok(Some(true));
                }
                let head = ty_noun(self.slab);
                let tail = ty_noun(self.slab);
                let cell_ty = ty_cell(self.slab, head, tail);
                if self.nest_noun(cell_ty, ty)? {
                    return Ok(Some(false));
                }
                Ok(None)
            }
            BaseType::Atom(_) => {
                let atom_ty = ty_atom(self.slab, "$", None);
                if self.nest_noun(atom_ty, ty)? {
                    return Ok(Some(true));
                }
                let head = ty_noun(self.slab);
                let tail = ty_noun(self.slab);
                let cell_ty = ty_cell(self.slab, head, tail);
                if self.nest_noun(cell_ty, ty)? {
                    return Ok(Some(false));
                }
                Ok(None)
            }
            BaseType::Null => {
                let exact = ty_atom(self.slab, "$", Some(D(0)));
                if self.nest_noun(exact, ty)? {
                    Ok(Some(true))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn cell_skin_match_static(
        &mut self,
        ty: Noun,
        head: &Skin,
        tail: &Skin,
    ) -> Result<Option<bool>> {
        let atom_ty = ty_atom(self.slab, "$", None);
        if self.nest_noun(atom_ty, ty)? {
            return Ok(Some(false));
        }

        let cell_head = ty_noun(self.slab);
        let cell_tail = ty_noun(self.slab);
        let cell_ty = ty_cell(self.slab, cell_head, cell_tail);
        let known_cell = self.nest_noun(cell_ty, ty)?;

        let head_ty = self.peek_noun(ty, Way::Free, 2u64)?;
        let tail_ty = self.peek_noun(ty, Way::Free, 3u64)?;
        let head_match = self.skin_match_static(head_ty, head)?;
        let tail_match = self.skin_match_static(tail_ty, tail)?;

        if known_cell {
            Ok(match (head_match, tail_match) {
                (Some(h), Some(t)) => Some(h && t),
                (Some(false), _) | (_, Some(false)) => Some(false),
                _ => None,
            })
        } else if matches!(head_match, Some(false)) || matches!(tail_match, Some(false)) {
            Ok(Some(false))
        } else {
            Ok(None)
        }
    }

    fn skin_match_static(&mut self, ty: Noun, skin: &Skin) -> Result<Option<bool>> {
        #[cfg(test)]
        {
            self.skin_match_static_calls = self.skin_match_static_calls.saturating_add(1);
        }
        match skin {
            Skin::Dbug(_, inner) => self.skin_match_static(ty, inner),
            Skin::Help(_, inner) => self.skin_match_static(ty, inner),
            Skin::Name(_, inner) => self.skin_match_static(ty, inner),
            Skin::Base(base) => self.base_match_static(ty, base),
            Skin::Leaf(_aura, atom) => {
                let value = parsed_atom_to_noun(self.slab, atom);
                let exact = ty_atom(self.slab, "$", Some(value));
                if self.nest_noun(exact, ty)? {
                    Ok(Some(true))
                } else {
                    Ok(None)
                }
            }
            Skin::Cell(head, tail) => self.cell_skin_match_static(ty, head.as_ref(), tail.as_ref()),
            _ => Ok(None),
        }
    }

    fn mint_wtts(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        wing: &WingType,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // hoon-138 mint expansion:
        //   [%wtts p=spec q=wing] => [%fits ~(example ax p) q]
        let example = self.spec_example_cached(spec);
        self.mint_fits(sut, gol, example.as_ref(), wing)
    }

    fn base_test_formula(&mut self, base: &BaseType, slot: FormulaId) -> Result<FormulaId> {
        match base {
            BaseType::NounExpr => Ok(self.formula_quote(D(0))),
            BaseType::Void => Ok(self.formula_quote(D(1))),
            BaseType::Cell => Ok(self.formula_op(NockOpcode::CELL, &[slot])),
            BaseType::Atom(_) => {
                let test = self.formula_op(NockOpcode::CELL, &[slot]);
                let false_formula = self.formula_quote(D(1));
                let true_formula = self.formula_quote(D(0));
                Ok(self.formula_cond(test, false_formula, true_formula))
            }
            BaseType::Null => {
                let zero = self.formula_quote(D(0));
                Ok(self.formula_op(NockOpcode::EQUAL, &[zero, slot]))
            }
            BaseType::Flag => {
                let zero = self.formula_quote(D(0));
                let one = self.formula_quote(D(1));
                let eq_zero = self.formula_op(NockOpcode::EQUAL, &[slot, zero]);
                let eq_one = self.formula_op(NockOpcode::EQUAL, &[slot, one]);
                let atom_test = self.base_test_formula(&BaseType::Atom("$".to_string()), slot)?;
                let flag_value_test = self.formula_flor(eq_zero, eq_one);
                Ok(self.formula_flan(atom_test, flag_value_test))
            }
        }
    }

    fn type_test_formula_on_axis<A: Into<BigUint>>(
        &mut self,
        typ: NRc<NTy>,
        axis: A,
    ) -> Result<FormulaId> {
        let axis = axis.into();
        if let Some(cached) = self.fish_boundary_lookup(&typ, &axis)? {
            return Ok(cached);
        }
        let mut seen_holds: Vec<NRc<NTy>> = Vec::new();
        let result =
            self.type_test_formula_on_axis_inner(typ.clone(), axis.clone(), &mut seen_holds)?;
        self.fish_boundary_store(&typ, &axis, result)?;
        Ok(result)
    }

    fn type_test_formula_on_axis_inner(
        &mut self,
        typ: NRc<NTy>,
        axis: BigUint,
        seen_holds: &mut Vec<NRc<NTy>>,
    ) -> Result<FormulaId> {
        match &*typ {
            NTy::Void => Ok(self.formula_quote(D(1))),
            NTy::Noun => Ok(self.formula_quote(D(0))),
            NTy::Atom { .. } => {
                // The atom value is a carried leaf; lower the type and decode it
                // with the noun helper.
                let typ_noun = live_to_noun(&mut self.cx, &typ, self.slab);
                let (_aura, value) = type_atom_parts(typ_noun, &self.slab.noun_space())?;
                let slot = self.formula_slot(axis);
                if let Some(value) = value {
                    let const_value = self.formula_quote(value);
                    Ok(self.formula_op(NockOpcode::EQUAL, &[const_value, slot]))
                } else {
                    // Canonical ++fish: unknown atom narrows via flip([3 [0 axis]]).
                    let is_cell = self.formula_op(NockOpcode::CELL, &[slot]);
                    Ok(self.formula_flip(is_cell))
                }
            }
            NTy::Cell(head, tail) => {
                let head = head.clone();
                let tail = tail.clone();
                let slot = self.formula_slot(axis.clone());
                let is_cell = self.formula_op(NockOpcode::CELL, &[slot]);
                let head_formula = self.type_test_formula_on_axis_inner(
                    head,
                    peg_axis_big(axis.clone(), 2)?,
                    seen_holds,
                )?;
                let tail_formula =
                    self.type_test_formula_on_axis_inner(tail, peg_axis_big(axis, 3)?, seen_holds)?;
                let both = self.formula_flan(head_formula, tail_formula);
                Ok(self.formula_flan(is_cell, both))
            }
            NTy::Core { .. } => Err(CompilerError::Noun("fish-core".to_string())),
            NTy::Face { inner, .. } => {
                let inner = inner.clone();
                self.type_test_formula_on_axis_inner(inner, axis, seen_holds)
            }
            NTy::Hint { payload, .. } => {
                let inner = payload.clone();
                self.type_test_formula_on_axis_inner(inner, axis, seen_holds)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&typ)?;
                self.type_test_formula_on_axis_fork(&options, axis, seen_holds)
            }
            NTy::Hold { .. } => {
                for prior in seen_holds.iter() {
                    if NRc::ptr_eq(prior, &typ) {
                        return Err(CompilerError::Noun("fish-loop".to_string()));
                    }
                }
                seen_holds.push(typ.clone());
                let repo = self.repo(typ.clone())?;
                let out = self.type_test_formula_on_axis_inner(repo, axis, seen_holds);
                seen_holds.pop();
                out
            }
        }
    }

    fn skin_test_formula(&mut self, sut: Noun, axis: BigUint, skin: &Skin) -> Result<FormulaId> {
        let ref_type = self.peek_noun(sut, Way::Free, axis.clone())?;
        if let Some(matches) = self.skin_match_static(ref_type, skin)? {
            return Ok(self.formula_quote(D(if matches { 0 } else { 1 })));
        }

        let slot = self.formula_slot(axis.clone());
        match skin {
            Skin::Base(BaseType::Flag) => {
                let atom_skin = Skin::Base(BaseType::Atom("$".to_string()));
                let atom_test = self.skin_test_formula(sut, axis.clone(), &atom_skin)?;
                let zero = self.formula_quote(D(0));
                let one = self.formula_quote(D(1));
                let eq_zero = self.formula_op(NockOpcode::EQUAL, &[slot, zero]);
                let eq_one = self.formula_op(NockOpcode::EQUAL, &[slot, one]);
                let flag_value_test = self.formula_flor(eq_zero, eq_one);
                Ok(self.formula_flan(atom_test, flag_value_test))
            }
            Skin::Base(base) => self.base_test_formula(base, slot),
            Skin::Leaf(_aura, atom) => {
                let value = parsed_atom_to_noun(self.slab, atom);
                let const_val = self.formula_quote(value);
                Ok(self.formula_op(NockOpcode::EQUAL, &[const_val, slot]))
            }
            Skin::Cell(head, tail) => {
                let is_cell = self.formula_op(NockOpcode::CELL, &[slot]);
                let head_axis = peg_axis_big(axis.clone(), 2)?;
                let tail_axis = peg_axis_big(axis, 3)?;
                let head_test = self.skin_test_formula(sut, head_axis, head)?;
                let tail_test = self.skin_test_formula(sut, tail_axis, tail)?;
                let both = self.formula_arena.and(head_test, tail_test);
                let false_formula = self.formula_quote(D(1));
                Ok(self.formula_cond(is_cell, both, false_formula))
            }
            Skin::Dbug(_, inner) => self.skin_test_formula(sut, axis, inner),
            Skin::Help(_, inner) => self.skin_test_formula(sut, axis, inner),
            Skin::Name(_, inner) => self.skin_test_formula(sut, axis, inner),
            Skin::Over(wing, inner) => {
                let rel_axis = self.resolve_wing_axis_noun(sut, wing)?;
                let axis = peg_axis_big_pair(axis, &rel_axis)?;
                self.skin_test_formula(sut, axis, inner)
            }
            Skin::Spec(spec, inner) => {
                let ref_type = self.peek_noun(sut, Way::Free, axis.clone())?;
                let example = self.spec_example_cached(spec);
                let hit = self.play_noun(sut, example.as_ref())?;
                if !self.nest_noun(hit, ref_type)? {
                    return Err(CompilerError::Noun("native mint: wthx spec".to_string()));
                }
                self.skin_test_formula(sut, axis, inner)
            }
            Skin::Wash(_) => Ok(self.formula_quote(D(0))),
            Skin::Term(name) => {
                // Canonical `ar` treats an atomic skin as a `%spec` skin whose
                // spec is a like-reference to that term and whose inner skin is
                // `%noun`.  Do not resolve the term directly with `find`: in a
                // gate such as `|=  a=pair  ?#(pair a)`, direct lookup can see
                // the sample/core namespace rather than the mold spec path and
                // incorrectly reject a statically valid test.
                let wing = vec![Limb::Term(name.clone())];
                let spec = Spec::Like(wing, Vec::new());
                let converted =
                    Skin::Spec(Box::new(spec), Box::new(Skin::Base(BaseType::NounExpr)));
                self.skin_test_formula(sut, axis, &converted)
            }
        }
    }

    fn type_test_formula_on_axis_fork(
        &mut self,
        options: &[NRc<NTy>],
        axis: BigUint,
        seen_holds: &mut Vec<NRc<NTy>>,
    ) -> Result<FormulaId> {
        let Some((head, tail)) = options.split_first() else {
            return Ok(self.formula_quote(D(1)));
        };
        let head_formula =
            self.type_test_formula_on_axis_inner(head.clone(), axis.clone(), seen_holds)?;
        let tail_formula = self.type_test_formula_on_axis_fork(tail, axis, seen_holds)?;
        Ok(self.formula_flor(head_formula, tail_formula))
    }

    fn mint_wtls(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        default: &Hoon,
        list: &[(Spec, Hoon)],
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Match hoon-138 open() lowering:
        //   [%wtls p=wing q=default r=list] => [%wthp p (weld r [[[%base %noun] q] ~])]
        let expanded = expand_wutlus(wing, default, list);
        self.mint(sut, gol, &expanded)
    }

    // `?-`: switch on the type of a wing.
    fn mint_wthp(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        list: &[(Spec, Hoon)],
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Match hoon-138 open() lowering:
        //   ?~ q.gen [%lost [%wing p.gen]] :^ %wtcl [%wtts p.i.q.gen p.gen] q.i.q.gen $(q.gen t.q.gen)
        let expanded = expand_wuthep(wing, list);
        self.mint(sut, gol, &expanded)
    }

    fn mint_ktsl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let played = self.play(sut.clone(), p)?;
        let hif = self.nice(sut.clone(), gol, played)?;
        let (_q_ty, q_formula) = self.mint(sut, hif.clone(), q)?;
        Ok((hif, q_formula))
    }

    fn mint_kthp(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Canonical hoon-138 open() lowering:
        //   [%kthp p q] => [%ktls ~(example ax p) q]
        let example = self.spec_example_cached(spec);
        let hif = self.play(sut.clone(), example.as_ref())?;
        let hif = self.nice(sut.clone(), gol, hif)?;
        let (_q_ty, q_formula) = self.mint(sut, hif.clone(), q)?;
        Ok((hif, q_formula))
    }

    fn mint_kttr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Canonical hoon-138 open() lowering:
        //   [%kttr p] => [%ktsg ~(example ax p)]
        let example = self.spec_example_cached(spec);
        self.mint_ktsg(sut, gol, example.as_ref())
    }

    fn mint_ktcl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let opened = self.spec_factory_open_cached(spec);
        self.mint(sut, gol, opened.as_ref())
    }

    fn mint_ktsg(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        self.blow_ktsg(sut, gol, p)
    }

    fn blow_ktsg(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        gen: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Canonical hoon-138:
        //   pro=(mint gol gen)
        //   jon=(apex:musk bran q.pro)
        //   if jon is ~ or %wait => keep q.pro
        //   else rewrite to %1 noun
        let (ty, formula) = self.mint(sut.clone(), gol, gen)?;
        let bran = self.bran_canonical_semi(sut)?;
        let fold_key = FoldKey {
            subject: bran,
            formula,
        };
        if let Some(cached) = self.ktsg_fold_cache.get(&fold_key).copied() {
            return Ok(match cached {
                Some(noun) => (ty, self.formula_quote(noun)),
                None => (ty, formula),
            });
        }
        let formula_noun = self.formula_materialize(formula);
        let jon = self.musk_apex_output(bran, formula_noun)?;
        let collapsed = match jon {
            MuskOutput::Done(noun) => Some(noun),
            MuskOutput::Stop | MuskOutput::Wait => None,
        };
        self.ktsg_fold_cache.insert(fold_key, collapsed);
        if let Some(noun) = collapsed {
            return Ok((ty, self.formula_quote(noun)));
        }
        Ok((ty, formula))
    }

    #[inline]
    fn value_import(&mut self, noun: Noun) -> ValueId {
        let space = self.slab.noun_space();
        self.value_arena
            .import(noun, &space)
            .expect("compiler-generated seminoun data must be a valid noun")
    }

    #[inline]
    fn semi_full_complete(&mut self, data: Noun) -> SemiId {
        let value = self.value_import(data);
        self.semi_arena.complete(value)
    }

    #[inline]
    fn semi_full_complete_with_id(&mut self, value: ValueId) -> SemiId {
        self.semi_arena.complete(value)
    }

    #[inline]
    fn semi_full_blocked(&mut self) -> SemiId {
        self.semi_arena.blocked()
    }

    fn semi_blocks_root_blocked(&mut self) -> Noun {
        if let Some(blocks) = self.semi_root_blocked_set {
            return blocks;
        }
        let blocks = T(self.slab, &[D(0), D(0), D(0)]);
        self.semi_root_blocked_set = Some(blocks);
        blocks
    }

    fn semi_noun_blocked_encoded(&mut self) -> Noun {
        // hoon-138 `*seminoun` and `++complete` use the singleton root
        // block set `[~ ~ ~]` for a fully blocked noun.
        if let Some(semi) = self.semi_full_blocked_interned {
            return semi;
        }
        let blocks = self.semi_blocks_root_blocked();
        let mask = T(self.slab, &[D(SEMI_TAG_FULL), blocks]);
        let semi = self.semi_make(mask, D(0));
        self.semi_full_blocked_interned = Some(semi);
        semi
    }

    fn semi_noun_lazy_root(&mut self, resolver_id: LazyResolverId) -> Noun {
        let resolve = noun_u64(self.slab, resolver_id.0);
        let frag = noun_biguint(self.slab, BigUint::from(1u32));
        let mask = T(self.slab, &[D(SEMI_TAG_LAZY), frag, resolve]);
        self.semi_make(mask, D(0))
    }

    #[inline]
    fn semi_make(&mut self, mask: Noun, data: Noun) -> Noun {
        T(self.slab, &[mask, data])
    }

    fn lazy_resolver_new_id(&mut self) -> LazyResolverId {
        let id = self.lazy_resolver_next_id;
        self.lazy_resolver_next_id = LazyResolverId(self.lazy_resolver_next_id.0.wrapping_add(1));
        if self.lazy_resolver_next_id == LazyResolverId(0) {
            self.lazy_resolver_next_id = LazyResolverId(1);
        }
        id
    }

    /// Returns one stable resolver ID per structurally equal recursive core, so
    /// `cons_core` interns their lazy forms to a single `Rc` and the pointer-keyed
    /// recursion cuts (mint cache, `arm_goal_for_hoon_in_progress`, fond
    /// `hold_path`, fish, bran) converge. A lazy core is identified by
    /// `(sut, tomes_sig, poly)`: `garb` is a function of prefix and poly, and
    /// `tomes_sig` already folds in `prefix_signature`. The resolver depends only
    /// on the core type, poly, and arms (the lazy callback goal is always `%noun`,
    /// `vet` is read at resolve time, and `gol` is not captured). With a fresh ID
    /// per call, equal lazy cores would be pointer-distinct and the cuts would fire
    /// only at the redo-gil backstop depth. Completed cores carry an ID-free
    /// `[%full ~]` semi, so reusing an ID never changes emitted bytes. `sut` is
    /// keyed by its arena ID, which is stable for the whole compile.
    fn lazy_resolver_canonical_id(
        &mut self,
        sut: &NRc<NTy>,
        tomes_sig: TomesSignature,
        poly: Poly,
    ) -> LazyResolverId {
        let poly_key = PolyKey::from(poly);
        let key = LazyCoreKey {
            subject: sut.arena_id(),
            tomes: tomes_sig,
            poly: poly_key,
        };
        if let Some(&id) = self.lazy_resolver_canonical_ids.get(&key) {
            return id;
        }
        let id = self.lazy_resolver_new_id();
        self.lazy_resolver_canonical_ids.insert(key, id);
        id
    }

    fn lazy_resolver_register_context(
        &mut self,
        resolver_id: LazyResolverId,
        core_type: NRc<NTy>,
        poly: Poly,
        arms_by_axis: HashMap<BigUint, LazyResolverArmEntry>,
    ) {
        // Lazy resolvers live for the whole compile and resolve on demand by
        // re-minting an arm against `core_type`, possibly from another core's arm.
        // `core_type` is a heap `Rc` and the arm AST nouns live in the compile's
        // single slab, so nothing needs relocation here.
        self.lazy_resolvers.insert(
            resolver_id,
            LazyResolverContext {
                core_type,
                poly,
                arms_by_axis,
                cached_formula_by_axis: HashMap::new(),
                in_progress_axes: HashSet::new(),
            },
        );
    }

    fn lazy_resolver_resolve_axis(
        &mut self,
        resolver_id: LazyResolverId,
        fragment: &BigUint,
    ) -> Result<Option<FormulaId>> {
        // hoon-138 `++laze`: the resolver answers exact arm axes only
        // (`(~(get by tal) axe)`). Any other fragment, including 1 (the whole
        // battery), produces `~`, which the caller treats as blocked.
        // Completed cores never reach here: `mint_core` stores a
        // materialized `[[%full ~] battery]` semi in the result type.
        if let Some(ctx) = self.lazy_resolvers.get(&resolver_id) {
            if let Some(cached) = ctx.cached_formula_by_axis.get(fragment) {
                return Ok(Some(*cached));
            }
            if ctx.arms_by_axis.contains_key(fragment) {
                return self.lazy_resolver_compile_arm(resolver_id, fragment.clone());
            }
        }
        Ok(None)
    }

    fn lazy_resolver_compile_arm(
        &mut self,
        resolver_id: LazyResolverId,
        fragment: BigUint,
    ) -> Result<Option<FormulaId>> {
        let Some((core_type, poly, arm_entry)) = ({
            let Some(ctx) = self.lazy_resolvers.get_mut(&resolver_id) else {
                return Ok(None);
            };
            if let Some(cached) = ctx.cached_formula_by_axis.get(&fragment) {
                return Ok(Some(*cached));
            }
            if !ctx.in_progress_axes.insert(fragment.clone()) {
                return Ok(None);
            }
            Some((
                ctx.core_type.clone(),
                ctx.poly,
                ctx.arms_by_axis.get(&fragment).cloned(),
            ))
        }) else {
            return Ok(None);
        };
        let Some(arm_entry) = arm_entry else {
            if let Some(ctx) = self.lazy_resolvers.get_mut(&resolver_id) {
                ctx.in_progress_axes.remove(&fragment);
            }
            return Ok(None);
        };
        let lazy_goal = cons_noun(&mut self.cx);
        let effective_vet = if poly == Poly::Wet { false } else { self.vet };
        // A true compile cycle (a fold inside an arm requesting that same
        // arm's formula) declines; hoon-138 cannot compile such code either
        // (its pure `++laze` resolver would recurse forever). Sibling and
        // already-finished arms always resolve, matching hoonc.
        if self
            .arm_goal_for_hoon_in_progress(
                core_type.clone(),
                arm_entry.hoon_noun,
                lazy_goal.clone(),
                effective_vet,
            )?
            .is_some()
        {
            if let Some(ctx) = self.lazy_resolvers.get_mut(&resolver_id) {
                ctx.in_progress_axes.remove(&fragment);
            }
            return Ok(None);
        }
        let hoon = self
            .hoon_ast_lookup_result(arm_entry.hoon_noun)
            .map_err(|err| {
                CompilerError::Noun(format!("native mint: lazy resolver arm ast missing: {err}"))
            })?;
        // Canonical hoon-138 `++laze` callback compiles via `hemp` with `gol=%noun`.
        let compiled = self.build_arm_formula_direct(
            Arc::clone(&arm_entry.arm_name),
            core_type,
            poly,
            lazy_goal,
            hoon.as_ref(),
            arm_entry.hoon_noun,
        );
        if let Some(ctx) = self.lazy_resolvers.get_mut(&resolver_id) {
            ctx.in_progress_axes.remove(&fragment);
        }
        match compiled {
            Ok(formula) => {
                // Cache the formula for the whole compile. Other cores read it, and
                // re-minting on a later miss would run in the caller's %hold/fan
                // scope instead of this arm's and could produce a wrong type.
                // Caching keeps the resolver a pure lookup.
                if let Some(ctx) = self.lazy_resolvers.get_mut(&resolver_id) {
                    ctx.cached_formula_by_axis.insert(fragment, formula);
                }
                Ok(Some(formula))
            }
            Err(err) => Err(err),
        }
    }

    fn semi_import_noun(&mut self, semi: Noun) -> Result<SemiId> {
        let raw = NounIdentity::of(semi);
        if let Some(id) = self.semi_arena.raw_lookup(raw) {
            return Ok(id);
        }
        let (mask, data) = {
            let space = self.slab.noun_space();
            let cell = semi
                .in_space(&space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("seminoun not cell: {err}")))?;
            (cell.head().noun(), cell.tail().noun())
        };
        let id = self.semi_import_parts(mask, data)?;
        self.semi_arena.raw_register(raw, id);
        Ok(id)
    }

    fn semi_import_parts(&mut self, mask: Noun, data: Noun) -> Result<SemiId> {
        let (tag, tail) = {
            let space = self.slab.noun_space();
            let mask_cell = mask
                .in_space(&space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("semi mask not cell: {err}")))?;
            let tag = mask_cell
                .head()
                .as_atom()
                .ok()
                .and_then(|atom| atom.as_u64().ok());
            (tag, mask_cell.tail().noun())
        };
        match tag {
            Some(SEMI_TAG_FULL) => {
                if noun_is_zero(tail) {
                    Ok(self.semi_full_complete(data))
                } else {
                    Ok(self.semi_full_blocked())
                }
            }
            Some(SEMI_TAG_HALF) => {
                let (left_mask, right_mask, head_data, tail_data) = {
                    let space = self.slab.noun_space();
                    let masks = tail.in_space(&space).as_cell().map_err(|err| {
                        CompilerError::Decode(format!("semi half mask tail not cell: {err}"))
                    })?;
                    let values = data.in_space(&space).as_cell().map_err(|err| {
                        CompilerError::Decode(format!("semi half data not cell: {err}"))
                    })?;
                    (
                        masks.head().noun(),
                        masks.tail().noun(),
                        values.head().noun(),
                        values.tail().noun(),
                    )
                };
                let head = self.semi_import_parts(left_mask, head_data)?;
                let tail = self.semi_import_parts(right_mask, tail_data)?;
                self.semi_combine(head, tail)
            }
            Some(SEMI_TAG_LAZY) => {
                let (fragment, resolver_id) = {
                    let space = self.slab.noun_space();
                    let parts = tail.in_space(&space).as_cell().map_err(|err| {
                        CompilerError::Decode(format!("semi lazy mask tail not cell: {err}"))
                    })?;
                    let fragment = parts.head().as_atom().map_err(|err| {
                        CompilerError::Decode(format!("semi lazy fragment not atom: {err}"))
                    })?;
                    let resolver = parts.tail().as_atom().map_err(|err| {
                        CompilerError::Decode(format!("semi lazy resolver not atom: {err}"))
                    })?;
                    (
                        noun_axis_atom_to_big(fragment),
                        resolver.as_u64().map_err(|err| {
                            CompilerError::Decode(format!("semi lazy resolver not u64: {err}"))
                        })?,
                    )
                };
                Ok(self.semi_arena.lazy(fragment, LazyResolverId(resolver_id)))
            }
            _ => Ok(self.semi_full_blocked()),
        }
    }

    fn semi_combine(&mut self, head: SemiId, tail: SemiId) -> Result<SemiId> {
        let head_node = self.semi_arena.node(head).clone();
        let tail_node = self.semi_arena.node(tail).clone();
        match (head_node, tail_node) {
            (SemiNode::Complete(head_value), SemiNode::Complete(tail_value)) => {
                let data = T(
                    self.slab,
                    &[self.value_arena.noun(head_value), self.value_arena.noun(tail_value)],
                );
                let value = self
                    .value_arena
                    .intern_cell_with_noun(data, head_value, tail_value);
                Ok(self.semi_full_complete_with_id(value))
            }
            (SemiNode::Blocked, SemiNode::Blocked) => Ok(self.semi_full_blocked()),
            _ => Ok(self.semi_arena.half(head, tail)),
        }
    }

    fn semi_complete(&mut self, bus: SemiId) -> Result<SemiId> {
        match self.semi_arena.node(bus).clone() {
            SemiNode::Complete(_) | SemiNode::Blocked => Ok(bus),
            SemiNode::Lazy {
                fragment,
                resolver_id,
            } => {
                // hoon-138 `++complete`: fragment 1 is the whole in-progress
                // battery and therefore cannot be resolved while compiling it.
                if fragment == BigUint::from(1u32) {
                    return Ok(self.semi_full_blocked());
                }
                match self.lazy_resolver_resolve_axis(resolver_id, &fragment)? {
                    Some(value) => {
                        let value = self.formula_materialize(value);
                        Ok(self.semi_full_complete(value))
                    }
                    None => Ok(self.semi_full_blocked()),
                }
            }
            SemiNode::Half { head, tail } => {
                let head = self.semi_complete(head)?;
                let tail = self.semi_complete(tail)?;
                self.semi_combine(head, tail)
            }
        }
    }

    fn semi_fragment(&mut self, axis: u64, bus: SemiId) -> Result<Option<SemiId>> {
        if axis == 1 {
            return Ok(Some(bus));
        }
        if axis == 0 {
            return Ok(None);
        }
        let (cap, rest) = axis_cap_mas(axis)?;
        match self.semi_arena.node(bus).clone() {
            SemiNode::Complete(value) => {
                let Some((head, tail)) = self.value_arena.children(value) else {
                    return Ok(None);
                };
                let next = if cap == 2 { head } else { tail };
                let next = self.semi_full_complete_with_id(next);
                self.semi_fragment(rest, next)
            }
            SemiNode::Blocked => Ok(Some(bus)),
            SemiNode::Half { head, tail } => {
                self.semi_fragment(rest, if cap == 2 { head } else { tail })
            }
            SemiNode::Lazy {
                fragment,
                resolver_id,
            } => {
                let fragment = peg_axis_big_pair(fragment, &BigUint::from(axis))?;
                Ok(Some(self.semi_arena.lazy(fragment, resolver_id)))
            }
        }
    }

    fn semi_fragment_big(&mut self, axis: &BigUint, bus: SemiId) -> Result<Option<SemiId>> {
        if let Ok(small) = u64::try_from(axis) {
            return self.semi_fragment(small, bus);
        }
        if axis == &BigUint::from(0u32) {
            return Ok(None);
        }
        if axis == &BigUint::from(1u32) {
            return Ok(Some(bus));
        }
        let (cap, rest) = axis_big_cap_mas(axis)?;
        match self.semi_arena.node(bus).clone() {
            SemiNode::Complete(value) => {
                let Some((head, tail)) = self.value_arena.children(value) else {
                    return Ok(None);
                };
                let next = if cap == 2 { head } else { tail };
                let next = self.semi_full_complete_with_id(next);
                self.semi_fragment_big(&rest, next)
            }
            SemiNode::Blocked => Ok(Some(bus)),
            SemiNode::Half { head, tail } => {
                self.semi_fragment_big(&rest, if cap == 2 { head } else { tail })
            }
            SemiNode::Lazy {
                fragment,
                resolver_id,
            } => {
                let fragment = peg_axis_big_pair(fragment, axis)?;
                Ok(Some(self.semi_arena.lazy(fragment, resolver_id)))
            }
        }
    }

    fn semi_mutate(
        &mut self,
        axis: u64,
        replacement: SemiId,
        target: SemiId,
    ) -> Result<Option<SemiId>> {
        if axis == 0 {
            return Ok(None);
        }
        if axis == 1 {
            return Ok(Some(replacement));
        }
        if axis == 2 {
            let Some(tail) = self.semi_fragment(3, target)? else {
                return Ok(None);
            };
            return self.semi_combine(replacement, tail).map(Some);
        }
        if axis == 3 {
            let Some(head) = self.semi_fragment(2, target)? else {
                return Ok(None);
            };
            return self.semi_combine(head, replacement).map(Some);
        }
        let (cap, rest) = axis_cap_mas(axis)?;
        let Some(head) = self.semi_fragment(2, target)? else {
            return Ok(None);
        };
        let Some(tail) = self.semi_fragment(3, target)? else {
            return Ok(None);
        };
        if cap == 2 {
            let Some(mutated) = self.semi_mutate(rest, replacement, head)? else {
                return Ok(None);
            };
            return self.semi_combine(mutated, tail).map(Some);
        }
        let Some(mutated) = self.semi_mutate(rest, replacement, tail)? else {
            return Ok(None);
        };
        self.semi_combine(head, mutated).map(Some)
    }

    fn semi_mutate_big(
        &mut self,
        axis: &BigUint,
        replacement: SemiId,
        target: SemiId,
    ) -> Result<Option<SemiId>> {
        if let Ok(small) = u64::try_from(axis) {
            return self.semi_mutate(small, replacement, target);
        }
        if axis == &BigUint::from(0u32) {
            return Ok(None);
        }
        if axis == &BigUint::from(1u32) {
            return Ok(Some(replacement));
        }
        let (cap, rest) = axis_big_cap_mas(axis)?;
        let Some(head) = self.semi_fragment(2, target)? else {
            return Ok(None);
        };
        let Some(tail) = self.semi_fragment(3, target)? else {
            return Ok(None);
        };
        if cap == 2 {
            let Some(mutated) = self.semi_mutate_big(&rest, replacement, head)? else {
                return Ok(None);
            };
            return self.semi_combine(mutated, tail).map(Some);
        }
        let Some(mutated) = self.semi_mutate_big(&rest, replacement, tail)? else {
            return Ok(None);
        };
        self.semi_combine(head, mutated).map(Some)
    }

    fn semi_require<F>(&mut self, value: Option<SemiId>, then: F) -> Result<Option<SemiId>>
    where
        F: FnOnce(&mut Self, Noun) -> Result<Option<SemiId>>,
    {
        let Some(value) = value else {
            return Ok(None);
        };
        let complete = self.semi_complete(value)?;
        match self.semi_arena.node(complete).clone() {
            SemiNode::Complete(value) => then(self, self.value_arena.noun(value)),
            SemiNode::Blocked | SemiNode::Half { .. } | SemiNode::Lazy { .. } => {
                Ok(Some(self.semi_full_blocked()))
            }
        }
    }

    fn musk_araw(
        &mut self,
        bus: SemiId,
        fol: Noun,
        memo: &mut FastHashMap<ArawKey, SemiId>,
    ) -> Result<Option<SemiId>> {
        let key = ArawKey {
            subject: bus,
            formula: NounIdentity::of(fol),
        };
        if let Some(cached) = memo.get(&key) {
            return Ok(Some(*cached));
        }
        let result = self.musk_araw_uncached(bus, fol, memo);
        let result = result?;
        if let Some(noun) = result {
            memo.insert(key, noun);
        }
        Ok(result)
    }

    fn musk_araw_dynamic(
        &mut self,
        bus: SemiId,
        fol: Noun,
        memo: &mut FastHashMap<ArawKey, SemiId>,
    ) -> Result<Option<SemiId>> {
        let key = ArawKey {
            subject: bus,
            formula: NounIdentity::of(fol),
        };
        if let Some(cached) = memo.get(&key) {
            return Ok(Some(*cached));
        }
        // Literal formula descent is acyclic because nouns cannot contain a
        // reference to an ancestor. Only op 2 and partial op 9 jump to a
        // formula obtained at run time, so guard those edges rather than
        // checking every recursive step. Re-entering the same dynamic state
        // cannot make progress; for a `^~` fold it must be a compiler error,
        // not an excuse to leave a divergent assertion unfolded.
        if self.musk.araw_active.contains(&key) {
            return Err(CompilerError::Noun("musk-loop".to_string()));
        }
        self.musk.araw_active.push(key);
        let result = self.musk_araw(bus, fol, memo);
        let popped = self.musk.araw_active.pop();
        debug_assert_eq!(popped, Some(key));
        result
    }

    fn musk_araw_uncached(
        &mut self,
        bus: SemiId,
        fol: Noun,
        memo: &mut FastHashMap<ArawKey, SemiId>,
    ) -> Result<Option<SemiId>> {
        let space = self.slab.noun_space();
        let (head, tail) = {
            let fol_cell = match fol.in_space(&space).as_cell() {
                Ok(cell) => cell,
                Err(_) => return Ok(None),
            };
            (fol_cell.head().noun(), fol_cell.tail().noun())
        };

        if head.in_space(&space).as_cell().is_ok() {
            let Some(hed) = self.musk_araw(bus, head, memo)? else {
                return Ok(None);
            };
            let Some(tal) = self.musk_araw(bus, tail, memo)? else {
                return Ok(None);
            };
            return self.semi_combine(hed, tal).map(Some);
        }

        let op_atom = match head.in_space(&space).as_atom() {
            Ok(atom) => atom,
            Err(_) => return Ok(None),
        };
        let op = match op_atom.as_u64() {
            Ok(op) => op,
            Err(_) => return Ok(None),
        };

        match op {
            0 => {
                let axis_atom = match tail.in_space(&space).as_atom() {
                    Ok(atom) => atom,
                    Err(_) => return Ok(None),
                };
                match axis_atom.as_u64() {
                    Ok(axis) => self.semi_fragment(axis, bus),
                    Err(_) => {
                        let axis = noun_axis_atom_to_big(axis_atom);
                        self.semi_fragment_big(&axis, bus)
                    }
                }
            }
            1 => Ok(Some(self.semi_full_complete(tail))),
            2 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = args.head().noun();
                let c = args.tail().noun();
                let c_eval = self.musk_araw(bus, c, memo)?;
                self.semi_require(c_eval, |ut, ryf| {
                    let Some(lub) = ut.musk_araw(bus, b, memo)? else {
                        return Ok(None);
                    };
                    ut.musk_araw_dynamic(lub, ryf, memo)
                })
            }
            3 => {
                let tail_eval = self.musk_araw(bus, tail, memo)?;
                self.semi_require(tail_eval, |ut, fig| {
                    let val = if fig.as_cell().is_ok() { D(0) } else { D(1) };
                    Ok(Some(ut.semi_full_complete(val)))
                })
            }
            4 => {
                let tail_eval = self.musk_araw(bus, tail, memo)?;
                self.semi_require(tail_eval, |ut, fig| {
                    let fig_space = ut.slab.noun_space();
                    let atom = match fig.in_space(&fig_space).as_atom() {
                        Ok(atom) => atom,
                        Err(_) => return Ok(None),
                    };
                    if let Ok(val) = atom.as_u64() {
                        if let Some(inc) = val.checked_add(1) {
                            let val = Atom::new(ut.slab, inc).as_noun();
                            return Ok(Some(ut.semi_full_complete(val)));
                        }
                    }

                    let mut bytes = atom.as_ne_bytes().to_vec();
                    let mut carry = 1u8;
                    for byte in &mut bytes {
                        let (next, overflow) = byte.overflowing_add(carry);
                        *byte = next;
                        carry = u8::from(overflow);
                        if carry == 0 {
                            break;
                        }
                    }
                    if carry != 0 {
                        bytes.push(carry);
                    }
                    let val = Atom::from_bytes(ut.slab, &bytes).as_noun();
                    Ok(Some(ut.semi_full_complete(val)))
                })
            }
            5 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = args.head().noun();
                let c = args.tail().noun();
                let b_eval = self.musk_araw(bus, b, memo)?;
                self.semi_require(b_eval, |ut, hed| {
                    let c_eval = ut.musk_araw(bus, c, memo)?;
                    ut.semi_require(c_eval, |ut, tal| {
                        let eq = noun_eq(hed, tal, &ut.slab.noun_space())?;
                        let val = if eq { D(0) } else { D(1) };
                        Ok(Some(ut.semi_full_complete(val)))
                    })
                })
            }
            6 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = args.head().noun();
                let rest = match args.tail().as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let c = rest.head().noun();
                let d = rest.tail().noun();
                let b_eval = self.musk_araw(bus, b, memo)?;
                self.semi_require(b_eval, |ut, fig| {
                    if noun_eq(fig, D(0), &ut.slab.noun_space())? {
                        return ut.musk_araw(bus, c, memo);
                    }
                    if noun_eq(fig, D(1), &ut.slab.noun_space())? {
                        return ut.musk_araw(bus, d, memo);
                    }
                    Ok(None)
                })
            }
            7 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = args.head().noun();
                let c = args.tail().noun();
                let Some(one) = self.musk_araw(bus, b, memo)? else {
                    return Ok(None);
                };
                self.musk_araw(one, c, memo)
            }
            8 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = args.head().noun();
                let c = args.tail().noun();
                let Some(one) = self.musk_araw(bus, b, memo)? else {
                    return Ok(None);
                };
                let combined = self.semi_combine(one, bus)?;
                self.musk_araw(combined, c, memo)
            }
            9 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = args.head().noun();
                let c = args.tail().noun();
                let axis_atom = match b.in_space(&space).as_atom() {
                    Ok(atom) => atom,
                    Err(_) => return Ok(None),
                };
                let axis_small = axis_atom.as_u64().ok();
                let axis_big = if axis_small.is_none() {
                    Some(noun_axis_atom_to_big(axis_atom))
                } else {
                    None
                };
                let Some(one) = self.musk_araw(bus, c, memo)? else {
                    return Ok(None);
                };
                // hoon-138 op-9 macks only when the raw core semi is already
                // canonical-complete (`[[%full ~] *]`); partial cores take the
                // fragment/require path below, like `++araw`.
                if let Some((core, core_id)) = self.semi_full_complete_data(one) {
                    if let Some(axis) = axis_small {
                        let Some(value) = self.musk_mack_constant_core(core, core_id, axis)? else {
                            return self.musk_araw_arm_partial(one, axis, memo);
                        };
                        return Ok(Some(self.semi_full_complete(value)));
                    }

                    let Some(axis_big) = axis_big.as_ref() else {
                        return Ok(None);
                    };
                    let Some(value) =
                        self.musk_interpret_mack_axis_noun(core, axis_atom.as_noun().noun())?
                    else {
                        return self.musk_araw_arm_partial_big(one, axis_big, memo);
                    };
                    return Ok(Some(self.semi_full_complete(value)));
                }
                let frag = if let Some(axis) = axis_small {
                    self.semi_fragment(axis, one)?
                } else {
                    let Some(axis_big) = axis_big.as_ref() else {
                        return Ok(None);
                    };
                    self.semi_fragment_big(axis_big, one)?
                };
                let partial =
                    self.semi_require(frag, |ut, ryf| ut.musk_araw_dynamic(one, ryf, memo))?;
                Ok(partial)
            }
            10 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let bc = args.head().noun();
                let d = args.tail().noun();
                let bc_cell = match bc.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = bc_cell.head().noun();
                let c = bc_cell.tail().noun();
                let axis_atom = match b.in_space(&space).as_atom() {
                    Ok(atom) => atom,
                    Err(_) => return Ok(None),
                };
                let axis_small = axis_atom.as_u64().ok();
                let axis_big = if axis_small.is_none() {
                    Some(noun_axis_atom_to_big(axis_atom))
                } else {
                    None
                };
                let Some(tar) = self.musk_araw(bus, d, memo)? else {
                    return Ok(None);
                };
                let Some(inn) = self.musk_araw(bus, c, memo)? else {
                    return Ok(None);
                };
                match axis_small {
                    Some(axis) => self.semi_mutate(axis, inn, tar),
                    None => {
                        let Some(axis_big) = axis_big.as_ref() else {
                            return Ok(None);
                        };
                        self.semi_mutate_big(axis_big, inn, tar)
                    }
                }
            }
            11 => {
                let args = match tail.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let b = args.head().noun();
                let c = args.tail().noun();
                if b.in_space(&space).as_atom().is_ok() {
                    return self.musk_araw(bus, c, memo);
                }
                let b_cell = match b.in_space(&space).as_cell() {
                    Ok(cell) => cell,
                    Err(_) => return Ok(None),
                };
                let hint_expr = b_cell.tail().noun();
                let Some(_noy) = self.musk_araw(bus, hint_expr, memo)? else {
                    return Ok(None);
                };
                self.musk_araw(bus, c, memo)
            }
            _ => Ok(None),
        }
    }

    fn musk_araw_arm_partial(
        &mut self,
        one: SemiId,
        axis: u64,
        memo: &mut FastHashMap<ArawKey, SemiId>,
    ) -> Result<Option<SemiId>> {
        let frag = self.semi_fragment(axis, one)?;
        let partial = self.semi_require(frag, |ut, ryf| ut.musk_araw_dynamic(one, ryf, memo))?;
        Ok(partial)
    }

    fn musk_araw_arm_partial_big(
        &mut self,
        one: SemiId,
        axis: &BigUint,
        memo: &mut FastHashMap<ArawKey, SemiId>,
    ) -> Result<Option<SemiId>> {
        let frag = self.semi_fragment_big(axis, one)?;
        self.semi_require(frag, |ut, ryf| ut.musk_araw_dynamic(one, ryf, memo))
    }

    #[cfg(test)]
    fn semi_complete_value_id(&self, semi: SemiId) -> Result<ValueId> {
        match self.semi_arena.node(semi) {
            SemiNode::Complete(value) => Ok(*value),
            _ => Err(CompilerError::Noun(
                "value identity requested for incomplete seminoun".into(),
            )),
        }
    }

    fn semi_full_complete_data(&self, semi: SemiId) -> Option<(Noun, ValueId)> {
        match self.semi_arena.node(semi) {
            SemiNode::Complete(value) => Some((self.value_arena.noun(*value), *value)),
            _ => None,
        }
    }

    fn musk_mack_constant_core(
        &mut self,
        core: Noun,
        core_id: ValueId,
        axis: u64,
    ) -> Result<Option<Noun>> {
        if Self::slot_axis(core, axis, &self.slab.noun_space()).is_none() {
            return Ok(None);
        }

        let key = MackKey {
            core: core_id,
            axis: SmallAxis(axis),
        };
        if let Some(result) = self.musk.mack_cache_by_value.get(&key).copied() {
            return Ok(result);
        }

        let result = self.musk_interpret_mack(core, axis);
        self.musk.mack_cache_by_value.insert(key, result);
        Ok(result)
    }

    fn musk_interpret_mack(&mut self, core: Noun, axis: u64) -> Option<Noun> {
        let slab: *mut NounSlab = &mut *self.slab;
        let slab_space = self.slab.noun_space();
        let context = &mut self.musk.context as *mut NockContext;
        unsafe {
            let context = &mut *context;
            let core = self.musk_mack_cached_core_in_context(context, core, &slab_space)?;
            Self::musk_interpret_mack_in_context(context, slab, core, axis)
        }
    }

    unsafe fn musk_interpret_mack_in_context(
        context: &mut NockContext,
        slab: *mut NounSlab,
        core: Noun,
        axis: u64,
    ) -> Option<Noun> {
        let snapshot = context.save();
        let stack_checkpoint = context.stack.checkpoint();
        // Interpretation can exhaust the eval NockStack (e.g. an unjetted
        // divergent loop allocating per step). hoonc's mack treats
        // infeasible folds as failures rather than crashing the compiler,
        // so catch the allocation panic, restore the stack to its pre-call
        // position, and report fold failure.
        //
        // On success, `with_stack_frame` preserves cold/warm/cache out of the
        // frame instead of rolling them back. Jet registrations and the
        // pointer unifications from their battery comparisons are pure and
        // monotone; discarding them would make every later mack redo full
        // structural equality walks against the jet state, which is
        // quadratic and dominates fold-heavy kernel compiles.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            context.with_stack_frame(0, |context| {
                let axis_noun = Atom::new(&mut context.stack, axis).as_noun();
                let slot_one = T(&mut context.stack, &[D(0), D(1)]);
                let formula = T(&mut context.stack, &[D(9), axis_noun, slot_one]);
                match interpret(context, core, formula) {
                    Ok(value) => T(&mut context.stack, &[D(0), value]),
                    Err(_) => D(0),
                }
            })
        }));
        match outcome {
            Ok(result) => {
                let source_space = context.stack.noun_space();
                let Ok(cell) = result.in_space(&source_space).as_cell() else {
                    return None;
                };
                Some((*slab).copy_into(cell.tail().noun(), &source_space))
            }
            Err(_) => {
                unsafe { context.stack.restore_checkpoint(&stack_checkpoint) };
                context.restore(&snapshot);
                None
            }
        }
    }

    fn musk_interpret_mack_axis_noun(&mut self, core: Noun, axis: Noun) -> Result<Option<Noun>> {
        let slab: *mut NounSlab = &mut *self.slab;
        let slab_space = self.slab.noun_space();
        let context = &mut self.musk.context as *mut NockContext;
        unsafe {
            let context = &mut *context;
            let Some(core) = self.musk_mack_cached_core_in_context(context, core, &slab_space)
            else {
                return Ok(None);
            };
            Self::musk_interpret_mack_axis_noun_in_context(context, slab, core, axis, &slab_space)
        }
    }

    unsafe fn musk_interpret_mack_axis_noun_in_context(
        context: &mut NockContext,
        slab: *mut NounSlab,
        core: Noun,
        axis: Noun,
        axis_space: &NounSpace,
    ) -> Result<Option<Noun>> {
        let snapshot = context.save();
        let stack_checkpoint = context.stack.checkpoint();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            context.with_stack_frame(0, |context| {
                let axis = context.stack.copy_into(axis, axis_space);
                let slot_one = T(&mut context.stack, &[D(0), D(1)]);
                let formula = T(&mut context.stack, &[D(9), axis, slot_one]);
                match interpret(context, core, formula) {
                    Ok(value) => T(&mut context.stack, &[D(0), value]),
                    Err(_) => D(0),
                }
            })
        }));
        match outcome {
            Ok(result) => {
                let source_space = context.stack.noun_space();
                let Ok(cell) = result.in_space(&source_space).as_cell() else {
                    return Ok(None);
                };
                Ok(Some((*slab).copy_into(cell.tail().noun(), &source_space)))
            }
            Err(_) => {
                unsafe { context.stack.restore_checkpoint(&stack_checkpoint) };
                context.restore(&snapshot);
                Ok(None)
            }
        }
    }

    fn musk_apex_output(&mut self, bus: SemiId, fol: Noun) -> Result<MuskOutput> {
        let mut memo: FastHashMap<ArawKey, SemiId> = Default::default();
        self.musk.araw_active.clear();
        let result = self.musk_araw(bus, fol, &mut memo);
        self.musk.araw_active.clear();
        let Some(noy) = result? else {
            return Ok(MuskOutput::Stop);
        };
        let complete = self.semi_complete(noy)?;
        match self.semi_arena.node(complete) {
            SemiNode::Complete(value) => Ok(MuskOutput::Done(self.value_arena.noun(*value))),
            _ => Ok(MuskOutput::Wait),
        }
    }

    // hoon-138 `++bran`: project a native type onto a seminoun. The %hold cycle
    // guard compares `Rc` pointers and the memo keys on the subject's arena ID.
    fn bran_canonical_semi(&mut self, sut: NRc<NTy>) -> Result<SemiId> {
        let mut seen_holds: Vec<NRc<NTy>> = Vec::new();
        self.bran_canonical_semi_inner(sut, &mut seen_holds)
    }

    fn bran_seen_holds_signature(seen_holds: &[NRc<NTy>]) -> SetSignature {
        let mut sum = 0u64;
        let mut xor = 0u64;
        for hold in seen_holds {
            let id = u64::from(native_type_id(hold).0);
            let component = id.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            sum = sum.wrapping_add(component);
            xor ^= component.rotate_left((id as u32) & 31);
        }
        SetSignature {
            sum,
            xor,
            len: seen_holds.len(),
        }
    }

    fn bran_semi_cache_key(&self, sut: &NRc<NTy>, seen_holds: &[NRc<NTy>]) -> BranSemiKey {
        let semantic = self.semantic_context_key();
        let seen = Self::bran_seen_holds_signature(seen_holds);
        BranSemiKey {
            subject: sut.arena_id(),
            vet: semantic.vet_key,
            fan: semantic.fan_context_key,
            seen,
        }
    }

    fn bran_seen_holds_equal(left: &[NRc<NTy>], right: &[NRc<NTy>]) -> bool {
        if left.len() != right.len() {
            return false;
        }
        left.iter()
            .zip(right.iter())
            .all(|(l, r)| NRc::ptr_eq(l, r))
    }

    fn bran_seen_holds_contains(seen_holds: &[NRc<NTy>], sut: &NRc<NTy>) -> bool {
        if !matches!(&**sut, NTy::Hold { .. }) {
            return false;
        }
        seen_holds.iter().any(|prior| NRc::ptr_eq(prior, sut))
    }

    fn bran_semi_cache_lookup(
        &mut self,
        sut: &NRc<NTy>,
        seen_holds: &[NRc<NTy>],
    ) -> Result<Option<SemiId>> {
        let key = self.bran_semi_cache_key(sut, seen_holds);
        let Some(entries) = self.bran_semi_memo.get(&key) else {
            return Ok(None);
        };
        for entry in entries.iter().rev() {
            if NRc::ptr_eq(&entry.sut, sut)
                && Self::bran_seen_holds_equal(&entry.seen_holds, seen_holds)
            {
                return Ok(Some(entry.semi));
            }
        }
        Ok(None)
    }

    fn bran_semi_cache_store(&mut self, sut: &NRc<NTy>, seen_holds: &[NRc<NTy>], semi: SemiId) {
        let key = self.bran_semi_cache_key(sut, seen_holds);
        let bucket = self
            .bran_semi_memo
            .ensure_key(key, Self::BRAN_SEMI_CACHE_KEY_LIMIT);
        if bucket.len() >= Self::BRAN_SEMI_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(BranSemiCacheEntry {
            sut: sut.clone(),
            seen_holds: seen_holds.to_vec(),
            semi,
        });
    }

    fn bran_canonical_semi_inner(
        &mut self,
        sut: NRc<NTy>,
        seen_holds: &mut Vec<NRc<NTy>>,
    ) -> Result<SemiId> {
        // Mirrors hoon-138 `++bran`: cycles break only at `%hold` (`seen_holds` is
        // hoon's `gil`), and every other shared subtree is re-descended and
        // memoized (`bran_semi_memo` is hoon's `~+`). A non-hold subtree met again
        // through a `%hold` expansion is re-descended and blocks only at the inner
        // hold, giving a partial seminoun. Blocking the whole subtree instead
        // yields the `[%full [~ ~ ~]]` bunt where hoonc computes `[%full ~]`.
        // This terminates because the `Rc` type DAG is acyclic and its only
        // logical cycles run through `%hold`, which `seen_holds` guards.
        if Self::bran_seen_holds_contains(seen_holds, &sut) {
            return Ok(self.semi_full_blocked());
        }
        if let Some(cached) = self.bran_semi_cache_lookup(&sut, seen_holds)? {
            return Ok(cached);
        }

        let out = self.bran_canonical_semi_inner_impl(sut.clone(), seen_holds)?;
        self.bran_semi_cache_store(&sut, seen_holds, out);
        Ok(out)
    }

    fn bran_canonical_semi_inner_impl(
        &mut self,
        sut: NRc<NTy>,
        seen_holds: &mut Vec<NRc<NTy>>,
    ) -> Result<SemiId> {
        match &*sut {
            NTy::Noun | NTy::Void => Ok(self.semi_full_blocked()),
            NTy::Atom { .. } => {
                // Atoms carry only leaves: lower the atom type and decode it with
                // the noun helper, as `atom_nest` does.
                let sut_noun = live_to_noun(&mut self.cx, &sut, self.slab);
                let (_aura, bits) = type_atom_parts(sut_noun, &self.slab.noun_space())?;
                Ok(match bits {
                    Some(noun) => self.semi_full_complete(noun),
                    None => self.semi_full_blocked(),
                })
            }
            NTy::Cell(head, tail) => {
                let head = head.clone();
                let tail = tail.clone();
                let hed = self.bran_canonical_semi_inner(head, seen_holds)?;
                let tal = self.bran_canonical_semi_inner(tail, seen_holds)?;
                self.semi_combine(hed, tal)
            }
            NTy::Core { payload, rest, .. } => {
                // Recurse into the payload; lower only the `rest` leaf to read the
                // coil's battery seminoun (hoon's `p.r.q.sut`).
                let payload = payload.clone();
                let rest_noun = live_leaf_to_noun(&mut self.cx, rest, self.slab);
                let space = self.slab.noun_space();
                let rest_cell = rest_noun.in_space(&space).as_cell().map_err(|err| {
                    CompilerError::Decode(format!("bran core rest not cell: {err}"))
                })?;
                let coil_semi = rest_cell.head().noun();
                let payload_semi = self.bran_canonical_semi_inner(payload, seen_holds)?;
                let coil_semi = self.semi_import_noun(coil_semi)?;
                self.semi_combine(coil_semi, payload_semi)
            }
            NTy::Face { .. } | NTy::Hint { .. } => match self.repo(sut.clone()) {
                Ok(inner) => self.bran_canonical_semi_inner(inner, seen_holds),
                Err(_) => Ok(self.semi_full_blocked()),
            },
            NTy::Fork { .. } => Ok(self.semi_full_blocked()),
            NTy::Hold { .. } => {
                if seen_holds.iter().any(|prior| NRc::ptr_eq(prior, &sut)) {
                    return Ok(self.semi_full_blocked());
                }
                seen_holds.push(sut.clone());
                let out = match self.repo(sut.clone()) {
                    Ok(inner) => self.bran_canonical_semi_inner(inner, seen_holds),
                    Err(_) => Ok(self.semi_full_blocked()),
                };
                seen_holds.pop();
                out
            }
        }
    }

    fn mint_ketvar(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        vair: Vair,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let (p_ty, p_formula) = self.mint(sut.clone(), gol.clone(), p)?;
        let wrapped = self.wrap_type(p_ty, vair)?;
        let ty = self.nice(sut, gol, wrapped)?;
        Ok((ty, p_formula))
    }

    fn mint_dtkt(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        const HOON_VERSION: u64 = 138;
        let (ty, _formula) = self.mint(sut.clone(), gol, &Hoon::KetTar(Box::new(spec.clone())))?;
        // The minted type is embedded in a nock %12 hint formula (noun): lower it.
        let ty_noun = live_to_noun(&mut self.cx, &ty, self.slab);
        let hint_inner = T(self.slab, &[D(HOON_VERSION), ty_noun]);
        let hint = T(self.slab, &[D(1), hint_inner]);
        let goal = cons_noun(&mut self.cx);
        let (_q_ty, q_formula) = self.mint(sut, goal, q)?;
        let hint = self.formula_import(hint)?;
        let formula = self.formula_op(NockOpcode::SCRY, &[hint, q_formula]);
        Ok((ty, formula))
    }

    fn mint_dtls(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let atom_ty = ty_atom_n(&mut self.cx, self.slab, "$", None).1;
        let (_p_ty, p_formula) = self.mint(sut.clone(), atom_ty.clone(), p)?;
        let formula = self.formula_op(NockOpcode::INCREMENT, &[p_formula]);
        let ty = self.nice(sut, gol, atom_ty)?;
        Ok((ty, formula))
    }

    fn mint_dttr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let noun_ty = cons_noun(&mut self.cx);
        let (_p_ty, p_formula) = self.mint(sut.clone(), noun_ty.clone(), p)?;
        let (_q_ty, q_formula) = self.mint(sut.clone(), noun_ty.clone(), q)?;
        let formula = self.formula_arena.eval(p_formula, q_formula);
        let ty = self.nice(sut, gol, noun_ty)?;
        Ok((ty, formula))
    }

    fn mint_dtts(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let noun_ty = cons_noun(&mut self.cx);
        let (_p_ty, p_formula) = self.mint(sut.clone(), noun_ty.clone(), p)?;
        let (_q_ty, q_formula) = self.mint(sut.clone(), noun_ty, q)?;
        let formula = self.formula_op(NockOpcode::EQUAL, &[p_formula, q_formula]);
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let ty = self.nice(sut, gol, bool_ty)?;
        Ok((ty, formula))
    }

    fn mint_dtwt(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let noun_ty = cons_noun(&mut self.cx);
        let (_p_ty, p_formula) = self.mint(sut.clone(), noun_ty, p)?;
        let formula = self.formula_op(NockOpcode::CELL, &[p_formula]);
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let ty = self.nice(sut, gol, bool_ty)?;
        Ok((ty, formula))
    }

    fn play_wtts(&mut self, _sut: NRc<NTy>, _spec: &Spec, _wing: &WingType) -> Result<NRc<NTy>> {
        Ok(ty_bool_n(&mut self.cx, self.slab).1)
    }

    fn play_ketvar(&mut self, sut: NRc<NTy>, p: &Hoon, vair: Vair) -> Result<NRc<NTy>> {
        let p_ty = self.play(sut, p)?;
        self.wrap_type(p_ty, vair)
    }

    fn play_dbug(&mut self, sut: NRc<NTy>, inner: &Hoon) -> Result<NRc<NTy>> {
        // Canonical `++play` for `%dbug` preserves the inner type and only adds tracing.
        self.play(sut, inner)
    }

    fn play_note(&mut self, sut: NRc<NTy>, note: &Note, inner: &Hoon) -> Result<NRc<NTy>> {
        // Canonical `++play`:
        //   [%note *]  (hint [sut p.gen] $(gen q.gen))
        let payload = self.play(sut.clone(), inner)?;
        let note_noun = note_to_noun(self.slab, note)?;
        self.hint_type(sut, note_noun, payload)
    }

    fn play_kthp(&mut self, sut: NRc<NTy>, spec: &Spec, q: &Hoon) -> Result<NRc<NTy>> {
        let gen = Hoon::KetHep(Box::new(spec.clone()), Box::new(q.clone()));
        self.play_opened(sut, &gen)
    }

    fn play_kttr(&mut self, sut: NRc<NTy>, spec: &Spec) -> Result<NRc<NTy>> {
        let gen = Hoon::KetTar(Box::new(spec.clone()));
        self.play_opened(sut, &gen)
    }

    fn play_ktcl(&mut self, sut: NRc<NTy>, spec: &Spec) -> Result<NRc<NTy>> {
        let gen = Hoon::KetCol(Box::new(spec.clone()));
        self.play_opened(sut, &gen)
    }

    fn play_dtls(&mut self, _sut: NRc<NTy>, _p: &Hoon) -> Result<NRc<NTy>> {
        Ok(ty_atom_n(&mut self.cx, self.slab, "$", None).1)
    }

    fn play_wing(&mut self, sut: NRc<NTy>, wing: &WingType) -> Result<NRc<NTy>> {
        self.epla(sut, wing, &[])
    }

    fn axis_contains(container: &BigUint, contained: &BigUint) -> bool {
        let container_bits = container.bits();
        let contained_bits = contained.bits();
        if contained_bits < container_bits {
            return false;
        }
        let shift = contained_bits - container_bits;
        (contained >> usize::try_from(shift).expect("axis bit length exceeds usize")) == *container
    }

    fn axis_parent(axis: &BigUint) -> BigUint {
        axis >> 1
    }

    fn axis_sibling(axis: &BigUint) -> BigUint {
        if axis & BigUint::from(1u32) == BigUint::from(0u32) {
            axis + BigUint::from(1u32)
        } else {
            axis - BigUint::from(1u32)
        }
    }

    fn hike_insert(
        &mut self,
        axe: BigUint,
        fol: FormulaId,
        rel: &mut std::collections::BTreeMap<BigUint, FormulaId>,
    ) -> Result<()> {
        let mut probe = axe.clone();
        while probe != BigUint::from(1u32) {
            if rel.contains_key(&probe) {
                return Ok(());
            }
            probe = Self::axis_parent(&probe);
        }

        let existing: Vec<BigUint> = rel.keys().cloned().collect();
        for key in existing {
            if Self::axis_contains(&axe, &key) {
                rel.remove(&key);
            }
        }

        let sib = Self::axis_sibling(&axe);
        if let Some(sib_fol) = rel.remove(&sib) {
            let parent = Self::axis_parent(&sib);
            let merged = if sib > axe {
                self.formula_cons(fol, sib_fol)
            } else {
                self.formula_cons(sib_fol, fol)
            };
            self.hike_insert(parent, merged, rel)?;
        } else {
            rel.insert(axe, fol);
        }
        Ok(())
    }

    fn hike_formula(
        &mut self,
        root_axis: BigUint,
        edits: &[(BigUint, FormulaId)],
    ) -> Result<FormulaId> {
        let mut rel: std::collections::BTreeMap<BigUint, FormulaId> =
            std::collections::BTreeMap::new();
        for (axis, formula) in edits {
            self.hike_insert(axis.clone(), *formula, &mut rel)?;
        }
        let mut out = self.formula_slot(root_axis);
        // hoon-138 `++hike` sorts with `gth` (descending) and recurses from head to tail:
        //   [%10 [hi ...] [%10 [lo ...] [%0 a]]]
        // To build the same tree iteratively, apply edits in ascending order so larger axes
        // wrap later and become outer nodes.
        let keys: Vec<BigUint> = rel.keys().cloned().collect();
        for axis in keys {
            let edit_formula = rel
                .get(&axis)
                .copied()
                .ok_or_else(|| CompilerError::Noun("missing hike edit".to_string()))?;
            out = self.formula_arena.edit(axis, edit_formula, out);
        }
        Ok(out)
    }

    fn cnts_tack(
        &mut self,
        sut: NRc<NTy>,
        wing: &WingType,
        mur: NRc<NTy>,
    ) -> Result<(BigUint, NRc<NTy>)> {
        let port = self.find(sut.clone(), Way::Rite, wing)?;
        match port {
            Port::Palo(palo) => {
                let mur_for_duz = mur.clone();
                let duz = |_ut: &mut Self, _a: NRc<NTy>| Ok(mur_for_duz.clone());
                self.take(sut, &palo.vein, &duz)
            }
            Port::Synthetic { .. } => Err(CompilerError::Noun("tack".to_string())),
        }
    }

    fn cnts_toss(
        &mut self,
        wing: &WingType,
        mur: NRc<NTy>,
        arms: &[(NRc<NTy>, Noun)],
    ) -> Result<(BigUint, Vec<(NRc<NTy>, Noun)>)> {
        let mut axis_match: Option<BigUint> = None;
        let mut out = Vec::with_capacity(arms.len());
        for (typ, foot) in arms {
            let (axis, new_typ) = self.cnts_tack(typ.clone(), wing, mur.clone())?;
            if let Some(existing) = &axis_match {
                if existing != &axis {
                    return Err(CompilerError::Noun("mate".to_string()));
                }
            } else {
                axis_match = Some(axis);
            }
            out.push((new_typ, *foot));
        }
        let axis = axis_match.ok_or_else(|| CompilerError::Noun("need".to_string()))?;
        Ok((axis, out))
    }

    // HOON138:arm=ut:tack lines=10848-10853 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++tack`; full parity review is still in progress
    fn tack(
        &mut self,
        sut: NRc<NTy>,
        hyp: &WingType,
        mur: NRc<NTy>,
    ) -> Result<(BigUint, NRc<NTy>)> {
        self.cnts_tack(sut, hyp, mur)
    }

    // HOON138:arm=ut:toss lines=10860-10871 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++toss`; full parity review is still in progress
    fn toss(
        &mut self,
        hyp: &WingType,
        mur: NRc<NTy>,
        men: &[(NRc<NTy>, Noun)],
    ) -> Result<(BigUint, Vec<(NRc<NTy>, Noun)>)> {
        self.cnts_toss(hyp, mur, men)
    }

    fn play_cnts_apply_leg_patches(
        &mut self,
        sut: NRc<NTy>,
        mut typ: NRc<NTy>,
        pairs: &[(WingType, Hoon)],
    ) -> Result<NRc<NTy>> {
        for (sub_wing, expr) in pairs {
            let patch_type = self.play(sut.clone(), expr)?;
            let (_axis, edited) = self.tack(typ, sub_wing, patch_type)?;
            typ = edited;
        }
        Ok(typ)
    }

    // HOON138:arm=ut:et:play lines=9244-9248 map=envelope status=partial reviewed=2026-03-06
    // HOON138_NOTE:implements the native `%cnts` play path using canonical `et/play` and `elbo`
    fn play_cnts(
        &mut self,
        sut: NRc<NTy>,
        wing: &WingType,
        pairs: &[(WingType, Hoon)],
    ) -> Result<NRc<NTy>> {
        let port = self.cnts_base_port(sut.clone(), wing)?;
        let palo = match port {
            Port::Palo(palo) => palo,
            Port::Synthetic { typ, .. } => {
                if pairs.is_empty() {
                    return Ok(typ);
                }
                return Err(CompilerError::Noun("hoon".to_string()));
            }
        };
        match palo.opal {
            Opal::Leg(typ) => self.play_cnts_apply_leg_patches(sut, typ, pairs),
            Opal::Arm { arms, .. } => {
                let mut hag = arms;
                for (sub_wing, expr) in pairs {
                    let patch_type = self.play(sut.clone(), expr)?;
                    let (_axis, next_hag) = self.toss(sub_wing, patch_type, &hag)?;
                    hag = next_hag;
                }
                self.fire(&hag)
            }
        }
    }

    fn epla(
        &mut self,
        sut: NRc<NTy>,
        hyp: &WingType,
        rig: &[(WingType, Hoon)],
    ) -> Result<NRc<NTy>> {
        self.play_cnts(sut, hyp, rig)
    }

    fn play_limb(&mut self, sut: NRc<NTy>, name: &str) -> Result<NRc<NTy>> {
        // Canonical hoon-138 open() lowering:
        //   [%limb p] => [%cnts [p ~] ~]
        let wing = vec![Limb::Term(name.to_string())];
        self.epla(sut, &wing, &[])
    }

    fn mint_hand(&mut self, typ: &Type, nock: &Nock) -> Result<(NRc<NTy>, FormulaId)> {
        let typ_noun = type_to_noun(self.slab, typ)?;
        let typ_native = native_of(&mut self.cx, typ_noun, &self.slab.noun_space())?;
        let nock_noun = nock_to_noun(self.slab, nock);
        let formula = self.formula_import(nock_noun)?;
        Ok((typ_native, formula))
    }

    fn play_tune(&mut self, sut: NRc<NTy>, tune: &TermOrTune) -> Result<NRc<NTy>> {
        // Wraps the whole subject in a %face that shares `sut`'s `Rc`.
        let tool = term_or_tune_to_noun(self.slab, tune)?;
        let tool_leaf = live_leaf_from_noun(&mut self.cx, tool, &self.slab.noun_space());
        Ok(cons_face(&mut self.cx, tool_leaf, sut))
    }

    fn mint_siggar(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        hint: &TermOrPair,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let (q_ty, q_formula) = self.mint(sut.clone(), gol, q)?;
        let hint_noun = match hint {
            TermOrPair::Term(name) => term_to_noun(self.slab, name),
            TermOrPair::Pair(name, hoon) => {
                // hoon-138 `%sggr`: pair hint payload always mints under `%noun`.
                let noun_goal = cons_noun(&mut self.cx);
                let (_ty, formula) = self.mint(sut, noun_goal, hoon)?;
                let name_noun = term_to_noun(self.slab, name);
                let formula = self.formula_materialize(formula);
                T(self.slab, &[name_noun, formula])
            }
        };
        let space = self.slab.noun_space();
        let formula = self.formula_arena.hint(hint_noun, q_formula, &space);
        Ok((q_ty, formula))
    }

    fn mint_sigzap(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let _ = self.play(sut.clone(), p)?;
        self.mint(sut, gol, q)
    }

    fn mint_zpcom(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let ty = self.play(sut.clone(), p)?;
        let ty = self.nice(sut, gol, ty)?;
        let q_noun = self.hoon_noun_for_node(q);
        let formula = self.formula_quote(q_noun);
        Ok((ty, formula))
    }

    fn mint_zpmc(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let goal = cons_noun(&mut self.cx);
        let (vos_ty, vos_formula) = self.mint(sut.clone(), goal.clone(), q)?;
        let (ref_ty, _ref_formula) = self.mint(sut.clone(), goal, p)?;
        let cell_ty = cons_cell(&mut self.cx, ref_ty, vos_ty.clone());
        let ty = self.nice(sut, gol, cell_ty)?;
        // burp_type takes/returns a noun: lower vos_ty.
        let vos_ty_noun = live_to_noun(&mut self.cx, &vos_ty, self.slab);
        let burped = self.burp_type(vos_ty_noun)?;
        let head = self.formula_quote(burped);
        let formula = self.formula_cons(head, vos_formula);
        Ok((ty, formula))
    }

    fn mint_zpgl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let typ_expr = Hoon::KetTar(Box::new(spec.clone()));
        let typ = self.play(sut.clone(), &typ_expr)?;
        let typ = self.nice(sut.clone(), gol, typ)?;

        // Mirrors the hoon-138 `%zpgl` mint expansion without an extra binding, so
        // the compiled formula matches the dynock fixtures.
        let target_type = Hoon::TisGar(
            Box::new(Hoon::ZapGar(Box::new(Hoon::KetTar(Box::new(spec.clone()))))),
            Box::new(Hoon::Axis((2u64).into())),
        );
        let actual_type = Hoon::TisGar(Box::new(q.clone()), Box::new(Hoon::Axis((2u64).into())));
        let cond = Hoon::CenCol(
            Box::new(Hoon::Limb("levi".to_string())),
            vec![target_type, actual_type],
        );
        let yes = Hoon::TisGar(Box::new(q.clone()), Box::new(Hoon::Axis((3u64).into())));
        let expanded = Hoon::WutCol(Box::new(cond), Box::new(yes), Box::new(Hoon::ZapZap));
        let goal = cons_noun(&mut self.cx);
        let (_val_ty, val_formula) = self.mint(sut, goal, &expanded)?;
        Ok((typ, val_formula))
    }

    fn mint_zpts(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let noun_ty = cons_noun(&mut self.cx);
        let ty = self.nice(sut.clone(), gol, noun_ty.clone())?;
        // hoon-138 `%zpts` mints the inner hoon under `vet=|`, where the goal is unchecked.
        self.with_vet_off(|ut| {
            let (_p_ty, p_formula) = ut.mint(sut, noun_ty, p)?;
            let p_formula = ut.formula_materialize(p_formula);
            let formula = ut.formula_quote(p_formula);
            Ok((ty, formula))
        })
    }

    fn mint_zppt(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wings: &[WingType],
        q: &Hoon,
        r: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let found = self.feel(sut.clone(), wings)?;
        if found {
            self.mint(sut, gol, q)
        } else {
            self.mint(sut, gol, r)
        }
    }

    fn mint_tsbr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Canonical hoon-138 open() lowering:
        //   [%tsbr *] => [%tsls ~(example ax p.gen) q.gen]
        // This is not `=+ *spec ...`: going through `%kttr` would add `%ktsg` folding,
        // which `%tsbr` does not do.
        let example = self.spec_example_cached(spec);
        let expanded = Hoon::TisLus(Box::new(example.as_ref().clone()), Box::new(q.clone()));
        self.mint(sut, gol, &expanded)
    }

    fn mint_tsfs(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        skin: &Skin,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let expanded = Hoon::TisLus(
            Box::new(Hoon::KetTis(skin.clone(), Box::new(p.clone()))),
            Box::new(q.clone()),
        );
        self.mint(sut, gol, &expanded)
    }

    fn mint_tsmc(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        skin: &Skin,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let expanded = Hoon::TisFas(skin.clone(), Box::new(q.clone()), Box::new(p.clone()));
        self.mint(sut, gol, &expanded)
    }

    fn mint_wing(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let port = self.find(sut.clone(), Way::Read, wing)?;
        let (ty, formula) = self.fine(&port)?;
        let ty = self.nice(sut, gol, ty)?;
        Ok((ty, formula))
    }

    fn arm_goal_for_hoon_in_progress(
        &mut self,
        core: NRc<NTy>,
        hoon: Noun,
        goal: NRc<NTy>,
        vet: bool,
    ) -> Result<Option<NRc<NTy>>> {
        let hoon_raw = NounIdentity::of(hoon);

        // Core and goal are hash-consed, so pointer identity is structural equality.
        for entry in self.arm_goal_in_progress.iter().rev() {
            let entry_hoon_raw = NounIdentity::of(entry.hoon);
            let core_match = NRc::ptr_eq(&entry.core, &core);
            if !core_match {
                continue;
            }
            if entry.vet != vet {
                continue;
            }
            let hoon_match =
                entry_hoon_raw == hoon_raw || noun_eq(entry.hoon, hoon, &self.slab.noun_space())?;
            if !hoon_match {
                continue;
            }
            let goal_match = NRc::ptr_eq(&entry.goal, &goal);
            if !goal_match {
                continue;
            }
            return Ok(Some(entry.goal.clone()));
        }

        Ok(None)
    }

    fn cnts_base_port(&mut self, sut: NRc<NTy>, wing: &WingType) -> Result<Port> {
        self.find(sut, Way::Read, wing)
    }

    /// Content-keyed `native_of` for recursive types. `redo` and `fire` rebuild
    /// structurally equal nouns at fresh addresses, which miss the address-keyed
    /// decode memo and force a re-walk and re-jam of the jammed leaves (fork
    /// treaps, batteries). A mug hit confirmed by `noun_eq` reuses the interned
    /// `Rc` without decoding. Returns the same canonical `Rc` as `native_of`;
    /// mug collisions fall through to a real decode.
    fn native_of_cached(&mut self, noun: Noun) -> Result<NRc<NTy>> {
        let mug = self.noun_mug_cached(noun);
        for cand in native_of_mug_candidates(&self.cx, mug) {
            let cand_noun = live_to_noun(&mut self.cx, &cand, self.slab);
            let space = self.slab.noun_space();
            if noun_eq(noun, cand_noun, &space)? {
                return Ok(cand);
            }
        }
        let rc = native_of(&mut self.cx, noun, &self.slab.noun_space())?;
        native_of_mug_insert(&mut self.cx, mug, rc.clone());
        Ok(rc)
    }

    // HOON138:arm=ut:et:mint lines=9250-9257 map=envelope status=partial reviewed=2026-03-06
    // HOON138_NOTE:implements the native `%cnts` mint path using canonical `et/mint` and `ergo`
    fn mint_cnts(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        pairs: &[(WingType, Hoon)],
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let port = self.cnts_base_port(sut.clone(), wing)?;
        let palo = match port {
            Port::Palo(palo) => palo,
            Port::Synthetic { typ, formula } => {
                if pairs.is_empty() {
                    let ty = self.nice(sut, gol, typ)?;
                    return Ok((ty, formula));
                }
                return Err(CompilerError::Noun("hoon".to_string()));
            }
        };
        let base_axis = tend_big(&palo.vein)?;
        let mut edits: Vec<(BigUint, FormulaId)> = Vec::with_capacity(pairs.len());
        match palo.opal {
            Opal::Leg(mut base_leg) => {
                for (_idx, (sub_wing, expr)) in pairs.iter().enumerate() {
                    let goal = cons_noun(&mut self.cx);
                    let (patch_ty, patch_formula) = self.mint(sut.clone(), goal, expr)?;
                    let (edit_axis, edited_leg) = self.tack(base_leg, sub_wing, patch_ty)?;
                    base_leg = edited_leg;
                    edits.push((edit_axis, patch_formula));
                }
                // hoon-138 `++ergo` conses each edit onto `hej` (`[[p.dar q.zil] hej]`), so
                // `hike` sees edits in reverse traversal order.
                edits.reverse();
                let ty = self.nice(sut, gol, base_leg)?;
                let formula = self.hike_formula(base_axis, &edits)?;
                Ok((ty, formula))
            }
            Opal::Arm {
                axis: arm_axis,
                arms,
            } => {
                let mut hag = arms;
                for (_idx, (sub_wing, expr)) in pairs.iter().enumerate() {
                    let goal = cons_noun(&mut self.cx);
                    let (patch_ty, patch_formula) = self.mint(sut.clone(), goal, expr)?;
                    let (edit_axis, next_hag) = self.toss(sub_wing, patch_ty, &hag)?;
                    // hoon-138 `++ergo` always threads `hag` through `q.dix`, even when the
                    // sample edit is `%void`; the later `++fire` rejects non-core arm
                    // subjects.
                    hag = next_hag;
                    edits.push((edit_axis, patch_formula));
                }
                // Match hoon-138 `++ergo` edit accumulation order for arm patches too.
                edits.reverse();

                let hike = self.hike_formula(base_axis, &edits)?;
                let formula = self.formula_arena.kick(arm_axis, hike);
                let arm_ty = self.fire(&hag)?;
                let ty = self.nice(sut, gol, arm_ty)?;
                Ok((ty, formula))
            }
        }
    }

    fn emin(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        hyp: &WingType,
        rig: &[(WingType, Hoon)],
    ) -> Result<(NRc<NTy>, FormulaId)> {
        self.mint_cnts(sut, gol, hyp, rig)
    }

    fn mint_limb(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        name: &str,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Canonical hoon-138 open() lowering:
        //   [%limb p] => [%cnts [p ~] ~]
        let wing = vec![Limb::Term(name.to_string())];
        self.emin(sut, gol, &wing, &[])
    }

    fn mint_tune(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        tune: &TermOrTune,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let tool = term_or_tune_to_noun(self.slab, tune)?;
        // `%face` over the subject; `cons_face` collapses a void subject to void.
        let tool_leaf = live_leaf_from_noun(&mut self.cx, tool, &self.slab.noun_space());
        let ty = cons_face(&mut self.cx, tool_leaf, sut.clone());
        let ty = self.nice(sut, gol, ty)?;
        let formula = self.formula_slot_u64(1);
        Ok((ty, formula))
    }

    fn hoon_ast_ptr_key(gen: &Hoon) -> HoonIdentity {
        HoonIdentity::of(gen)
    }

    fn cache_hoon_ast_ptr(
        cache: &mut HashMap<HoonIdentity, (Option<NounIdentity>, Noun)>,
        order: &mut VecDeque<HoonIdentity>,
        ptr: HoonIdentity,
        raw: Option<NounIdentity>,
        noun: Noun,
    ) {
        if !cache.contains_key(&ptr) {
            order.push_back(ptr);
            if order.len() > Self::HOON_CACHE_RAW_KEY_LIMIT {
                if let Some(evict) = order.pop_front() {
                    cache.remove(&evict);
                }
            }
        }
        cache.insert(ptr, (raw, noun));
    }

    fn cache_hoon_ast_for_node(&mut self, gen: &Hoon) {
        if !self.exact_hoon_ast_lookup_enabled {
            return;
        }

        let hoon_noun = self.hoon_noun_for_node(gen);
        let hoon_raw = NounIdentity::of(hoon_noun);
        let hoon_mug = self.noun_mug_cached(hoon_noun);
        let ast = Arc::new(gen.clone());

        if !self.hoon_cache_raw.contains_key(&hoon_raw) {
            self.hoon_cache_raw_order.push_back(hoon_raw);
            if self.hoon_cache_raw_order.len() > Self::HOON_CACHE_RAW_KEY_LIMIT {
                if let Some(evict) = self.hoon_cache_raw_order.pop_front() {
                    self.hoon_cache_raw.remove(&evict);
                }
            }
        }
        self.hoon_cache_raw.insert(hoon_raw, Arc::clone(&ast));
        Self::cache_hoon_ast_ptr(
            &mut self.hoon_ast_ptr_cache,
            &mut self.hoon_ast_ptr_cache_order,
            Self::hoon_ast_ptr_key(gen),
            Some(hoon_raw),
            hoon_noun,
        );

        if !self.hoon_cache_struct.contains_key(&hoon_mug) {
            self.hoon_cache_struct_order.push_back(hoon_mug);
            if self.hoon_cache_struct_order.len() > Self::HOON_CACHE_STRUCT_KEY_LIMIT {
                if let Some(evict) = self.hoon_cache_struct_order.pop_front() {
                    self.hoon_cache_struct.remove(&evict);
                }
            }
        }
        let bucket = self.hoon_cache_struct.entry(hoon_mug).or_default();
        for (cached_noun, cached_ast) in bucket.iter_mut() {
            if NounIdentity::of(cached_noun) == hoon_raw {
                *cached_ast = ast;
                return;
            }
        }
        if bucket.len() >= Self::HOON_CACHE_STRUCT_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back((hoon_noun, ast));
    }

    fn spec_example_cached(&mut self, spec: &Spec) -> Arc<Hoon> {
        let Some(sig) = Sig64::spec_signature_spot_sensitive(spec) else {
            return Arc::new(spec_example(spec));
        };
        if let Some(bucket) = self.spec_example_cache.get(&sig) {
            for (cached_spec, cached) in bucket.iter().rev() {
                if cached_spec == spec {
                    return Arc::clone(cached);
                }
            }
        }

        let expanded = Arc::new(spec_example(spec));
        if !self.spec_example_cache.contains_key(&sig) {
            self.spec_example_cache_order.push_back(sig);
            if self.spec_example_cache_order.len() > Self::SPEC_CACHE_KEY_LIMIT {
                if let Some(evict) = self.spec_example_cache_order.pop_front() {
                    self.spec_example_cache.remove(&evict);
                }
            }
        }
        let bucket = self.spec_example_cache.entry(sig).or_default();
        if bucket.len() >= Self::SPEC_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back((spec.clone(), Arc::clone(&expanded)));
        expanded
    }

    fn spec_factory_open_cached(&mut self, spec: &Spec) -> Arc<Hoon> {
        let Some(sig) = Sig64::spec_signature_spot_sensitive(spec) else {
            return Arc::new(factory(
                spec.clone(),
                1u64.into(),
                Vec::new(),
                HashMap::new(),
                Vec::new(),
                None,
                None,
            ));
        };
        if let Some(bucket) = self.spec_factory_open_cache.get(&sig) {
            for (cached_spec, cached) in bucket.iter().rev() {
                if cached_spec == spec {
                    return Arc::clone(cached);
                }
            }
        }

        let expanded = Arc::new(factory(
            spec.clone(),
            1u64.into(),
            Vec::new(),
            HashMap::new(),
            Vec::new(),
            None,
            None,
        ));
        if !self.spec_factory_open_cache.contains_key(&sig) {
            self.spec_factory_open_cache_order.push_back(sig);
            if self.spec_factory_open_cache_order.len() > Self::SPEC_CACHE_KEY_LIMIT {
                if let Some(evict) = self.spec_factory_open_cache_order.pop_front() {
                    self.spec_factory_open_cache.remove(&evict);
                }
            }
        }
        let bucket = self.spec_factory_open_cache.entry(sig).or_default();
        if bucket.len() >= Self::SPEC_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back((spec.clone(), Arc::clone(&expanded)));
        expanded
    }

    fn open_cached(&mut self, gen: &Hoon) -> Option<Arc<Hoon>> {
        let Some(sig) = self.mint_cache_signature(gen) else {
            let opened = open(gen.clone());
            return (&opened != gen).then(|| Arc::new(opened));
        };
        if let Some(id) = self.hoon_arena.id_for(gen) {
            if let Some(cached) = &self.hoon_arena.entry(id).opened {
                return cached.clone();
            }
            let opened = open(gen.clone());
            let cached = (&opened != gen).then(|| Arc::new(opened));
            self.hoon_arena.entry_mut(id).opened = Some(cached.clone());
            return cached;
        }
        let ptr = Self::hoon_ast_ptr_key(gen);
        if let Some((cached_sig, cached)) = self.open_cache.get(&ptr) {
            if *cached_sig == sig {
                return cached.clone();
            }
        }

        let opened = open(gen.clone());
        let cached = (&opened != gen).then(|| Arc::new(opened));
        if !self.open_cache.contains_key(&ptr) {
            self.open_cache_order.push_back(ptr);
            if self.open_cache_order.len() > Self::HOON_CACHE_RAW_KEY_LIMIT {
                if let Some(evict) = self.open_cache_order.pop_front() {
                    self.open_cache.remove(&evict);
                }
            }
        }
        self.open_cache.insert(ptr, (sig, cached.clone()));
        cached
    }

    fn arm_key_term(&mut self, key_noun: Noun) -> Result<Arc<str>> {
        let space = self.slab.noun_space();
        let key_atom = key_noun
            .in_space(&space)
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("arm key not atom: {err}")))?;
        let key = atom_to_string(key_atom)
            .map_err(|err| CompilerError::Decode(format!("arm key: {err}")))?;
        Ok(Arc::<str>::from(key))
    }

    // HOON138:arm=ut:mine lines=9768-9916 map=envelope status=partial reviewed=2026-03-06
    // HOON138_NOTE:core-construction path implementing canonical `++mine` with native helpers
    fn mint_core(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        prefix: &Option<String>,
        tomes: &HashMap<String, Tome>,
        poly: Poly,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        // Layered cores keep prior arms in the payload/context ancestry. The new core's
        // tomes hold only the newly declared arms; `fond` resolves inherited arms by
        // walking `%core` payloads, not by copying old tomes into the new battery.
        // `sut` is never lowered to a noun; `gol` is lowered once for the noun-side
        // walk in `goal_core_for_mine`. The core's payload and context are both `sut`.
        let gol_noun = live_to_noun(&mut self.cx, &gol, self.slab);
        let tomes_map = self.tomes_map_from_ast(tomes)?;
        if let Some((cached_ty, cached_formula)) =
            self.core_mint_cache_lookup(&sut, &gol, tomes_map, prefix, poly)?
        {
            return Ok((cached_ty, cached_formula));
        }
        let garb = garb_native(prefix.as_deref(), poly, Vair::Gold);
        // Match hoon-138/hoonc layered-core payload layout and formula shape.
        let payload_formula = self.formula_slot_u64(1);
        let goal_core_for_arms = self.goal_core_for_mine(gol_noun, tomes_map)?;
        let (goal_for_arms, expected_tomes_map) = match goal_core_for_arms {
            Some(goal_core) => {
                let (_goal_payload, goal_coil) =
                    type_core_parts(goal_core, &self.slab.noun_space())?;
                let (_goal_garb, _goal_context, goal_rest) =
                    coil_parts(goal_coil, &self.slab.noun_space())?;
                let goal_tomes_map = coil_tomes(goal_rest, &self.slab.noun_space())?;
                (goal_core, Some(goal_tomes_map))
            }
            None => (gol_noun, None),
        };
        if let Some(expected_tomes_map) = expected_tomes_map {
            let actual_count = self.map_entry_count(tomes_map)?;
            let expected_count = self.map_entry_count(expected_tomes_map)?;
            if actual_count != expected_count {
                return Err(CompilerError::Noun("core-number-of-chapters".to_string()));
            }
        }

        // hoon-138 `++mine` compiles arm formulas against a `%lazy` battery
        // seminoun (`++laze`) in a single pass. The resolver id is content-addressed
        // (subject, tomes, poly), so structurally equal recursive cores intern to one
        // `Rc` and the pointer-keyed cuts converge at the natural settling depth.
        let tomes_sig = TomesSignature(u64::from(
            self.noun_mug_cached(tomes_map).0 ^ Self::prefix_signature(prefix.as_deref()).0,
        ));
        let resolver_id = self.lazy_resolver_canonical_id(&sut, tomes_sig, poly);
        let lazy_semi = self.semi_noun_lazy_root(resolver_id);
        let lazy_rest = T(self.slab, &[lazy_semi, tomes_map]);
        // Build the lazy core once, with payload and context both `sut`. Every arm
        // reuses this interned `Rc`, so the context is shared rather than copied per
        // arm. The resolver context stores the same `Rc`, so cross-arm in-progress
        // cycle detection compares pointer identity.
        let core_native_lazy = {
            let space = self.slab.noun_space();
            let rest_leaf = live_leaf_from_noun(&mut self.cx, lazy_rest, &space);
            cons_core(
                &mut self.cx,
                sut.clone(),
                garb.clone(),
                sut.clone(),
                rest_leaf,
            )
        };
        // The same id can come back (for example with a different gol or vet).
        // Register it once: re-registering would reset `cached_formula_by_axis` and
        // `in_progress_axes`, dropping the cross-arm formula cache and the in-progress
        // guard.
        if !self.lazy_resolvers.contains_key(&resolver_id) {
            let mut lazy_arms = HashMap::new();
            self.collect_lazy_resolver_arms_from_tomes_map(
                tomes_map,
                BigUint::from(1u32),
                &mut lazy_arms,
            )?;
            self.lazy_resolver_register_context(
                resolver_id,
                core_native_lazy.clone(),
                poly,
                lazy_arms,
            );
        }
        // The resolver context stays registered after the core completes.
        // hoon-138's `++laze` resolver is a pure gate embedded in the lazy
        // seminoun: types that captured the lazy battery (e.g. `%hold`s of
        // in-flight arms, cached mint results) keep resolving single arms
        // forever. Removing the context would leave dead resolver ids inside
        // long-lived types, blocking their batteries permanently and breaking
        // constant folds that hoonc performs.
        let battery = self.build_tomes_battery_from_maps(
            tomes_map, core_native_lazy, poly, goal_for_arms, expected_tomes_map,
        )?;
        let semi_noun = self.semi_noun_full(battery);
        let rest = T(self.slab, &[semi_noun, tomes_map]);
        let core_native = {
            let space = self.slab.noun_space();
            let rest_leaf = live_leaf_from_noun(&mut self.cx, rest, &space);
            cons_core(
                &mut self.cx,
                sut.clone(),
                garb.clone(),
                sut.clone(),
                rest_leaf,
            )
        };
        let battery_formula = self.formula_quote(battery);
        let formula = self.formula_cons(battery_formula, payload_formula);
        // `nice` returns its input on success, so `ty` is `core_native`.
        let ty = self.nice(sut.clone(), gol.clone(), core_native.clone())?;
        self.core_mint_cache_store(&sut, &gol, tomes_map, prefix, poly, ty, formula)?;
        Ok((core_native, formula))
    }

    fn mine(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        mel: Vair,
        nym: Option<&str>,
        hud: Poly,
        dom: &HashMap<String, Tome>,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let prefix = nym.map(str::to_string);
        let (mut ty, formula) = self.mint_core(sut.clone(), gol.clone(), &prefix, dom, hud)?;
        if mel != Vair::Gold {
            let wrapped = self.wrap_type(ty, mel)?;
            ty = self.nice(sut, gol, wrapped)?;
        }
        Ok((ty, formula))
    }

    fn play_core(
        &mut self,
        sut: NRc<NTy>,
        prefix: &Option<String>,
        tomes: &HashMap<String, Tome>,
        poly: Poly,
    ) -> Result<NRc<NTy>> {
        // The core's payload and context are both `sut`, which is never lowered to a
        // noun. Only the `rest` leaf is built as a noun. `cons_core` collapses a void
        // payload to void, as `ty_core` does.
        let tomes_map = self.tomes_map_from_ast(tomes)?;
        let garb = garb_native(prefix.as_deref(), poly, Vair::Gold);
        // Canonical hoon-138 `%play` builds cores with `*seminoun` (blocked by default).
        let semi_noun = self.semi_noun_blocked();
        let rest = T(self.slab, &[semi_noun, tomes_map]);
        let space = self.slab.noun_space();
        let rest_leaf = live_leaf_from_noun(&mut self.cx, rest, &space);
        let native = cons_core(&mut self.cx, sut.clone(), garb, sut.clone(), rest_leaf);
        Ok(native)
    }

    fn tomes_map_from_ast(&mut self, tomes: &HashMap<String, Tome>) -> Result<Noun> {
        let mut keys: Vec<_> = tomes.keys().cloned().collect();
        keys.sort();
        let mut map = D(0);
        for key in keys {
            let tome = tomes
                .get(&key)
                .ok_or_else(|| CompilerError::Noun("missing tome".to_string()))?;
            let arms_map = self.arms_map_from_ast(&tome.1)?;
            let what = tome
                .0
                .as_ref()
                .map(|what| noun_expr_to_noun(self.slab, what))
                .unwrap_or_else(|| D(0));
            let tome_noun = T(self.slab, &[what, arms_map]);
            let key_noun = term_to_noun(self.slab, &key);
            map = map_put_mug(self.slab, map, key_noun, tome_noun)?;
        }
        Ok(map)
    }

    fn arms_map_from_ast(&mut self, arms: &HashMap<String, Hoon>) -> Result<Noun> {
        let mut keys: Vec<_> = arms.keys().cloned().collect();
        keys.sort();
        let mut map = D(0);
        for key in keys {
            let hoon = arms
                .get(&key)
                .ok_or_else(|| CompilerError::Noun("missing arm".to_string()))?;
            let hoon_noun = self.hoon_noun_for_node(hoon);
            self.cache_hoon_ast_for_node(hoon);
            let key_noun = term_to_noun(self.slab, &key);
            map = map_put_mug(self.slab, map, key_noun, hoon_noun)?;
        }
        Ok(map)
    }

    fn map_entry_count(&mut self, map: Noun) -> Result<usize> {
        let mut count = 0usize;
        let mut stack = vec![map];
        while let Some(current) = stack.pop() {
            let Some((_node, left, right)) = map_node(current, &self.slab.noun_space())? else {
                continue;
            };
            count = count.saturating_add(1);
            if !noun_is_zero(left) {
                stack.push(left);
            }
            if !noun_is_zero(right) {
                stack.push(right);
            }
        }
        Ok(count)
    }

    fn check_goal_core_chapter_counts(
        &mut self,
        goal: Noun,
        actual_count: usize,
        seen_holds: &mut HashMap<NounMug, Vec<Noun>>,
    ) -> Result<()> {
        match type_tag(goal, &self.slab.noun_space())?.as_str() {
            "noun" => Ok(()),
            "void" | "atom" => Err(CompilerError::Noun("core-nice".to_string())),
            "cell" => {
                let (head, _tail) = type_cell_parts(goal, &self.slab.noun_space())?;
                let noun = ty_noun(self.slab);
                if self.nest_noun(head, noun)? {
                    Ok(())
                } else {
                    Err(CompilerError::Noun("core-nice".to_string()))
                }
            }
            "core" => {
                let (_payload, coil) = type_core_parts(goal, &self.slab.noun_space())?;
                let (_garb, _context, rest) = coil_parts(coil, &self.slab.noun_space())?;
                let expected_count =
                    self.map_entry_count(coil_tomes(rest, &self.slab.noun_space())?)?;
                if actual_count != expected_count {
                    return Err(CompilerError::Noun("core-number-of-chapters".to_string()));
                }
                Ok(())
            }
            "fork" => {
                for option in type_fork_options(goal, &self.slab.noun_space())? {
                    self.check_goal_core_chapter_counts(option, actual_count, seen_holds)?;
                }
                Ok(())
            }
            "face" => self.check_goal_core_chapter_counts(
                type_face_inner(goal, &self.slab.noun_space())?,
                actual_count,
                seen_holds,
            ),
            "hint" => self.check_goal_core_chapter_counts(
                type_hint_inner(goal, &self.slab.noun_space())?,
                actual_count,
                seen_holds,
            ),
            "hold" => {
                if !self.noun_seen_insert_structural(seen_holds, goal)? {
                    return Ok(());
                }
                let expanded = self.repo_noun(goal)?;
                self.check_goal_core_chapter_counts(expanded, actual_count, seen_holds)
            }
            _ => Ok(()),
        }
    }

    fn goal_core_for_mine(&mut self, goal: Noun, actual_tomes_map: Noun) -> Result<Option<Noun>> {
        let actual_count = self.map_entry_count(actual_tomes_map)?;
        let mut seen_holds: HashMap<NounMug, Vec<Noun>> = HashMap::new();
        self.check_goal_core_chapter_counts(goal, actual_count, &mut seen_holds)?;

        let mut current = goal;
        seen_holds.clear();
        for _ in 0..128 {
            match type_tag(current, &self.slab.noun_space())?.as_str() {
                "core" => {
                    let (payload, coil) = type_core_parts(current, &self.slab.noun_space())?;
                    let (garb, context, rest) = coil_parts(coil, &self.slab.noun_space())?;
                    let garb = self.garb_with_vair(garb, Vair::Gold)?;
                    let coil = coil_from_parts(self.slab, garb, context, rest);
                    return Ok(Some(ty_core(self.slab, payload, coil)));
                }
                // hoon-138 `++get-tomes` returns `~` for forked goal cores after the
                // chapter-count check, so the arm count, name, and type checks are skipped.
                "fork" => return Ok(None),
                "face" => current = type_face_inner(current, &self.slab.noun_space())?,
                "hint" => current = type_hint_inner(current, &self.slab.noun_space())?,
                "hold" => {
                    if !self.noun_seen_insert_structural(&mut seen_holds, current)? {
                        return Ok(None);
                    }
                    current = self.repo_noun(current)?;
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    fn goal_chapter_expected_arms_map(
        &mut self,
        expected_tomes_map: Option<Noun>,
        chapter_key: Noun,
    ) -> Result<Option<Noun>> {
        let space = self.slab.noun_space();
        let Some(expected_tomes_map) = expected_tomes_map else {
            return Ok(None);
        };
        let Some((_chapter_axis, tome_noun)) = self.look(chapter_key, expected_tomes_map)? else {
            return Err(CompilerError::Noun("unexpcted-chapter".to_string()));
        };
        let tome_cell = tome_noun
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("goal chapter tome not cell: {err}")))?;
        Ok(Some(tome_cell.tail().noun()))
    }

    fn goal_arm_expected_type(
        &mut self,
        goal_for_arms: Noun,
        expected_arms_map: Option<Noun>,
        arm_key: Noun,
    ) -> Result<NRc<NTy>> {
        // The per-arm goal is `%noun` unless the goal is a core; then it is the goal
        // core's matching arm, played against the goal core. Only this rare path
        // decodes `goal_for_arms`, which is not the deepening core.
        let Some(expected_arms_map) = expected_arms_map else {
            return Ok(cons_noun(&mut self.cx));
        };
        let Some((_arm_axis, arm_hoon_noun)) = self.look(arm_key, expected_arms_map)? else {
            return Err(CompilerError::Noun("unexpected-arm".to_string()));
        };
        let arm_hoon = self.hoon_ast_lookup_result(arm_hoon_noun).map_err(|err| {
            CompilerError::Noun(format!("native mint: goal arm ast missing: {err}"))
        })?;
        let goal_subject = native_of(&mut self.cx, goal_for_arms, &self.slab.noun_space())?;
        self.play(goal_subject, arm_hoon.as_ref())
    }

    fn collect_lazy_resolver_arms_from_arms_map(
        &mut self,
        arms_map: Noun,
        axis: BigUint,
        out: &mut HashMap<BigUint, LazyResolverArmEntry>,
    ) -> Result<()> {
        let space = self.slab.noun_space();
        let Some((node, left, right)) = map_node(arms_map, &self.slab.noun_space())? else {
            return Ok(());
        };
        let node_cell = node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("arm node not cell: {err}")))?;
        let arm_key_noun = node_cell.head().noun();
        let arm_name = self.arm_key_term(arm_key_noun)?;
        let hoon_noun = node_cell.tail().noun();
        let left_empty = noun_is_zero(left);
        let right_empty = noun_is_zero(right);
        let arm_axis = if left_empty && right_empty {
            axis.clone()
        } else {
            peg_axis_big(axis.clone(), 2)?
        };
        if out
            .insert(
                arm_axis,
                LazyResolverArmEntry {
                    arm_name,
                    hoon_noun,
                },
            )
            .is_some()
        {
            return Err(CompilerError::Decode(
                "lazy resolver duplicate arm axis".to_string(),
            ));
        }
        match (left_empty, right_empty) {
            (true, true) => Ok(()),
            (true, false) => {
                self.collect_lazy_resolver_arms_from_arms_map(right, peg_axis_big(axis, 3)?, out)
            }
            (false, true) => {
                self.collect_lazy_resolver_arms_from_arms_map(left, peg_axis_big(axis, 3)?, out)
            }
            (false, false) => {
                self.collect_lazy_resolver_arms_from_arms_map(
                    left,
                    peg_axis_big(axis.clone(), 6)?,
                    out,
                )?;
                self.collect_lazy_resolver_arms_from_arms_map(right, peg_axis_big(axis, 7)?, out)
            }
        }
    }

    fn collect_lazy_resolver_arms_from_tomes_map(
        &mut self,
        tomes_map: Noun,
        axis: BigUint,
        out: &mut HashMap<BigUint, LazyResolverArmEntry>,
    ) -> Result<()> {
        let space = self.slab.noun_space();
        let Some((node, left, right)) = map_node(tomes_map, &self.slab.noun_space())? else {
            return Ok(());
        };
        let node_cell = node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("tome node not cell: {err}")))?;
        let tome_noun = node_cell.tail().noun();
        let tome_cell = tome_noun
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("tome value not cell: {err}")))?;
        let arms_map = tome_cell.tail().noun();
        let left_empty = noun_is_zero(left);
        let right_empty = noun_is_zero(right);
        let chapter_axis = if left_empty && right_empty {
            axis.clone()
        } else {
            peg_axis_big(axis.clone(), 2)?
        };
        self.collect_lazy_resolver_arms_from_arms_map(arms_map, chapter_axis, out)?;
        match (left_empty, right_empty) {
            (true, true) => Ok(()),
            (true, false) => {
                self.collect_lazy_resolver_arms_from_tomes_map(right, peg_axis_big(axis, 3)?, out)
            }
            (false, true) => {
                self.collect_lazy_resolver_arms_from_tomes_map(left, peg_axis_big(axis, 3)?, out)
            }
            (false, false) => {
                self.collect_lazy_resolver_arms_from_tomes_map(
                    left,
                    peg_axis_big(axis.clone(), 6)?,
                    out,
                )?;
                self.collect_lazy_resolver_arms_from_tomes_map(right, peg_axis_big(axis, 7)?, out)
            }
        }
    }

    fn build_tomes_battery_from_maps(
        &mut self,
        tomes_map: Noun,
        // The lazy core from `mint_core`, passed through to every arm's `mint`.
        core_type: NRc<NTy>,
        poly: Poly,
        goal_for_arms: Noun,
        expected_tomes_map: Option<Noun>,
    ) -> Result<Noun> {
        let space = self.slab.noun_space();
        let Some((node, left, right)) = map_node(tomes_map, &self.slab.noun_space())? else {
            return Ok(D(0));
        };
        let node_cell = node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("tome node not cell: {err}")))?;
        let tome_noun = node_cell.tail().noun();
        let tome_cell = tome_noun
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("tome value not cell: {err}")))?;
        let arms_map = tome_cell.tail().noun();
        let chapter_key = node_cell.head().noun();
        let expected_arms_map =
            self.goal_chapter_expected_arms_map(expected_tomes_map, chapter_key)?;
        if let Some(expected_arms_map) = expected_arms_map {
            let actual_count = self.map_entry_count(arms_map)?;
            let expected_count = self.map_entry_count(expected_arms_map)?;
            if actual_count != expected_count {
                return Err(CompilerError::Noun("core-number-of-arms".to_string()));
            }
        }
        let chapter_battery = self.build_arms_battery_from_map(
            arms_map,
            core_type.clone(),
            poly,
            goal_for_arms,
            expected_arms_map,
        )?;

        let left_empty = noun_is_zero(left);
        let right_empty = noun_is_zero(right);
        if left_empty && right_empty {
            return Ok(chapter_battery);
        }

        if left_empty {
            let right_bat = self.build_tomes_battery_from_maps(
                right, core_type, poly, goal_for_arms, expected_tomes_map,
            )?;
            return Ok(T(self.slab, &[chapter_battery, right_bat]));
        }
        if right_empty {
            let left_bat = self.build_tomes_battery_from_maps(
                left, core_type, poly, goal_for_arms, expected_tomes_map,
            )?;
            return Ok(T(self.slab, &[chapter_battery, left_bat]));
        }

        let left_bat = self.build_tomes_battery_from_maps(
            left,
            core_type.clone(),
            poly,
            goal_for_arms,
            expected_tomes_map,
        )?;
        let right_bat = self.build_tomes_battery_from_maps(
            right, core_type, poly, goal_for_arms, expected_tomes_map,
        )?;
        Ok(T(self.slab, &[chapter_battery, left_bat, right_bat]))
    }

    fn build_arm_formula_direct(
        &mut self,
        key: Arc<str>,
        core_type: NRc<NTy>,
        poly: Poly,
        goal: NRc<NTy>,
        hoon: &Hoon,
        hoon_noun: Noun,
    ) -> Result<FormulaId> {
        fn with_arm_context(key: &str, err: CompilerError) -> CompilerError {
            match err {
                CompilerError::UnsupportedExpr(s) => {
                    CompilerError::UnsupportedExpr(format!("arm {key}: {s}"))
                }
                CompilerError::Backend(s) => CompilerError::Backend(format!("arm {key}: {s}")),
                CompilerError::Decode(s) => CompilerError::Decode(format!("arm {key}: {s}")),
                CompilerError::Noun(s) => CompilerError::Noun(format!("arm {key}: {s}")),
                CompilerError::Parse(s) => CompilerError::Parse(format!("arm {key}: {s}")),
                CompilerError::Io(err) => CompilerError::Io(err),
                CompilerError::Detailed {
                    kind,
                    message,
                    metadata,
                } => CompilerError::Detailed {
                    kind,
                    message: format!("arm {key}: {message}"),
                    metadata,
                },
            }
        }

        let skip_vet = poly == Poly::Wet;
        let prev_vet = self.vet;
        let arm_vet = if skip_vet { false } else { prev_vet };
        self.vet = arm_vet;

        // The in-progress key uses the core's canonical type ID.
        let core_type_id = native_type_id(&core_type);
        let in_progress_key = (Arc::clone(&key), core_type_id);
        let in_progress_entry = ArmInProgressEntry {
            key: Arc::clone(&key),
            core: core_type.clone(),
            hoon: hoon_noun,
            goal: goal.clone(),
            vet: arm_vet,
        };
        self.arm_in_progress.insert(in_progress_key.clone());
        self.arm_goal_in_progress.push(in_progress_entry);
        self.arm_epoch = ArmEpoch(self.arm_epoch.0.wrapping_add(1));
        let result = self.mint(core_type.clone(), goal.clone(), hoon);
        self.vet = prev_vet;
        self.arm_in_progress.remove(&in_progress_key);
        let popped = self.arm_goal_in_progress.pop();
        debug_assert_eq!(
            popped.map(|entry| (
                entry.key,
                entry.core.arena_id(),
                NounIdentity::of(entry.hoon),
                entry.goal.arena_id(),
                entry.vet,
            )),
            Some((
                Arc::clone(&key),
                core_type.arena_id(),
                NounIdentity::of(hoon_noun),
                goal.arena_id(),
                arm_vet,
            ))
        );
        self.arm_epoch = ArmEpoch(self.arm_epoch.0.wrapping_add(1));
        let (ty, formula) = match result {
            Ok(ok) => ok,
            Err(err) => {
                return Err(with_arm_context(key.as_ref(), err));
            }
        };
        // `prune_recursive_holds` takes a noun type. `ty` is the arm's result, not the
        // deepening core, so lowering it per arm is bounded.
        let ty_noun = live_to_noun(&mut self.cx, &ty, self.slab);
        let _ty = self.prune_recursive_holds(ty_noun, hoon_noun)?;

        Ok(formula)
    }

    fn build_arms_battery_from_map(
        &mut self,
        arms_map: Noun,
        // `goal` is the goal core as a noun, the play subject for
        // `goal_arm_expected_type`; that helper produces each arm's mint goal.
        core_type: NRc<NTy>,
        poly: Poly,
        goal: Noun,
        expected_arms_map: Option<Noun>,
    ) -> Result<Noun> {
        let space = self.slab.noun_space();
        let Some((node, left, right)) = map_node(arms_map, &self.slab.noun_space())? else {
            return Ok(D(0));
        };
        let node_cell = node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("arm node not cell: {err}")))?;
        let key_noun = node_cell.head().noun();
        let key = self.arm_key_term(key_noun)?;
        let hoon_noun = node_cell.tail().noun();
        let hoon = self
            .hoon_ast_lookup_result(hoon_noun)
            .map_err(|err| CompilerError::Noun(format!("arm ast missing: {err}")))?;
        let arm_goal = self.goal_arm_expected_type(goal, expected_arms_map, key_noun)?;

        let formula = self.build_arm_formula_direct(
            Arc::clone(&key),
            core_type.clone(),
            poly,
            arm_goal,
            hoon.as_ref(),
            hoon_noun,
        )?;

        let formula = self.formula_materialize(formula);
        let left_empty = noun_is_zero(left);
        let right_empty = noun_is_zero(right);
        if left_empty && right_empty {
            return Ok(formula);
        }

        if left_empty {
            let right_bat =
                self.build_arms_battery_from_map(right, core_type, poly, goal, expected_arms_map)?;
            return Ok(T(self.slab, &[formula, right_bat]));
        }
        if right_empty {
            let left_bat =
                self.build_arms_battery_from_map(left, core_type, poly, goal, expected_arms_map)?;
            return Ok(T(self.slab, &[formula, left_bat]));
        }

        let left_bat = self.build_arms_battery_from_map(
            left,
            core_type.clone(),
            poly,
            goal,
            expected_arms_map,
        )?;
        let right_bat =
            self.build_arms_battery_from_map(right, core_type, poly, goal, expected_arms_map)?;
        Ok(T(self.slab, &[formula, left_bat, right_bat]))
    }

    fn semi_noun_full(&mut self, noun: Noun) -> Noun {
        let full = term_to_noun(self.slab, "full");
        let blocks = D(0);
        let stencil = T(self.slab, &[full, blocks]);
        T(self.slab, &[stencil, noun])
    }

    fn semi_noun_blocked(&mut self) -> Noun {
        self.semi_noun_blocked_encoded()
    }

    fn mint_opened(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        gen: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let opened = self.open_cached(gen);
        if let Some(opened) = opened {
            return self.mint(sut, gol, opened.as_ref());
        }
        Err(CompilerError::UnsupportedExpr(format!(
            "native mint: unsupported {gen:?}"
        )))
    }

    fn mint_dbug(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spot: &Spot,
        inner: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let location = Self::location_from_spot(spot);
        self.dbug_locations.push(location);
        let result = self.mint(sut, gol, inner);
        self.dbug_locations.pop();
        let (ty, formula) = result?;
        let spot_noun = spot_to_noun(self.slab, spot)?;
        let hint_inner = T(self.slab, &[D(1), spot_noun]);
        let spot_tag = term_to_noun(self.slab, "spot");
        let hint = T(self.slab, &[spot_tag, hint_inner]);
        let space = self.slab.noun_space();
        let formula = self.formula_arena.hint(hint, formula, &space);
        Ok((ty, formula))
    }

    fn mint_note(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        note: &Note,
        inner: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let (payload_ty, formula) = self.mint(sut.clone(), gol, inner)?;
        let note_noun = note_to_noun(self.slab, note)?;
        let hinted = self.hint_type(sut, note_noun, payload_ty)?;
        Ok((hinted, formula))
    }

    fn mint_lost(&mut self, sut: NRc<NTy>, gol: NRc<NTy>) -> Result<(NRc<NTy>, FormulaId)> {
        if self.vet {
            return Err(CompilerError::Noun("mint-lost".to_string()));
        }
        let ty = cons_void(&mut self.cx);
        let ty = self.nice(sut, gol, ty)?;
        let formula = self.formula_slot_u64(0);
        Ok((ty, formula))
    }

    fn play_opened(&mut self, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        let opened = open(gen.clone());
        if &opened != gen {
            return self.play(sut, &opened);
        }
        Err(CompilerError::UnsupportedExpr(format!(
            "native play: unsupported {gen:?}"
        )))
    }

    fn mint_wtkt(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        q: &Hoon,
        r: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let test = Hoon::WutTis(
            Box::new(Spec::Base(BaseType::Atom("$".to_string()))),
            wing.clone(),
        );
        // Match hoon-138 open() lowering: wtkt -> wtcl(wtts atom p, r, q)
        let expanded = Hoon::WutCol(Box::new(test), Box::new(r.clone()), Box::new(q.clone()));
        self.mint(sut, gol, &expanded)
    }

    fn mint_wtzp(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
    ) -> Result<(NRc<NTy>, FormulaId)> {
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let (_p_ty, p_formula) = self.mint(sut.clone(), bool_ty.clone(), p)?;
        let false_formula = self.formula_quote(D(1));
        let true_formula = self.formula_quote(D(0));
        let formula = self.formula_cond(p_formula, false_formula, true_formula);
        let ty = self.nice(sut, gol, bool_ty)?;
        Ok((ty, formula))
    }

    fn play_rock(&mut self, aura: &str, expr: &NounExpr) -> NRc<NTy> {
        match expr {
            NounExpr::ParsedAtom(atom) => {
                let value = parsed_atom_to_noun(self.slab, atom);
                ty_atom_n(&mut self.cx, self.slab, aura, Some(value)).1
            }
            NounExpr::Cell(head, tail) => {
                let head = self.play_rock(aura, head);
                let tail = self.play_rock(aura, tail);
                cons_cell(&mut self.cx, head, tail)
            }
        }
    }

    fn play_sand(&mut self, aura: &str, expr: &NounExpr) -> Result<NRc<NTy>> {
        match expr {
            NounExpr::ParsedAtom(atom) => {
                if aura == "n" {
                    if !atom_is_zero(atom) {
                        return Err(CompilerError::Noun("sand-null".to_string()));
                    }
                    let value = parsed_atom_to_noun(self.slab, atom);
                    return Ok(ty_atom_n(&mut self.cx, self.slab, aura, Some(value)).1);
                }
                if aura == "f" {
                    if !atom_is_flag(atom) {
                        return Err(CompilerError::Noun("sand-flag".to_string()));
                    }
                    return Ok(ty_bool_n(&mut self.cx, self.slab).1);
                }
                Ok(ty_atom_n(&mut self.cx, self.slab, aura, None).1)
            }
            _ => Ok(self.play_rock(aura, expr)),
        }
    }

    fn nice(&mut self, _sut: NRc<NTy>, gol: NRc<NTy>, typ: NRc<NTy>) -> Result<NRc<NTy>> {
        // Mirrors `mull_nice`. Returns `typ` unchanged on success.
        if !self.vet {
            return Ok(typ);
        }
        if matches!(&*gol, NTy::Noun)
            || NRc::ptr_eq(&gol, &typ)
            || self.nest(gol.clone(), typ.clone())?
        {
            return Ok(typ);
        }
        Err(CompilerError::Noun("mint-nice".to_string()))
    }

    fn hint_type(&mut self, inner: NRc<NTy>, note: Noun, payload: NRc<NTy>) -> Result<NRc<NTy>> {
        // The hint head is the noun pair `[inner note]`. A void or noun payload is
        // returned unchanged.
        match &*payload {
            NTy::Void | NTy::Noun => return Ok(payload),
            _ => {}
        }
        let inner_noun = live_to_noun(&mut self.cx, &inner, self.slab);
        let head = T(self.slab, &[inner_noun, note]);
        let head_leaf = live_leaf_from_noun(&mut self.cx, head, &self.slab.noun_space());
        Ok(cons_hint(&mut self.cx, head_leaf, payload))
    }

    /// Test helper: `hint_type` over a (noun, native) pair. On a void or noun
    /// payload hoon-138 returns `payload` itself, so both halves pass through.
    #[cfg(test)]
    fn hint_type_n(
        &mut self,
        inner: Noun,
        note: Noun,
        payload: (Noun, NRc<NTy>),
    ) -> Result<(Noun, NRc<NTy>)> {
        let tag = type_tag(payload.0, &self.slab.noun_space())?;
        if tag == "void" || tag == "noun" {
            return Ok(payload);
        }
        Ok(ty_hint_n(&mut self.cx, self.slab, inner, note, payload))
    }

    fn hoon_ast_lookup_cached(&mut self, hoon_noun: Noun) -> Option<Arc<Hoon>> {
        if !self.exact_hoon_ast_lookup_enabled {
            return None;
        }

        let hoon_raw = NounIdentity::of(hoon_noun);
        if let Some(ast) = self.hoon_cache_raw.get(&hoon_raw).cloned() {
            return Some(ast);
        }
        if let Some(ast) = self.decoded_hold_hoon_cache_raw.get(&hoon_raw).cloned() {
            return Some(ast);
        }
        let hoon_mug = self.noun_mug_cached(hoon_noun);
        let bucket = self.hoon_cache_struct.get(&hoon_mug)?;
        for (cached_noun, cached_ast) in bucket.iter().rev() {
            if NounIdentity::of(cached_noun) == hoon_raw {
                return Some(Arc::clone(cached_ast));
            }
            if let Ok(true) = noun_eq(*cached_noun, hoon_noun, &self.slab.noun_space()) {
                return Some(Arc::clone(cached_ast));
            }
        }
        None
    }

    fn decode_hold_hoon_ast(&mut self, hoon_noun: Noun) -> std::result::Result<Arc<Hoon>, String> {
        let hoon_raw = NounIdentity::of(hoon_noun);
        if let Some(ast) = self.decoded_hold_hoon_cache_raw.get(&hoon_raw).cloned() {
            return Ok(ast);
        }
        let space = self.slab.noun_space();
        let ast = Arc::new(noun_to_hoon(hoon_noun.in_space(&space))?);
        if !self.decoded_hold_hoon_cache_raw.contains_key(&hoon_raw) {
            self.decoded_hold_hoon_cache_order.push_back(hoon_raw);
            if self.decoded_hold_hoon_cache_order.len() > Self::HOON_CACHE_RAW_KEY_LIMIT {
                if let Some(evict) = self.decoded_hold_hoon_cache_order.pop_front() {
                    self.decoded_hold_hoon_cache_raw.remove(&evict);
                    self.decoded_hold_hoon_ptr_cache
                        .retain(|_, (raw, _)| *raw != Some(evict));
                }
            }
        }
        self.decoded_hold_hoon_cache_raw
            .insert(hoon_raw, Arc::clone(&ast));
        self.decoded_hold_hoon_ptr_cache.insert(
            Self::hoon_ast_ptr_key(ast.as_ref()),
            (Some(hoon_raw), hoon_noun),
        );
        Ok(ast)
    }

    fn hoon_ast_lookup_result(
        &mut self,
        hoon_noun: Noun,
    ) -> std::result::Result<Arc<Hoon>, String> {
        if let Some(ast) = self.hoon_ast_lookup_cached(hoon_noun) {
            return Ok(ast);
        }
        self.decode_hold_hoon_ast(hoon_noun)
    }

    fn hoon_ast_lookup(&mut self, hoon_noun: Noun) -> Option<Arc<Hoon>> {
        self.hoon_ast_lookup_result(hoon_noun).ok()
    }

    fn hoon_noun_for_node(&mut self, gen: &Hoon) -> Noun {
        let ptr = Self::hoon_ast_ptr_key(gen);
        if self.exact_hoon_ast_lookup_enabled {
            if let Some((_raw, noun)) = self.decoded_hold_hoon_ptr_cache.get(&ptr).copied() {
                return noun;
            }
        }
        let Some(id) = self.hoon_arena.id_for(gen) else {
            return hoon_to_noun(self.slab, gen);
        };
        if let Some(noun) = self.hoon_arena.entry(id).noun {
            return noun;
        }
        let by_ptr = &self.hoon_arena.by_ptr;
        let entries = &mut self.hoon_arena.entries;
        hoon_to_noun_with_cache(self.slab, gen, |ptr, noun| {
            if let Some(id) = by_ptr.get(&HoonIdentity(ptr)) {
                entries[id.0 as usize].noun = Some(noun);
            }
        })
    }

    fn prune_recursive_holds(&mut self, typ: Noun, hoon_noun: Noun) -> Result<Noun> {
        // Large recursive molds make this traversal very deep, so it uses an explicit
        // stack to avoid Rust stack overflows.
        let mut seen: HashSet<NounIdentity> = HashSet::new();
        let mut todo: Vec<Noun> = vec![typ];
        let mut post: Vec<Noun> = Vec::new();
        while let Some(node) = todo.pop() {
            let raw = NounIdentity::of(node);
            if !seen.insert(raw) {
                continue;
            }
            post.push(node);
            match type_tag(node, &self.slab.noun_space())?.as_str() {
                "fork" => {
                    for option in type_fork_options(node, &self.slab.noun_space())? {
                        todo.push(option);
                    }
                }
                "cell" => {
                    let (head, tail) = type_cell_parts(node, &self.slab.noun_space())?;
                    todo.push(head);
                    todo.push(tail);
                }
                "face" => {
                    todo.push(type_face_inner(node, &self.slab.noun_space())?);
                }
                "hint" => {
                    let (_inner, _note, payload) = type_hint_parts(node, &self.slab.noun_space())?;
                    todo.push(payload);
                }
                "core" => {
                    let (payload, coil) = type_core_parts(node, &self.slab.noun_space())?;
                    let (_garb, context, _rest) = coil_parts(coil, &self.slab.noun_space())?;
                    todo.push(payload);
                    todo.push(context);
                }
                _ => {}
            }
        }
        let mut memo: HashMap<NounIdentity, Noun> =
            HashMap::with_capacity(post.len().saturating_mul(2));
        for node in post.into_iter().rev() {
            let raw = NounIdentity::of(node);
            let result = match type_tag(node, &self.slab.noun_space())?.as_str() {
                "hold" => {
                    let _ = hoon_noun;
                    let (_inner, _hoon) = type_hold_parts(node, &self.slab.noun_space())?;
                    node
                }
                "fork" => {
                    let options = type_fork_options(node, &self.slab.noun_space())?;
                    let mut kept = Vec::with_capacity(options.len());
                    for option in options {
                        let opt_raw = NounIdentity::of(option);
                        let pruned = memo.get(&opt_raw).copied().unwrap_or(option);
                        if type_tag(pruned, &self.slab.noun_space())? == "void" {
                            continue;
                        }
                        kept.push(pruned);
                    }
                    match kept.len() {
                        0 => ty_void(self.slab),
                        1 => kept[0],
                        _ => self.fork_from_options(kept)?,
                    }
                }
                "cell" => {
                    let (head, tail) = type_cell_parts(node, &self.slab.noun_space())?;
                    let head_raw = NounIdentity::of(head);
                    let tail_raw = NounIdentity::of(tail);
                    let head = memo.get(&head_raw).copied().unwrap_or(head);
                    let tail = memo.get(&tail_raw).copied().unwrap_or(tail);
                    ty_cell(self.slab, head, tail)
                }
                "face" => {
                    let inner = type_face_inner(node, &self.slab.noun_space())?;
                    let inner_raw = NounIdentity::of(inner);
                    let inner = memo.get(&inner_raw).copied().unwrap_or(inner);
                    type_face_with_inner(self.slab, node, inner)?
                }
                "hint" => {
                    let (inner, note, payload) = type_hint_parts(node, &self.slab.noun_space())?;
                    let payload_raw = NounIdentity::of(payload);
                    let payload = memo.get(&payload_raw).copied().unwrap_or(payload);
                    ty_hint(self.slab, inner, note, payload)
                }
                "core" => {
                    let (payload, coil) = type_core_parts(node, &self.slab.noun_space())?;
                    let (garb, context, rest) = coil_parts(coil, &self.slab.noun_space())?;
                    let payload_raw = NounIdentity::of(payload);
                    let context_raw = NounIdentity::of(context);
                    let payload = memo.get(&payload_raw).copied().unwrap_or(payload);
                    let context = memo.get(&context_raw).copied().unwrap_or(context);
                    let new_coil = coil_from_parts(self.slab, garb, context, rest);
                    ty_core(self.slab, payload, new_coil)
                }
                _ => node,
            };
            memo.insert(raw, result);
        }
        let root_raw = NounIdentity::of(typ);
        Ok(*memo.get(&root_raw).unwrap_or(&typ))
    }

    fn nest(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<bool> {
        // Walks native types. Cell, face, hint, and core children stay native; leaf
        // parts (core rest, fork set, atoms) are lowered through the memoized
        // `live_to_noun`/`live_leaf_to_noun` and decoded with the noun helpers. The
        // seen-hold, gil, and memo sets key on canonical type IDs. The boundary cache
        // in intern.rs is keyed natively, since keying on noun mugs would lower the
        // deepening subject on every call.
        let semantic = self.semantic_context_key();
        // nest descends into and repos `%hold`s on both sut and ref, so the fan is
        // scoped on the union of both legsets.
        let fan = self.fan_context_key_scoped_pair(&sut, &ref_)?;
        if let Some(cached) = nest_cache_lookup(&self.cx, &sut, &ref_, semantic.vet_key, fan) {
            return Ok(cached);
        }
        let mut seen_sut_holds = NestSeenSet::new();
        let mut seen_ref_holds = NestSeenSet::new();
        let mut gil = NestPairSet::new();
        let mut memo: FastHashMap<NestMemoKey, bool> = Default::default();
        let result = self.nest_inner(
            sut.clone(),
            ref_.clone(),
            0,
            &mut seen_sut_holds,
            &mut seen_ref_holds,
            &mut gil,
            &mut memo,
        )?;
        // The top-level call always has empty seg and reg, so every result is cacheable.
        let seg_empty = true;
        let reg_empty = true;
        let cacheable = (result && reg_empty) || (!result && seg_empty);
        if cacheable {
            nest_cache_store(&mut self.cx, &sut, &ref_, semantic.vet_key, fan, result);
        }
        Ok(result)
    }

    /// `nest` on noun types: decodes both with `native_of` and runs the native `nest`.
    fn nest_noun(&mut self, sut: Noun, ref_: Noun) -> Result<bool> {
        let space = self.slab.noun_space();
        let sut_n = native_of(&mut self.cx, sut, &space)?;
        let ref_n = native_of(&mut self.cx, ref_, &space)?;
        self.nest(sut_n, ref_n)
    }

    /// Returns the native children of a `%fork`. The first traversal decodes them
    /// transiently; the second caches the vector on the fork node, so later walks
    /// read a slice. The Hoon set treap stays attached for serialization.
    fn fork_options_native<'fork>(
        &mut self,
        fork: &'fork NRc<NTy>,
    ) -> Result<NativeForkOptions<'fork>> {
        let set = match &**fork {
            NTy::Fork { set, options, .. } => {
                if let Some(options) = options.get() {
                    return Ok(NativeForkOptions::Cached(options));
                }
                set.clone()
            }
            _ => {
                return Err(CompilerError::Decode(
                    "native fork options requested for non-fork type".to_string(),
                ))
            }
        };

        let set_noun = live_leaf_to_noun(&mut self.cx, &set, self.slab);
        let space = self.slab.noun_space();
        let mut native_members: SmallVec<[NRc<NTy>; 4]> = SmallVec::new();
        visit_fork_set_members(set_noun, &space, |member| {
            native_members.push(native_of(&mut self.cx, member, &space)?);
            Ok(())
        })?;
        let NTy::Fork {
            options,
            options_seen,
            ..
        } = &**fork
        else {
            unreachable!()
        };
        if !options_seen.replace(true) {
            return Ok(NativeForkOptions::Transient(native_members));
        }
        let _ = options.set(native_members.into_vec());
        Ok(NativeForkOptions::Cached(
            options
                .get()
                .expect("fork options populated after repeated traversal"),
        ))
    }

    /// Builds hoon-138's `dox` core, `[%core q.q.p q.p(r.p %gold)]`: the context
    /// becomes the payload and the vair is forced to gold. Nothing is lowered.
    fn core_dox_native(
        &mut self,
        garb: &NGarb,
        context: &NRc<NTy>,
        rest: &NLeaf,
    ) -> Result<NRc<NTy>> {
        let new_garb = garb.with_vair(Vair::Gold);
        Ok(cons_core(
            &mut self.cx,
            context.clone(),
            new_garb,
            context.clone(),
            rest.clone(),
        ))
    }

    fn nest_inner(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        self.nest_inner_impl(sut, ref_, depth, seen_sut_holds, seen_ref_holds, gil, memo)
    }

    fn nest_inner_impl(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        let next_depth = depth.saturating_add(1);
        (|| -> Result<bool> {
            if NRc::ptr_eq(&sut, &ref_) {
                return Ok(true);
            }
            let memo_key = NestMemoKey {
                sut_id: native_type_id(&sut),
                ref_id: native_type_id(&ref_),
                seg: seen_sut_holds.snapshot(),
                reg: seen_ref_holds.snapshot(),
                gil: gil.snapshot(),
            };
            if let Some(cached) = memo.get(&memo_key).copied() {
                return Ok(cached);
            }
            let result = match &*sut {
                NTy::Void => self.nest_sint(
                    sut.clone(),
                    ref_.clone(),
                    next_depth,
                    seen_sut_holds,
                    seen_ref_holds,
                    gil,
                    memo,
                ),
                NTy::Noun => Ok(true),
                NTy::Atom { .. } => match &*ref_ {
                    NTy::Atom { .. } => self.atom_nest(sut.clone(), ref_.clone()),
                    _ => self.nest_sint(
                        sut.clone(),
                        ref_.clone(),
                        next_depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    ),
                },
                NTy::Cell(s_head, s_tail) => match &*ref_ {
                    NTy::Cell(r_head, r_tail) => {
                        let s_head = s_head.clone();
                        let s_tail = s_tail.clone();
                        let r_head = r_head.clone();
                        let r_tail = r_tail.clone();
                        // Canonical ++nest resets seg/reg at each %cell child descent.
                        let mut branch_sut_holds = NestSeenSet::new();
                        let mut branch_ref_holds = NestSeenSet::new();
                        if !self.nest_inner(
                            s_head, r_head, next_depth, &mut branch_sut_holds,
                            &mut branch_ref_holds, gil, memo,
                        )? {
                            return Ok(false);
                        }
                        branch_sut_holds.clear();
                        branch_ref_holds.clear();
                        self.nest_inner(
                            s_tail, r_tail, next_depth, &mut branch_sut_holds,
                            &mut branch_ref_holds, gil, memo,
                        )
                    }
                    _ => self.nest_sint(
                        sut.clone(),
                        ref_.clone(),
                        next_depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    ),
                },
                NTy::Core { .. } => match &*ref_ {
                    NTy::Core { .. } => self.nest_core(
                        sut.clone(),
                        ref_.clone(),
                        next_depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    ),
                    _ => self.nest_sint(
                        sut.clone(),
                        ref_.clone(),
                        next_depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    ),
                },
                NTy::Face { inner, .. } => {
                    let inner = inner.clone();
                    self.nest_inner(
                        inner,
                        ref_.clone(),
                        next_depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    )
                }
                NTy::Fork { .. } => {
                    let ref_is_simple = matches!(
                        &*ref_,
                        NTy::Atom { .. } | NTy::Noun | NTy::Cell(..) | NTy::Core { .. }
                    );
                    if !ref_is_simple {
                        return self.nest_sint(
                            sut.clone(),
                            ref_.clone(),
                            next_depth,
                            seen_sut_holds,
                            seen_ref_holds,
                            gil,
                            memo,
                        );
                    }
                    let options = self.fork_options_native(&sut)?;
                    for option in options {
                        let ok = self.nest_inner(
                            option,
                            ref_.clone(),
                            next_depth,
                            seen_sut_holds,
                            seen_ref_holds,
                            gil,
                            memo,
                        )?;
                        if ok {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }
                NTy::Hint { payload, .. } => {
                    let inner = payload.clone();
                    self.nest_inner(
                        inner,
                        ref_.clone(),
                        next_depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    )
                }
                NTy::Hold { .. } => {
                    let sut_id = native_type_id(&sut);
                    let ref_id = native_type_id(&ref_);
                    if !seen_sut_holds.insert_id(sut_id) {
                        return Ok(false);
                    }
                    if !gil.insert_id(sut_id, ref_id) {
                        let removed = seen_sut_holds.remove_id(sut_id);
                        debug_assert!(removed);
                        return Ok(true);
                    }
                    let result = (|| -> Result<bool> {
                        let inner = self.repo(sut.clone())?;
                        self.nest_inner(
                            inner,
                            ref_.clone(),
                            next_depth,
                            seen_sut_holds,
                            seen_ref_holds,
                            gil,
                            memo,
                        )
                    })();
                    let gil_removed = gil.remove_id(sut_id, ref_id);
                    debug_assert!(gil_removed);
                    let sut_removed = seen_sut_holds.remove_id(sut_id);
                    debug_assert!(sut_removed);
                    result
                }
            }?;
            memo.insert(memo_key, result);
            Ok(result)
        })()
    }

    fn nest_sint(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        match &*ref_ {
            NTy::Void => Ok(true),
            NTy::Noun | NTy::Atom { .. } | NTy::Cell(..) => Ok(false),
            NTy::Core { .. } => {
                let repo_ref = self.repo(ref_.clone())?;
                self.nest_inner(
                    sut, repo_ref, depth, seen_sut_holds, seen_ref_holds, gil, memo,
                )
            }
            NTy::Face { inner, .. } => {
                let inner = inner.clone();
                self.nest_inner(sut, inner, depth, seen_sut_holds, seen_ref_holds, gil, memo)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                for option in options {
                    if !self.nest_inner(
                        sut.clone(),
                        option,
                        depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            NTy::Hint { payload, .. } => {
                let inner = payload.clone();
                self.nest_inner(sut, inner, depth, seen_sut_holds, seen_ref_holds, gil, memo)
            }
            NTy::Hold { .. } => {
                let sut_id = native_type_id(&sut);
                let ref_id = native_type_id(&ref_);
                if !seen_ref_holds.insert_id(ref_id) {
                    return Ok(true);
                }
                if !gil.insert_id(sut_id, ref_id) {
                    let removed = seen_ref_holds.remove_id(ref_id);
                    debug_assert!(removed);
                    return Ok(true);
                }
                let result = (|| -> Result<bool> {
                    let repo_ref = self.repo(ref_.clone())?;
                    self.nest_inner(
                        sut.clone(),
                        repo_ref,
                        depth,
                        seen_sut_holds,
                        seen_ref_holds,
                        gil,
                        memo,
                    )
                })();
                let gil_removed = gil.remove_id(sut_id, ref_id);
                debug_assert!(gil_removed);
                let ref_removed = seen_ref_holds.remove_id(ref_id);
                debug_assert!(ref_removed);
                result
            }
        }
    }

    fn nest_core(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        let (sut_payload, sut_garb, sut_context_n, sut_rest) = match &*sut {
            NTy::Core {
                payload,
                garb,
                context,
                rest,
            } => (payload.clone(), garb.clone(), context.clone(), rest.clone()),
            _ => return Err(CompilerError::Noun("nest_core: sut not a core".to_string())),
        };
        let (ref_payload, ref_garb, ref_context_n, ref_rest) = match &*ref_ {
            NTy::Core {
                payload,
                garb,
                context,
                rest,
            } => (payload.clone(), garb.clone(), context.clone(), rest.clone()),
            _ => return Err(CompilerError::Noun("nest_core: ref not a core".to_string())),
        };
        // Equal coils reduce to a payload nest. Contexts are hash-consed, so pointer
        // equality is structural equality.
        if sut_garb == ref_garb
            && NRc::ptr_eq(&sut_context_n, &ref_context_n)
            && sut_rest == ref_rest
        {
            return self.nest_inner(
                sut_payload, ref_payload, depth, seen_sut_holds, seen_ref_holds, gil, memo,
            );
        }
        // Only the small rest leaves are lowered, for the noun tome decoders; the
        // contexts stay native.
        let sut_rest_noun = live_leaf_to_noun(&mut self.cx, &sut_rest, self.slab);
        let ref_rest_noun = live_leaf_to_noun(&mut self.cx, &ref_rest, self.slab);
        let sut_poly = sut_garb.poly;
        let ref_poly = ref_garb.poly;
        if sut_poly != ref_poly {
            return Ok(false);
        }
        if !self.nest_meet(
            sut_context_n.clone(),
            sut_payload,
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        if !self.nest_inner(
            ref_context_n.clone(),
            ref_payload,
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        let sut_vair = sut_garb.vair;
        let ref_vair = ref_garb.vair;
        if !self.deem_variance(
            sut_context_n.clone(),
            ref_context_n.clone(),
            sut_vair,
            ref_vair,
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        let sut_tomes = rest_tomes(sut_rest_noun, &self.slab.noun_space())?;
        let ref_tomes = rest_tomes(ref_rest_noun, &self.slab.noun_space())?;
        if sut_poly == Poly::Wet {
            if !noun_eq(sut_tomes, ref_tomes, &self.slab.noun_space())? {
                return Ok(false);
            }
            return Ok(true);
        }
        let sut_id = native_type_id(&sut);
        let ref_id = native_type_id(&ref_);
        if !gil.insert_id(sut_id, ref_id) {
            return Ok(true);
        }
        let result = (|| -> Result<bool> {
            let sut_dox = self.core_dox_native(&sut_garb, &sut_context_n, &sut_rest)?;
            let ref_dox = self.core_dox_native(&ref_garb, &ref_context_n, &ref_rest)?;
            self.nest_deep_tomes(
                sut_tomes, ref_tomes, sut_dox, ref_dox, depth, seen_sut_holds, seen_ref_holds, gil,
                memo,
            )
        })();
        let gil_removed = gil.remove_id(sut_id, ref_id);
        debug_assert!(gil_removed);
        result
    }

    fn deem_variance(
        &mut self,
        sut_ctx: NRc<NTy>,
        ref_ctx: NRc<NTy>,
        sut_vair: Vair,
        ref_vair: Vair,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        if sut_vair != ref_vair && sut_vair != Vair::Lead && ref_vair != Vair::Gold {
            return Ok(false);
        }
        match sut_vair {
            Vair::Lead => Ok(true),
            Vair::Gold => self.nest_meet(
                sut_ctx, ref_ctx, depth, seen_sut_holds, seen_ref_holds, gil, memo,
            ),
            Vair::Iron => {
                // Bootstrap `+nest` compares `%iron` in this orientation.
                let sut_peek = self.peek(ref_ctx, Way::Rite, 2u64)?;
                let ref_peek = self.peek(sut_ctx, Way::Rite, 2u64)?;
                self.nest_inner(
                    sut_peek, ref_peek, depth, seen_sut_holds, seen_ref_holds, gil, memo,
                )
            }
            Vair::Zinc => {
                let sut_peek = self.peek(sut_ctx, Way::Read, 2u64)?;
                let ref_peek = self.peek(ref_ctx, Way::Read, 2u64)?;
                self.nest_inner(
                    sut_peek, ref_peek, depth, seen_sut_holds, seen_ref_holds, gil, memo,
                )
            }
        }
    }

    fn nest_meet(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        if !self.nest_inner(
            sut.clone(),
            ref_.clone(),
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        self.nest_inner(ref_, sut, depth, seen_sut_holds, seen_ref_holds, gil, memo)
    }

    fn nest_deep_tomes(
        &mut self,
        dom: Noun,
        vim: Noun,
        sut_dox: NRc<NTy>,
        ref_dox: NRc<NTy>,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        let space = self.slab.noun_space();
        let (Some((dom_node, dom_left, dom_right)), Some((vim_node, vim_left, vim_right))) = (
            map_node(dom, &self.slab.noun_space())?,
            map_node(vim, &self.slab.noun_space())?,
        ) else {
            return Ok(noun_is_zero(dom) && noun_is_zero(vim));
        };
        let dom_node_cell = dom_node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("deep tome node not cell: {err}")))?;
        let vim_node_cell = vim_node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("deep tome node not cell: {err}")))?;
        let dom_key = dom_node_cell.head().noun();
        let vim_key = vim_node_cell.head().noun();
        if !noun_eq(dom_key, vim_key, &self.slab.noun_space())? {
            return Ok(false);
        }
        if !self.nest_deep_tomes(
            dom_left,
            vim_left,
            sut_dox.clone(),
            ref_dox.clone(),
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        if !self.nest_deep_tomes(
            dom_right,
            vim_right,
            sut_dox.clone(),
            ref_dox.clone(),
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        let dom_tome = dom_node_cell.tail().noun();
        let vim_tome = vim_node_cell.tail().noun();
        let dom_tome_cell = dom_tome
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("deep tome value not cell: {err}")))?;
        let vim_tome_cell = vim_tome
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("deep tome value not cell: {err}")))?;
        let dom_arms = dom_tome_cell.tail().noun();
        let vim_arms = vim_tome_cell.tail().noun();
        self.nest_deep_arms(
            dom_arms, vim_arms, sut_dox, ref_dox, depth, seen_sut_holds, seen_ref_holds, gil, memo,
        )
    }

    fn nest_deep_arms(
        &mut self,
        dab: Noun,
        hem: Noun,
        sut_dox: NRc<NTy>,
        ref_dox: NRc<NTy>,
        depth: usize,
        seen_sut_holds: &mut NestSeenSet,
        seen_ref_holds: &mut NestSeenSet,
        gil: &mut NestPairSet,
        memo: &mut FastHashMap<NestMemoKey, bool>,
    ) -> Result<bool> {
        let space = self.slab.noun_space();
        let (Some((dab_node, dab_left, dab_right)), Some((hem_node, hem_left, hem_right))) = (
            map_node(dab, &self.slab.noun_space())?,
            map_node(hem, &self.slab.noun_space())?,
        ) else {
            return Ok(noun_is_zero(dab) && noun_is_zero(hem));
        };
        let dab_node_cell = dab_node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("deep arm node not cell: {err}")))?;
        let hem_node_cell = hem_node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("deep arm node not cell: {err}")))?;
        let dab_key = dab_node_cell.head().noun();
        let hem_key = hem_node_cell.head().noun();
        if !noun_eq(dab_key, hem_key, &self.slab.noun_space())? {
            return Ok(false);
        }
        if !self.nest_deep_arms(
            dab_left,
            hem_left,
            sut_dox.clone(),
            ref_dox.clone(),
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        if !self.nest_deep_arms(
            dab_right,
            hem_right,
            sut_dox.clone(),
            ref_dox.clone(),
            depth,
            seen_sut_holds,
            seen_ref_holds,
            gil,
            memo,
        )? {
            return Ok(false);
        }
        let dab_hoon_noun = dab_node_cell.tail().noun();
        let hem_hoon_noun = hem_node_cell.tail().noun();
        let dab_hoon = self.hoon_ast_lookup_result(dab_hoon_noun).map_err(|err| {
            let tag = Self::hoon_noun_tag(dab_hoon_noun, &space)
                .unwrap_or_else(|| "<unknown>".to_string());
            CompilerError::Noun(format!(
                "native nest deep: arm ast missing tag={tag} decode_err={err}"
            ))
        })?;
        let hem_hoon = self.hoon_ast_lookup_result(hem_hoon_noun).map_err(|err| {
            let tag = Self::hoon_noun_tag(hem_hoon_noun, &space)
                .unwrap_or_else(|| "<unknown>".to_string());
            CompilerError::Noun(format!(
                "native nest deep: arm ast missing tag={tag} decode_err={err}"
            ))
        })?;
        let dab_ty = self.play(sut_dox.clone(), dab_hoon.as_ref())?;
        let hem_ty = self.play(ref_dox.clone(), hem_hoon.as_ref())?;
        self.nest_inner(
            dab_ty, hem_ty, depth, seen_sut_holds, seen_ref_holds, gil, memo,
        )
    }

    fn wrap_type(&mut self, typ: NRc<NTy>, vair: Vair) -> Result<NRc<NTy>> {
        // Rebuilds with the collapse-aware `cons_*` constructors.
        match &*typ {
            NTy::Cell(head, tail) => {
                let head = head.clone();
                let tail = tail.clone();
                let head = self.wrap_type(head, vair)?;
                let tail = self.wrap_type(tail, vair)?;
                Ok(cons_cell(&mut self.cx, head, tail))
            }
            NTy::Core {
                payload,
                garb,
                context,
                rest,
            } => {
                let payload = payload.clone();
                let context = context.clone();
                let rest = rest.clone();
                let current_vair = garb.vair;
                if current_vair != Vair::Gold && vair != Vair::Lead {
                    return Err(CompilerError::Noun("wrap-core".to_string()));
                }
                let new_garb = garb.with_vair(vair);
                Ok(cons_core(&mut self.cx, payload, new_garb, context, rest))
            }
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let inner = self.wrap_type(inner, vair)?;
                Ok(cons_face(&mut self.cx, tool, inner))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&typ)?;
                let mut wrapped = Vec::with_capacity(options.len());
                for option in options {
                    let w = self.wrap_type(option, vair)?;
                    wrapped.push(w);
                }
                self.cons_fork(wrapped)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let payload = self.wrap_type(payload, vair)?;
                Ok(cons_hint(&mut self.cx, head, payload))
            }
            NTy::Hold { .. } => {
                let r = self.repo(typ.clone())?;
                self.wrap_type(r, vair)
            }
            NTy::Void | NTy::Noun | NTy::Atom { .. } => Ok(typ.clone()),
        }
    }

    fn burp_fork_set_run(&mut self, set: Noun) -> Result<Noun> {
        let mut out = D(0);
        let mut stack = vec![set];
        while let Some(tree) = stack.pop() {
            if noun_is_zero(tree) {
                continue;
            }
            let (key, left, right) = set_parts(tree, &self.slab.noun_space())?;
            let key = self.burp_type(key)?;
            out = set_put_mug(self.slab, out, key)?;
            stack.push(right);
            stack.push(left);
        }
        Ok(out)
    }

    pub fn burp_type(&mut self, typ: Noun) -> Result<Noun> {
        let space = self.slab.noun_space();
        let raw = NounIdentity::of(typ);
        if let Some(cached) = self.burp_type_cache.get(&raw) {
            return Ok(*cached);
        }

        let tag = type_tag(typ, &self.slab.noun_space())?;
        let burped = match tag.as_str() {
            "cell" => {
                let (head, tail) = type_cell_parts(typ, &self.slab.noun_space())?;
                let head = self.burp_type(head)?;
                let tail = self.burp_type(tail)?;
                Ok(ty_cell(self.slab, head, tail))
            }
            "core" => {
                let (payload, coil) = type_core_parts(typ, &self.slab.noun_space())?;
                let (garb, context, rest) = coil_parts(coil, &self.slab.noun_space())?;
                let payload = self.burp_type(payload)?;
                let context = self.burp_type(context)?;
                let rest_cell = rest
                    .in_space(&space)
                    .as_cell()
                    .map_err(|err| CompilerError::Decode(format!("core rest not cell: {err}")))?;
                let semi = rest_cell.head().noun();
                let tomes = rest_cell.tail().noun();
                let semi = if self.semi_is_full_complete(semi)? {
                    // hoon-138 `++burp` keeps a `[%full ~]` coil seminoun as-is, so no
                    // `%spot` stripping happens here. hoonc keeps the spot on cores
                    // embedded in type specs and mold samples (such as the `++map` `$|`
                    // validator's `(tree (pair))` sample) but drops it on top-level
                    // output coil cores. honk spots both; the output-coil cores are a
                    // known self-mint divergence that apps do not hit.
                    semi
                } else {
                    // Canonical hoon-138 `++burp` replaces any unresolved seminoun state
                    // with a blocked `%full` seminoun.
                    self.semi_noun_blocked()
                };
                let new_rest = T(self.slab, &[semi, tomes]);
                let new_coil = coil_from_parts(self.slab, garb, context, new_rest);
                Ok(ty_core(self.slab, payload, new_coil))
            }
            "face" => {
                let inner = type_face_inner(typ, &self.slab.noun_space())?;
                let inner = self.burp_type(inner)?;
                type_face_with_inner(self.slab, typ, inner)
            }
            "fork" => {
                let set = type_fork_set(typ, &self.slab.noun_space())?;
                let set = self.burp_fork_set_run(set)?;
                let tag = term_to_noun(self.slab, "fork");
                Ok(T(self.slab, &[tag, set]))
            }
            "hint" => {
                let (inner, note, payload) = type_hint_parts(typ, &self.slab.noun_space())?;
                let inner = self.burp_type(inner)?;
                let payload = self.burp_type(payload)?;
                Ok(ty_hint(self.slab, inner, note, payload))
            }
            "hold" => {
                let (inner, hoon) = type_hold_parts(typ, &self.slab.noun_space())?;
                let inner = self.burp_type(inner)?;
                self.ty_hold_cached(inner, hoon)
            }
            _ => Ok(typ),
        }?;

        let burped = if noun_eq(typ, burped, &self.slab.noun_space())? {
            typ
        } else {
            burped
        };

        if self.burp_type_cache.len() < Self::BURP_TYPE_CACHE_LIMIT {
            self.burp_type_cache.insert(raw, burped);
        }

        Ok(burped)
    }

    fn semi_is_full_complete(&mut self, semi: Noun) -> Result<bool> {
        let space = self.slab.noun_space();
        let Ok(cell) = semi.in_space(&space).as_cell() else {
            return Ok(false);
        };
        let stencil = cell.head().noun();
        let Ok(stencil_cell) = stencil.in_space(&space).as_cell() else {
            return Ok(false);
        };
        let Ok(atom) = stencil_cell.head().as_atom() else {
            return Ok(false);
        };
        if atom_to_string(atom)
            .map_err(|err| CompilerError::Decode(format!("semi stencil: {err}")))?
            != "full"
        {
            return Ok(false);
        }
        Ok(noun_is_zero(stencil_cell.tail().noun()))
    }

    fn feel(&mut self, sut: NRc<NTy>, wings: &[WingType]) -> Result<bool> {
        let mut current = sut;
        for wing in wings.iter().rev() {
            let pony = self.fond(current, Way::Free, wing)?;
            let port = match pony {
                Pony::Void | Pony::Unmatched(_) => return Ok(false),
                Pony::Palo(palo) => Port::Palo(palo),
                Pony::Synthetic { typ, formula } => Port::Synthetic { typ, formula },
            };
            let (ty, _formula) = self.fine(&port)?;
            current = ty;
        }
        Ok(true)
    }

    fn take<F>(
        &mut self,
        sut: NRc<NTy>,
        vein: &[Option<BigUint>],
        duz: &F,
    ) -> Result<(BigUint, NRc<NTy>)>
    where
        F: Fn(&mut Self, NRc<NTy>) -> Result<NRc<NTy>>,
    {
        let axis = tend_big(vein)?;
        let vit: Vec<Option<BigUint>> = vein.iter().rev().cloned().collect();
        let ty = self.take_inner(sut, &vit, duz)?;
        Ok((axis, ty))
    }

    fn take_inner<F>(&mut self, sut: NRc<NTy>, vit: &[Option<BigUint>], duz: &F) -> Result<NRc<NTy>>
    where
        F: Fn(&mut Self, NRc<NTy>) -> Result<NRc<NTy>>,
    {
        let Some((head, tail)) = vit.split_first() else {
            return duz(self, sut);
        };
        self.take_inner_head_tail(sut, head.clone(), tail, duz)
    }

    fn take_inner_head_tail<F>(
        &mut self,
        sut: NRc<NTy>,
        head: Option<BigUint>,
        tail: &[Option<BigUint>],
        duz: &F,
    ) -> Result<NRc<NTy>>
    where
        F: Fn(&mut Self, NRc<NTy>) -> Result<NRc<NTy>>,
    {
        match head {
            None => match &*sut {
                NTy::Face { tool, inner } => {
                    let tool = tool.clone();
                    let inner = inner.clone();
                    let new_inner = self.take_inner(inner, tail, duz)?;
                    Ok(cons_face(&mut self.cx, tool, new_inner))
                }
                NTy::Hint { head: hd, payload } => {
                    let hd = hd.clone();
                    let payload = payload.clone();
                    let new_payload = self.take_inner_head_tail(payload, None, tail, duz)?;
                    Ok(cons_hint(&mut self.cx, hd, new_payload))
                }
                NTy::Fork { .. } => {
                    let options = self.fork_options_native(&sut)?;
                    let mut out = Vec::with_capacity(options.len());
                    for option in options {
                        let opt_ty = self.take_inner_head_tail(option, None, tail, duz)?;
                        out.push(opt_ty);
                    }
                    self.cons_fork(out)
                }
                NTy::Hold { .. } => {
                    let inner = self.repo(sut.clone())?;
                    self.take_inner_head_tail(inner, None, tail, duz)
                }
                _ => self.take_inner(sut, tail, duz),
            },
            Some(step) => {
                let mut vil: HashSet<TypeId> = HashSet::new();
                self.take_axis(sut, step, tail, duz, &mut vil)
            }
        }
    }

    fn take_axis<F>(
        &mut self,
        sut: NRc<NTy>,
        step: BigUint,
        tail: &[Option<BigUint>],
        duz: &F,
        vil: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>>
    where
        F: Fn(&mut Self, NRc<NTy>) -> Result<NRc<NTy>>,
    {
        if step == BigUint::from(1u32) {
            return self.take_inner(sut, tail, duz);
        }
        let (cap, mas) = axis_big_cap_mas(&step)?;
        match &*sut {
            NTy::Noun => {
                let noun = cons_noun(&mut self.cx);
                let cell = cons_cell(&mut self.cx, noun.clone(), noun);
                self.take_axis(cell, step, tail, duz, vil)
            }
            NTy::Void => Ok(cons_void(&mut self.cx)),
            NTy::Atom { .. } => Ok(cons_void(&mut self.cx)),
            NTy::Cell(head_ty, tail_ty) => {
                let head_ty = head_ty.clone();
                let tail_ty = tail_ty.clone();
                if cap == 2 {
                    let new_head = self.take_axis(head_ty, mas, tail, duz, vil)?;
                    Ok(cons_cell(&mut self.cx, new_head, tail_ty))
                } else {
                    let new_tail = self.take_axis(tail_ty, mas, tail, duz, vil)?;
                    Ok(cons_cell(&mut self.cx, head_ty, new_tail))
                }
            }
            NTy::Core {
                payload,
                garb,
                context,
                rest,
            } => {
                if cap == 2 {
                    let repo = self.repo(sut.clone())?;
                    self.take_axis(repo, step, tail, duz, vil)
                } else {
                    let payload = payload.clone();
                    let garb = garb.clone();
                    let context = context.clone();
                    let rest = rest.clone();
                    let new_payload = self.take_axis(payload, mas, tail, duz, vil)?;
                    Ok(cons_core(&mut self.cx, new_payload, garb, context, rest))
                }
            }
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let new_inner = self.take_axis(inner, step, tail, duz, vil)?;
                Ok(cons_face(&mut self.cx, tool, new_inner))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&sut)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt_ty = self.take_axis(option, step.clone(), tail, duz, vil)?;
                    out.push(opt_ty);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let new_payload = self.take_axis(payload, step, tail, duz, vil)?;
                Ok(cons_hint(&mut self.cx, head, new_payload))
            }
            NTy::Hold { .. } => {
                let sut_id = native_type_id(&sut);
                if !vil.insert(sut_id) {
                    return Ok(cons_void(&mut self.cx));
                }
                let result = (|| -> Result<NRc<NTy>> {
                    let inner = self.repo(sut.clone())?;
                    self.take_axis(inner, step, tail, duz, vil)
                })();
                let removed = vil.remove(&sut_id);
                debug_assert!(removed, "take %hold vil should unwind per branch");
                result
            }
        }
    }

    fn gain(&mut self, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        self.chip(true, sut, gen)
    }

    fn lose(&mut self, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        self.chip(false, sut, gen)
    }

    fn chip(&mut self, how: bool, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        match gen {
            // Source-location/debug wrappers are non-semantic.
            Hoon::Dbug(_, inner) | Hoon::Note(_, inner) => self.chip(how, sut, inner.as_ref()),
            Hoon::WutTis(spec, wing) => {
                let example = self.spec_example_cached(spec);
                let ref_type = self.play(sut.clone(), example.as_ref())?;
                self.cool(how, sut, wing, ref_type)
            }
            Hoon::WutHax(skin, wing) => {
                let port = self.find(sut.clone(), Way::Both, wing)?;
                let palo = match port {
                    Port::Palo(palo) => palo,
                    Port::Synthetic { .. } => return Ok(sut),
                };
                let sut_for_duz = sut.clone();
                let duz = |ut: &mut Self, a: NRc<NTy>| {
                    if how {
                        ut.gain_skin(sut_for_duz.clone(), a, skin)
                    } else {
                        ut.lose_skin(sut_for_duz.clone(), a, skin)
                    }
                };
                let (_axis, ty) = self.take(sut, &palo.vein, &duz)?;
                Ok(ty)
            }
            Hoon::WutPam(list) if how => {
                let mut acc = sut;
                for item in list {
                    acc = self.chip(how, acc, item)?;
                }
                Ok(acc)
            }
            Hoon::WutBar(list) if !how => {
                let mut acc = sut;
                for item in list {
                    acc = self.chip(how, acc, item)?;
                }
                Ok(acc)
            }
            _ => {
                if let Some(opened) = self.open_cached(gen) {
                    self.chip(how, sut, opened.as_ref())
                } else {
                    Ok(sut)
                }
            }
        }
    }

    fn cool(
        &mut self,
        pol: bool,
        sut: NRc<NTy>,
        wing: &WingType,
        ref_type: NRc<NTy>,
    ) -> Result<NRc<NTy>> {
        let port = self.find(sut.clone(), Way::Both, wing)?;
        let palo = match port {
            Port::Palo(palo) => palo,
            Port::Synthetic { .. } => return Ok(sut),
        };
        let ref_for_duz = ref_type.clone();
        let duz = |ut: &mut Self, a: NRc<NTy>| {
            if pol {
                ut.fuse(a, ref_for_duz.clone())
            } else {
                ut.crop(a, ref_for_duz.clone())
            }
        };
        let (_axis, ty) = self.take(sut.clone(), &palo.vein, &duz)?;
        let ty = if NRc::ptr_eq(&ty, &sut) { sut } else { ty };
        Ok(ty)
    }

    fn gain_skin(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>, skin: &Skin) -> Result<NRc<NTy>> {
        let mut seen: HashSet<TypeId> = HashSet::new();
        self.gain_skin_inner(sut, ref_, skin, &mut seen)
    }

    fn gain_skin_inner(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        skin: &Skin,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match skin {
            Skin::Term(name) => {
                let wing = vec![Limb::Term(name.clone())];
                let spec = Spec::Like(wing, Vec::new());
                let converted =
                    Skin::Spec(Box::new(spec), Box::new(Skin::Base(BaseType::NounExpr)));
                self.gain_skin_inner(sut, ref_, &converted, seen)
            }
            Skin::Base(base) => match base {
                BaseType::Cell => {
                    let head = Skin::Base(BaseType::NounExpr);
                    let tail = Skin::Base(BaseType::NounExpr);
                    let skin = Skin::Cell(Box::new(head), Box::new(tail));
                    self.gain_skin_inner(sut, ref_, &skin, seen)
                }
                BaseType::Flag => {
                    let yes = Skin::Leaf("f".to_string(), ParsedAtom::Small(0));
                    let no = Skin::Leaf("f".to_string(), ParsedAtom::Small(1));
                    let yes_ty = self.gain_skin_inner(sut.clone(), ref_.clone(), &yes, seen)?;
                    let no_ty = self.gain_skin_inner(sut, ref_, &no, seen)?;
                    self.cons_fork(vec![yes_ty, no_ty])
                }
                BaseType::Null => {
                    let skin = Skin::Leaf("n".to_string(), ParsedAtom::Small(0));
                    self.gain_skin_inner(sut, ref_, &skin, seen)
                }
                BaseType::Void => Ok(cons_void(&mut self.cx)),
                BaseType::NounExpr => {
                    let void = cons_void(&mut self.cx);
                    if self.nest(void, ref_.clone())? {
                        Ok(cons_void(&mut self.cx))
                    } else {
                        Ok(ref_)
                    }
                }
                BaseType::Atom(aura) => {
                    let mut seen_ref: HashSet<TypeId> = HashSet::new();
                    self.gain_atom_skin(sut, ref_, aura, &mut seen_ref)
                }
            },
            Skin::Cell(head, tail) => {
                let mut seen_ref: HashSet<TypeId> = HashSet::new();
                self.gain_cell_skin(sut, ref_, head, tail, &mut seen_ref)
            }
            Skin::Leaf(aura, atom) => {
                let mut seen_ref: HashSet<TypeId> = HashSet::new();
                self.gain_leaf_skin(sut, ref_, aura, atom, &mut seen_ref)
            }
            Skin::Dbug(_, inner) => self.gain_skin_inner(sut, ref_, inner, seen),
            Skin::Help(help, inner) => {
                let payload = self.gain_skin_inner(sut.clone(), ref_, inner, seen)?;
                let help_noun = noun_expr_to_noun(self.slab, help);
                let note_noun = tagged1(self.slab, "help", help_noun);
                // The hint head is `[sut note]`, as in `hint_type`; `cons_hint` applies
                // the same void/noun collapse.
                let sut_noun = live_to_noun(&mut self.cx, &sut, self.slab);
                let head_noun = T(self.slab, &[sut_noun, note_noun]);
                let head_leaf =
                    live_leaf_from_noun(&mut self.cx, head_noun, &self.slab.noun_space());
                Ok(cons_hint(&mut self.cx, head_leaf, payload))
            }
            Skin::Name(name, inner) => {
                let inner_ty = self.gain_skin_inner(sut, ref_, inner, seen)?;
                let tool_noun = term_to_noun(self.slab, name);
                let tool_leaf =
                    live_leaf_from_noun(&mut self.cx, tool_noun, &self.slab.noun_space());
                Ok(cons_face(&mut self.cx, tool_leaf, inner_ty))
            }
            Skin::Over(wing, inner) => {
                let next_sut = self.play(sut, &Hoon::Wing(wing.clone()))?;
                self.gain_skin_inner(next_sut, ref_, inner, seen)
            }
            Skin::Spec(spec, inner) => {
                let example = self.spec_example_cached(spec);
                let hit = self.play(sut.clone(), example.as_ref())?;
                let inner_ty = self.gain_skin_inner(sut, ref_.clone(), inner, seen)?;
                if !self.nest(hit.clone(), inner_ty)? {
                    return Err(CompilerError::Noun("native mint: gain spec".to_string()));
                }
                self.fuse(ref_, hit)
            }
            Skin::Wash(_) => Ok(ref_),
        }
    }

    fn gain_atom_skin(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        aura: &str,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Void => Ok(cons_void(&mut self.cx)),
            NTy::Noun => {
                let noun = ty_atom(self.slab, aura, None);
                native_of(&mut self.cx, noun, &self.slab.noun_space())
            }
            NTy::Atom { .. } => {
                let ref_noun = live_to_noun(&mut self.cx, &ref_, self.slab);
                let (ref_aura, ref_val) = type_atom_parts(ref_noun, &self.slab.noun_space())?;
                let aura_tag = if aura.is_empty() { "$" } else { aura };
                let aura_noun = term_to_noun(self.slab, aura_tag);
                let aura_any = aura_tag == "$" || aura_tag == "@";
                if !aura_any && !self.fitz(aura_noun, ref_aura)? {
                    return Err(CompilerError::Noun(
                        "native mint: atom-mismatch".to_string(),
                    ));
                }
                let max_aura = if aura_any {
                    ref_aura
                } else {
                    self.atom_max(aura_noun, ref_aura)?
                };
                let space = self.slab.noun_space();
                let aura_atom = max_aura
                    .in_space(&space)
                    .as_atom()
                    .map_err(|err| CompilerError::Decode(format!("atom aura: {err}")))?;
                let aura_str = atom_to_string(aura_atom)
                    .map_err(|err| CompilerError::Decode(format!("atom aura: {err}")))?;
                let noun = ty_atom(self.slab, &aura_str, ref_val);
                native_of(&mut self.cx, noun, &self.slab.noun_space())
            }
            NTy::Cell(..) => Ok(cons_void(&mut self.cx)),
            NTy::Core { .. } => Ok(cons_void(&mut self.cx)),
            NTy::Face { inner, .. } => {
                let inner = inner.clone();
                self.gain_atom_skin(sut, inner, aura, seen)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = self.gain_atom_skin(sut.clone(), option, aura, seen)?;
                    out.push(opt);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let payload = self.gain_atom_skin(sut, payload, aura, seen)?;
                Ok(cons_hint(&mut self.cx, head, payload))
            }
            NTy::Hold { .. } => {
                let ref_id = native_type_id(&ref_);
                if !seen.insert(ref_id) {
                    return Ok(cons_void(&mut self.cx));
                }
                let result = self
                    .repo(ref_.clone())
                    .and_then(|inner| self.gain_atom_skin(sut, inner, aura, seen));
                seen.remove(&ref_id);
                result
            }
        }
    }

    fn gain_cell_skin(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        head: &Skin,
        tail: &Skin,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Void => Ok(cons_void(&mut self.cx)),
            NTy::Noun => {
                let head_ty = self.gain_skin_inner(sut.clone(), ref_.clone(), head, seen)?;
                if matches!(&*head_ty, NTy::Void) {
                    return Ok(cons_void(&mut self.cx));
                }
                let tail_ty = self.gain_skin_inner(sut, ref_, tail, seen)?;
                Ok(cons_cell(&mut self.cx, head_ty, tail_ty))
            }
            NTy::Atom { .. } => Ok(cons_void(&mut self.cx)),
            NTy::Cell(ref_head, ref_tail) => {
                let ref_head = ref_head.clone();
                let ref_tail = ref_tail.clone();
                let head_ty = self.gain_skin_inner(sut.clone(), ref_head, head, seen)?;
                if matches!(&*head_ty, NTy::Void) {
                    return Ok(cons_void(&mut self.cx));
                }
                let tail_ty = self.gain_skin_inner(sut, ref_tail, tail, seen)?;
                Ok(cons_cell(&mut self.cx, head_ty, tail_ty))
            }
            NTy::Core {
                payload,
                garb,
                context,
                rest,
            } => {
                let payload = payload.clone();
                let garb = garb.clone();
                let context = context.clone();
                let rest = rest.clone();
                let head_ty = self.gain_skin_inner(sut.clone(), payload, head, seen)?;
                if matches!(&*head_ty, NTy::Void) {
                    return Ok(cons_void(&mut self.cx));
                }
                // hoon-138 `ar:gain` preserves a core only for the generic cell skin tail
                // (`[%cell head %noun]`). More specific tail skins refine the core as an
                // ordinary cell and must not leave the arm namespace available.
                if matches!(tail, Skin::Base(BaseType::NounExpr)) {
                    Ok(cons_core(&mut self.cx, head_ty, garb, context, rest))
                } else {
                    let noun = cons_noun(&mut self.cx);
                    let tail_ty = self.gain_skin_inner(sut, noun, tail, seen)?;
                    Ok(cons_cell(&mut self.cx, head_ty, tail_ty))
                }
            }
            NTy::Face { inner, .. } => {
                let inner = inner.clone();
                self.gain_cell_skin(sut, inner, head, tail, seen)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = self.gain_cell_skin(sut.clone(), option, head, tail, seen)?;
                    out.push(opt);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head: hd, payload } => {
                let hd = hd.clone();
                let payload = payload.clone();
                let payload = self.gain_cell_skin(sut, payload, head, tail, seen)?;
                Ok(cons_hint(&mut self.cx, hd, payload))
            }
            NTy::Hold { .. } => {
                let ref_id = native_type_id(&ref_);
                if !seen.insert(ref_id) {
                    return Ok(cons_void(&mut self.cx));
                }
                let result = self
                    .repo(ref_.clone())
                    .and_then(|inner| self.gain_cell_skin(sut, inner, head, tail, seen));
                seen.remove(&ref_id);
                result
            }
        }
    }

    fn gain_leaf_skin(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        aura: &str,
        atom: &ParsedAtom,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Void => Ok(cons_void(&mut self.cx)),
            NTy::Noun => {
                let value = parsed_atom_to_noun(self.slab, atom);
                let noun = ty_atom(self.slab, aura, Some(value));
                native_of(&mut self.cx, noun, &self.slab.noun_space())
            }
            NTy::Atom { .. } => {
                let ref_noun = live_to_noun(&mut self.cx, &ref_, self.slab);
                let (ref_aura, ref_val) = type_atom_parts(ref_noun, &self.slab.noun_space())?;
                let value = parsed_atom_to_noun(self.slab, atom);
                if let Some(ref_val) = ref_val {
                    if !noun_eq(ref_val, value, &self.slab.noun_space())? {
                        return Ok(cons_void(&mut self.cx));
                    }
                }
                let aura_tag = if aura.is_empty() { "$" } else { aura };
                let aura_noun = term_to_noun(self.slab, aura_tag);
                let aura_any = aura_tag == "$" || aura_tag == "@";
                if !aura_any && !self.fitz(aura_noun, ref_aura)? {
                    return Err(CompilerError::Noun(
                        "native mint: atom-mismatch".to_string(),
                    ));
                }
                let max_aura = if aura_any {
                    ref_aura
                } else {
                    self.atom_max(aura_noun, ref_aura)?
                };
                let space = self.slab.noun_space();
                let aura_atom = max_aura
                    .in_space(&space)
                    .as_atom()
                    .map_err(|err| CompilerError::Decode(format!("atom aura: {err}")))?;
                let aura_str = atom_to_string(aura_atom)
                    .map_err(|err| CompilerError::Decode(format!("atom aura: {err}")))?;
                let noun = ty_atom(self.slab, &aura_str, Some(value));
                native_of(&mut self.cx, noun, &self.slab.noun_space())
            }
            NTy::Cell(..) => Ok(cons_void(&mut self.cx)),
            NTy::Core { .. } => Ok(cons_void(&mut self.cx)),
            NTy::Face { inner, .. } => {
                let inner = inner.clone();
                self.gain_leaf_skin(sut, inner, aura, atom, seen)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = self.gain_leaf_skin(sut.clone(), option, aura, atom, seen)?;
                    out.push(opt);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let payload = self.gain_leaf_skin(sut, payload, aura, atom, seen)?;
                Ok(cons_hint(&mut self.cx, head, payload))
            }
            NTy::Hold { .. } => {
                let ref_id = native_type_id(&ref_);
                if !seen.insert(ref_id) {
                    return Ok(cons_void(&mut self.cx));
                }
                let result = self
                    .repo(ref_.clone())
                    .and_then(|inner| self.gain_leaf_skin(sut, inner, aura, atom, seen));
                seen.remove(&ref_id);
                result
            }
        }
    }

    fn lose_skin(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>, skin: &Skin) -> Result<NRc<NTy>> {
        let mut seen: HashSet<TypeId> = HashSet::new();
        self.lose_skin_inner(sut, ref_, skin, &mut seen)
    }

    fn lose_skin_inner(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        skin: &Skin,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match skin {
            Skin::Term(name) => {
                let wing = vec![Limb::Term(name.clone())];
                let spec = Spec::Like(wing, Vec::new());
                let converted =
                    Skin::Spec(Box::new(spec), Box::new(Skin::Base(BaseType::NounExpr)));
                self.lose_skin_inner(sut, ref_, &converted, seen)
            }
            Skin::Base(base) => match base {
                BaseType::Cell => {
                    let head = Skin::Base(BaseType::NounExpr);
                    let tail = Skin::Base(BaseType::NounExpr);
                    let skin = Skin::Cell(Box::new(head), Box::new(tail));
                    self.lose_skin_inner(sut, ref_, &skin, seen)
                }
                BaseType::Flag => {
                    let yes = Skin::Leaf("f".to_string(), ParsedAtom::Small(0));
                    let no = Skin::Leaf("f".to_string(), ParsedAtom::Small(1));
                    let without_yes = self.lose_skin_inner(sut.clone(), ref_, &yes, seen)?;
                    self.lose_skin_inner(sut, without_yes, &no, seen)
                }
                BaseType::Null => {
                    let skin = Skin::Leaf("n".to_string(), ParsedAtom::Small(0));
                    self.lose_skin_inner(sut, ref_, &skin, seen)
                }
                BaseType::Void => Ok(ref_),
                BaseType::NounExpr => Ok(cons_void(&mut self.cx)),
                BaseType::Atom(aura) => {
                    let mut seen_ref: HashSet<TypeId> = HashSet::new();
                    self.lose_atom_skin(sut, ref_, aura, &mut seen_ref)
                }
            },
            Skin::Cell(head, tail) => {
                let mut seen_ref: HashSet<TypeId> = HashSet::new();
                self.lose_cell_skin(sut, ref_, head, tail, &mut seen_ref)
            }
            Skin::Leaf(aura, atom) => {
                let mut seen_ref: HashSet<TypeId> = HashSet::new();
                self.lose_leaf_skin(sut, ref_, aura, atom, &mut seen_ref)
            }
            Skin::Dbug(_, inner) => self.lose_skin_inner(sut, ref_, inner, seen),
            Skin::Help(_, inner) => self.lose_skin_inner(sut, ref_, inner, seen),
            Skin::Name(_, inner) => self.lose_skin_inner(sut, ref_, inner, seen),
            Skin::Over(wing, inner) => {
                let next_sut = self.play(sut, &Hoon::Wing(wing.clone()))?;
                self.lose_skin_inner(next_sut, ref_, inner, seen)
            }
            Skin::Spec(spec, inner) => {
                let example = self.spec_example_cached(spec);
                let hit = self.play(sut.clone(), example.as_ref())?;
                let inner_ty = self.lose_skin_inner(sut, ref_.clone(), inner, seen)?;
                if !self.nest(hit.clone(), inner_ty)? {
                    return Err(CompilerError::Noun("native mint: lose spec".to_string()));
                }
                self.crop(ref_, hit)
            }
            Skin::Wash(_) => Ok(ref_),
        }
    }

    fn lose_atom_skin(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        _aura: &str,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Void => Ok(cons_void(&mut self.cx)),
            NTy::Noun => {
                let noun = cons_noun(&mut self.cx);
                Ok(cons_cell(&mut self.cx, noun.clone(), noun))
            }
            NTy::Atom { .. } => Ok(cons_void(&mut self.cx)),
            NTy::Cell(..) => Ok(ref_),
            NTy::Core { .. } => Ok(ref_),
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let inner = self.lose_atom_skin(sut, inner, _aura, seen)?;
                Ok(cons_face(&mut self.cx, tool, inner))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = self.lose_atom_skin(sut.clone(), option, _aura, seen)?;
                    out.push(opt);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let payload = self.lose_atom_skin(sut, payload, _aura, seen)?;
                Ok(cons_hint(&mut self.cx, head, payload))
            }
            NTy::Hold { .. } => {
                let ref_id = native_type_id(&ref_);
                if !seen.insert(ref_id) {
                    return Ok(cons_void(&mut self.cx));
                }
                let result = self
                    .repo(ref_.clone())
                    .and_then(|inner| self.lose_atom_skin(sut, inner, _aura, seen));
                seen.remove(&ref_id);
                result
            }
        }
    }

    fn lose_cell_skin(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        head: &Skin,
        tail: &Skin,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Void => Ok(cons_void(&mut self.cx)),
            NTy::Noun => {
                let is_noun_cell = matches!(head, Skin::Base(BaseType::NounExpr))
                    && matches!(tail, Skin::Base(BaseType::NounExpr));
                if is_noun_cell {
                    let noun = ty_atom(self.slab, "$", None);
                    native_of(&mut self.cx, noun, &self.slab.noun_space())
                } else {
                    Ok(ref_)
                }
            }
            NTy::Atom { .. } => Ok(ref_),
            NTy::Cell(ref_head, ref_tail) => {
                let ref_head = ref_head.clone();
                let ref_tail = ref_tail.clone();
                let lef = self.lose_skin_inner(sut.clone(), ref_head.clone(), head, seen)?;
                let rig = self.lose_skin_inner(sut, ref_tail.clone(), tail, seen)?;
                // Three-way fork; `cons_fork` builds hoon-138's mug-ordered treap.
                let cell_lr = cons_cell(&mut self.cx, lef.clone(), rig.clone());
                let cell_l = cons_cell(&mut self.cx, lef, ref_tail);
                let cell_r = cons_cell(&mut self.cx, ref_head, rig);
                self.cons_fork(vec![cell_lr, cell_l, cell_r])
            }
            NTy::Core {
                payload,
                garb,
                context,
                rest,
            } => {
                let payload = payload.clone();
                let garb = garb.clone();
                let context = context.clone();
                let rest = rest.clone();
                let head_ty = self.lose_skin_inner(sut.clone(), payload, head, seen)?;
                if matches!(&*head_ty, NTy::Void) {
                    return Ok(cons_void(&mut self.cx));
                }
                // hoon-138 `ar:lose` uses the same core-vs-cell split as `ar:gain` here.
                if matches!(tail, Skin::Base(BaseType::NounExpr)) {
                    Ok(cons_core(&mut self.cx, head_ty, garb, context, rest))
                } else {
                    let noun = cons_noun(&mut self.cx);
                    let tail_ty = self.lose_skin_inner(sut, noun, tail, seen)?;
                    Ok(cons_cell(&mut self.cx, head_ty, tail_ty))
                }
            }
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let inner = self.lose_cell_skin(sut, inner, head, tail, seen)?;
                Ok(cons_face(&mut self.cx, tool, inner))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = self.lose_cell_skin(sut.clone(), option, head, tail, seen)?;
                    out.push(opt);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head: hd, payload } => {
                let hd = hd.clone();
                let payload = payload.clone();
                let payload = self.lose_cell_skin(sut, payload, head, tail, seen)?;
                Ok(cons_hint(&mut self.cx, hd, payload))
            }
            NTy::Hold { .. } => {
                let ref_id = native_type_id(&ref_);
                if !seen.insert(ref_id) {
                    return Ok(cons_void(&mut self.cx));
                }
                let result = self
                    .repo(ref_.clone())
                    .and_then(|inner| self.lose_cell_skin(sut, inner, head, tail, seen));
                seen.remove(&ref_id);
                result
            }
        }
    }

    fn lose_leaf_skin(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        _aura: &str,
        atom: &ParsedAtom,
        seen: &mut HashSet<TypeId>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Void => Ok(cons_void(&mut self.cx)),
            NTy::Noun => Ok(cons_noun(&mut self.cx)),
            NTy::Atom { .. } => {
                let ref_noun = live_to_noun(&mut self.cx, &ref_, self.slab);
                let (_ref_aura, ref_val) = type_atom_parts(ref_noun, &self.slab.noun_space())?;
                let value = parsed_atom_to_noun(self.slab, atom);
                if let Some(ref_val) = ref_val {
                    if noun_eq(ref_val, value, &self.slab.noun_space())? {
                        return Ok(cons_void(&mut self.cx));
                    }
                }
                Ok(ref_)
            }
            NTy::Cell(..) => Ok(ref_),
            NTy::Core { .. } => Ok(ref_),
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let inner = self.lose_leaf_skin(sut, inner, _aura, atom, seen)?;
                Ok(cons_face(&mut self.cx, tool, inner))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = self.lose_leaf_skin(sut.clone(), option, _aura, atom, seen)?;
                    out.push(opt);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let payload = self.lose_leaf_skin(sut, payload, _aura, atom, seen)?;
                Ok(cons_hint(&mut self.cx, head, payload))
            }
            NTy::Hold { .. } => {
                let ref_id = native_type_id(&ref_);
                if !seen.insert(ref_id) {
                    return Ok(cons_void(&mut self.cx));
                }
                let result = self
                    .repo(ref_.clone())
                    .and_then(|inner| self.lose_leaf_skin(sut, inner, _aura, atom, seen));
                seen.remove(&ref_id);
                result
            }
        }
    }

    fn fuse(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<NRc<NTy>> {
        // The boundary cache keys on canonical type IDs, so `sut` is never lowered.
        if let Some(cached) = self.fuse_boundary_lookup(&sut, &ref_)? {
            return Ok(cached);
        }
        let mut seen: HashSet<(TypeId, TypeId)> = HashSet::new();
        let result = self.fuse_inner(sut.clone(), ref_.clone(), &mut seen)?;
        self.fuse_boundary_store(&sut, &ref_, result.clone())?;
        Ok(result)
    }

    /// Test helper: `fuse` on noun types.
    #[cfg(test)]
    fn fuse_noun(&mut self, sut: Noun, ref_: Noun) -> Result<Noun> {
        let sn = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let rn = native_of(&mut self.cx, ref_, &self.slab.noun_space())?;
        let r = self.fuse(sn, rn)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    fn miss(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<bool> {
        // `seen` and the memo key on canonical type IDs.
        let mut seen: Vec<(TypeId, TypeId)> = Vec::new();
        if let Some((stored_epoch, mut memo)) = self.miss_memo_persist.take() {
            let context = self.cache_context_key();
            let epoch = context;
            if stored_epoch != epoch {
                memo.clear();
            }
            let result = self.miss_dext(sut, ref_, &mut seen, &mut memo);
            self.miss_memo_persist = Some((epoch, memo));
            return result;
        }
        let mut memo: FastHashMap<MissKey, bool> = Default::default();
        self.miss_dext(sut, ref_, &mut seen, &mut memo)
    }

    /// Test helper: `miss` on noun types.
    #[cfg(test)]
    fn miss_noun(&mut self, sut: Noun, ref_: Noun) -> Result<bool> {
        let sn = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let rn = native_of(&mut self.cx, ref_, &self.slab.noun_space())?;
        self.miss(sn, rn)
    }
    /// Enable or disable cross-call `miss` memo persistence (prelude mint
    /// only); returns the previous state so callers can restore it.
    pub fn set_miss_memo_persistence(&mut self, enabled: bool) -> bool {
        let was_enabled = self.miss_memo_persist.is_some();
        if enabled && !was_enabled {
            // An empty memo is valid in the current context. Later context
            // changes clear it before any lookup.
            self.miss_memo_persist = Some((self.cache_context_key(), Default::default()));
        } else if !enabled {
            self.miss_memo_persist = None;
        }
        was_enabled
    }

    #[inline]
    fn miss_memo_key(&self, sut: &NRc<NTy>, ref_: &NRc<NTy>) -> MissKey {
        MissKey {
            subject: sut.arena_id(),
            reference: ref_.arena_id(),
            vet: VetMode(self.vet),
        }
    }
    /// Memo over (sut, ref, vet) keys. Without it, sibling fork branches
    /// re-explore identical hold expansions and one outer `miss` over hoon-138
    /// types makes more than 10^8 recursive calls. `miss` reaches
    /// `repo`/`rest`/`redo`, whose cached state changes during a build, so a
    /// verdict memoized under earlier state can flip (seen as redo-match
    /// miscompiles in batch builds). The memo therefore lives for one outer
    /// `miss` call, or, under `set_miss_memo_persistence`, until the cache
    /// context key changes.
    fn miss_dext(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        seen: &mut Vec<(TypeId, TypeId)>,
        memo: &mut FastHashMap<MissKey, bool>,
    ) -> Result<bool> {
        let key = self.miss_memo_key(&sut, &ref_);
        if let Some(&cached) = memo.get(&key) {
            return Ok(cached);
        }
        let result = self.miss_dext_uncached(sut, ref_, seen, memo)?;
        memo.insert(key, result);
        Ok(result)
    }
    fn miss_dext_uncached(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        seen: &mut Vec<(TypeId, TypeId)>,
        memo: &mut FastHashMap<MissKey, bool>,
    ) -> Result<bool> {
        if NRc::ptr_eq(&sut, &ref_) {
            let void = cons_void(&mut self.cx);
            return self.nest(void, sut.clone());
        }
        if matches!(&*ref_, NTy::Void) {
            return Ok(true);
        }
        match &*sut {
            NTy::Void => Ok(true),
            NTy::Noun => {
                let void = cons_void(&mut self.cx);
                self.nest(void, ref_.clone())
            }
            NTy::Atom { .. } | NTy::Cell(..) => self.miss_sint(sut, ref_, seen, memo),
            NTy::Core { .. } => {
                let head = cons_noun(&mut self.cx);
                let tail = cons_noun(&mut self.cx);
                let cell = cons_cell(&mut self.cx, head, tail);
                self.miss_sint(cell, ref_, seen, memo)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&sut)?;
                for option in options {
                    if !self.miss_dext(option, ref_.clone(), seen, memo)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            NTy::Face { inner, .. } => {
                let inner = inner.clone();
                self.miss_dext(inner, ref_, seen, memo)
            }
            NTy::Hint { payload, .. } => {
                let payload = payload.clone();
                self.miss_dext(payload, ref_, seen, memo)
            }
            NTy::Hold { .. } => {
                let sp = native_type_id(&sut);
                let rp = native_type_id(&ref_);
                for (a, b) in seen.iter() {
                    if (*a == sp && *b == rp) || (*a == rp && *b == sp) {
                        return Ok(true);
                    }
                }
                seen.push((sp, rp));
                let repo = self.repo(sut.clone())?;
                let result = self.miss_dext(repo, ref_, seen, memo);
                seen.pop();
                result
            }
        }
    }
    fn miss_sint(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        seen: &mut Vec<(TypeId, TypeId)>,
        memo: &mut FastHashMap<MissKey, bool>,
    ) -> Result<bool> {
        match &*ref_ {
            NTy::Atom { .. } => {
                if !matches!(&*sut, NTy::Atom { .. }) {
                    return Ok(true);
                }
                let sut_noun = live_to_noun(&mut self.cx, &sut, self.slab);
                let ref_noun = live_to_noun(&mut self.cx, &ref_, self.slab);
                let space = self.slab.noun_space();
                let (_sut_aura, sut_val) = type_atom_parts(sut_noun, &space)?;
                let (_ref_aura, ref_val) = type_atom_parts(ref_noun, &space)?;
                match (sut_val, ref_val) {
                    (Some(sut_val), Some(ref_val)) => {
                        Ok(!noun_eq(sut_val, ref_val, &self.slab.noun_space())?)
                    }
                    _ => Ok(false),
                }
            }
            NTy::Cell(rh, rt) => {
                let (sh, st) = match &*sut {
                    NTy::Cell(sh, st) => (sh.clone(), st.clone()),
                    _ => return Ok(true),
                };
                let rh = rh.clone();
                let rt = rt.clone();
                let head_miss = self.miss_dext(sh, rh, seen, memo)?;
                let tail_miss = self.miss_dext(st, rt, seen, memo)?;
                Ok(head_miss || tail_miss)
            }
            _ => self.miss_dext(ref_, sut, seen, memo),
        }
    }
    fn fuse_inner(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        seen: &mut HashSet<(TypeId, TypeId)>,
    ) -> Result<NRc<NTy>> {
        if NRc::ptr_eq(&sut, &ref_) || matches!(&*ref_, NTy::Noun) {
            return Ok(sut);
        }
        match &*sut {
            NTy::Atom { .. } => match &*ref_ {
                NTy::Atom { .. } => {
                    let sut_noun = live_to_noun(&mut self.cx, &sut, self.slab);
                    let ref_noun = live_to_noun(&mut self.cx, &ref_, self.slab);
                    let space = self.slab.noun_space();
                    let (sut_aura, sut_val) = type_atom_parts(sut_noun, &space)?;
                    let (ref_aura, ref_val) = type_atom_parts(ref_noun, &space)?;
                    let foc = if self.fitz(ref_aura, sut_aura)? {
                        sut_aura
                    } else {
                        ref_aura
                    };
                    let value = match (sut_val, ref_val) {
                        (Some(sv), Some(rv)) => {
                            if noun_eq(sv, rv, &self.slab.noun_space())? {
                                Some(sv)
                            } else {
                                return Ok(cons_void(&mut self.cx));
                            }
                        }
                        (Some(v), None) | (None, Some(v)) => Some(v),
                        (None, None) => None,
                    };
                    let space = self.slab.noun_space();
                    let aura_atom = foc
                        .in_space(&space)
                        .as_atom()
                        .map_err(|err| CompilerError::Decode(format!("atom aura: {err}")))?;
                    let aura_str = atom_to_string(aura_atom)
                        .map_err(|err| CompilerError::Decode(format!("atom aura: {err}")))?;
                    Ok(ty_atom_n(&mut self.cx, self.slab, &aura_str, value).1)
                }
                NTy::Cell(..) => Ok(cons_void(&mut self.cx)),
                _ => self.fuse_inner(ref_.clone(), sut.clone(), seen),
            },
            NTy::Cell(sh, st) => match &*ref_ {
                NTy::Cell(rh, rt) => {
                    let sh = sh.clone();
                    let st = st.clone();
                    let rh = rh.clone();
                    let rt = rt.clone();
                    let head = self.fuse_inner(sh, rh, seen)?;
                    let tail = self.fuse_inner(st, rt, seen)?;
                    Ok(cons_cell(&mut self.cx, head, tail))
                }
                _ => self.fuse_inner(ref_.clone(), sut.clone(), seen),
            },
            NTy::Core { .. } => {
                let inner = self.repo(sut.clone())?;
                self.fuse_inner(inner, ref_, seen)
            }
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let fused = self.fuse_inner(inner, ref_, seen)?;
                Ok(cons_face(&mut self.cx, tool, fused))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&sut)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let f = self.fuse_inner(option, ref_.clone(), seen)?;
                    out.push(f);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let fused = self.fuse_inner(payload, ref_, seen)?;
                Ok(cons_hint(&mut self.cx, head, fused))
            }
            NTy::Hold { .. } => {
                let key = (native_type_id(&sut), native_type_id(&ref_));
                if seen.contains(&key) {
                    return Err(CompilerError::UnsupportedExpr(
                        "native mint: fuse-loop".to_string(),
                    ));
                }
                seen.insert(key);
                let inner = self.repo(sut.clone())?;
                let result = self.fuse_inner(inner, ref_.clone(), seen);
                seen.remove(&key);
                result
            }
            NTy::Noun => Ok(ref_),
            NTy::Void => Ok(cons_void(&mut self.cx)),
        }
    }

    fn crop(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<NRc<NTy>> {
        // The boundary cache keys on the interned (sut, ref) `Rc` pointers.
        if let Some(cached) = self.crop_boundary_lookup(&sut, &ref_)? {
            return Ok(cached);
        }
        let mut seen: HashSet<(TypeId, TypeId)> = HashSet::new();
        let result = self.crop_inner(sut.clone(), ref_.clone(), &mut seen)?;
        self.crop_boundary_store(&sut, &ref_, result.clone())?;
        Ok(result)
    }

    /// Noun-bridged `crop` for tests: lifts both types, crops, lowers the result.
    #[cfg(test)]
    fn crop_noun(&mut self, sut: Noun, ref_: Noun) -> Result<Noun> {
        let sn = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let rn = native_of(&mut self.cx, ref_, &self.slab.noun_space())?;
        let r = self.crop(sn, rn)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    fn crop_inner(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        seen: &mut HashSet<(TypeId, TypeId)>,
    ) -> Result<NRc<NTy>> {
        if NRc::ptr_eq(&sut, &ref_) || matches!(&*ref_, NTy::Noun) {
            return Ok(cons_void(&mut self.cx));
        }
        if matches!(&*ref_, NTy::Void) {
            return Ok(sut);
        }
        match &*sut {
            NTy::Atom { .. } => match &*ref_ {
                NTy::Atom { .. } => {
                    let sut_noun = live_to_noun(&mut self.cx, &sut, self.slab);
                    let ref_noun = live_to_noun(&mut self.cx, &ref_, self.slab);
                    let space = self.slab.noun_space();
                    let (_sut_aura, sut_val) = type_atom_parts(sut_noun, &space)?;
                    let (_ref_aura, ref_val) = type_atom_parts(ref_noun, &space)?;
                    let out = match (sut_val, ref_val) {
                        (Some(sv), Some(rv)) => {
                            if noun_eq(sv, rv, &self.slab.noun_space())? {
                                cons_void(&mut self.cx)
                            } else {
                                sut
                            }
                        }
                        (Some(_), None) => cons_void(&mut self.cx),
                        (None, Some(_)) => sut,
                        (None, None) => cons_void(&mut self.cx),
                    };
                    Ok(out)
                }
                NTy::Cell(..) => Ok(sut),
                _ => self.crop_sint(sut, ref_, seen),
            },
            NTy::Cell(sh, st) => match &*ref_ {
                NTy::Atom { .. } => Ok(sut),
                NTy::Cell(rh, rt) => {
                    let sh = sh.clone();
                    let st = st.clone();
                    let rh = rh.clone();
                    let rt = rt.clone();
                    // hoon-138 `(nest(sut p.ref) | p.sut)`, through the noun bridge.
                    let rh_noun = live_to_noun(&mut self.cx, &rh, self.slab);
                    let sh_noun = live_to_noun(&mut self.cx, &sh, self.slab);
                    if !self.nest_noun(rh_noun, sh_noun)? {
                        return Ok(cons_cell(&mut self.cx, sh, st));
                    }
                    let tail = self.crop_inner(st, rt, seen)?;
                    Ok(cons_cell(&mut self.cx, sh, tail))
                }
                _ => self.crop_sint(sut, ref_, seen),
            },
            NTy::Core { .. } => match &*ref_ {
                NTy::Atom { .. } | NTy::Cell(..) => Ok(sut),
                _ => self.crop_sint(sut, ref_, seen),
            },
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let cropped = self.crop_inner(inner, ref_, seen)?;
                Ok(cons_face(&mut self.cx, tool, cropped))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&sut)?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let c = self.crop_inner(option, ref_.clone(), seen)?;
                    out.push(c);
                }
                self.cons_fork(out)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let cropped = self.crop_inner(payload, ref_, seen)?;
                Ok(cons_hint(&mut self.cx, head, cropped))
            }
            NTy::Hold { .. } => {
                let key = (native_type_id(&sut), native_type_id(&ref_));
                if seen.contains(&key) {
                    return Err(CompilerError::UnsupportedExpr(
                        "native mint: crop-loop".to_string(),
                    ));
                }
                seen.insert(key);
                let inner = self.repo(sut.clone())?;
                let result = self.crop_inner(inner, ref_.clone(), seen);
                seen.remove(&key);
                result
            }
            NTy::Noun => {
                let repo = self.repo(sut.clone())?;
                self.crop_inner(repo, ref_, seen)
            }
            NTy::Void => Ok(cons_void(&mut self.cx)),
        }
    }
    fn crop_sint(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        seen: &mut HashSet<(TypeId, TypeId)>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Core { .. } => Ok(sut),
            NTy::Face { .. } | NTy::Hint { .. } | NTy::Hold { .. } => {
                let inner = self.repo(ref_.clone())?;
                self.crop_inner(sut, inner, seen)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&ref_)?;
                let mut acc = sut;
                for option in options {
                    acc = self.crop_inner(acc, option, seen)?;
                }
                Ok(acc)
            }
            _ => Ok(sut),
        }
    }
    fn fitz(&mut self, yaz: Noun, wix: Noun) -> Result<bool> {
        fn trim_bytes(bytes: &[u8]) -> Vec<u8> {
            let mut out = bytes.to_vec();
            while let Some(true) = out.last().map(|b| *b == 0) {
                out.pop();
            }
            out
        }

        fn fiz(slab: &mut NounSlab, noun: Noun) -> Result<(u64, Noun)> {
            let space = slab.noun_space();
            let atom = noun
                .in_space(&space)
                .as_atom()
                .map_err(|err| CompilerError::Decode(format!("fitz atom: {err}")))?;
            let bytes = trim_bytes(atom.as_ne_bytes());
            if bytes.is_empty() {
                return Ok((0, term_to_noun(slab, "$")));
            }
            let top = bytes[bytes.len() - 1];
            if (b'A'..=b'Z').contains(&top) {
                let p = (top - b'@') as u64;
                let rest = bytes[..bytes.len() - 1].to_vec();
                // `from_bytes([])` creates an invalid indirect atom (size=0) in nockvm.
                // In this aura encoding, empty is treated as the `$` terminator.
                if rest.is_empty() {
                    return Ok((p, term_to_noun(slab, "$")));
                }
                let rest_atom = <Atom as AtomExt>::from_bytes(slab, &rest);
                return Ok((p, rest_atom.as_noun()));
            }
            Ok((0, atom.as_noun().noun()))
        }

        fn end_bytes(slab: &mut NounSlab, noun: Noun, n: usize) -> Result<Noun> {
            let space = slab.noun_space();
            let atom = noun
                .in_space(&space)
                .as_atom()
                .map_err(|err| CompilerError::Decode(format!("fitz end atom: {err}")))?;
            let bytes = trim_bytes(atom.as_ne_bytes());
            let end = if n >= bytes.len() {
                bytes
            } else {
                bytes[..n].to_vec()
            };
            if end.is_empty() {
                return Ok(term_to_noun(slab, "$"));
            }
            Ok(<Atom as AtomExt>::from_bytes(slab, &end).as_noun())
        }

        fn rsh_bytes(slab: &mut NounSlab, noun: Noun, n: usize) -> Result<Noun> {
            let space = slab.noun_space();
            let atom = noun
                .in_space(&space)
                .as_atom()
                .map_err(|err| CompilerError::Decode(format!("fitz rsh atom: {err}")))?;
            let bytes = trim_bytes(atom.as_ne_bytes());
            if n >= bytes.len() {
                return Ok(term_to_noun(slab, "$"));
            }
            let rest = bytes[n..].to_vec();
            if rest.is_empty() {
                return Ok(term_to_noun(slab, "$"));
            }
            Ok(<Atom as AtomExt>::from_bytes(slab, &rest).as_noun())
        }

        let (p_y, mut q_y) = fiz(self.slab, yaz)?;
        let (p_w, mut q_w) = fiz(self.slab, wix)?;
        if !(p_y == 0 || p_w == 0 || (p_w != 0 && p_w <= p_y)) {
            return Ok(false);
        }
        let dollar = term_to_noun(self.slab, "$");
        loop {
            if noun_eq(q_y, dollar, &self.slab.noun_space())?
                || noun_eq(q_w, dollar, &self.slab.noun_space())?
            {
                return Ok(true);
            }
            let end_y = end_bytes(self.slab, q_y, 1)?;
            let end_w = end_bytes(self.slab, q_w, 1)?;
            if !noun_eq(end_y, end_w, &self.slab.noun_space())? {
                return Ok(false);
            }
            q_y = rsh_bytes(self.slab, q_y, 1)?;
            q_w = rsh_bytes(self.slab, q_w, 1)?;
        }
    }

    fn atom_max(&mut self, left: Noun, right: Noun) -> Result<Noun> {
        let space = self.slab.noun_space();
        let left_atom = left
            .in_space(&space)
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("atom max left: {err}")))?;
        let right_atom = right
            .in_space(&space)
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("atom max right: {err}")))?;
        if lth_b(self.slab, left_atom.atom(), right_atom.atom(), &space) {
            Ok(right)
        } else {
            Ok(left)
        }
    }

    fn fork_from_options(&mut self, options: Vec<Noun>) -> Result<Noun> {
        let mut set = D(0);
        for option in options {
            set = fork_set_insert(self.slab, set, option)?;
        }
        if noun_is_zero(set) {
            return Ok(ty_void(self.slab));
        }
        let (key, left, right) = set_parts(set, &self.slab.noun_space())?;
        if noun_is_zero(left) && noun_is_zero(right) {
            return Ok(key);
        }
        let tag = term_to_noun(self.slab, "fork");
        Ok(T(self.slab, &[tag, set]))
    }

    /// Test helper: `fork_from_options` paired with its native form. `native_of`
    /// keeps the mug-ordered set treap intact and covers the void and
    /// single-member collapses.
    #[cfg(test)]
    fn fork_from_options_n(&mut self, options: Vec<Noun>) -> Result<(Noun, NRc<NTy>)> {
        let noun = self.fork_from_options(options)?;
        let native = native_of(&mut self.cx, noun, &self.slab.noun_space())?;
        Ok((noun, native))
    }

    /// Native `%fork` constructor. Takes native options and returns the
    /// canonical interned fork type. The treap is built by `fork_from_options`
    /// over each option's memoized lowering, so the `%set` leaf matches
    /// hoon-138's mug-ordered treap byte for byte, and the collapse rules
    /// (empty to void, single to the bare member, void-drop, nested-fork union)
    /// come from there too. The result goes through `native_of_cached`, so
    /// structurally equal forks share one interned `Rc`.
    fn cons_fork(&mut self, options: Vec<NRc<NTy>>) -> Result<NRc<NTy>> {
        // Empty and single-option inputs collapse here, as in hoon-138 `++fork`,
        // without building, mugging, and decoding a treap.
        match options.as_slice() {
            [] => return Ok(cons_void(&mut self.cx)),
            [only] if matches!(&**only, NTy::Void) => {
                return Ok(cons_void(&mut self.cx));
            }
            [only] => return Ok(only.clone()),
            _ => {}
        }

        let mut noun_opts = Vec::with_capacity(options.len());
        for opt in &options {
            noun_opts.push(live_to_noun(&mut self.cx, opt, self.slab));
        }
        let fork_noun = self.fork_from_options(noun_opts)?;
        let result = self.native_of_cached(fork_noun)?;
        Ok(result)
    }

    fn atom_nest(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<bool> {
        // Atom types are small, so lowering them to nouns is cheap and lets the
        // noun decoder read the aura and constant.
        let sut = live_to_noun(&mut self.cx, &sut, self.slab);
        let ref_ = live_to_noun(&mut self.cx, &ref_, self.slab);
        let (sut_aura, sut_val) = type_atom_parts(sut, &self.slab.noun_space())
            .map_err(|err| CompilerError::Decode(format!("atom nest sut: {err}")))?;
        let (ref_aura, ref_val) = type_atom_parts(ref_, &self.slab.noun_space())
            .map_err(|err| CompilerError::Decode(format!("atom nest ref: {err}")))?;
        let dollar = term_to_noun(self.slab, "$");
        let at = term_to_noun(self.slab, "@");
        let aura_any = noun_eq(sut_aura, dollar, &self.slab.noun_space())?
            || noun_eq(sut_aura, at, &self.slab.noun_space())?;
        if !aura_any && !self.fitz(sut_aura, ref_aura)? {
            return Ok(false);
        }
        match (sut_val, ref_val) {
            (None, _) => Ok(true),
            (Some(sut_val), Some(ref_val)) => {
                Ok(noun_eq(sut_val, ref_val, &self.slab.noun_space())?)
            }
            (Some(_), None) => Ok(false),
        }
    }

    fn peek<A: Into<BigUint>>(&mut self, sut: NRc<NTy>, way: Way, axis: A) -> Result<NRc<NTy>> {
        // The seen-hold set keys on the lowered hold noun and the axis; the rest
        // of the walk reads native types.
        fn seen_hold(
            ut: &mut Ut<'_>,
            seen: &mut HashMap<HoldAxisHash, Vec<(Noun, BigUint)>>,
            hold: Noun,
            axis: &BigUint,
        ) -> Result<bool> {
            let axis_noun = noun_biguint(ut.slab, axis.clone());
            let mug = HoldAxisHash(
                ut.noun_mug_cached(hold).0 ^ slab_mug(axis_noun, &ut.slab.noun_space()),
            );
            if let Some(bucket) = seen.get(&mug) {
                for (prior, prior_axis) in bucket {
                    if prior_axis == axis && noun_eq(*prior, hold, &ut.slab.noun_space())? {
                        return Ok(true);
                    }
                }
            }
            seen.entry(mug).or_default().push((hold, axis.clone()));
            Ok(false)
        }

        fn unsee_hold(
            ut: &mut Ut<'_>,
            seen: &mut HashMap<HoldAxisHash, Vec<(Noun, BigUint)>>,
            hold: Noun,
            axis: &BigUint,
        ) -> Result<()> {
            let axis_noun = noun_biguint(ut.slab, axis.clone());
            let mug = HoldAxisHash(
                ut.noun_mug_cached(hold).0 ^ slab_mug(axis_noun, &ut.slab.noun_space()),
            );
            if let Some(bucket) = seen.get_mut(&mug) {
                let space = ut.slab.noun_space();
                let mut idx = 0;
                while idx < bucket.len() {
                    if &bucket[idx].1 == axis && noun_eq(bucket[idx].0, hold, &space)? {
                        bucket.swap_remove(idx);
                        break;
                    }
                    idx += 1;
                }
                if bucket.is_empty() {
                    seen.remove(&mug);
                }
            }
            Ok(())
        }

        fn go(
            ut: &mut Ut<'_>,
            sut: NRc<NTy>,
            way: Way,
            axis: BigUint,
            seen_holds: &mut HashMap<HoldAxisHash, Vec<(Noun, BigUint)>>,
        ) -> Result<NRc<NTy>> {
            if axis == BigUint::from(1u32) {
                return Ok(sut);
            }
            match &*sut {
                NTy::Noun => Ok(cons_noun(&mut ut.cx)),
                NTy::Void => Ok(cons_void(&mut ut.cx)),
                NTy::Atom { .. } => Ok(cons_void(&mut ut.cx)),
                NTy::Cell(head, tail) => {
                    let (cap, mas) = axis_big_cap_mas(&axis)?;
                    let child = if cap == 2 { head.clone() } else { tail.clone() };
                    go(ut, child, way, mas, seen_holds)
                }
                NTy::Core { payload, garb, .. } => {
                    let (cap, mas) = axis_big_cap_mas(&axis)?;
                    if cap != 3 {
                        return Ok(cons_noun(&mut ut.cx));
                    }
                    let payload = payload.clone();
                    // Only the garb's variance matters; the core's context is unused.
                    let vair = garb.vair;
                    let (sam, con) = peel(way, vair);
                    let tow = if mas == BigUint::from(1u32) {
                        1
                    } else {
                        axis_big_cap_mas(&mas)?.0
                    };
                    if (sam && con) || (sam && tow == 2) || (con && tow == 3) {
                        return go(ut, payload, way, mas, seen_holds);
                    }
                    if way != Way::Read {
                        return Err(CompilerError::Noun("payload-block".to_string()));
                    }
                    let sam_type = if sam {
                        go(ut, payload.clone(), way, BigUint::from(2u32), seen_holds)?
                    } else {
                        cons_noun(&mut ut.cx)
                    };
                    let con_type = if con {
                        go(ut, payload.clone(), way, BigUint::from(3u32), seen_holds)?
                    } else {
                        cons_noun(&mut ut.cx)
                    };
                    let blocked = cons_cell(&mut ut.cx, sam_type, con_type);
                    go(ut, blocked, way, mas, seen_holds)
                }
                NTy::Face { inner, .. } => {
                    let inner = inner.clone();
                    go(ut, inner, way, axis, seen_holds)
                }
                NTy::Hint { payload, .. } => {
                    let payload = payload.clone();
                    go(ut, payload, way, axis, seen_holds)
                }
                NTy::Hold { .. } => {
                    let hold_noun = live_to_noun(&mut ut.cx, &sut, ut.slab);
                    if seen_hold(ut, seen_holds, hold_noun, &axis)? {
                        return Ok(cons_void(&mut ut.cx));
                    }
                    let expanded = ut.repo(sut.clone())?;
                    let result = go(ut, expanded, way, axis.clone(), seen_holds);
                    unsee_hold(ut, seen_holds, hold_noun, &axis)?;
                    result
                }
                NTy::Fork { .. } => {
                    let options = ut.fork_options_native(&sut)?;
                    let mut peeks = Vec::with_capacity(options.len());
                    for option in options {
                        peeks.push(go(ut, option, way, axis.clone(), seen_holds)?);
                    }
                    ut.cons_fork(peeks)
                }
            }
        }

        let mut seen_holds: HashMap<HoldAxisHash, Vec<(Noun, BigUint)>> = HashMap::new();
        go(self, sut, way, axis.into(), &mut seen_holds)
    }

    /// Noun-bridged `peek`: lifts `sut`, runs native `peek`, lowers the result.
    fn peek_noun<A: Into<BigUint>>(&mut self, sut: Noun, way: Way, axis: A) -> Result<Noun> {
        let native = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let r = self.peek(native, way, axis)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    /// Grows the native stack before recursing into deep type operations
    /// (`redo`, `mull` and its arm walkers, `reachable_legs`). Without this,
    /// deeply nested types overflow a default 8 MB thread stack. Constants
    /// match `NativeCompiler::with_large_stack` in `native/mod.rs`.
    fn with_stack_guard<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        #[cfg(test)]
        {
            self.stack_guard_calls = self.stack_guard_calls.saturating_add(1);
        }
        stacker::maybe_grow(32 * 1024, 64 * 1024 * 1024, || f(self))
    }

    // =========================================================================
    // ++mull: Dual-perspective type checker for wet polymorphism
    //
    // Matches hoon-138 ++mull (line 10103-10260).
    //
    // For every sub-expression, mull computes types from both sut (call-site)
    // and dox (definition-site) perspectives. The core invariant: the nock code
    // generated from sut and dox must be identical. Where this could be violated
    // (type tests, conditional branches, existence checks), mull explicitly
    // validates and crashes with mull-bonk-* errors.
    // =========================================================================

    /// Main mull entry point. Returns (sut-side type, dox-side type).
    fn mull(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        dox: NRc<NTy>,
        gen: &Hoon,
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        // The boundary cache keys on the interned (sut, gol, dox) `Rc` pointers
        // and the gene's spot-sensitive signature; the gene is lowered to a noun
        // only when the AST has no signature.
        let gen_sig = match self.mint_cache_signature(gen) {
            Some(signature) => signature,
            None => {
                let gen_noun = self.hoon_noun_for_node(gen);
                HoonSignature(u64::from(self.noun_mug_cached(gen_noun).0))
            }
        };
        if let Some(cached) = self.mull_cache_lookup(&sut, &gol, &dox, gen_sig)? {
            return Ok(cached);
        }
        // hoon-138 pre-check: mull-none if sut is void
        if matches!(&*sut, NTy::Void) {
            return Err(CompilerError::Noun("mull-none".to_string()));
        }

        let result =
            self.with_stack_guard(|ut| ut.mull_inner(sut.clone(), gol.clone(), dox.clone(), gen))?;
        self.mull_cache_store(
            &sut,
            &gol,
            &dox,
            gen_sig,
            result.0.clone(),
            result.1.clone(),
        )?;
        Ok(result)
    }

    /// Noun-bridged `mull` for tests: lifts sut/gol/dox to native, runs `mull`,
    /// and lowers both result types.
    #[cfg(test)]
    fn mull_noun(&mut self, sut: Noun, gol: Noun, dox: Noun, gen: &Hoon) -> Result<(Noun, Noun)> {
        let space = self.slab.noun_space();
        let sut_n = native_of(&mut self.cx, sut, &space)?;
        let gol_n = native_of(&mut self.cx, gol, &space)?;
        let dox_n = native_of(&mut self.cx, dox, &space)?;
        let (p, q) = self.mull(sut_n, gol_n, dox_n, gen)?;
        let p_noun = live_to_noun(&mut self.cx, &p, self.slab);
        let q_noun = live_to_noun(&mut self.cx, &q, self.slab);
        Ok((p_noun, q_noun))
    }

    fn mull_inner(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        dox: NRc<NTy>,
        gen: &Hoon,
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        match gen {
            // ---- Cell literal [^ *] ----
            Hoon::Pair(p, q) => {
                let noun_gol = cons_noun(&mut self.cx);
                let hed = self.mull(sut.clone(), noun_gol.clone(), dox.clone(), p)?;
                let tal = self.mull(sut.clone(), noun_gol, dox.clone(), q)?;
                let p_ty = cons_cell(&mut self.cx, hed.0, tal.0);
                let p_ty = self.mull_nice(sut.clone(), gol, p_ty)?;
                let q_ty = cons_cell(&mut self.cx, hed.1, tal.1);
                Ok((p_ty, q_ty))
            }

            // ---- Core construction ----
            // %brcn (|% dry core)
            Hoon::BarCen(prefix, tomes) => self.mull_grow(
                sut,
                gol,
                dox,
                Vair::Gold,
                prefix.as_deref(),
                Poly::Dry,
                &Hoon::Axis((1u64).into()),
                tomes,
            ),
            // %brpt (|@ wet core)
            Hoon::BarPat(prefix, tomes) => self.mull_grow(
                sut,
                gol,
                dox,
                Vair::Gold,
                prefix.as_deref(),
                Poly::Wet,
                &Hoon::Axis((1u64).into()),
                tomes,
            ),

            // ---- Wing resolution: %cnts ----
            Hoon::CenTis(wing, pairs) => self.emul(sut, gol, dox, wing, pairs),

            // ---- Scry: %dtkt ----
            Hoon::DotKet(spec, q) => {
                let noun_gol = cons_noun(&mut self.cx);
                let _q_result = self.mull(sut.clone(), noun_gol, dox.clone(), q)?;
                // Mull the type spec (via KetTar lowering, matching hoon-138)
                self.mull(
                    sut,
                    gol,
                    dox,
                    &Hoon::KetTar(Box::new(spec.as_ref().clone())),
                )
            }

            // ---- Increment: %dtls ----
            Hoon::DotLus(p) => {
                let atom_gol = ty_atom_n(&mut self.cx, self.slab, "$", None).1;
                let _p_result = self.mull(sut.clone(), atom_gol, dox.clone(), p)?;
                let atom_ty = ty_atom_n(&mut self.cx, self.slab, "$", None).1;
                self.mull_beth(sut, gol, atom_ty)
            }

            // ---- Constants: %sand, %rock ----
            Hoon::Sand(_, _) | Hoon::Rock(_, _) => {
                let ty = self.play(sut.clone(), gen)?;
                self.mull_beth(sut, gol, ty)
            }

            // ---- Nock eval: %dttr ----
            Hoon::DotTar(p, q) => {
                let noun_gol = cons_noun(&mut self.cx);
                let _p_result = self.mull(sut.clone(), noun_gol.clone(), dox.clone(), p)?;
                let _q_result = self.mull(sut.clone(), noun_gol, dox.clone(), q)?;
                let noun_ty = cons_noun(&mut self.cx);
                self.mull_beth(sut, gol, noun_ty)
            }

            // ---- Equality: %dtts ----
            Hoon::DotTis(p, q) => {
                let noun_gol = cons_noun(&mut self.cx);
                let _p_result = self.mull(sut.clone(), noun_gol.clone(), dox.clone(), p)?;
                let _q_result = self.mull(sut.clone(), noun_gol, dox.clone(), q)?;
                let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
                self.mull_beth(sut, gol, bool_ty)
            }

            // ---- Type test: %dtwt ----
            Hoon::DotWut(p) => {
                let noun_gol = cons_noun(&mut self.cx);
                let _p_result = self.mull(sut.clone(), noun_gol, dox.clone(), p)?;
                let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
                self.mull_beth(sut, gol, bool_ty)
            }

            // ---- Precomputed: %hand ----
            Hoon::Hand(typ, _nock) => {
                let typ_noun = type_to_noun(self.slab, typ)?;
                let typ_native = native_of(&mut self.cx, typ_noun, &self.slab.noun_space())?;
                Ok((typ_native.clone(), typ_native))
            }

            // ---- Iron wrap: %ktbr ----
            Hoon::KetBar(p) => {
                let vat = self.mull(sut, gol, dox, p)?;
                let p_wrapped = self.wrap_type(vat.0, Vair::Iron)?;
                let q_wrapped = self.wrap_type(vat.1, Vair::Iron)?;
                Ok((p_wrapped, q_wrapped))
            }

            // ---- Cast: %ktdt ----
            Hoon::KetDot(p, q) => {
                let lowered = Self::lower_ktdt(p, q);
                self.mull(sut, gol, dox, &lowered)
            }

            // ---- Cast: %ktls ----
            Hoon::KetLus(p, q) => {
                let p_sut = self.play(sut.clone(), p)?;
                let p_sut = self.mull_nice(sut.clone(), gol, p_sut)?;
                let q_dox = self.play(dox.clone(), p)?;
                let _q_result = self.mull(sut, p_sut.clone(), dox, q)?;
                Ok((p_sut, q_dox))
            }

            // ---- Zinc wrap: %ktpm ----
            Hoon::KetPam(p) => {
                let vat = self.mull(sut, gol, dox, p)?;
                let p_wrapped = self.wrap_type(vat.0, Vair::Zinc)?;
                let q_wrapped = self.wrap_type(vat.1, Vair::Zinc)?;
                Ok((p_wrapped, q_wrapped))
            }

            // ---- Face: %tune ----
            Hoon::Tune(tune) => {
                let tool = term_or_tune_to_noun(self.slab, tune)?;
                let tool_leaf = live_leaf_from_noun(&mut self.cx, tool, &self.slab.noun_space());
                let p_ty = cons_face(&mut self.cx, tool_leaf.clone(), sut);
                let q_ty = cons_face(&mut self.cx, tool_leaf, dox);
                Ok((p_ty, q_ty))
            }

            // ---- Lead wrap: %ktwt ----
            Hoon::KetWut(p) => {
                let vat = self.mull(sut, gol, dox, p)?;
                let p_wrapped = self.wrap_type(vat.0, Vair::Lead)?;
                let q_wrapped = self.wrap_type(vat.1, Vair::Lead)?;
                Ok((p_wrapped, q_wrapped))
            }

            // ---- Hint note: %note ----
            Hoon::Note(note, inner) => {
                let vat = self.mull(sut.clone(), gol, dox.clone(), inner)?;
                let note_noun = note_to_noun(self.slab, note)?;
                let p_ty = self.hint_type(sut, note_noun, vat.0)?;
                let q_ty = self.hint_type(dox, note_noun, vat.1)?;
                Ok((p_ty, q_ty))
            }

            // ---- Constant fold: %ktsg ----
            Hoon::KetSig(p) => self.mull(sut, gol, dox, p),

            // ---- Type-print hint: %sgzp ----
            Hoon::SigZap(p, q) => {
                let _ = self.play(sut.clone(), p)?;
                self.mull(sut, gol, dox, q)
            }

            // ---- Hint: %sggr ----
            Hoon::SigGar(_hint, q) => self.mull(sut, gol, dox, q),

            // ---- Hint: %sgbr ----
            Hoon::SigBar(p, q) => {
                let lowered = Self::lower_sigbar(p, q);
                self.mull(sut, gol, dox, &lowered)
            }

            // ---- Compose: %tsgr ----
            Hoon::TisGar(p, q) => {
                let noun_gol = cons_noun(&mut self.cx);
                let lem = self.mull(sut, noun_gol, dox, p)?;
                self.mull(lem.0, gol, lem.1, q)
            }

            // ---- Merge/busk: %tscm ----
            Hoon::TisCom(p, q) => {
                let boc = self.busk(sut, p);
                let nuf = self.busk(dox, p);
                self.mull(boc, gol, nuf, q)
            }

            // ---- Conditional: %wtcl ----
            Hoon::WutCol(p, q, r) => {
                let bool_gol = ty_bool_n(&mut self.cx, self.slab).1;
                let _nor = self.mull(sut.clone(), bool_gol, dox.clone(), p)?;

                // True branch: apply gain to both sut and dox.
                let hiq: (NRc<NTy>, NRc<NTy>) = {
                    let fex_p = self.gain(sut.clone(), p)?;
                    let fex_q = self.gain(dox.clone(), p)?;
                    if matches!(&*fex_p, NTy::Void) {
                        let q_ty = if matches!(&*fex_q, NTy::Void) {
                            cons_void(&mut self.cx)
                        } else {
                            // sut-side void, dox-side non-void: play dox side
                            self.play(fex_q, q)?
                        };
                        (cons_void(&mut self.cx), q_ty)
                    } else if matches!(&*fex_q, NTy::Void) {
                        // sut-side non-void, dox-side void: mull-bonk-b
                        return Err(CompilerError::Noun("mull-bonk-b".to_string()));
                    } else {
                        self.mull(fex_p, gol.clone(), fex_q, q)?
                    }
                };

                // False branch: apply lose to both sut and dox
                let ran: (NRc<NTy>, NRc<NTy>) = {
                    let wux_p = self.lose(sut.clone(), p)?;
                    let wux_q = self.lose(dox.clone(), p)?;
                    if matches!(&*wux_p, NTy::Void) {
                        let q_ty = if matches!(&*wux_q, NTy::Void) {
                            cons_void(&mut self.cx)
                        } else {
                            self.play(wux_q, r)?
                        };
                        (cons_void(&mut self.cx), q_ty)
                    } else if matches!(&*wux_q, NTy::Void) {
                        return Err(CompilerError::Noun("mull-bonk-c".to_string()));
                    } else {
                        self.mull(wux_p, gol.clone(), wux_q, r)?
                    }
                };

                // Fork both results (cons_fork keeps the mug-ordered treap).
                let p_ty = self.cons_fork(vec![hiq.0, ran.0])?;
                let p_ty = self.mull_nice(sut, gol, p_ty)?;
                let q_ty = self.cons_fork(vec![hiq.1, ran.1])?;
                Ok((p_ty, q_ty))
            }

            // ---- Type test: %fits ----
            Hoon::Fits(p, wing) => {
                // Play the pattern from both perspectives.
                let waz_p = self.play(sut.clone(), p)?;
                let waz_q = self.play(dox.clone(), p)?;

                // Mint the wing from both to get axes (via cove); take the formula.
                let noun_gol = cons_noun(&mut self.cx);
                let wing_hoon = Hoon::Wing(wing.clone());
                let (_sut_ty, sut_formula) =
                    self.mint(sut.clone(), noun_gol.clone(), &wing_hoon)?;
                let syx_p = self.cove(sut_formula)?;
                let (_dox_ty, dox_formula) = self.mint(dox.clone(), noun_gol, &wing_hoon)?;
                let syx_q = self.cove(dox_formula)?;

                // Generate fish (runtime type test) from both
                let pov_p = self.type_test_formula_on_axis(waz_p, syx_p.clone())?;
                let pov_q = self.type_test_formula_on_axis(waz_q, syx_q.clone())?;

                // Assert the axes and the fish formulas are identical.
                if syx_p != syx_q || !self.formula_arena.equal(pov_p, pov_q) {
                    return Err(CompilerError::Noun("mull-bonk-a".to_string()));
                }
                let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
                self.mull_beth(sut, gol, bool_ty)
            }

            // ---- Aura test: %wthx ----
            Hoon::WutHax(_skin, wing) => {
                // fend from both perspectives.
                let (new_type, new_axis) = self.fend(sut.clone(), Way::Read, wing)?;
                let (old_type, old_axis) = self.fend(dox.clone(), Way::Read, wing)?;

                // Assert axes match
                if new_axis != old_axis {
                    return Err(CompilerError::Noun("mull-bonk-x".to_string()));
                }
                // Assert the new type nests in the old (type.new ⊆ type.old)
                if !self.nest(old_type, new_type)? {
                    return Err(CompilerError::Noun("mull-bonk-x".to_string()));
                }
                let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
                self.mull_beth(sut, gol, bool_ty)
            }

            // ---- Debug: %dbug ----
            Hoon::Dbug(_spot, inner) => self.mull(sut, gol, dox, inner),

            // ---- Quote: %zpcm ----
            Hoon::ZapCom(p, _q) => {
                let p_sut = self.play(sut.clone(), p)?;
                let p_sut = self.mull_nice(sut.clone(), gol, p_sut)?;
                let q_dox = self.play(dox.clone(), p)?;
                Ok((p_sut, q_dox))
            }

            // ---- Lost/error: %lost ----
            Hoon::Lost(_) => {
                if self.vet {
                    return Err(CompilerError::Noun("mull-skip".to_string()));
                }
                let void_ty = cons_void(&mut self.cx);
                self.mull_beth(sut, gol, void_ty)
            }

            // ---- Vase: %zpts ----
            // hoon-138: (beth %noun), with no recursion on p.gen
            Hoon::ZapTis(_p) => {
                let noun_ty = cons_noun(&mut self.cx);
                self.mull_beth(sut, gol, noun_ty)
            }

            // ---- Vase construction: %zpmc ----
            Hoon::ZapMic(p, q) => {
                let noun_gol = cons_noun(&mut self.cx);
                let vos = self.mull(sut.clone(), noun_gol, dox.clone(), q)?;
                let p_play_sut = self.play(sut.clone(), p)?;
                let p_play_dox = self.play(dox.clone(), p)?;
                let p_ty = cons_cell(&mut self.cx, p_play_sut, vos.0);
                let p_ty = self.mull_nice(sut.clone(), gol, p_ty)?;
                let q_ty = cons_cell(&mut self.cx, p_play_dox, vos.1);
                Ok((p_ty, q_ty))
            }

            // ---- Type extraction: %zpgl ----
            Hoon::ZapGal(spec, _q) => {
                // hoon-138: (beth (play [%kttr p.gen]))
                // hoon-138 marks this arm "XX is this right?".
                let kttr = Hoon::KetTar(Box::new(spec.as_ref().clone()));
                let ty = self.play(sut.clone(), &kttr)?;
                self.mull_beth(sut, gol, ty)
            }

            // ---- Conditional compilation: %zppt ----
            Hoon::ZapPat(wings, q, r) => {
                let feel_sut = self.feel(sut.clone(), wings)?;
                let feel_dox = self.feel(dox.clone(), wings)?;
                if feel_sut != feel_dox {
                    return Err(CompilerError::Noun("mull-bonk-f".to_string()));
                }
                if feel_sut {
                    self.mull(sut, gol, dox, q)
                } else {
                    self.mull(sut, gol, dox, r)
                }
            }

            // ---- Crash: %zpzp ----
            Hoon::ZapZap => {
                let void_ty = cons_void(&mut self.cx);
                self.mull_beth(sut, gol, void_ty)
            }

            // ---- Error sentinel ----
            Hoon::Eror(_) => {
                let void_ty = cons_void(&mut self.cx);
                self.mull_beth(sut, gol, void_ty)
            }

            // ---- Sugar forms lowered before mull ----
            // TisLus (=+) lowers to TisGar => handled above
            Hoon::TisLus(p, q) => {
                let lowered = Hoon::TisGar(
                    Box::new(Hoon::Pair(p.clone(), Box::new(Hoon::Axis((1u64).into())))),
                    q.clone(),
                );
                self.mull(sut, gol, dox, &lowered)
            }
            // TisHep (=-) is TisLus with args reversed
            Hoon::TisHep(p, q) => {
                let lowered = Hoon::TisGar(
                    Box::new(Hoon::Pair(q.clone(), Box::new(Hoon::Axis((1u64).into())))),
                    p.clone(),
                );
                self.mull(sut, gol, dox, &lowered)
            }
            // TisGal (=<) is compose reversed: =<(p, q) => =>(q, p)
            Hoon::TisGal(_, _) => self.mull_open_then_recurse(sut, gol, dox, gen),

            // ---- Open/macro fallback ----
            // All remaining forms: try open(gen), crash mull-open if unchanged
            _ => self.mull_open_then_recurse(sut, gol, dox, gen),
        }
    }

    /// Macro-expand gen via open() and recurse; crash mull-open if no expansion.
    fn mull_open_then_recurse(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        dox: NRc<NTy>,
        gen: &Hoon,
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        let opened = self.open_cached(gen);
        if let Some(opened) = opened {
            return self.mull(sut, gol, dox, opened.as_ref());
        }
        Err(CompilerError::Noun("mull-open".to_string()))
    }

    /// mull_nice: nest check against goal (matches hoon-138 ++nice in mull context)
    fn mull_nice(&mut self, _sut: NRc<NTy>, gol: NRc<NTy>, typ: NRc<NTy>) -> Result<NRc<NTy>> {
        if !self.vet {
            return Ok(typ);
        }
        if matches!(&*gol, NTy::Noun)
            || NRc::ptr_eq(&gol, &typ)
            || self.nest(gol.clone(), typ.clone())?
        {
            return Ok(typ);
        }
        Err(CompilerError::Noun("mull-nice".to_string()))
    }

    /// mull_beth: wrap a single type as both results (matches hoon-138 ++beth)
    fn mull_beth(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        typ: NRc<NTy>,
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        let p = self.mull_nice(sut, gol, typ.clone())?;
        Ok((p, typ))
    }

    /// cove: extract axis from a [0 N] nock formula (matches hoon-138 ++cove)
    fn cove(&self, formula: FormulaId) -> Result<BigUint> {
        self.formula_arena.cove(formula)
    }

    /// mull_grow: Handle core construction (%brcn, %brpt) in mull.
    /// Matches hoon-138 ++grow in the mull context (line 10252-10259).
    fn mull_grow(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        dox: NRc<NTy>,
        mel: Vair,
        nym: Option<&str>,
        hud: Poly,
        ruf: &Hoon,
        tomes: &HashMap<String, Tome>,
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        // Mull the payload (ruf, typically Hoon::Axis((1u64).into()))
        let noun_gol = cons_noun(&mut self.cx);
        let dan = self.mull(sut.clone(), noun_gol, dox, ruf)?;
        // Construct both core types and validate arms
        let yaz = self.mile(dan.0, dan.1, mel, nym, hud, tomes)?;
        let p_ty = self.mull_nice(sut, gol, yaz.0)?;
        Ok((p_ty, yaz.1))
    }

    fn mile(
        &mut self,
        sut: NRc<NTy>,
        dox: NRc<NTy>,
        mel: Vair,
        nym: Option<&str>,
        hud: Poly,
        dom: &HashMap<String, Tome>,
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        self.mull_mile(sut, dox, mel, nym, hud, dom)
    }

    /// mull_mile: Construct both core types and validate all arms.
    /// Matches hoon-138 ++mile (line 9758-9766).
    fn mull_mile(
        &mut self,
        sut: NRc<NTy>,
        dox: NRc<NTy>,
        _mel: Vair,
        nym: Option<&str>,
        hud: Poly,
        tomes: &HashMap<String, Tome>,
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        // Payload and context stay shared native types. The coil rest is a noun
        // leaf pairing a fully blocked seminoun (standing in for `laze`) with the
        // arm map; cons_core collapses a void payload to void, like ty_core.
        let tomes_map = self.tomes_map_from_ast(tomes)?;
        // Construct yet = core(sut, [nym hud gold], sut, laze, dom)
        let garb = garb_native(nym, hud, Vair::Gold);
        let semi_noun = self.semi_noun_blocked();
        let rest = T(self.slab, &[semi_noun, tomes_map]);
        let yet = {
            let space = self.slab.noun_space();
            let rest_leaf = live_leaf_from_noun(&mut self.cx, rest, &space);
            cons_core(
                &mut self.cx,
                sut.clone(),
                garb.clone(),
                sut.clone(),
                rest_leaf,
            )
        };

        // Construct hum = core(dox, [nym hud gold], dox, laze, dom)
        let garb_hum = garb_native(nym, hud, Vair::Gold);
        let hum = {
            let space = self.slab.noun_space();
            let rest_leaf = live_leaf_from_noun(&mut self.cx, rest, &space);
            cons_core(&mut self.cx, dox.clone(), garb_hum, dox.clone(), rest_leaf)
        };

        // Validate arms: balk(sut=yet) hum hud dom
        self.mull_balk(yet.clone(), hum.clone(), hud, tomes)?;
        Ok((yet, hum))
    }

    /// mull_balk: Walk chapters and validate each.
    /// Matches hoon-138 ++balk (line 9745-9756).
    fn mull_balk(
        &mut self,
        sut: NRc<NTy>,
        dox: NRc<NTy>,
        hud: Poly,
        tomes: &HashMap<String, Tome>,
    ) -> Result<()> {
        self.with_stack_guard(|ut| {
            for (_chapter_name, tome) in tomes {
                let (_chapter_note, arms) = tome;
                ut.mull_bake(sut.clone(), dox.clone(), hud, arms)?;
            }
            Ok(())
        })
    }

    /// mull_bake: Walk arm map and mull each dry arm.
    /// Matches hoon-138 ++bake (line 9726-9743).
    fn mull_bake(
        &mut self,
        sut: NRc<NTy>,
        dox: NRc<NTy>,
        hud: Poly,
        arms: &HashMap<String, Hoon>,
    ) -> Result<()> {
        self.with_stack_guard(|ut| {
            for (_arm_name, hoon) in arms {
                match hud {
                    Poly::Dry => {
                        let noun_gol = cons_noun(&mut ut.cx);
                        let _ = ut.mull(sut.clone(), noun_gol, dox.clone(), hoon)?;
                    }
                    Poly::Wet => {
                        // Wet arms are checked lazily at fire time, not at definition time
                    }
                }
            }
            Ok(())
        })
    }

    /// mull_cnts: Handle %cnts (wing edits) in mull.
    /// Matches hoon-138 ++et.mull (line 9258-9267).
    fn mull_cnts(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        dox: NRc<NTy>,
        wing: &WingType,
        pairs: &[(WingType, Hoon)],
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        let lug_p = self.find(sut.clone(), Way::Read, wing)?;
        let lug_q = self.find(dox.clone(), Way::Read, wing)?;
        self.mull_cnts_with_ports(sut, gol, dox, &lug_p, &lug_q, pairs)
    }

    fn emul(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        dox: NRc<NTy>,
        hyp: &WingType,
        rig: &[(WingType, Hoon)],
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        self.mull_cnts(sut, gol, dox, hyp, rig)
    }

    fn mull_cnts_with_ports(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        dox: NRc<NTy>,
        lug_p: &Port,
        lug_q: &Port,
        pairs: &[(WingType, Hoon)],
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        match (lug_p, lug_q) {
            // Both synthetic: assert no edits, return both types
            (Port::Synthetic { typ: typ_p, .. }, Port::Synthetic { typ: typ_q, .. }) => {
                if !pairs.is_empty() {
                    return Err(CompilerError::Noun("mull-bonk-cnts".to_string()));
                }
                let p_ty = self.mull_nice(sut, gol, typ_p.clone())?;
                Ok((p_ty, typ_q.clone()))
            }
            // Mixed synthetic/natural: reject (hoon-138 asserts)
            (Port::Synthetic { .. }, Port::Palo(_)) | (Port::Palo(_), Port::Synthetic { .. }) => {
                Err(CompilerError::Noun("mull-bonk-cnts".to_string()))
            }
            // Both natural: delegate to mull_endo
            (Port::Palo(palo_p), Port::Palo(palo_q)) => {
                let result =
                    self.mull_endo(sut.clone(), gol.clone(), dox, palo_p, palo_q, pairs)?;
                // Apply nice check on sut-side result
                if self.vet {
                    let _ = self.mull_nice(sut, gol, result.0.clone())?;
                }
                Ok(result)
            }
        }
    }

    /// mull_endo: Handle wing edits for natural ports.
    /// Matches hoon-138 ++endo (line 9206-9239).
    fn mull_endo(
        &mut self,
        sut: NRc<NTy>,
        _gol: NRc<NTy>,
        dox: NRc<NTy>,
        palo_p: &Palo,
        palo_q: &Palo,
        rig: &[(WingType, Hoon)],
    ) -> Result<(NRc<NTy>, NRc<NTy>)> {
        self.with_stack_guard(|ut| match (&palo_p.opal, &palo_q.opal) {
            // Both legs: tack both sides for each edit
            (Opal::Leg(leg_p), Opal::Leg(leg_q)) => {
                let mut current_p = leg_p.clone();
                let mut current_q = leg_q.clone();
                for (sub_wing, expr) in rig {
                    let noun_gol = cons_noun(&mut ut.cx);
                    let zil = ut.mull(sut.clone(), noun_gol, dox.clone(), expr)?;
                    let dar_p = ut.tack(current_p, sub_wing, zil.0)?;
                    let dar_q = ut.tack(current_q, sub_wing, zil.1)?;
                    // Assert axes match (bonk check)
                    if dar_p.0 != dar_q.0 {
                        return Err(CompilerError::Noun(
                            "mull-bonk: endo leg axis mismatch".to_string(),
                        ));
                    }
                    current_p = dar_p.1;
                    current_q = dar_q.1;
                }
                Ok((current_p, current_q))
            }
            // Both arms: toss both sides, then fire
            (
                Opal::Arm {
                    axis: axis_p,
                    arms: arms_p,
                },
                Opal::Arm {
                    axis: axis_q,
                    arms: arms_q,
                },
            ) => {
                if axis_p != axis_q {
                    return Err(CompilerError::Noun(
                        "mull-bonk: endo arm axis mismatch".to_string(),
                    ));
                }
                let mut hag_p = arms_p.clone();
                let mut hag_q = arms_q.clone();
                for (sub_wing, expr) in rig {
                    let noun_gol = cons_noun(&mut ut.cx);
                    let zil = ut.mull(sut.clone(), noun_gol, dox.clone(), expr)?;
                    let dix_p = ut.toss(sub_wing, zil.0, &hag_p)?;
                    let dix_q = ut.toss(sub_wing, zil.1, &hag_q)?;
                    // Assert axes match
                    if dix_p.0 != dix_q.0 {
                        return Err(CompilerError::Noun(
                            "mull-bonk: endo arm toss axis mismatch".to_string(),
                        ));
                    }
                    hag_p = dix_p.1;
                    hag_q = dix_q.1;
                }
                // Fire the sut side with the current vet, the dox side with vet off.
                let p_ty = ut.fire(&hag_p)?;
                let q_ty = ut.with_vet_off(|ut| ut.fire(&hag_q))?;
                Ok((p_ty, q_ty))
            }
            // Mismatched opal types: one leg, one arm
            _ => Err(CompilerError::Noun(
                "mull-bonk: endo mismatched leg/arm opals".to_string(),
            )),
        })
    }
}

fn atom_is_zero(atom: &ParsedAtom) -> bool {
    match atom {
        ParsedAtom::Small(n) => *n == 0,
        ParsedAtom::Big(b) => b.to_bytes_le().is_empty(),
    }
}

fn atom_is_flag(atom: &ParsedAtom) -> bool {
    match atom {
        ParsedAtom::Small(n) => *n <= 1,
        ParsedAtom::Big(b) => {
            let bytes = b.to_bytes_le();
            if bytes.is_empty() {
                return true;
            }
            bytes.len() == 1 && bytes[0] == 1
        }
    }
}

fn noun_eq(a: Noun, b: Noun, space: &NounSpace) -> Result<bool> {
    // Local alias for the shared structural equality in `native::noun`.
    crate::native::noun::noun_eq(a, b, space)
}

fn noun_is_zero(noun: Noun) -> bool {
    unsafe { noun.raw_equals(&D(0)) }
}

fn noun_u64(slab: &mut NounSlab, value: u64) -> Noun {
    Atom::new(slab, value).as_noun()
}

fn noun_biguint(slab: &mut NounSlab, value: BigUint) -> Noun {
    let atom = ParsedAtom::from_biguint(value);
    parsed_atom_to_noun(slab, &atom)
}

// See `HONK_EVAL_STACK_SIZE` in bin/honk.rs: fold-heavy entries intern
// mack-core copies on this stack for the duration of an entry compile.
const HONK_EVAL_STACK_SIZE: usize = NOCK_STACK_SIZE_MEDIUM; // 16GB

fn create_musk_eval_context() -> NockContext {
    let mut stack = NockStack::new(HONK_EVAL_STACK_SIZE, 0);
    let cold = Cold::new(&mut stack);
    create_context(
        stack,
        native_hot_state(),
        cold,
        None,
        vec![],
        JetDispatchMode::HintBlind,
    )
}

fn slot_formula_axis_noun(slab: &mut NounSlab, axis_noun: Noun) -> Noun {
    T(slab, &[D(0), axis_noun])
}

fn slot_formula_axis_big(slab: &mut NounSlab, axis: BigUint) -> Noun {
    let axis_noun = noun_biguint(slab, axis);
    slot_formula_axis_noun(slab, axis_noun)
}

fn slab_mug(noun: Noun, space: &NounSpace) -> u32 {
    // Local alias for the shared iterative mug in `native::noun`.
    crate::native::noun::slab_mug(noun, space)
}

fn install_musk_cold_state(context: &mut NockContext, raw: &[u8], label: &str) -> Result<()> {
    let cold_noun = <Noun as NounExt>::cue_bytes_slice(&mut context.stack, raw)
        .map_err(|err| CompilerError::Decode(format!("cue {label} cold jam: {err:?}")))?;
    let stack_space = context.stack.noun_space();
    // The cold noun was cued into this same stack above, so the resident
    // decode borrows batteries and paths in place instead of deep-copying
    // every core a second time.
    let (battery_to_paths, root_to_paths, path_to_batteries) =
        nockvm::jets::cold::cold_from_noun_resident(&mut context.stack, &cold_noun, &stack_space)
            .map_err(|err| CompilerError::Decode(format!("decode {label} cold state: {err:?}")))?;
    context.cold = Cold::from_vecs(
        &mut context.stack, battery_to_paths, root_to_paths, path_to_batteries,
    );
    context.warm = Warm::init(
        &mut context.stack, &mut context.cold, &context.hot, &context.test_jets,
        context.jet_dispatch,
    );
    Ok(())
}

fn dor(slab: &mut NounSlab, a: Noun, b: Noun) -> bool {
    if unsafe { a.raw_equals(&b) } {
        return true;
    }
    let space = slab.noun_space();
    let a_mug = get_mug(a, &space).unwrap_or_else(|| slab_mug(a, &space));
    let b_mug = get_mug(b, &space).unwrap_or_else(|| slab_mug(b, &space));
    if a_mug == b_mug && matches!(noun_eq(a, b, &space), Ok(true)) {
        return true;
    }
    let a_atom = a.is_atom();
    let b_atom = b.is_atom();
    match (a_atom, b_atom) {
        (true, true) => {
            let atom_a = a
                .in_space(&space)
                .as_atom()
                .expect("atom expected when a.is_atom()");
            let atom_b = b
                .in_space(&space)
                .as_atom()
                .expect("atom expected when b.is_atom()");
            lth_b(slab, atom_a.atom(), atom_b.atom(), &space)
        }
        (true, false) => true,
        (false, true) => false,
        (false, false) => {
            let cell_a = a
                .in_space(&space)
                .as_cell()
                .expect("cell expected when a.is_cell()");
            let cell_b = b
                .in_space(&space)
                .as_cell()
                .expect("cell expected when b.is_cell()");
            let a_head = cell_a.head().noun();
            let b_head = cell_b.head().noun();
            let a_tail = cell_a.tail().noun();
            let b_tail = cell_b.tail().noun();
            if unsafe { a_head.raw_equals(&b_head) }
                || (slab_mug(a_head, &space) == slab_mug(b_head, &space)
                    && matches!(noun_eq(a_head, b_head, &space), Ok(true)))
            {
                dor(slab, a_tail, b_tail)
            } else {
                dor(slab, a_head, b_head)
            }
        }
    }
}

fn gor_mug(slab: &mut NounSlab, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    match slab_mug(a, &space).cmp(&slab_mug(b, &space)) {
        cmp::Ordering::Less => true,
        cmp::Ordering::Greater => false,
        cmp::Ordering::Equal => dor(slab, a, b),
    }
}

fn mor_mug(slab: &mut NounSlab, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    let mug_a = slab_mug(a, &space);
    let mug_b = slab_mug(b, &space);
    let mug_mug_a = slab_mug(noun_u64(slab, mug_a as u64), &space);
    let mug_mug_b = slab_mug(noun_u64(slab, mug_b as u64), &space);
    match mug_mug_a.cmp(&mug_mug_b) {
        cmp::Ordering::Less => true,
        cmp::Ordering::Greater => false,
        cmp::Ordering::Equal => dor(slab, a, b),
    }
}

fn set_node(slab: &mut NounSlab, key: Noun, left: Noun, right: Noun) -> Noun {
    T(slab, &[key, left, right])
}

fn set_parts(tree: Noun, space: &NounSpace) -> Result<(Noun, Noun, Noun)> {
    let cell = tree
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("set node not cell: {err}")))?;
    let branches = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("set node missing branches: {err}")))?;
    Ok((
        cell.head().noun(),
        branches.head().noun(),
        branches.tail().noun(),
    ))
}

fn set_put_mug(slab: &mut NounSlab, tree: Noun, key: Noun) -> Result<Noun> {
    if noun_is_zero(tree) {
        return Ok(set_node(slab, key, D(0), D(0)));
    }

    let space = slab.noun_space();
    let (node_key, left, right) = set_parts(tree, &space)?;

    if noun_eq(key, node_key, &space)? {
        return Ok(tree);
    }

    if gor_mug(slab, key, node_key) {
        let d = set_put_mug(slab, left, key)?;
        let space = slab.noun_space();
        let (d_key, d_left, d_right) = set_parts(d, &space)?;

        if mor_mug(slab, node_key, d_key) {
            Ok(set_node(slab, node_key, d, right))
        } else {
            let new_a = set_node(slab, node_key, d_right, right);
            Ok(set_node(slab, d_key, d_left, new_a))
        }
    } else {
        let d = set_put_mug(slab, right, key)?;
        let space = slab.noun_space();
        let (d_key, d_left, d_right) = set_parts(d, &space)?;

        if mor_mug(slab, node_key, d_key) {
            Ok(set_node(slab, node_key, left, d))
        } else {
            let new_a = set_node(slab, node_key, left, d_left);
            Ok(set_node(slab, d_key, new_a, d_right))
        }
    }
}

fn type_fork_set(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("fork not cell: {err}")))?;
    Ok(cell.tail().noun())
}

fn fork_set_insert(slab: &mut NounSlab, set: Noun, option: Noun) -> Result<Noun> {
    let space = slab.noun_space();
    match type_tag(option, &space)?.as_str() {
        "void" => Ok(set),
        "fork" => set_uni_mug(slab, set, type_fork_set(option, &space)?),
        _ => set_put_mug(slab, set, option),
    }
}

fn set_uni_mug(slab: &mut NounSlab, a: Noun, b: Noun) -> Result<Noun> {
    // Direct translation of hoon-138 `++uni` for `%set` treaps.
    //
    // `++fork` does not rebuild nested forks by tapping their keys back through
    // `++put`; it calls `~(uni in set)`, whose split-and-merge recursion fixes the
    // exact treap shape. Re-inserting every key preserves membership but not bytes:
    // large named-type forks can move a `%hint` key to a different branch and then
    // diverge in `~|`/`^~` embedded type artifacts.
    let space = slab.noun_space();
    if unsafe { a.raw_equals(&b) } || noun_eq(a, b, &space)? {
        return Ok(a);
    }
    if noun_is_zero(b) {
        return Ok(a);
    }
    if noun_is_zero(a) {
        return Ok(b);
    }

    let (a_key, a_left, a_right) = set_parts(a, &space)?;
    let (b_key, b_left, b_right) = set_parts(b, &space)?;

    if noun_eq(b_key, a_key, &space)? {
        let left = set_uni_mug(slab, a_left, b_left)?;
        let right = set_uni_mug(slab, a_right, b_right)?;
        return Ok(set_node(slab, b_key, left, right));
    }

    if mor_mug(slab, a_key, b_key) {
        if gor_mug(slab, b_key, a_key) {
            let b_without_right = set_node(slab, b_key, b_left, D(0));
            let left = set_uni_mug(slab, a_left, b_without_right)?;
            let merged_a = set_node(slab, a_key, left, a_right);
            set_uni_mug(slab, merged_a, b_right)
        } else {
            let b_without_left = set_node(slab, b_key, D(0), b_right);
            let right = set_uni_mug(slab, a_right, b_without_left)?;
            let merged_a = set_node(slab, a_key, a_left, right);
            set_uni_mug(slab, merged_a, b_left)
        }
    } else if gor_mug(slab, a_key, b_key) {
        let a_without_right = set_node(slab, a_key, a_left, D(0));
        let left = set_uni_mug(slab, a_without_right, b_left)?;
        let merged_b = set_node(slab, b_key, left, b_right);
        set_uni_mug(slab, a_right, merged_b)
    } else {
        let a_without_left = set_node(slab, a_key, D(0), a_right);
        let right = set_uni_mug(slab, a_without_left, b_right)?;
        let merged_b = set_node(slab, b_key, b_left, right);
        set_uni_mug(slab, a_left, merged_b)
    }
}

fn map_put_mug(slab: &mut NounSlab, tree: Noun, key: Noun, value: Noun) -> Result<Noun> {
    if noun_is_zero(tree) {
        let node = T(slab, &[key, value]);
        return Ok(T(slab, &[node, D(0), D(0)]));
    }

    let space = slab.noun_space();
    let tree_cell = tree
        .in_space(&space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("map put tree not cell: {err}")))?;
    let node = tree_cell.head().noun();
    let rest_cell = tree_cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("map put tree missing branches: {err}")))?;
    let left = rest_cell.head().noun();
    let right = rest_cell.tail().noun();

    let node_cell = node
        .in_space(&space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("map put node not cell: {err}")))?;
    let node_key = node_cell.head().noun();
    let node_val = node_cell.tail().noun();

    if noun_eq(key, node_key, &space)? {
        if noun_eq(value, node_val, &space)? {
            return Ok(tree);
        }
        let new_node = T(slab, &[key, value]);
        return Ok(T(slab, &[new_node, left, right]));
    }

    if gor_mug(slab, key, node_key) {
        let d = map_put_mug(slab, left, key, value)?;
        let space = slab.noun_space();
        let d_cell = d
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("map put left not cell: {err}")))?;
        let d_node = d_cell.head().noun();
        let d_rest_cell = d_cell.tail().as_cell().map_err(|err| {
            CompilerError::Decode(format!("map put left missing branches: {err}"))
        })?;
        let d_left = d_rest_cell.head().noun();
        let d_right = d_rest_cell.tail().noun();
        let d_node_cell = d_node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("map put left node not cell: {err}")))?;
        let d_key = d_node_cell.head().noun();

        if mor_mug(slab, node_key, d_key) {
            Ok(T(slab, &[node, d, right]))
        } else {
            let new_a = T(slab, &[node, d_right, right]);
            Ok(T(slab, &[d_node, d_left, new_a]))
        }
    } else {
        let d = map_put_mug(slab, right, key, value)?;
        let space = slab.noun_space();
        let d_cell = d
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("map put right not cell: {err}")))?;
        let d_node = d_cell.head().noun();
        let d_rest_cell = d_cell.tail().as_cell().map_err(|err| {
            CompilerError::Decode(format!("map put right missing branches: {err}"))
        })?;
        let d_left = d_rest_cell.head().noun();
        let d_right = d_rest_cell.tail().noun();
        let d_node_cell = d_node
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("map put right node not cell: {err}")))?;
        let d_key = d_node_cell.head().noun();

        if mor_mug(slab, node_key, d_key) {
            Ok(T(slab, &[node, left, d]))
        } else {
            let new_a = T(slab, &[node, left, d_left]);
            Ok(T(slab, &[d_node, new_a, d_right]))
        }
    }
}

fn map_to_noun(slab: &mut NounSlab, pairs: Vec<(Noun, Noun)>) -> Result<Noun> {
    let mut map = D(0);
    for (key, val) in pairs {
        map = map_put_mug(slab, map, key, val)?;
    }
    Ok(map)
}

fn map_node(noun: Noun, space: &NounSpace) -> Result<Option<(Noun, Noun, Noun)>> {
    if noun_is_zero(noun) {
        return Ok(None);
    }
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("map node not cell: {err}")))?;
    let node = cell.head().noun();
    let rest = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("map node missing branches: {err}")))?;
    let left = rest.head().noun();
    let right = rest.tail().noun();
    Ok(Some((node, left, right)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeTagKind {
    Void,
    Noun,
    Atom,
    Cell,
    Core,
    Face,
    Fork,
    Hint,
    Hold,
}

fn type_tag_atom<'a>(noun: Noun, space: &'a NounSpace) -> Result<AtomHandle<'a>> {
    let noun = noun.in_space(space);
    if let Ok(atom) = noun.as_atom() {
        return Ok(atom);
    }
    let cell = noun
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("type tag noun not cell: {err}")))?;
    cell.head()
        .as_atom()
        .map_err(|err| CompilerError::Decode(format!("type tag head not atom: {err}")))
}

fn type_tag_kind(noun: Noun, space: &NounSpace) -> Result<TypeTagKind> {
    let atom = type_tag_atom(noun, space)?;
    if atom.eq_bytes(b"void") {
        Ok(TypeTagKind::Void)
    } else if atom.eq_bytes(b"noun") {
        Ok(TypeTagKind::Noun)
    } else if atom.eq_bytes(b"atom") {
        Ok(TypeTagKind::Atom)
    } else if atom.eq_bytes(b"cell") {
        Ok(TypeTagKind::Cell)
    } else if atom.eq_bytes(b"core") {
        Ok(TypeTagKind::Core)
    } else if atom.eq_bytes(b"face") {
        Ok(TypeTagKind::Face)
    } else if atom.eq_bytes(b"fork") {
        Ok(TypeTagKind::Fork)
    } else if atom.eq_bytes(b"hint") {
        Ok(TypeTagKind::Hint)
    } else if atom.eq_bytes(b"hold") {
        Ok(TypeTagKind::Hold)
    } else {
        let tag = type_tag(noun, space)?;
        Err(CompilerError::Decode(format!("unknown type tag: {tag}")))
    }
}

fn type_tag(noun: Noun, space: &NounSpace) -> Result<String> {
    tag(noun, space).map_err(|err| CompilerError::Decode(format!("type tag: {err}")))
}

fn type_cell_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("type cell not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("type cell missing tail: {err}")))?;
    Ok((tail.head().noun(), tail.tail().noun()))
}

fn type_face_inner(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("face not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("face missing tail: {err}")))?;
    Ok(tail.tail().noun())
}

#[cfg(test)]
fn type_face_name_if_atom(noun: Noun, space: &NounSpace) -> Result<Option<String>> {
    let tool = type_face_tool(noun, space)?;
    let name_atom = match tool.in_space(space).as_atom() {
        Ok(atom) => atom,
        Err(_) => return Ok(None),
    };
    atom_to_string(name_atom)
        .map(Some)
        .map_err(|err| CompilerError::Decode(format!("face name: {err}")))
}

fn type_face_tool(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("face not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("face missing tail: {err}")))?;
    Ok(tail.head().noun())
}

fn type_face_with_inner(slab: &mut NounSlab, face: Noun, inner: Noun) -> Result<Noun> {
    let space = slab.noun_space();
    let tool = type_face_tool(face, &space)?;
    Ok(ty_face_tool(slab, tool, inner))
}

fn type_hint_inner(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hint not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hint missing tail: {err}")))?;
    Ok(tail.tail().noun())
}

fn type_hint_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hint not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hint missing tail: {err}")))?;
    let inner_note = tail
        .head()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hint inner not cell: {err}")))?;
    let inner = inner_note.head().noun();
    let note = inner_note.tail().noun();
    let payload = tail.tail().noun();
    Ok((inner, note, payload))
}

#[cfg(test)]
fn type_hold_type(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hold not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hold missing tail: {err}")))?;
    Ok(tail.head().noun())
}

fn type_hold_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hold not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("hold missing tail: {err}")))?;
    Ok((tail.head().noun(), tail.tail().noun()))
}

fn fork_set_options(noun: Noun, space: &NounSpace) -> Result<Vec<Noun>> {
    let mut out = Vec::new();
    let mut stack: Vec<Noun> = Vec::new();
    let mut current = noun;
    let mut visited_nodes: usize = 0;
    loop {
        while !noun_is_zero(current) {
            visited_nodes = visited_nodes.saturating_add(1);
            if visited_nodes > 1_000_000 {
                return Err(CompilerError::Decode(
                    "fork set decode exceeded node budget".to_string(),
                ));
            }
            let cell = current
                .in_space(space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("fork set node not cell: {err}")))?;
            let branches = cell.tail().as_cell().map_err(|err| {
                CompilerError::Decode(format!("fork set node missing branches: {err}"))
            })?;
            stack.push(current);
            current = branches.tail().noun();
        }

        let Some(node) = stack.pop() else {
            break;
        };
        let cell = node
            .in_space(space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("fork set node not cell: {err}")))?;
        out.push(cell.head().noun());
        let branches = cell.tail().as_cell().map_err(|err| {
            CompilerError::Decode(format!("fork set node missing branches: {err}"))
        })?;
        current = branches.head().noun();
    }
    Ok(out)
}

fn type_fork_options(noun: Noun, space: &NounSpace) -> Result<Vec<Noun>> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("fork not cell: {err}")))?;
    fork_set_options(cell.tail().noun(), space)
}

fn type_atom_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Option<Noun>)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("atom not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("atom missing tail: {err}")))?;
    let aura_atom = tail
        .head()
        .as_atom()
        .map_err(|err| CompilerError::Decode(format!("atom aura not atom: {err}")))?;
    let aura = aura_atom.as_noun().noun();
    let bits = opt_from_noun(tail.tail().noun(), space)?;
    Ok((aura, bits))
}

fn type_core_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("core not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("core missing tail: {err}")))?;
    Ok((tail.head().noun(), tail.tail().noun()))
}

fn coil_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("coil not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("coil missing tail: {err}")))?;
    Ok((cell.head().noun(), tail.head().noun(), tail.tail().noun()))
}

fn coil_tomes(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("coil rest not cell: {err}")))?;
    Ok(cell.tail().noun())
}

fn garb_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("garb not cell: {err}")))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("garb missing tail: {err}")))?;
    Ok((cell.head().noun(), tail.head().noun(), tail.tail().noun()))
}

fn foot_parts(noun: Noun, space: &NounSpace) -> Result<(Poly, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("foot not cell: {err}")))?;
    let tag_atom = cell
        .head()
        .as_atom()
        .map_err(|err| CompilerError::Decode(format!("foot tag not atom: {err}")))?;
    let tag = atom_to_string(tag_atom)
        .map_err(|err| CompilerError::Decode(format!("foot tag: {err}")))?;
    let hoon = cell.tail().noun();
    let poly = match tag.as_str() {
        "wet" => Poly::Wet,
        "dry" => Poly::Dry,
        _ => {
            return Err(CompilerError::UnsupportedExpr(format!(
                "native mint: foot {tag}"
            )))
        }
    };
    Ok((poly, hoon))
}

fn foot_from_poly(slab: &mut NounSlab, poly: Poly, hoon: Noun) -> Noun {
    let tag = term_to_noun(
        slab,
        match poly {
            Poly::Wet => "wet",
            Poly::Dry => "dry",
        },
    );
    T(slab, &[tag, hoon])
}

#[cfg(test)]
fn find_face_axis_skip(
    noun: Noun,
    name: &str,
    skip: u64,
    space: &NounSpace,
) -> Result<Option<BigUint>> {
    let (found, _skip) = find_face_axis_skip_inner(noun, name, skip, space)?;
    Ok(found)
}

#[cfg(test)]
fn find_face_axis_skip_inner(
    noun: Noun,
    name: &str,
    skip: u64,
    space: &NounSpace,
) -> Result<(Option<BigUint>, u64)> {
    let tag = type_tag(noun, space)?;
    match tag.as_str() {
        "face" => {
            let mut remaining = skip;
            if type_face_name_if_atom(noun, space)?.as_deref() == Some(name) {
                if remaining == 0 {
                    return Ok((Some(BigUint::from(1u32)), remaining));
                }
                remaining = remaining.saturating_sub(1);
            }
            let inner = type_face_inner(noun, space)?;
            find_face_axis_skip_inner(inner, name, remaining, space)
        }
        "cell" => {
            let (head, tail) = type_cell_parts(noun, space)?;
            let (found_head, remaining) = find_face_axis_skip_inner(head, name, skip, space)?;
            if let Some(axis) = found_head {
                return Ok((
                    Some(peg_axis_big_pair(BigUint::from(2u32), &axis)?),
                    remaining,
                ));
            }
            let (found_tail, remaining) = find_face_axis_skip_inner(tail, name, remaining, space)?;
            if let Some(axis) = found_tail {
                return Ok((
                    Some(peg_axis_big_pair(BigUint::from(3u32), &axis)?),
                    remaining,
                ));
            }
            Ok((None, remaining))
        }
        "hint" => {
            let inner = type_hint_inner(noun, space)?;
            find_face_axis_skip_inner(inner, name, skip, space)
        }
        "hold" => {
            let inner = type_hold_type(noun, space)?;
            find_face_axis_skip_inner(inner, name, skip, space)
        }
        "core" => {
            let (payload, coil) = type_core_parts(noun, space)?;
            let (_garb, context, _rest) = coil_parts(coil, space)?;
            let (found, rem) = find_face_axis_skip_inner(payload, name, skip, space)?;
            if let Some(axis) = found {
                return Ok((Some(peg_axis_big_pair(BigUint::from(3u32), &axis)?), rem));
            }
            let (found, rem) = find_face_axis_skip_inner(context, name, rem, space)?;
            if let Some(axis) = found {
                return Ok((Some(peg_axis_big_pair(BigUint::from(7u32), &axis)?), rem));
            }
            Ok((None, rem))
        }
        "fork" => {
            let options = type_fork_options(noun, space)?;
            let mut remaining = skip;
            let mut found_axis: Option<BigUint> = None;
            for option in options {
                let (found, rem) = find_face_axis_skip_inner(option, name, remaining, space)?;
                remaining = rem;
                if let Some(axis) = found {
                    if let Some(existing) = &found_axis {
                        if existing != &axis {
                            return Ok((None, remaining));
                        }
                    } else {
                        found_axis = Some(axis);
                    }
                }
            }
            Ok((found_axis, remaining))
        }
        _ => Ok((None, skip)),
    }
}

fn axis_cap_mas(axis: u64) -> Result<(u64, u64)> {
    if axis <= 1 {
        return Err(CompilerError::Decode("axis must be > 1".to_string()));
    }
    let bit_len = 64u32
        .checked_sub(axis.leading_zeros())
        .ok_or_else(|| CompilerError::Decode("axis bit length underflow".to_string()))?;
    if bit_len < 2 {
        return Err(CompilerError::Decode(
            "axis bit length too small".to_string(),
        ));
    }

    let head_shift = bit_len - 2;
    let head_bit = (axis >> head_shift) & 1;
    let cap = if head_bit == 0 { 2 } else { 3 };

    let suffix_bits = head_shift;
    let suffix_mask = if suffix_bits == 0 {
        0
    } else {
        (1u64 << suffix_bits) - 1
    };
    let suffix = axis & suffix_mask;
    let mas = (1u64 << suffix_bits) | suffix;
    Ok((cap, mas))
}

fn noun_axis_atom_to_big(atom: AtomHandle<'_>) -> BigUint {
    BigUint::from_bytes_le(atom.as_ne_bytes())
}

fn axis_big_cap_mas(axis: &BigUint) -> Result<(u64, BigUint)> {
    if axis <= &BigUint::from(1u32) {
        return Err(CompilerError::Decode("axis must be > 1".to_string()));
    }
    let bit_len = axis.bits();
    if bit_len < 2 {
        return Err(CompilerError::Decode(
            "axis bit length too small".to_string(),
        ));
    }

    let head_shift = bit_len - 2;
    let head_shift_usize = usize::try_from(head_shift)
        .map_err(|_| CompilerError::Decode(format!("axis shift exceeds usize: {head_shift}")))?;
    let head_bit = (axis >> head_shift_usize) & BigUint::from(1u32);
    let cap = if head_bit == BigUint::from(0u32) {
        2
    } else {
        3
    };

    let base = BigUint::from(1u32) << head_shift_usize;
    let suffix_mask = &base - BigUint::from(1u32);
    let suffix = axis & &suffix_mask;
    let mas = base + suffix;
    Ok((cap, mas))
}

fn peel(way: Way, met: Vair) -> (bool, bool) {
    if met == Vair::Gold {
        return (true, true);
    }
    match way {
        Way::Both => (false, false),
        Way::Free => (true, true),
        Way::Read => (met == Vair::Zinc, false),
        Way::Rite => (met == Vair::Iron, false),
    }
}

fn peg_axis_big_pair(mut a: BigUint, b: &BigUint) -> Result<BigUint> {
    if b == &BigUint::from(0u32) {
        return Err(CompilerError::Decode(
            "peg axis: peg: a and b must be non-zero".to_string(),
        ));
    }

    let k = b.bits().saturating_sub(1);
    let k_usize = usize::try_from(k)
        .map_err(|_| CompilerError::Decode(format!("axis shift exceeds usize: {k}")))?;
    let base = BigUint::from(1u32) << k_usize;
    let offset = b - &base;
    a <<= k;
    a += offset;
    Ok(a)
}

#[track_caller]
fn peg_axis_big(a: BigUint, b: u64) -> Result<BigUint> {
    peg_axis_big_pair(a, &BigUint::from(b))
}

fn tend_big(vein: &[Option<BigUint>]) -> Result<BigUint> {
    let mut axis = BigUint::from(1u32);
    for step in vein.iter().rev() {
        let step = step.clone().unwrap_or_else(|| BigUint::from(1u32));
        axis = peg_axis_big_pair(axis, &step)?;
    }
    Ok(axis)
}

/// Test-only prototype of a chunked mint for a `=> p1 => … => body` compose
/// chain. Only `chunked_tisgar_chain_matches_monolithic_mint` calls it; the
/// production chunked prelude mint is `mint_honc_prelude_chunked` in
/// `bin/honk.rs`.
///
/// Composes like `mint_tsgr_arena`: each pre-body layer mints with goal `%noun`
/// against the carried subject, the body with the outer `gol`, and the formulas
/// fold right through `comb`. Each layer mints in its own `Ut` and slab, and
/// only the subject type and the layer's formula are copied into `out_slab`.
///
/// The output matches the monolithic mint byte for byte when each layer's cores
/// have full batteries, so cross-layer name resolution never needs the lazy
/// resolvers dropped with each working slab. `out_slab` keeps every carried
/// subject copy, though only the latest is live.
#[cfg(test)]
pub(crate) fn mint_tisgar_chain_chunked(
    out_slab: &mut NounSlab,
    sut: Noun,
    gol: Noun,
    chain: &Hoon,
) -> Result<(Noun, Noun)> {
    let mut layers: Vec<&Hoon> = Vec::new();
    let mut cur = chain;
    while let Hoon::TisGar(p, q) = cur {
        layers.push(p.as_ref());
        cur = q.as_ref();
    }
    layers.push(cur);

    let mut subject = sut;
    let mut layer_formulas: Vec<Noun> = Vec::with_capacity(layers.len());
    let last = layers.len() - 1;
    for (i, layer) in layers.iter().enumerate() {
        // Each layer gets a fresh slab and a fresh `Ut`, whose `Context` holds
        // slab-bound nouns in its intern table and lowering memos; sharing one
        // across layers would hand out nouns from a dropped slab. Only the
        // `subject` and `gol` nouns cross layers, copied in below.
        let mut layer_slab: NounSlab = NounSlab::new();
        {
            let mut ut = Ut::new(&mut layer_slab);
            let sub_in = ut.slab.copy_into(subject, &out_slab.noun_space());
            let goal = if i == last {
                ut.slab.copy_into(gol, &out_slab.noun_space())
            } else {
                ty_noun(&mut *ut.slab)
            };
            let (ty, formula) = ut.mint_noun(sub_in, goal, layer)?;
            let ut_space = ut.slab.noun_space();
            subject = out_slab.copy_into(ty, &ut_space);
            layer_formulas.push(out_slab.copy_into(formula, &ut_space));
        }
        // layer_slab dropped here, reclaiming this layer's working memory.
    }

    let mut formula = layer_formulas.pop().expect("compose chain has a body");
    while let Some(head) = layer_formulas.pop() {
        formula = crate::native::formula::comb(out_slab, head, formula)?;
    }
    Ok((subject, formula))
}

pub fn ty_noun(slab: &mut NounSlab) -> Noun {
    term_to_noun(slab, "noun")
}

pub fn ty_void(slab: &mut NounSlab) -> Noun {
    term_to_noun(slab, "void")
}

fn ty_cell(slab: &mut NounSlab, head: Noun, tail: Noun) -> Noun {
    let tag = term_to_noun(slab, "cell");
    T(slab, &[tag, head, tail])
}

fn ty_atom(slab: &mut NounSlab, aura: &str, value: Option<Noun>) -> Noun {
    let aura = if aura.is_empty() { "$" } else { aura };
    let aura_noun = term_to_noun(slab, aura);
    let bits = opt_to_noun(slab, value);
    let tag = term_to_noun(slab, "atom");
    T(slab, &[tag, aura_noun, bits])
}

#[cfg(test)]
fn ty_face(slab: &mut NounSlab, name: &str, inner: Noun) -> Noun {
    let name_noun = term_to_noun(slab, name);
    ty_face_tool(slab, name_noun, inner)
}

fn ty_face_tool(slab: &mut NounSlab, tool: Noun, inner: Noun) -> Noun {
    // Match hoon-138: face(name, void) → void
    if matches!(type_tag(inner, &slab.noun_space()).as_deref(), Ok("void")) {
        return ty_void(slab);
    }
    let tag = term_to_noun(slab, "face");
    T(slab, &[tag, tool, inner])
}

fn ty_hint(slab: &mut NounSlab, inner: Noun, note: Noun, payload: Noun) -> Noun {
    // Match hoon-138: hint(p, void) → void; hint(p, noun) → noun
    match type_tag(payload, &slab.noun_space()).as_deref() {
        Ok("void") => return ty_void(slab),
        Ok("noun") => return ty_noun(slab),
        _ => {}
    }
    let tag = term_to_noun(slab, "hint");
    let inner_note = T(slab, &[inner, note]);
    T(slab, &[tag, inner_note, payload])
}

fn ty_hold(slab: &mut NounSlab, inner: Noun, hoon: Noun) -> Noun {
    let tag = term_to_noun(slab, "hold");
    T(slab, &[tag, inner, hoon])
}

fn ty_core(slab: &mut NounSlab, payload: Noun, coil: Noun) -> Noun {
    // Match hoon-138: core(void, coil) → void
    if matches!(
        type_tag_kind(payload, &slab.noun_space()),
        Ok(TypeTagKind::Void)
    ) {
        return ty_void(slab);
    }
    let tag = term_to_noun(slab, "core");
    T(slab, &[tag, payload, coil])
}

fn ty_fork(slab: &mut NounSlab, options: Vec<Noun>) -> Noun {
    let mut set = D(0);
    for option in options {
        set = set_put_mug(slab, set, option).expect("ty_fork set_put_mug should succeed");
    }
    let tag = term_to_noun(slab, "fork");
    T(slab, &[tag, set])
}

fn ty_bool(slab: &mut NounSlab) -> Noun {
    let yes = ty_atom(slab, "f", Some(D(0)));
    let no = ty_atom(slab, "f", Some(D(1)));
    ty_fork(slab, vec![yes, no])
}

fn coil_from_parts(slab: &mut NounSlab, garb: Noun, context: Noun, rest: Noun) -> Noun {
    let tail = T(slab, &[context, rest]);
    T(slab, &[garb, tail])
}

// ---------------------------------------------------------------------------
// Paired type constructors.
//
// Each `ty_*_n` builds the same noun as its `ty_*` sibling, by calling it, and
// the matching interned native type from its children's native types, so no
// child is re-decoded. The face, hint, and core constructors read the result
// noun's tag, so a hoon-138 collapse yields a native Void or Noun exactly when
// the noun collapses. The atom, fork, and bool constructors lift the leaf noun
// with `native_of`. Nodes intern in the caller's `Context`. Only `ty_atom_n`
// and `ty_bool_n` are used outside tests; `native_ctor_tests` checks each pair
// with `assert_native_eq`.
// ---------------------------------------------------------------------------
use crate::native::ir::ty::TypeRef as NRc;

#[inline(always)]
fn native_type_id(ty: &NRc<NTy>) -> TypeId {
    ty.arena_id()
}

enum NativeForkOptions<'a> {
    Cached(&'a [NRc<NTy>]),
    Transient(SmallVec<[NRc<NTy>; 4]>),
}

impl std::ops::Deref for NativeForkOptions<'_> {
    type Target = [NRc<NTy>];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Cached(options) => options,
            Self::Transient(options) => options,
        }
    }
}

enum NativeForkOptionIter<'a> {
    Cached(std::slice::Iter<'a, NRc<NTy>>),
    Transient(smallvec::IntoIter<[NRc<NTy>; 4]>),
}

impl Iterator for NativeForkOptionIter<'_> {
    type Item = NRc<NTy>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Cached(options) => options.next().cloned(),
            Self::Transient(options) => options.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = match self {
            Self::Cached(options) => options.len(),
            Self::Transient(options) => options.len(),
        };
        (len, Some(len))
    }
}

impl ExactSizeIterator for NativeForkOptionIter<'_> {}

impl<'a> IntoIterator for NativeForkOptions<'a> {
    type Item = NRc<NTy>;
    type IntoIter = NativeForkOptionIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Cached(options) => NativeForkOptionIter::Cached(options.iter()),
            Self::Transient(options) => NativeForkOptionIter::Transient(options.into_iter()),
        }
    }
}

use crate::native::ir::intern::{
    cons_cell, cons_core, cons_face, cons_hint, cons_noun, cons_void,
    core_mint_cache_lookup as native_core_mint_cache_lookup,
    core_mint_cache_store as native_core_mint_cache_store,
    crop_cache_lookup as native_crop_cache_lookup, crop_cache_store as native_crop_cache_store,
    fish_cache_lookup as native_fish_cache_lookup, fish_cache_store as native_fish_cache_store,
    fuse_cache_lookup as native_fuse_cache_lookup, fuse_cache_store as native_fuse_cache_store,
    legset_memo_lookup, legset_memo_store, live_intern, live_leaf_from_noun, live_leaf_to_noun,
    live_to_noun, mint_cache_lookup as native_mint_cache_lookup,
    mint_cache_store as native_mint_cache_store, mull_cache_lookup as native_mull_cache_lookup,
    mull_cache_store as native_mull_cache_store, native_of, native_of_mug_candidates,
    native_of_mug_insert, nest_cache_lookup, nest_cache_store, Context,
};
use crate::native::ir::leaf::Leaf as NLeaf;
use crate::native::ir::ty::{garb_native, visit_fork_set_members, Garb as NGarb, Type as NTy};

#[cfg(test)]
fn ty_noun_n(cx: &mut Context, slab: &mut NounSlab) -> (Noun, NRc<NTy>) {
    (ty_noun(slab), live_intern(cx, NTy::Noun))
}

#[cfg(test)]
fn ty_void_n(cx: &mut Context, slab: &mut NounSlab) -> (Noun, NRc<NTy>) {
    (ty_void(slab), live_intern(cx, NTy::Void))
}

fn ty_atom_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    aura: &str,
    value: Option<Noun>,
) -> (Noun, NRc<NTy>) {
    let noun = ty_atom(slab, aura, value);
    let native = native_of(cx, noun, &slab.noun_space()).expect("ty_atom_n native");
    (noun, native)
}

#[cfg(test)]
fn ty_cell_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    head: (Noun, NRc<NTy>),
    tail: (Noun, NRc<NTy>),
) -> (Noun, NRc<NTy>) {
    let noun = ty_cell(slab, head.0, tail.0);
    let native = live_intern(cx, NTy::Cell(head.1, tail.1));
    (noun, native)
}

#[cfg(test)]
fn ty_face_tool_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    tool: Noun,
    inner: (Noun, NRc<NTy>),
) -> (Noun, NRc<NTy>) {
    let noun = ty_face_tool(slab, tool, inner.0);
    let kind = type_tag_kind(noun, &slab.noun_space());
    let native = match kind {
        Ok(TypeTagKind::Void) => live_intern(cx, NTy::Void),
        _ => {
            let tool_leaf = live_leaf_from_noun(cx, tool, &slab.noun_space());
            live_intern(
                cx,
                NTy::Face {
                    tool: tool_leaf,
                    inner: inner.1,
                },
            )
        }
    };
    (noun, native)
}

#[cfg(test)]
fn ty_face_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    name: &str,
    inner: (Noun, NRc<NTy>),
) -> (Noun, NRc<NTy>) {
    let name_noun = term_to_noun(slab, name);
    ty_face_tool_n(cx, slab, name_noun, inner)
}

#[cfg(test)]
fn ty_hint_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    inner: Noun,
    note: Noun,
    payload: (Noun, NRc<NTy>),
) -> (Noun, NRc<NTy>) {
    let noun = ty_hint(slab, inner, note, payload.0);
    let space = slab.noun_space();
    let native = match type_tag_kind(noun, &space) {
        Ok(TypeTagKind::Void) => live_intern(cx, NTy::Void),
        Ok(TypeTagKind::Noun) => live_intern(cx, NTy::Noun),
        _ => {
            // noun = [%hint [inner note] payload]; reuse the [inner note] cell
            // that ty_hint built.
            let head = noun
                .in_space(&space)
                .as_cell()
                .and_then(|c| c.tail().as_cell())
                .map(|c| c.head().noun())
                .expect("ty_hint_n: hint noun shape");
            let head_leaf = live_leaf_from_noun(cx, head, &space);
            live_intern(
                cx,
                NTy::Hint {
                    head: head_leaf,
                    payload: payload.1,
                },
            )
        }
    };
    (noun, native)
}

#[cfg(test)]
fn ty_hold_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    inner: (Noun, NRc<NTy>),
    hoon: Noun,
) -> (Noun, NRc<NTy>) {
    let noun = ty_hold(slab, inner.0, hoon);
    let gene = live_leaf_from_noun(cx, hoon, &slab.noun_space());
    let native = live_intern(
        cx,
        NTy::Hold {
            subject: inner.1,
            gene,
        },
    );
    (noun, native)
}

#[cfg(test)]
fn ty_core_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    payload: (Noun, NRc<NTy>),
    garb: Noun,
    context: (Noun, NRc<NTy>),
    rest: Noun,
) -> (Noun, NRc<NTy>) {
    let coil = coil_from_parts(slab, garb, context.0, rest);
    let noun = ty_core(slab, payload.0, coil);
    let kind = type_tag_kind(noun, &slab.noun_space());
    let native = match kind {
        Ok(TypeTagKind::Void) => live_intern(cx, NTy::Void),
        _ => {
            let garb_native = NGarb::from_noun(garb, &slab.noun_space()).expect("ty_core_n garb");
            let rest_leaf = live_leaf_from_noun(cx, rest, &slab.noun_space());
            live_intern(
                cx,
                NTy::Core {
                    payload: payload.1,
                    garb: garb_native,
                    context: context.1,
                    rest: rest_leaf,
                },
            )
        }
    };
    (noun, native)
}

#[cfg(test)]
fn ty_fork_n(cx: &mut Context, slab: &mut NounSlab, options: Vec<Noun>) -> (Noun, NRc<NTy>) {
    let noun = ty_fork(slab, options);
    let native = native_of(cx, noun, &slab.noun_space()).expect("ty_fork_n native");
    (noun, native)
}

fn ty_bool_n(cx: &mut Context, slab: &mut NounSlab) -> (Noun, NRc<NTy>) {
    let noun = ty_bool(slab);
    let native = native_of(cx, noun, &slab.noun_space()).expect("ty_bool_n native");
    (noun, native)
}

#[cfg(test)]
mod native_ctor_tests {
    use super::*;
    use crate::native::ir::intern::assert_native_eq;

    fn check(slab: &NounSlab, noun: Noun, native: &NRc<NTy>) {
        assert_native_eq(noun, native, &slab.noun_space());
    }

    #[test]
    fn native_ctors_byte_exact_all_tags_and_collapses() -> Result<()> {
        let mut cx = Context::new();
        let mut slab: NounSlab = NounSlab::new();

        // leaves
        let (n, t) = ty_noun_n(&mut cx, &mut slab);
        check(&slab, n, &t);
        let (n, t) = ty_void_n(&mut cx, &mut slab);
        check(&slab, n, &t);
        let (n, t) = ty_atom_n(&mut cx, &mut slab, "ud", None);
        check(&slab, n, &t);
        let (n, t) = ty_atom_n(&mut cx, &mut slab, "f", Some(D(0)));
        check(&slab, n, &t);
        // empty aura -> "$" (honk's ty_atom_local path); both value shapes
        let (n, t) = ty_atom_n(&mut cx, &mut slab, "", None);
        check(&slab, n, &t);
        let (n, t) = ty_atom_n(&mut cx, &mut slab, "", Some(D(42)));
        check(&slab, n, &t);

        // cell
        let h = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let tl = ty_noun_n(&mut cx, &mut slab);
        let (n, t) = ty_cell_n(&mut cx, &mut slab, h, tl);
        check(&slab, n, &t);

        // face non-collapse
        let inner = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let (n, t) = ty_face_n(&mut cx, &mut slab, "x", inner);
        check(&slab, n, &t);
        // face(_, void) -> void
        let vc = ty_void_n(&mut cx, &mut slab);
        let (n, t) = ty_face_n(&mut cx, &mut slab, "x", vc);
        check(&slab, n, &t);
        assert!(
            matches!(&*t, NTy::Void),
            "face(_,void) must collapse to Void"
        );

        // hint non-collapse
        let payload = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let note = term_to_noun(&mut slab, "fast");
        let (n, t) = ty_hint_n(&mut cx, &mut slab, D(0), note, payload);
        check(&slab, n, &t);
        // hint(_, void) -> void ; hint(_, noun) -> noun
        let pv = ty_void_n(&mut cx, &mut slab);
        let note2 = term_to_noun(&mut slab, "fast");
        let (n, t) = ty_hint_n(&mut cx, &mut slab, D(0), note2, pv);
        check(&slab, n, &t);
        assert!(matches!(&*t, NTy::Void));
        let pn = ty_noun_n(&mut cx, &mut slab);
        let note3 = term_to_noun(&mut slab, "fast");
        let (n, t) = ty_hint_n(&mut cx, &mut slab, D(0), note3, pn);
        check(&slab, n, &t);
        assert!(matches!(&*t, NTy::Noun));

        // hold
        let subj = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let hoon = T(&mut slab, &[D(1), D(0)]);
        let (n, t) = ty_hold_n(&mut cx, &mut slab, subj, hoon);
        check(&slab, n, &t);
        // hold with a cell gene (a non-direct leaf)
        let subj2 = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let g_inner = T(&mut slab, &[D(1), D(2)]);
        let gene2 = T(&mut slab, &[g_inner, D(3)]);
        let (n, t) = ty_hold_n(&mut cx, &mut slab, subj2, gene2);
        check(&slab, n, &t);

        // core non-collapse. `Garb::from_noun` needs a [nym poly vair] cell.
        let payload2 = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let ctx = ty_noun_n(&mut cx, &mut slab);
        let garb = {
            let poly = term_to_noun(&mut slab, "dry");
            let vair = term_to_noun(&mut slab, "gold");
            T(&mut slab, &[D(0), poly, vair])
        };
        let (n, t) = ty_core_n(&mut cx, &mut slab, payload2, garb, ctx, D(0));
        check(&slab, n, &t);
        // core(void, _) -> void
        let pv2 = ty_void_n(&mut cx, &mut slab);
        let ctx2 = ty_noun_n(&mut cx, &mut slab);
        let garb2 = {
            let poly = term_to_noun(&mut slab, "dry");
            let vair = term_to_noun(&mut slab, "gold");
            T(&mut slab, &[D(0), poly, vair])
        };
        let (n, t) = ty_core_n(&mut cx, &mut slab, pv2, garb2, ctx2, D(0));
        check(&slab, n, &t);
        assert!(matches!(&*t, NTy::Void));

        // fork + bool
        let o1 = ty_atom_n(&mut cx, &mut slab, "f", Some(D(0))).0;
        let o2 = ty_atom_n(&mut cx, &mut slab, "f", Some(D(1))).0;
        let (n, t) = ty_fork_n(&mut cx, &mut slab, vec![o1, o2]);
        check(&slab, n, &t);
        let (n, t) = ty_bool_n(&mut cx, &mut slab);
        check(&slab, n, &t);

        // cell_type_n: non-collapse + cell(void,_)->void + cell(_,void)->void
        let h = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let tl = ty_atom_n(&mut cx, &mut slab, "f", Some(D(0)));
        let (n, t) = cell_type_n(&mut cx, &mut slab, h, tl)?;
        check(&slab, n, &t);
        let hv = ty_void_n(&mut cx, &mut slab);
        let tl2 = ty_noun_n(&mut cx, &mut slab);
        let (n, t) = cell_type_n(&mut cx, &mut slab, hv, tl2)?;
        check(&slab, n, &t);
        assert!(
            matches!(&*t, NTy::Void),
            "cell(void,_) must collapse to Void"
        );
        let h2 = ty_noun_n(&mut cx, &mut slab);
        let tv = ty_void_n(&mut cx, &mut slab);
        let (n, t) = cell_type_n(&mut cx, &mut slab, h2, tv)?;
        check(&slab, n, &t);
        assert!(
            matches!(&*t, NTy::Void),
            "cell(_,void) must collapse to Void"
        );
        Ok(())
    }

    #[test]
    fn native_method_wrappers_byte_exact() -> Result<()> {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);

        // hint_type_n: non-collapse + void/noun collapse (returns payload native)
        let payload = ty_atom_n(&mut ut.cx, ut.slab, "ud", None);
        let note = term_to_noun(ut.slab, "fast");
        let (n, t) = ut.hint_type_n(D(0), note, payload)?;
        assert_native_eq(n, &t, &ut.slab.noun_space());
        let pv = ty_void_n(&mut ut.cx, ut.slab);
        let note2 = term_to_noun(ut.slab, "fast");
        let (n, t) = ut.hint_type_n(D(0), note2, pv)?;
        assert_native_eq(n, &t, &ut.slab.noun_space());
        assert!(matches!(&*t, NTy::Void));
        let pn = ty_noun_n(&mut ut.cx, ut.slab);
        let note3 = term_to_noun(ut.slab, "fast");
        let (n, t) = ut.hint_type_n(D(0), note3, pn)?;
        assert_native_eq(n, &t, &ut.slab.noun_space());
        assert!(matches!(&*t, NTy::Noun));

        // fork_from_options_n: multi-option + single-collapse + empty->void
        let o1 = ty_atom_n(&mut ut.cx, ut.slab, "f", Some(D(0))).0;
        let o2 = ty_atom_n(&mut ut.cx, ut.slab, "f", Some(D(1))).0;
        let (n, t) = ut.fork_from_options_n(vec![o1, o2])?;
        assert_native_eq(n, &t, &ut.slab.noun_space());
        let single = ty_atom_n(&mut ut.cx, ut.slab, "ud", None).0;
        let (n, t) = ut.fork_from_options_n(vec![single])?;
        assert_native_eq(n, &t, &ut.slab.noun_space());
        let (n, t) = ut.fork_from_options_n(vec![])?;
        assert_native_eq(n, &t, &ut.slab.noun_space());
        assert!(matches!(&*t, NTy::Void), "empty fork must collapse to Void");
        Ok(())
    }

    #[test]
    fn cons_fork_native_collapses_and_multi_member_parity() {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);
        let left_n = ty_atom_n(&mut ut.cx, ut.slab, "f", Some(D(0)));
        let right_n = ty_atom_n(&mut ut.cx, ut.slab, "f", Some(D(1)));

        let empty = ut.cons_fork(Vec::new()).unwrap();
        assert!(matches!(&*empty, NTy::Void));
        let singleton = ut.cons_fork(vec![left_n.1.clone()]).unwrap();
        assert!(NRc::ptr_eq(&singleton, &left_n.1));

        let fork = ut
            .cons_fork(vec![left_n.1.clone(), right_n.1.clone()])
            .unwrap();
        let NTy::Fork { options, .. } = &*fork else {
            panic!("two distinct members must produce a fork");
        };
        assert!(
            options.get().is_none(),
            "native fork edges must remain lazy until first traversal"
        );
        let transient = ut.fork_options_native(&fork).unwrap();
        assert_eq!(transient.len(), 2);
        let NTy::Fork { options, .. } = &*fork else {
            unreachable!()
        };
        assert!(
            options.get().is_none(),
            "a one-shot fork must not retain its child vector"
        );
        drop(transient);
        let materialized = ut.fork_options_native(&fork).unwrap();
        assert!(std::ptr::eq::<[NRc<NTy>]>(
            options.get().expect("fork edges populated").as_slice(),
            &*materialized
        ));
        let historical = ut.fork_from_options(vec![left_n.0, right_n.0]).unwrap();
        let emitted = live_to_noun(&mut ut.cx, &fork, ut.slab);
        assert!(noun_eq(historical, emitted, &ut.slab.noun_space()).unwrap());

        let singleton_fork = ut.cons_fork(vec![fork.clone()]).unwrap();
        assert!(NRc::ptr_eq(&singleton_fork, &fork));
    }

    // reachable_legs of a hold-over-cell DAG is the hold's leg-id, types with no
    // holds have none, and a scoped-fan intersection disjoint from the legset is
    // empty (the empty-fan key, 0). Also checks that legsets and subset IDs are
    // stable across calls.
    #[test]
    fn reachable_legs_and_scoped_fan_intersection() -> Result<()> {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);

        // A plain cell type has no holds.
        let a = ty_atom(ut.slab, "ud", None);
        let b = ty_atom(ut.slab, "t", None);
        let cell_noun = ty_cell(ut.slab, a, b);
        let cell = ut.native_of_cached(cell_noun)?;
        let legs = ut.reachable_legs(&cell)?;
        assert!(legs.is_empty(), "cell of atoms has no reachable legs");

        // Build [%hold cell gen] over the cell above; legset = {leg_id(hold)}.
        let gen = T(ut.slab, &[D(1), D(2)]); // a trivial gene noun
        let hold_noun = ty_hold(ut.slab, cell_noun, gen);
        let hold = ut.native_of_cached(hold_noun)?;
        let hold_legs = ut.reachable_legs(&hold)?;
        assert_eq!(
            hold_legs.len(),
            1,
            "single hold has exactly one reachable leg"
        );
        let leg_id = ut.hold_repo_fan_leg_id_for_hold_native(&hold)?;
        assert_eq!(hold_legs[0], leg_id, "legset is the hold's own leg-id");

        // Wrap the hold in a cell; legset is still {leg_id}.
        let other = ty_noun(ut.slab);
        let outer_noun = ty_cell(ut.slab, hold_noun, other);
        let outer = ut.native_of_cached(outer_noun)?;
        let outer_legs = ut.reachable_legs(&outer)?;
        assert_eq!(
            outer_legs.as_ref(),
            &[leg_id],
            "cell-wrapped hold keeps the leg"
        );

        // Memoized: second call returns the same Rc-shared slice content.
        let outer_legs2 = ut.reachable_legs(&outer)?;
        assert_eq!(outer_legs.as_ref(), outer_legs2.as_ref());

        // intersect-empty -> intern_fan_subset_id returns the empty sentinel 0.
        assert_eq!(ut.intern_fan_subset_id(&[]), FanContextId(0));
        let inter = Ut::intersect_sorted_legs(
            &[FanLegId(leg_id.0 + 7), FanLegId(leg_id.0 + 9)],
            &outer_legs,
        );
        assert!(
            inter.is_empty(),
            "disjoint active set -> empty intersection"
        );

        // intersect-nonempty -> a stable nonzero id, deterministic across calls.
        let inter2 = Ut::intersect_sorted_legs(&[leg_id], &outer_legs);
        assert_eq!(inter2, vec![leg_id]);
        let id1 = ut.intern_fan_subset_id(&inter2);
        let id2 = ut.intern_fan_subset_id(&inter2);
        assert_ne!(id1, FanContextId(0));
        assert_eq!(id1, id2, "equal subsets map to one stable id");

        // merge/intersect helper sanity.
        assert_eq!(
            Ut::merge_sorted_legs(
                &[FanLegId(1), FanLegId(3)],
                &[FanLegId(2), FanLegId(3), FanLegId(4)]
            ),
            vec![FanLegId(1), FanLegId(2), FanLegId(3), FanLegId(4)]
        );
        assert_eq!(
            Ut::intersect_sorted_legs(
                &[FanLegId(1), FanLegId(3), FanLegId(5)],
                &[FanLegId(3), FanLegId(5), FanLegId(7)]
            ),
            vec![FanLegId(3), FanLegId(5)]
        );
        Ok(())
    }
}

fn rest_tomes(rest: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = rest
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("rest not cell: {err}")))?;
    Ok(cell.tail().noun())
}

fn expand_wutpat(wing: &WingType, q: &Hoon, r: &Hoon) -> Hoon {
    let spec = Spec::Base(BaseType::Atom("$".to_string()));
    let test = Hoon::WutTis(Box::new(spec), wing.clone());
    Hoon::WutCol(Box::new(test), Box::new(q.clone()), Box::new(r.clone()))
}

fn expand_wutsig(wing: &WingType, q: &Hoon, r: &Hoon) -> Hoon {
    let spec = Spec::Base(BaseType::Null);
    let test = Hoon::WutTis(Box::new(spec), wing.clone());
    Hoon::WutCol(Box::new(test), Box::new(q.clone()), Box::new(r.clone()))
}

fn expand_wuthep(wing: &WingType, list: &[(Spec, Hoon)]) -> Hoon {
    let Some(((spec, branch), tail)) = list.split_first() else {
        // Canonical hoon-138 lowering:
        //   ?~ q.gen  [%lost [%wing p.gen]]
        return Hoon::Lost(Box::new(Hoon::Wing(wing.clone())));
    };
    let test = Hoon::WutTis(Box::new(spec.clone()), wing.clone());
    let fallback = expand_wuthep(wing, tail);
    Hoon::WutCol(Box::new(test), Box::new(branch.clone()), Box::new(fallback))
}

fn expand_wutlus(wing: &WingType, default: &Hoon, list: &[(Spec, Hoon)]) -> Hoon {
    let mut options: Vec<(Spec, Hoon)> = list.to_vec();
    options.push((Spec::Base(BaseType::NounExpr), default.clone()));
    expand_wuthep(wing, &options)
}

fn spec_example(spec: &Spec) -> Hoon {
    let hay = Vec::new();
    let cox: HashMap<String, Spec> = HashMap::new();
    let bug = Vec::new();
    let nut = None;
    let def = None;
    example(spec, 1u64.into(), &hay, &cox, &bug, &nut, &def)
}

fn term_or_tune_to_noun(slab: &mut NounSlab, tot: &TermOrTune) -> Result<Noun> {
    match tot {
        TermOrTune::Term(name) => Ok(term_to_noun(slab, name)),
        TermOrTune::Tune(tune) => tune_to_noun(slab, tune),
    }
}

fn tune_to_noun(slab: &mut NounSlab, tune: &Tune) -> Result<Noun> {
    let (map, vec) = tune;
    let map_pairs: Vec<_> = map
        .iter()
        .map(|(key, opt_val)| {
            let key_noun = term_to_noun(slab, key);
            let val_noun = match opt_val {
                None => D(0),
                Some(hoon) => {
                    let hoon_noun = hoon_to_noun(slab, hoon);
                    T(slab, &[D(0), hoon_noun])
                }
            };
            (key_noun, val_noun)
        })
        .collect();

    let map_noun = map_to_noun(slab, map_pairs)?;
    let vec_nouns: Vec<_> = vec.iter().map(|hoon| hoon_to_noun(slab, hoon)).collect();
    let vec_noun = vec_to_list(slab, vec_nouns);
    Ok(T(slab, &[map_noun, vec_noun]))
}

fn tagged1(slab: &mut NounSlab, tag: &str, a: Noun) -> Noun {
    let tag_noun = term_to_noun(slab, tag);
    T(slab, &[tag_noun, a])
}

fn tagged2(slab: &mut NounSlab, tag: &str, a: Noun, b: Noun) -> Noun {
    let tag_noun = term_to_noun(slab, tag);
    T(slab, &[tag_noun, a, b])
}

fn type_to_noun(slab: &mut NounSlab, typ: &Type) -> Result<Noun> {
    use Type::*;
    match typ {
        NounExpr => Ok(term_to_noun(slab, "noun")),
        Void => Ok(term_to_noun(slab, "void")),
        ParsedAtom(au, bits) => {
            let au_noun = term_to_noun(slab, au);
            let bits_noun = opt_to_noun(slab, bits.map(D));
            Ok(tagged2(slab, "atom", au_noun, bits_noun))
        }
        Cell(l, r) => {
            let l = type_to_noun(slab, l)?;
            let r = type_to_noun(slab, r)?;
            Ok(tagged2(slab, "cell", l, r))
        }
        Core(face, coil) => {
            let face_noun = type_to_noun(slab, face)?;
            let coil_noun = coil_to_noun(slab, coil)?;
            Ok(tagged2(slab, "core", face_noun, coil_noun))
        }
        Face(face_type, inner) => {
            let face_noun = face_type_to_noun(slab, face_type)?;
            let inner_noun = type_to_noun(slab, inner)?;
            Ok(tagged2(slab, "face", face_noun, inner_noun))
        }
        Fork(types) => {
            let types_vec: Vec<_> = types
                .iter()
                .map(|t| type_to_noun(slab, t))
                .collect::<Result<_>>()?;
            let types_noun = vec_to_list(slab, types_vec);
            Ok(tagged1(slab, "fork", types_noun))
        }
        Hint((inner, note), payload) => {
            let inner_noun = type_to_noun(slab, inner)?;
            let note_noun = note_to_noun(slab, note)?;
            let payload_noun = type_to_noun(slab, payload)?;
            let hint_inner = T(slab, &[inner_noun, note_noun]);
            Ok(tagged2(slab, "hint", hint_inner, payload_noun))
        }
        Hold(typ, hoon) => {
            let typ_noun = type_to_noun(slab, typ)?;
            let hoon_noun = hoon_to_noun(slab, hoon);
            Ok(tagged2(slab, "hold", typ_noun, hoon_noun))
        }
    }
}

fn face_type_to_noun(slab: &mut NounSlab, face_type: &FaceType) -> Result<Noun> {
    match face_type {
        FaceType::Term(name) => Ok(term_to_noun(slab, name)),
        FaceType::Tune(tune) => {
            let tune_noun = tune_to_noun(slab, tune)?;
            Ok(tagged1(slab, "tune", tune_noun))
        }
    }
}

fn note_to_noun(slab: &mut NounSlab, note: &Note) -> Result<Noun> {
    match note {
        Note::Help(help) => {
            let help_noun = noun_expr_to_noun(slab, help);
            Ok(tagged1(slab, "help", help_noun))
        }
        Note::Know(name) => {
            let name_noun = term_to_noun(slab, name);
            Ok(tagged1(slab, "know", name_noun))
        }
        Note::Made(name, opt_wings) => {
            let name_noun = term_to_noun(slab, name);
            let wings_noun = opt_wings
                .as_ref()
                .map(|wings| {
                    let wing_nouns: Vec<_> = wings
                        .iter()
                        .map(|wing| wing_to_noun(slab, wing))
                        .collect::<Result<_>>()?;
                    Ok::<Noun, CompilerError>(vec_to_list(slab, wing_nouns))
                })
                .transpose()?;
            let wings_noun = match wings_noun {
                None => D(0),
                Some(noun) => T(slab, &[D(0), noun]),
            };
            Ok(tagged2(slab, "made", name_noun, wings_noun))
        }
    }
}

fn wing_to_noun(slab: &mut NounSlab, wing: &WingType) -> Result<Noun> {
    let limbs: Vec<Noun> = wing
        .iter()
        .map(|l| limb_to_noun(slab, l))
        .collect::<Result<_>>()?;
    Ok(vec_to_list(slab, limbs))
}

fn limb_to_noun(slab: &mut NounSlab, limb: &Limb) -> Result<Noun> {
    match limb {
        Limb::Term(name) => Ok(term_to_noun(slab, name)),
        Limb::Axis(axis) => Ok(slot_formula_axis_big(slab, axis.as_biguint().clone())),
        Limb::Parent(skip, opt) => {
            let opt_noun = match opt {
                None => D(0),
                Some(name) => {
                    let name_noun = term_to_noun(slab, name);
                    T(slab, &[D(0), name_noun])
                }
            };
            let skip_noun = noun_u64(slab, *skip);
            Ok(T(slab, &[D(1), skip_noun, opt_noun]))
        }
    }
}

fn coil_to_noun(slab: &mut NounSlab, coil: &Coil) -> Result<Noun> {
    let garb_noun = garb_to_noun(slab, &coil.p)?;
    let type_noun = type_to_noun(slab, &coil.q)?;
    let semi_noun = semi_noun_expr_to_noun(slab, &coil.r.0)?;

    let tomes_entries: Vec<_> = coil
        .r
        .1
        .iter()
        .map(|(k, v)| {
            let (what, inner_map) = v;
            let k_noun = term_to_noun(slab, k);
            let what_noun = what
                .as_ref()
                .map(|what| noun_expr_to_noun(slab, what))
                .unwrap_or_else(|| D(0));
            let inner_entries: Vec<_> = inner_map
                .iter()
                .map(|(kk, vv)| Ok((term_to_noun(slab, kk), hoon_to_noun(slab, vv))))
                .collect::<Result<_>>()?;
            let v_noun = map_to_noun(slab, inner_entries)?;
            Ok((k_noun, T(slab, &[what_noun, v_noun])))
        })
        .collect::<Result<_>>()?;

    let tomes_noun = map_to_noun(slab, tomes_entries)?;
    Ok(T(slab, &[garb_noun, type_noun, semi_noun, tomes_noun]))
}

fn garb_to_noun(slab: &mut NounSlab, garb: &Garb) -> Result<Noun> {
    let name_noun = match garb.name.as_ref() {
        None => D(0),
        Some(name) => {
            let name_noun = term_to_noun(slab, name);
            T(slab, &[D(0), name_noun])
        }
    };
    let poly_noun = poly_to_noun(slab, &garb.poly);
    let vair_noun = vair_to_noun(slab, &garb.vair);
    Ok(T(slab, &[name_noun, poly_noun, vair_noun]))
}

fn poly_to_noun(slab: &mut NounSlab, poly: &AstPoly) -> Noun {
    match poly {
        AstPoly::Wet => term_to_noun(slab, "wet"),
        AstPoly::Dry => term_to_noun(slab, "dry"),
    }
}

fn vair_to_noun(slab: &mut NounSlab, vair: &AstVair) -> Noun {
    match vair {
        AstVair::Gold => term_to_noun(slab, "gold"),
        AstVair::Iron => term_to_noun(slab, "iron"),
        AstVair::Lead => term_to_noun(slab, "lead"),
        AstVair::Zinc => term_to_noun(slab, "zinc"),
    }
}

fn semi_noun_expr_to_noun(slab: &mut NounSlab, (stencil, expr): &SemiNounExpr) -> Result<Noun> {
    let stencil_noun = stencil_to_noun(slab, stencil)?;
    let expr_noun = noun_expr_to_noun(slab, expr);
    Ok(T(slab, &[stencil_noun, expr_noun]))
}

fn stencil_to_noun(slab: &mut NounSlab, stencil: &Stencil) -> Result<Noun> {
    match stencil {
        Stencil::Half { left, rite } => {
            let left = stencil_to_noun(slab, left)?;
            let right = stencil_to_noun(slab, rite)?;
            Ok(tagged2(slab, "half", left, right))
        }
        Stencil::Full { blocks } => {
            let blocks_vec: Vec<_> = blocks
                .iter()
                .map(|block| block_to_noun(slab, block))
                .collect::<Result<_>>()?;
            let blocks_noun = vec_to_list(slab, blocks_vec);
            Ok(tagged1(slab, "full", blocks_noun))
        }
        Stencil::Lazy { fragment, resolve } => {
            let gate_noun = gate_to_noun(slab, resolve)?;
            let fragment_noun = noun_biguint(slab, fragment.as_biguint().clone());
            Ok(tagged2(slab, "lazy", fragment_noun, gate_noun))
        }
    }
}

fn block_to_noun(slab: &mut NounSlab, block: &Block) -> Result<Noun> {
    let paths: Vec<_> = block
        .iter()
        .map(|path| path_to_noun(slab, path))
        .collect::<Result<_>>()?;
    Ok(vec_to_list(slab, paths))
}

fn path_to_noun(slab: &mut NounSlab, path: &Path) -> Result<Noun> {
    let knots: Vec<_> = path
        .iter()
        .map(|knot| knot_to_noun(slab, knot))
        .collect::<Result<_>>()?;
    Ok(vec_to_list(slab, knots))
}

fn knot_to_noun(slab: &mut NounSlab, knot: &Knot) -> Result<Noun> {
    Ok(cord_to_noun(slab, knot))
}

fn cord_to_noun(slab: &mut NounSlab, cord: &Cord) -> Noun {
    let atom = string_to_atom(cord.to_string());
    parsed_atom_to_noun(slab, &atom)
}

fn gate_to_noun(slab: &mut NounSlab, gate: &Gate) -> Result<Noun> {
    let (spec, body) = gate;
    let spec_noun = spec_to_noun(slab, spec)?;
    let body_noun = spec_to_noun(slab, body)?;
    Ok(T(slab, &[spec_noun, body_noun]))
}

fn spec_to_noun(slab: &mut NounSlab, spec: &Spec) -> Result<Noun> {
    use Spec::*;
    Ok(match spec {
        Base(bt) => {
            let bt_noun = basetype_to_noun(slab, bt);
            tagged1(slab, "base", bt_noun)
        }
        Dbug(spot, s) => {
            let spot_noun = spot_to_noun(slab, spot)?;
            let s_noun = spec_to_noun(slab, s)?;
            tagged2(slab, "dbug", spot_noun, s_noun)
        }
        Gist(help, s) => {
            let help_noun = noun_expr_to_noun(slab, help);
            let help = tagged1(slab, "help", help_noun);
            let s_noun = spec_to_noun(slab, s)?;
            tagged2(slab, "gist", help, s_noun)
        }
        Leaf(tag, atom) => {
            let tag_noun = term_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            tagged2(slab, "leaf", tag_noun, atom_noun)
        }
        Like(wing, wings) => {
            let wing_noun = wing_to_noun(slab, wing)?;
            let wings_vec: Vec<_> = wings
                .iter()
                .map(|w| wing_to_noun(slab, w))
                .collect::<Result<_>>()?;
            let wings_noun = vec_to_list(slab, wings_vec);
            tagged2(slab, "like", wing_noun, wings_noun)
        }
        Loop(name) => {
            let name_noun = term_to_noun(slab, name);
            tagged1(slab, "loop", name_noun)
        }
        Made((name, args), s) => {
            let name_noun = term_to_noun(slab, name);
            let args_vec: Vec<_> = args.iter().map(|a| term_to_noun(slab, a)).collect();
            let args_noun = vec_to_list(slab, args_vec);
            let s_noun = spec_to_noun(slab, s)?;
            let inner = T(slab, &[name_noun, args_noun]);
            tagged2(slab, "made", inner, s_noun)
        }
        Make(hoon, specs) => {
            let hoon_noun = hoon_to_noun(slab, hoon);
            let specs_vec: Vec<_> = specs
                .iter()
                .map(|s| spec_to_noun(slab, s))
                .collect::<Result<_>>()?;
            let specs_noun = vec_to_list(slab, specs_vec);
            tagged2(slab, "make", hoon_noun, specs_noun)
        }
        Name(name, s) => {
            let name_noun = term_to_noun(slab, name);
            let s_noun = spec_to_noun(slab, s)?;
            tagged2(slab, "name", name_noun, s_noun)
        }
        Over(wing, s) => {
            let wing_noun = wing_to_noun(slab, wing)?;
            let s_noun = spec_to_noun(slab, s)?;
            tagged2(slab, "over", wing_noun, s_noun)
        }
        BucGar(a, b) => {
            let a_noun = spec_to_noun(slab, a)?;
            let b_noun = spec_to_noun(slab, b)?;
            tagged2(slab, "bcgr", a_noun, b_noun)
        }
        BucBuc(a, map) => {
            let a_noun = spec_to_noun(slab, a)?;
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| Ok((term_to_noun(slab, k), spec_to_noun(slab, v)?)))
                .collect::<Result<_>>()?;
            let map_noun = map_to_noun(slab, entries)?;
            tagged2(slab, "bcbc", a_noun, map_noun)
        }
        BucBar(a, h) => {
            let a_noun = spec_to_noun(slab, a)?;
            let h_noun = hoon_to_noun(slab, h);
            tagged2(slab, "bcbr", a_noun, h_noun)
        }
        BucCab(h) => {
            let h_noun = hoon_to_noun(slab, h);
            tagged1(slab, "bccb", h_noun)
        }
        BucCol(a, specs) => {
            let a_noun = spec_to_noun(slab, a)?;
            let specs_vec: Vec<_> = specs
                .iter()
                .map(|s| spec_to_noun(slab, s))
                .collect::<Result<_>>()?;
            let specs_noun = vec_to_list(slab, specs_vec);
            tagged2(slab, "bccl", a_noun, specs_noun)
        }
        BucCen(a, specs) => {
            let a_noun = spec_to_noun(slab, a)?;
            let specs_vec: Vec<_> = specs
                .iter()
                .map(|s| spec_to_noun(slab, s))
                .collect::<Result<_>>()?;
            let specs_noun = vec_to_list(slab, specs_vec);
            tagged2(slab, "bccn", a_noun, specs_noun)
        }
        BucDot(a, map) => {
            let a_noun = spec_to_noun(slab, a)?;
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| Ok((term_to_noun(slab, k), spec_to_noun(slab, v)?)))
                .collect::<Result<_>>()?;
            let map_noun = map_to_noun(slab, entries)?;
            tagged2(slab, "bcdt", a_noun, map_noun)
        }
        BucGal(a, b) => {
            let a_noun = spec_to_noun(slab, a)?;
            let b_noun = spec_to_noun(slab, b)?;
            tagged2(slab, "bcgl", a_noun, b_noun)
        }
        BucHep(a, b) => {
            let a_noun = spec_to_noun(slab, a)?;
            let b_noun = spec_to_noun(slab, b)?;
            tagged2(slab, "bchp", a_noun, b_noun)
        }
        BucKet(a, b) => {
            let a_noun = spec_to_noun(slab, a)?;
            let b_noun = spec_to_noun(slab, b)?;
            tagged2(slab, "bckt", a_noun, b_noun)
        }
        BucLus(tag, s) => {
            let tag_noun = term_to_noun(slab, tag);
            let s_noun = spec_to_noun(slab, s)?;
            tagged2(slab, "bcls", tag_noun, s_noun)
        }
        BucFas(a, map) => {
            let a_noun = spec_to_noun(slab, a)?;
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| Ok((term_to_noun(slab, k), spec_to_noun(slab, v)?)))
                .collect::<Result<_>>()?;
            let map_noun = map_to_noun(slab, entries)?;
            tagged2(slab, "bcfs", a_noun, map_noun)
        }
        BucMic(h) => {
            let inner = hoon_to_noun(slab, h);
            tagged1(slab, "bcmc", inner)
        }
        BucPam(a, h) => {
            let a_noun = spec_to_noun(slab, a)?;
            let h_noun = hoon_to_noun(slab, h);
            tagged2(slab, "bcpm", a_noun, h_noun)
        }
        BucSig(h, a) => {
            let h_noun = hoon_to_noun(slab, h);
            let a_noun = spec_to_noun(slab, a)?;
            tagged2(slab, "bcsg", h_noun, a_noun)
        }
        BucTic(a, map) => {
            let a_noun = spec_to_noun(slab, a)?;
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| Ok((term_to_noun(slab, k), spec_to_noun(slab, v)?)))
                .collect::<Result<_>>()?;
            let map_noun = map_to_noun(slab, entries)?;
            tagged2(slab, "bctc", a_noun, map_noun)
        }
        BucTis(skin, a) => {
            let skin_noun = skin_to_noun(slab, skin)?;
            let a_noun = spec_to_noun(slab, a)?;
            tagged2(slab, "bcts", skin_noun, a_noun)
        }
        BucPat(a, b) => {
            let a_noun = spec_to_noun(slab, a)?;
            let b_noun = spec_to_noun(slab, b)?;
            tagged2(slab, "bcpt", a_noun, b_noun)
        }
        BucWut(a, specs) => {
            let a_noun = spec_to_noun(slab, a)?;
            let specs_vec: Vec<_> = specs
                .iter()
                .map(|s| spec_to_noun(slab, s))
                .collect::<Result<_>>()?;
            let specs_noun = vec_to_list(slab, specs_vec);
            tagged2(slab, "bcwt", a_noun, specs_noun)
        }
        BucZap(a, map) => {
            let a_noun = spec_to_noun(slab, a)?;
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| Ok((term_to_noun(slab, k), spec_to_noun(slab, v)?)))
                .collect::<Result<_>>()?;
            let map_noun = map_to_noun(slab, entries)?;
            tagged2(slab, "bczp", a_noun, map_noun)
        }
    })
}

fn basetype_to_noun(slab: &mut NounSlab, bt: &BaseType) -> Noun {
    match bt {
        BaseType::NounExpr => term_to_noun(slab, "noun"),
        BaseType::Cell => term_to_noun(slab, "cell"),
        BaseType::Flag => term_to_noun(slab, "flag"),
        BaseType::Null => term_to_noun(slab, "null"),
        BaseType::Void => term_to_noun(slab, "void"),
        BaseType::Atom(au) => {
            let at = term_to_noun(slab, au);
            tagged1(slab, "atom", at)
        }
    }
}

fn skin_to_noun(slab: &mut NounSlab, skin: &Skin) -> Result<Noun> {
    use Skin::*;
    Ok(match skin {
        Term(s) => term_to_noun(slab, s),
        Base(bt) => {
            let inner = basetype_to_noun(slab, bt);
            tagged1(slab, "base", inner)
        }
        Cell(l, r) => {
            let l = skin_to_noun(slab, l)?;
            let r = skin_to_noun(slab, r)?;
            tagged2(slab, "cell", l, r)
        }
        Dbug(spot, s) => {
            let spot_noun = spot_to_noun(slab, spot)?;
            let s_noun = skin_to_noun(slab, s)?;
            tagged2(slab, "dbug", spot_noun, s_noun)
        }
        Help(help, s) => {
            let help_noun = noun_expr_to_noun(slab, help);
            let s_noun = skin_to_noun(slab, s)?;
            tagged2(slab, "help", help_noun, s_noun)
        }
        Leaf(tag, atom) => {
            let tag_noun = term_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            tagged2(slab, "leaf", tag_noun, atom_noun)
        }
        Name(name, s) => {
            let name_noun = term_to_noun(slab, name);
            let s_noun = skin_to_noun(slab, s)?;
            tagged2(slab, "name", name_noun, s_noun)
        }
        Over(wing, s) => {
            let wing_noun = wing_to_noun(slab, wing)?;
            let s_noun = skin_to_noun(slab, s)?;
            tagged2(slab, "over", wing_noun, s_noun)
        }
        Spec(spec, s) => {
            let spec_noun = spec_to_noun(slab, spec)?;
            let s_noun = skin_to_noun(slab, s)?;
            tagged2(slab, "spec", spec_noun, s_noun)
        }
        Wash(n) => {
            let n_noun = noun_u64(slab, *n);
            tagged1(slab, "wash", n_noun)
        }
    })
}

fn spot_to_noun(slab: &mut NounSlab, spot: &Spot) -> Result<Noun> {
    let path_noun = path_to_noun(slab, &spot.p)?;
    let pint_noun = pint_to_noun(slab, &spot.q)?;
    Ok(T(slab, &[path_noun, pint_noun]))
}

fn pint_to_noun(slab: &mut NounSlab, pint: &Pint) -> Result<Noun> {
    let p0 = noun_u64(slab, pint.p.0);
    let p1 = noun_u64(slab, pint.p.1);
    let q0 = noun_u64(slab, pint.q.0);
    let q1 = noun_u64(slab, pint.q.1);
    let p = T(slab, &[p0, p1]);
    let q = T(slab, &[q0, q1]);
    Ok(T(slab, &[p, q]))
}

fn atom_to_noun(slab: &mut NounSlab, atom: &ParsedAtom) -> Noun {
    parsed_atom_to_noun(slab, atom)
}

fn nock_to_noun(slab: &mut NounSlab, nock: &Nock) -> Noun {
    use Nock::*;
    match nock {
        Pair(a, b) => {
            let a_noun = nock_to_noun(slab, a);
            let b_noun = nock_to_noun(slab, b);
            T(slab, &[D(2), a_noun, b_noun])
        }
        Const(expr) => {
            let expr_noun = noun_expr_to_noun(slab, expr);
            T(slab, &[D(1), expr_noun])
        }
        Compose(f, g) => {
            let f_noun = nock_to_noun(slab, f);
            let g_noun = nock_to_noun(slab, g);
            T(slab, &[D(7), f_noun, g_noun])
        }
        CellTest(n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(3), n_noun])
        }
        Increment(n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(4), n_noun])
        }
        Equality(a, b) => {
            let a_noun = nock_to_noun(slab, a);
            let b_noun = nock_to_noun(slab, b);
            T(slab, &[D(5), a_noun, b_noun])
        }
        IfThenElse(cond, yes, no) => {
            let cond_noun = nock_to_noun(slab, cond);
            let yes_noun = nock_to_noun(slab, yes);
            let no_noun = nock_to_noun(slab, no);
            T(slab, &[D(6), cond_noun, yes_noun, no_noun])
        }
        SerialCompose(f, g) => {
            let f_noun = nock_to_noun(slab, f);
            let g_noun = nock_to_noun(slab, g);
            T(slab, &[D(8), f_noun, g_noun])
        }
        PushSubject(n, subj) => {
            let n_noun = nock_to_noun(slab, n);
            let subj_noun = nock_to_noun(slab, subj);
            T(slab, &[D(9), n_noun, subj_noun])
        }
        SelectArm(axis, core) => {
            let core_noun = nock_to_noun(slab, core);
            let axis_noun = noun_biguint(slab, axis.as_biguint().clone());
            T(slab, &[D(10), axis_noun, core_noun])
        }
        Edit((axis, new), core) => {
            let new_noun = nock_to_noun(slab, new);
            let core_noun = nock_to_noun(slab, core);
            let axis_noun = noun_biguint(slab, axis.as_biguint().clone());
            let axis_cell = T(slab, &[axis_noun, new_noun]);
            T(slab, &[D(11), axis_cell, core_noun])
        }
        Hint(hint, n) => {
            let hint_noun = nock_hint_to_noun(slab, hint);
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(12), hint_noun, n_noun])
        }
        GrabData(core, path) => {
            let core_noun = nock_to_noun(slab, core);
            let path_noun = nock_to_noun(slab, path);
            T(slab, &[D(13), core_noun, path_noun])
        }
        AxisSelect(axis) => noun_biguint(slab, axis.as_biguint().clone()),
    }
}

fn nock_hint_to_noun(slab: &mut NounSlab, hint: &NockHint) -> Noun {
    match hint {
        NockHint::ParsedAtom(a) => noun_u64(slab, *a),
        NockHint::Pair(tag, n) => {
            let n_noun = nock_to_noun(slab, n);
            let tag_noun = noun_u64(slab, *tag);
            T(slab, &[tag_noun, n_noun])
        }
    }
}

#[cfg(test)]
fn is_const_bool_formula(formula: Noun, value: bool, space: &NounSpace) -> bool {
    let Ok((head, tail)) = noun_pair(formula, space) else {
        return false;
    };
    let expected = if value { 0 } else { 1 };
    noun_eq_direct(head, 1, space) && noun_eq_direct(tail, expected, space)
}

#[cfg(test)]
fn cell_type(slab: &mut NounSlab, head: Noun, tail: Noun) -> Result<Noun> {
    if crate::native::ir::intern::live_enabled() {
        let space = slab.noun_space();
        let mut cx = Context::new();
        let hn = native_of(&mut cx, head, &space)?;
        let tn = native_of(&mut cx, tail, &space)?;
        let (noun, native) = cell_type_n(&mut cx, slab, (head, hn), (tail, tn))?;
        crate::native::ir::intern::assert_native_eq(noun, &native, &slab.noun_space());
        return Ok(noun);
    }
    let space = slab.noun_space();
    if type_tag(head, &space)? == "void" || type_tag(tail, &space)? == "void" {
        return Ok(ty_void(slab));
    }
    Ok(ty_cell(slab, head, tail))
}

/// Paired `cell_type`: collapses cell(void,_) and cell(_,void) to void.
#[cfg(test)]
fn cell_type_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    head: (Noun, NRc<NTy>),
    tail: (Noun, NRc<NTy>),
) -> Result<(Noun, NRc<NTy>)> {
    let space = slab.noun_space();
    if type_tag(head.0, &space)? == "void" {
        return Ok(ty_void_n(cx, slab));
    }
    if type_tag(tail.0, &space)? == "void" {
        return Ok(ty_void_n(cx, slab));
    }
    Ok(ty_cell_n(cx, slab, head, tail))
}
