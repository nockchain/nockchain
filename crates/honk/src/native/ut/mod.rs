use std::cmp;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hasher;
use std::sync::Arc;

use hatch::ast::hoon::{
    Alas, BaseType, Beer, Block, Chum, Coil, Cord, FaceType, Garb, Gate, Hoon, Knot, Limb, Mane,
    Manx, Marl, Mart, Marx, Nock, NockHint, Note, NounExpr, ParsedAtom, Path, Pint,
    Poly as AstPoly, SemiNounExpr, Skin, Spec, Spot, Stencil, TermOrPair, TermOrTune, Tome, Tuna,
    TunaTail, Tune, Type, Tyre, Vair as AstVair, WingType, Woof, ZpwtArg,
};
use hatch::utils::{
    chum_to_nounexpr, example, factory, grip, hoon_to_noun, noun_to_hoon, open, peg, reek,
    string_to_atom,
};
use nockapp::noun::slab::NounSlab;
use nockapp::noun::NounAllocatorExt;
use nockapp::utils::{create_context, NOCK_STACK_SIZE_MEDIUM};
use nockvm::ext::{AtomExt, NounExt};
use nockvm::interpreter::{interpret, Context as NockContext};
use nockvm::jets::cold::{Cold, Nounable};
use nockvm::jets::math::util::lth_b;
use nockvm::jets::warm::Warm;
use nockvm::jets::JetDispatchMode;
use nockvm::mem::NockStack;
use nockvm::mug::{get_mug, set_mug};
use nockvm::noun::{Atom, AtomHandle, Noun, NounAllocator, NounSpace, D, T};
use num_bigint::BigUint;

use crate::errors::{
    CompilerError, CompilerErrorLocation, CompilerErrorMetadata, CompilerResolutionFact,
    CompilerSemanticFact, Result,
};
use crate::native::formula::{comb, cond, cons};
use crate::native::hot::native_hot_state;
use crate::native::ir::leaf::Leaf;
use crate::native::noun::{
    atom_to_string, noun_eq_direct, noun_expr_to_noun, noun_pair, opt_from_noun, opt_to_noun,
    parsed_atom_to_noun, tag, term_to_noun, vec_to_list,
};

mod find;
mod fire;
mod repo;
#[cfg(test)]
pub mod test;
pub mod types;
mod wet;
pub use types::*;

#[derive(Clone, Copy)]
struct HoldRepoFanLegIdEntry {
    id: u64,
    inner: Noun,
    hoon: Noun,
}

#[derive(Clone, Copy)]
struct HoldRepoFanHoldIdEntry {
    hold: Noun,
    id: u64,
}

#[derive(Clone, Copy)]
struct MuskMackCacheEntry {
    core: Noun,
    result: Option<Noun>,
}

/// Seminoun mask tags, decoded by direct atom comparison: the
/// `atom_to_string` route allocated a `String` per inspection on musk's
/// hottest path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SemiTag {
    Full,
    Half,
    Lazy,
    Other,
}

const SEMI_TAG_FULL: u64 = 1_819_047_270; // %full
const SEMI_TAG_HALF: u64 = 1_718_378_856; // %half
const SEMI_TAG_LAZY: u64 = 2_038_063_468; // %lazy

#[derive(Clone, Copy)]
struct KtsgFoldCacheEntry {
    bran: Noun,
    formula: Noun,
    result: Option<Noun>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SemanticContextKey {
    vet_key: u8,
    fan_context_key: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MemoContextKey {
    arm_epoch_key: u64,
    placeholder_context_key: u64,
}

struct MuskRuntime {
    context: NockContext,
    cold_state: Option<&'static [u8]>,
    // Copied interpreter-side cores keyed by the source core raw noun. These nouns live on the
    // runtime's long-lived eval stack, outside per-mack call frames, so repeated `^~` folds share
    // copied batteries/context instead of copying the same core tree for every arm invocation.
    mack_core_cache_raw: FastHashMap<u64, Noun>,
    mack_core_cache_context: Option<u64>,
    mack_cache_raw: FastHashMap<(u64, u64), Option<Noun>>,
    mack_cache: BucketMemo<(u32, u64), MuskMackCacheEntry>,
}

impl MuskRuntime {
    fn new() -> Self {
        Self {
            context: create_musk_eval_context(),
            cold_state: None,
            mack_core_cache_raw: Default::default(),
            mack_core_cache_context: None,
            mack_cache_raw: Default::default(),
            mack_cache: Default::default(),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheContextKey {
    semantic: SemanticContextKey,
    memo: MemoContextKey,
}

type SemanticLocationKey = (String, u64, u64, u64, u64);
type SemanticResolutionKey = (SemanticLocationKey, SemanticLocationKey, String);

pub struct Ut<'a> {
    pub slab: &'a mut NounSlab,
    // Owned per-compile native-IR state (intern table + decode/encode memos +
    // boundary caches) -- the single home for what used to be module thread-locals.
    // A disjoint field from `slab`, so the ir free fns can borrow `&mut self.cx`
    // and `self.slab` together. Its lifetime IS the compile (replaces live_reset).
    cx: Context,
    // Context taxonomy for last-mile parity work:
    // - semantic context: `vet` plus active `%rest`/`fan` scope
    // - memo context: recursion-sensitive arm epoch and placeholder signature
    // - recursion guards: in-progress arm state and wet `rib`
    pub vet: bool,
    dbug_locations: Vec<CompilerErrorLocation>,
    semantic_recording: bool,
    semantic_type_facts: Vec<CompilerSemanticFact>,
    semantic_type_fact_keys: HashSet<SemanticLocationKey>,
    semantic_resolution_facts: Vec<CompilerResolutionFact>,
    semantic_resolution_fact_keys: HashSet<SemanticResolutionKey>,
    // Memoization tables. New cache keys should derive semantic/memo state from the helper
    // accessors below rather than hand-assembling context tuples at each cache surface.
    // Recursion / in-progress guards. These are not caches; they constrain valid memo reuse and
    // are folded into the memo-context helpers when the cache policy requires it.
    pub arm_in_progress: HashSet<(Arc<str>, u64)>,
    pub arm_goal_in_progress: Vec<ArmInProgressEntry>,
    pub arm_placeholder_play_in_progress: HashSet<u64>,
    pub arm_epoch: u64,
    pub lazy_resolver_next_id: u64,
    pub lazy_resolvers: HashMap<u64, LazyResolverContext>,
    // RT-05 canonical lazy-core identity: maps a recursive core's structural key
    // (interned-sut ptr, tomes_sig, poly) to ONE canonical resolver id, so
    // structurally-equal lazy cores intern to one `Rc` and every pointer-keyed
    // recursion cut converges. Shares `lazy_resolvers`' whole-compile lifetime
    // (never cleared by `clear_build_transients`).
    pub lazy_resolver_canonical_ids: HashMap<(usize, u64, u8), u64>,
    // Hybrid oracle-sut parity compilation can opt into exact AST recovery from structurally equal
    // hoon nouns. Keep this disabled on the normal compiler path.
    pub exact_hoon_ast_lookup_enabled: bool,
    pub hoon_identity_cache_raw: HashMap<u64, Noun>,
    pub hoon_identity_cache_order: VecDeque<u64>,
    pub hoon_cache_raw: HashMap<u64, Arc<Hoon>>,
    pub hoon_cache_raw_order: VecDeque<u64>,
    pub hoon_cache_struct: FastHashMap<u32, VecDeque<(Noun, Arc<Hoon>)>>,
    pub hoon_cache_struct_order: VecDeque<u32>,
    pub hoon_identity_cache_struct: FastHashMap<u32, VecDeque<Noun>>,
    pub hoon_identity_cache_struct_order: VecDeque<u32>,
    // Bounded decode cache for `%hold` gene nouns that are discovered from type data rather than
    // from parser-owned AST pointers.
    pub decoded_hold_hoon_cache_raw: HashMap<u64, Arc<Hoon>>,
    pub decoded_hold_hoon_cache_order: VecDeque<u64>,
    pub decoded_hold_hoon_ptr_cache: HashMap<usize, (Option<u64>, Noun)>,
    pub hoon_ast_ptr_cache: HashMap<usize, (Option<u64>, Noun)>,
    pub hoon_ast_ptr_cache_order: VecDeque<usize>,
    pub hold_memo: HoldMemoSet,
    // Canonical `++fire` wet-arm validation tracks `[sut dox gen]` in `rib`
    // to avoid recursive re-entry during `mull` checks. `sut`/`dox` are now native
    // interned `NRc<NTy>` (ptr-identity == structural identity, hash-consed), so
    // the rib keys on their `Rc` pointers; the `gen` half stays a canonicalized
    // hoon noun (kept as both the live noun and its raw address for the fast path).
    fire_wet_rib: Vec<(NRc<NTy>, NRc<NTy>, Noun)>,
    fire_wet_rib_raw: FastHashSet<(usize, usize, u64)>,
    // Dynamic `%rest` / `%hold` fan scope. This is part of the semantic execution context.
    // Canonical `++rest` tracks active loop legs in `fan`; native interns each structural
    // `[inner hoon]` pair to a stable `leg_id` and keys the active semantic scope by that set.
    hold_repo_fan_leg_ids: FastHashMap<(u32, u32), Vec<HoldRepoFanLegIdEntry>>,
    hold_repo_fan_leg_raw_ids: FastHashMap<(u64, u64), u64>,
    hold_repo_fan_leg_id_by_hold_raw: FastHashMap<u64, u64>,
    hold_repo_fan_leg_id_by_hold_raw_order: VecDeque<u64>,
    hold_repo_fan_leg_id_by_hold_mug: FastHashMap<u32, VecDeque<HoldRepoFanHoldIdEntry>>,
    hold_repo_fan_leg_id_by_hold_mug_order: VecDeque<u32>,
    hold_repo_fan_active_leg_ids: Vec<u64>,
    hold_repo_fan_signature_sum: u64,
    hold_repo_fan_signature_xor: u64,
    hold_repo_fan_context_by_signature: FastHashMap<(u64, u64, usize), Vec<(Vec<u64>, u64)>>,
    pub hold_repo_fan_context_id: u64,
    pub hold_repo_fan_context_next_id: u64,
    hold_repo_fan_leg_next_id: u64,
    // Scope-precise fan key: per-Rc-ptr leg-id memo for %hold types, so
    // `reachable_legs` resolves a hold's leg-id in O(1) amortized (reusing the
    // existing noun-path leg intern once per distinct hold pointer). Persists for
    // the whole compile (leg-ids are compile-stable); cleared in clear_build_memos
    // alongside the fan/lazy state.
    hold_repo_fan_leg_id_by_ptr: FastHashMap<usize, u64>,
    // Memoized intersection ids for scoped fan subsets: maps a (active-context-id,
    // legset-id) pair to its interned subset id, avoiding re-interning the same
    // (active ∩ legset) Vec across calls. Reuses the same dedup discipline as
    // refresh_hold_repo_fan_context_id over arbitrary sorted subsets.
    hold_repo_fan_subset_by_signature: FastHashMap<(u64, u64, usize), Vec<(Vec<u64>, u64)>>,
    // Canonical-boundary memoization surface. These are the first caches being
    // consolidated around Vere-style ut boundaries (`mint`, `mull`, `rest`,
    // `nest`, `redo`, etc.) instead of helper-local policy tables.
    pub boundary_memo: BoundaryMemoSet,
    pub bran_semi_memo: BucketMemo<BranSemiMemoKey, BranSemiCacheEntry>,
    // Caches keyed by structural mug rather than raw pointer.  The native compiler frequently
    // reconstructs equal type nouns (e.g. `ty_atom("n", 0)`), which defeats pointer-keyed caches
    // and can lead to runaway `find/cool/chip` recursion in large molds (notably hoon-138).
    // Lookup/narrowing memo surface for canonical `find/fond/fine` and `ar`-adjacent helpers.
    pub lookup_memo: LookupMemoSet,
    pub spec_example_cache: HashMap<u64, VecDeque<(Spec, Arc<Hoon>)>>,
    pub spec_example_cache_order: VecDeque<u64>,
    pub spec_factory_open_cache: HashMap<u64, VecDeque<(Spec, Arc<Hoon>)>>,
    pub spec_factory_open_cache_order: VecDeque<u64>,
    pub burp_type_cache: HashMap<u64, Noun>,
    musk: MuskRuntime,
    // Cross-call persistence for the `miss` memo; only enabled during the
    // isolated prelude (hoon-138) mint. See miss().
    /// Cross-call `miss` memo (prelude mint only), retained for a single
    /// semantic epoch: `(vet, active fan, arm epoch, placeholder context)`.
    /// `miss` reaches `repo`/`rest`/`redo`, whose state evolves during a
    /// build; a verdict memoized under one hold-expansion state can flip
    /// under another (observed: ++dish's `~|` vase constant kept a `%hint`
    /// fork member hoonc resolves away). Clearing on any epoch change keeps
    /// within-arm reuse (the perf case) while never reusing across state.
    miss_memo_persist: Option<((u8, u64, u64, u64), FastHashMap<(u64, u64, u8), bool>)>,
    // `^~` fold outcomes keyed by (bran, formula). Folding is a pure
    // function of these two nouns: arm resolution through the persistent
    // lazy resolvers is time-invariant, so both successes and failures are
    // safe to reuse for the lifetime of the Ut.
    ktsg_fold_cache: BucketMemo<(u32, u32), KtsgFoldCacheEntry>,
    semi_mask_full_empty: Option<Noun>,
    semi_root_blocked_set: Option<Noun>,
    semi_full_blocked_interned: Option<Noun>,
    // Cache for `hatch::utils::open()`. Keyed by `&Hoon` pointer, guarded by a structural
    // signature to avoid incorrect hits when the allocator reuses freed AST node addresses.
    //
    // Value is `None` when `open(gen)` returns `gen` unchanged, avoiding storing duplicate clones.
    pub open_cache: HashMap<usize, (u64, Option<Arc<Hoon>>)>,
    pub open_cache_order: VecDeque<usize>,
    pub arm_key_term_cache: HashMap<u64, Arc<str>>,
    pub arm_key_term_cache_order: VecDeque<u64>,
    #[cfg(test)]
    pub skin_match_static_calls: usize,
    #[cfg(test)]
    pub stack_guard_calls: usize,
}

pub struct Sig64 {
    state: u64,
    include_dbug_spot: bool,
}

impl Sig64 {
    // FNV-1a-like rolling signature; this is used only as a cache guard against pointer reuse.
    // It must be stable for a given value, but does not need to be cryptographic.
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new_with_dbug_spots(include_dbug_spot: bool) -> Self {
        Self {
            state: Self::OFFSET,
            include_dbug_spot,
        }
    }

    fn finish(self) -> u64 {
        self.state
    }

    #[inline]
    fn write_byte(&mut self, byte: u8) {
        self.state ^= u64::from(byte);
        self.state = self.state.wrapping_mul(Self::PRIME);
    }

    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_byte(*byte);
        }
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    #[inline]
    fn write_u128(&mut self, value: u128) {
        self.write_bytes(&value.to_le_bytes());
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

    fn hoon_signature_spot_sensitive(hoon: &Hoon) -> Option<u64> {
        let mut sig = Self::new_with_dbug_spots(true);
        sig.write_hoon(hoon)?;
        Some(sig.finish())
    }

    fn spec_signature_spot_sensitive(spec: &Spec) -> Option<u64> {
        let mut sig = Self::new_with_dbug_spots(true);
        sig.write_spec(spec)?;
        Some(sig.finish())
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
                self.write_u64(*fragment);
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
                self.write_u64(*axis);
                self.write_nock(nock)?;
            }
            Nock::Edit((axis, nock), rest) => {
                self.write_byte(0x0b);
                self.write_u64(*axis);
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
                self.write_u64(*axis);
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
        match hoon {
            Hoon::Pair(a, b) => {
                self.write_byte(0x01);
                self.write_hoon(a)?;
                self.write_hoon(b)?;
            }
            Hoon::ZapZap => self.write_byte(0x02),
            Hoon::Axis(axis) => {
                self.write_byte(0x03);
                self.write_u64(*axis);
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
        Some(())
    }
}

impl<'a> Ut<'a> {
    pub fn new(slab: &'a mut NounSlab) -> Self {
        // Native-types flip: the wing-nav / skin / mull / nest families make the
        // per-compile intern table + the live_to_noun / live_leaf_to_noun memos
        // (keyed by Rc/noun pointer, bound to THIS compile's slab) the primary
        // path. Every fresh `Ut` gets a fresh `cx: Context::new()`, so a prior
        // compile's slab-bound nouns can never alias into this one (a freed slab's
        // address is reused by the next compile, otherwise returning a stale
        // interned Rc / dangling noun). This per-`Ut` Context replaces the old
        // thread-local `live_reset()` at every compile boundary — notably the test
        // harnesses that compile many exprs on one thread, which is where the
        // missing reset surfaced as decode errors.
        Self {
            slab,
            cx: Context::new(),
            vet: true,
            dbug_locations: Vec::new(),
            semantic_recording: false,
            semantic_type_facts: Vec::new(),
            semantic_type_fact_keys: HashSet::new(),
            semantic_resolution_facts: Vec::new(),
            semantic_resolution_fact_keys: HashSet::new(),
            arm_in_progress: HashSet::new(),
            arm_goal_in_progress: Vec::new(),
            arm_placeholder_play_in_progress: HashSet::new(),
            arm_epoch: 0,
            lazy_resolver_next_id: 1,
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
            hold_repo_fan_context_id: 0,
            hold_repo_fan_context_next_id: 1,
            hold_repo_fan_leg_next_id: 1,
            hold_repo_fan_leg_id_by_ptr: Default::default(),
            hold_repo_fan_subset_by_signature: Default::default(),
            boundary_memo: Default::default(),
            bran_semi_memo: Default::default(),
            lookup_memo: Default::default(),
            spec_example_cache: HashMap::new(),
            spec_example_cache_order: VecDeque::new(),
            spec_factory_open_cache: HashMap::new(),
            spec_factory_open_cache_order: VecDeque::new(),
            burp_type_cache: HashMap::new(),
            musk: MuskRuntime::new(),
            miss_memo_persist: None,
            ktsg_fold_cache: Default::default(),
            semi_mask_full_empty: None,
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

    fn clear_musk_context_dependent_caches(&mut self) {
        self.musk.clear_context_dependent_caches();
    }

    /// Upper bound on the cell-level eval-stack copy cache. The cache remembers
    /// every cell copied into the eval stack keyed by its *source* slab address,
    /// so structurally-shared subtrees within and across `^~` folds copy once.
    /// Over a full hoon-138 self-mint it accreted ~59M entries (~1.6 GB RSS):
    /// each freshly-minted (deepening) core copies ~150K cells at fresh
    /// addresses, almost all of which are never queried again once that core's
    /// copy completes. Capping it is byte-safe — the values are live nouns on
    /// the long-lived (virtual, non-resident) eval stack, so forgetting them only
    /// causes a later structurally-identical copy to be re-materialized on the
    /// eval stack rather than shared; the folded *result* is unchanged.
    const MUSK_CORE_CACHE_CAP: usize = 4_000_000;

    fn ensure_musk_mack_core_cache_context(&mut self, context: &NockContext) {
        let context_id = context.stack.frame_identity();
        if self.musk.mack_core_cache_context != Some(context_id) {
            self.musk.mack_core_cache_raw.clear();
            self.musk.mack_core_cache_context = Some(context_id);
        } else if self.musk.mack_core_cache_raw.len() > Self::MUSK_CORE_CACHE_CAP {
            // Bound RSS: drop the cell-dedup entries accumulated by prior folds.
            // The current fold re-warms what it needs. Measured on the hoon-138
            // self-mint: the uncapped map reaches ~59M entries (~13.0 GB peak
            // RSS); capping at 4M holds peak RSS to ~9.0 GB and *lowers* CPU time
            // (45.9s vs 47.6s) — the entries past the cap are overwhelmingly
            // stale, and the smaller hashmap has markedly better cache locality.
            self.musk.mack_core_cache_raw.clear();
        }
    }

    unsafe fn musk_mack_cached_core_in_context(
        &mut self,
        context: &mut NockContext,
        core: Noun,
        core_space: &NounSpace,
    ) -> Noun {
        self.ensure_musk_mack_core_cache_context(context);
        let raw = core.as_raw();
        if let Some(cached) = self.musk.mack_core_cache_raw.get(&raw).copied() {
            return cached;
        }
        // Fill the slab-side mug cache for the whole core before copying:
        // the copy below propagates cached mugs onto the eval-stack nouns,
        // which lets cold-state registration inside `interpret` short-circuit
        // its unifying-equality checks on mug mismatch instead of walking
        // whole battery structures per call.
        self.noun_mug_cached(core);
        let cached = self.copy_into_eval_stack_shared(context, core, core_space);
        self.musk.mack_core_cache_raw.insert(raw, cached);
        cached
    }

    /// Copy a slab noun onto the musk eval stack with structural sharing:
    /// every cell (and indirect atom) copied is remembered in
    /// `musk_mack_core_cache_raw`, so the large shared parts of mack call
    /// cores (batteries, the context chain) are copied exactly once per
    /// eval context. Successive calls of the same gate then only copy the
    /// fresh spine and sample. The copies are made outside the per-call
    /// interpreter snapshot, so they stay live on the eval stack for the
    /// lifetime of the context, exactly like the whole-core copies this
    /// replaces.
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
            let raw = noun.as_raw();
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
        let raw = noun.as_raw();
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

    pub fn begin_semantic_recording(&mut self) {
        self.semantic_type_facts.clear();
        self.semantic_type_fact_keys.clear();
        self.semantic_resolution_facts.clear();
        self.semantic_resolution_fact_keys.clear();
        self.semantic_recording = true;
    }

    pub fn finish_semantic_recording(
        &mut self,
    ) -> (Vec<CompilerSemanticFact>, Vec<CompilerResolutionFact>) {
        self.semantic_recording = false;
        (
            std::mem::take(&mut self.semantic_type_facts),
            std::mem::take(&mut self.semantic_resolution_facts),
        )
    }

    /// Run `f` with `vet` forced off, restoring the previous value afterward
    /// (on both Ok and Err). Replaces the hand-rolled raw-pointer `VetGuard`
    /// drop guards in the type-checker; vet is plain `Copy` state, so a
    /// save/restore around the call is sufficient and avoids `unsafe` in
    /// semantic logic. (A panic unwinding through here leaves vet off, but
    /// honk discards the Ut after any caught compile panic.)
    fn with_vet_off<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let prev = std::mem::replace(&mut self.vet, false);
        let result = f(self);
        self.vet = prev;
        result
    }

    pub fn clear_build_memos(&mut self) {
        self.clear_build_transients();
        self.arm_epoch = 0;
        self.hold_memo = Default::default();
        self.boundary_memo = Default::default();
        self.lookup_memo = Default::default();
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
        // Lazy resolver contexts are NOT transient: types cached across
        // build entries (mint/rest/redo caches) embed `[%lazy 1 id]` battery
        // semis, and hoon-138's equivalent resolver is a pure gate inside
        // the seminoun that remains callable forever. Clearing the registry
        // or reusing ids would dangle or cross-wire those references.
        self.fire_wet_rib.clear();
        self.fire_wet_rib_raw.clear();
        self.hold_repo_fan_active_leg_ids.clear();
        self.hold_repo_fan_signature_sum = 0;
        self.hold_repo_fan_signature_xor = 0;
        self.hold_repo_fan_context_id = 0;
        self.hold_repo_fan_subset_by_signature.clear();
        self.bran_semi_memo = Default::default();
        self.musk.mack_cache_raw.clear();
    }

    const NEST_MUG_BUCKET_LIMIT: usize = 32;
    const NEST_MUG_KEY_LIMIT: usize = 65_536;
    const HOON_CACHE_RAW_KEY_LIMIT: usize = 16_384;
    const BURP_TYPE_CACHE_LIMIT: usize = 65_536;
    const HOON_CACHE_STRUCT_KEY_LIMIT: usize = 16_384;
    const HOON_CACHE_STRUCT_BUCKET_LIMIT: usize = 8;
    const SPEC_CACHE_KEY_LIMIT: usize = 16_384;
    const SPEC_CACHE_BUCKET_LIMIT: usize = 8;
    const CORE_MINT_CACHE_KEY_LIMIT: usize = 16_384;
    const CORE_MINT_CACHE_BUCKET_LIMIT: usize = 8;
    const MINT_CACHE_BUCKET_LIMIT: usize = 4;
    const MINT_CACHE_KEY_LIMIT: usize = 16_384;
    const MUSK_MACK_CACHE_KEY_LIMIT: usize = 16_384;
    const MUSK_MACK_CACHE_BUCKET_LIMIT: usize = 8;
    const MULL_CACHE_BUCKET_LIMIT: usize = 4;
    const MULL_CACHE_KEY_LIMIT: usize = 16_384;
    const REDO_CACHE_BUCKET_LIMIT: usize = 8;
    const REDO_CACHE_KEY_LIMIT: usize = 32_768;
    const REST_CACHE_BUCKET_LIMIT: usize = 8;
    const REST_CACHE_KEY_LIMIT: usize = 32_768;
    const FISH_CACHE_BUCKET_LIMIT: usize = 8;
    const FISH_CACHE_KEY_LIMIT: usize = 32_768;
    const TYPE_BINARY_BOUNDARY_CACHE_BUCKET_LIMIT: usize = 8;
    const TYPE_BINARY_BOUNDARY_CACHE_KEY_LIMIT: usize = 32_768;
    const BRAN_SEMI_CACHE_KEY_LIMIT: usize = 65_536;
    const BRAN_SEMI_CACHE_BUCKET_LIMIT: usize = 8;
    const HOLD_TYPE_CACHE_BUCKET_LIMIT: usize = 8;
    const HOLD_TYPE_CACHE_KEY_LIMIT: usize = 65_536;
    const HOLD_TYPE_CACHE_RAW_KEY_LIMIT: usize = 65_536;
    const HOLD_REPO_FAN_LEG_HOLD_RAW_KEY_LIMIT: usize = 65_536;
    const HOLD_REPO_FAN_LEG_HOLD_MUG_KEY_LIMIT: usize = 65_536;
    const HOLD_REPO_FAN_LEG_HOLD_MUG_BUCKET_LIMIT: usize = 8;

    fn hold_repo_fan_context_key(&self) -> u64 {
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            0
        } else {
            self.hold_repo_fan_context_id
        }
    }

    fn hold_repo_fan_leg_signature_component(leg_id: u64) -> u64 {
        let mut hasher = FastHasher::default();
        hasher.mix_u64(leg_id);
        hasher.finish()
    }

    fn semantic_context_key(&self) -> SemanticContextKey {
        SemanticContextKey {
            vet_key: u8::from(self.vet),
            fan_context_key: self.hold_repo_fan_context_key(),
        }
    }

    fn hold_repo_fan_leg_lookup_id(&mut self, inner: Noun, hoon: Noun) -> Result<Option<u64>> {
        let raw_key = (unsafe { inner.as_raw() }, unsafe { hoon.as_raw() });
        if let Some(id) = self.hold_repo_fan_leg_raw_ids.get(&raw_key).copied() {
            return Ok(Some(id));
        }
        let key = (self.noun_mug_cached(inner), self.noun_mug_cached(hoon));
        let Some(entries) = self.hold_repo_fan_leg_ids.get(&key) else {
            return Ok(None);
        };
        let inner_raw = unsafe { inner.as_raw() };
        let hoon_raw = unsafe { hoon.as_raw() };
        for entry in entries.iter().rev() {
            let inner_match = unsafe { entry.inner.raw_equals(&inner) }
                || unsafe { entry.inner.as_raw() } == inner_raw
                || noun_eq(entry.inner, inner, &self.slab.noun_space())?;
            if !inner_match {
                continue;
            }
            let hoon_match = unsafe { entry.hoon.raw_equals(&hoon) }
                || unsafe { entry.hoon.as_raw() } == hoon_raw
                || noun_eq(entry.hoon, hoon, &self.slab.noun_space())?;
            if hoon_match {
                self.hold_repo_fan_leg_raw_ids.insert(raw_key, entry.id);
                return Ok(Some(entry.id));
            }
        }
        Ok(None)
    }

    fn hold_repo_fan_leg_intern_id(&mut self, inner: Noun, hoon: Noun) -> Result<u64> {
        if let Some(id) = self.hold_repo_fan_leg_lookup_id(inner, hoon)? {
            return Ok(id);
        }

        let key = (self.noun_mug_cached(inner), self.noun_mug_cached(hoon));
        let id = self.hold_repo_fan_leg_next_id.max(1);
        self.hold_repo_fan_leg_next_id = self.hold_repo_fan_leg_next_id.wrapping_add(1);
        if self.hold_repo_fan_leg_next_id == 0 {
            self.hold_repo_fan_leg_next_id = 1;
        }
        self.hold_repo_fan_leg_raw_ids
            .insert((unsafe { inner.as_raw() }, unsafe { hoon.as_raw() }), id);
        // The mug store is authoritative and lives for the whole compile; with no
        // frame arena, `inner`/`hoon` live in the single slab for the whole
        // compile, so id lookups (noun_eq) stay valid without any relocation.
        self.hold_repo_fan_leg_ids
            .entry(key)
            .or_default()
            .push(HoldRepoFanLegIdEntry { id, inner, hoon });
        Ok(id)
    }

    fn hold_repo_fan_leg_id_by_hold_raw_store(&mut self, hold_raw: u64, leg_id: u64) {
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

    fn hold_repo_fan_leg_id_by_hold_mug_lookup(&mut self, hold: Noun) -> Result<Option<u64>> {
        let hold_mug = self.noun_mug_cached(hold);
        let Some(entries) = self.hold_repo_fan_leg_id_by_hold_mug.get(&hold_mug) else {
            return Ok(None);
        };
        let hold_raw = unsafe { hold.as_raw() };
        let mut matched = None;
        for entry in entries.iter().rev() {
            let hold_match = unsafe { entry.hold.raw_equals(&hold) }
                || unsafe { entry.hold.as_raw() } == hold_raw
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

    fn hold_repo_fan_leg_id_by_hold_mug_store(&mut self, hold: Noun, leg_id: u64) -> Result<()> {
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
        let hold_raw = unsafe { hold.as_raw() };
        for entry in bucket.iter() {
            let hold_match = unsafe { entry.hold.raw_equals(&hold) }
                || unsafe { entry.hold.as_raw() } == hold_raw
                || noun_eq(entry.hold, hold, &self.slab.noun_space())?;
            if hold_match {
                return Ok(());
            }
        }
        if bucket.len() >= Self::HOLD_REPO_FAN_LEG_HOLD_MUG_BUCKET_LIMIT {
            bucket.pop_front();
        }
        // `hold` lives in the single compile slab; later cross-arm lookups
        // (noun_eq) stay valid without any relocation.
        bucket.push_back(HoldRepoFanHoldIdEntry { hold, id: leg_id });
        Ok(())
    }

    fn hold_repo_fan_leg_id_for_hold_type(
        &mut self,
        hold: Noun,
        inner: Noun,
        hoon: Noun,
    ) -> Result<u64> {
        let hold_raw = unsafe { hold.as_raw() };
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

    /// Per-`Rc`-ptr leg-id for a `%hold` native type (scope-precise fan key).
    /// Reuses the existing noun-path leg intern ONCE per distinct hold pointer
    /// (`hold_repo_fan_leg_id_for_hold_type`), then memoizes ptr -> leg-id, so
    /// `reachable_legs` resolves a hold's leg-id O(1) amortized. Byte-neutral:
    /// returns the SAME id `redo_subject_hold_in_fan` would (same intern path).
    fn hold_repo_fan_leg_id_for_hold_native(&mut self, hold: &NRc<NTy>) -> Result<u64> {
        let ptr = NRc::as_ptr(hold) as usize;
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

    /// The set of `%hold` leg-ids reachable from `t`, sorted and deduped, memoized
    /// per interned `Rc` pointer (sound because `intern_node` hash-conses, so
    /// ptr == structural identity). Bottom-up over the Rc DAG (acyclic: a node's
    /// hash depends on already-interned children), so each distinct node is
    /// visited at most once and the closure is O(1) amortized per node. See the
    /// linearity proof in the design memo. The `Fork` case decodes options once
    /// per fork ptr (also memoized) and unions their legsets.
    fn reachable_legs(&mut self, t: &NRc<NTy>) -> Result<NRc<[u64]>> {
        let ptr = NRc::as_ptr(t) as usize;
        if let Some(legs) = legset_memo_lookup(&self.cx, ptr) {
            return Ok(legs);
        }
        let legs = self.with_stack_guard(|ut| ut.reachable_legs_node(t))?;
        legset_memo_store(&mut self.cx, ptr, legs.clone());
        Ok(legs)
    }

    /// One node of `reachable_legs`' memoized recursion (the `with_stack_guard`
    /// wraps each level for deep DAGs). See `reachable_legs`.
    fn reachable_legs_node(&mut self, t: &NRc<NTy>) -> Result<NRc<[u64]>> {
        let legs: Vec<u64> = match &**t {
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
                // The fork's options are not Rc children; decode (memoized per
                // fork ptr inside fork_options_native via native_of's mug memo)
                // and union the option legsets.
                let options = self.fork_options_native(t)?;
                let mut acc: Vec<u64> = Vec::new();
                for opt in options {
                    let lo = self.reachable_legs(&opt)?;
                    acc = Self::merge_sorted_legs(&acc, &lo);
                }
                acc
            }
            NTy::Hold { subject, .. } => {
                // legset(Hold) = {leg_id(self)} ∪ legset(subject). NOT the
                // repo-expansion: the expansion's holds are reached at the next
                // descent level (which keys on its own scope). See the
                // correctness argument §1 (redo_subject_hold_in_fan tests only
                // leg_id(self); recursion keys per level).
                let subject = subject.clone();
                let self_leg = self.hold_repo_fan_leg_id_for_hold_native(t)?;
                let ls = self.reachable_legs(&subject)?;
                Self::merge_sorted_legs(&ls, std::slice::from_ref(&self_leg))
            }
        };
        Ok(NRc::from(legs.into_boxed_slice()))
    }

    /// Sorted-merge-dedup union of two sorted leg-id slices (mirrors the
    /// NestSeenSet binary-search insert discipline; here a linear merge).
    fn merge_sorted_legs(a: &[u64], b: &[u64]) -> Vec<u64> {
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

    /// Intersection of two sorted leg-id slices (both already sorted+deduped).
    fn intersect_sorted_legs(a: &[u64], b: &[u64]) -> Vec<u64> {
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

    /// Canonicalize an arbitrary sorted-deduped leg-id subset to a stable id,
    /// reusing the same (sum, xor, len)-bucketed dedup discipline as
    /// `refresh_hold_repo_fan_context_id` so equal subsets map to one id
    /// deterministically. Shares the `hold_repo_fan_context_next_id` id space and
    /// the same id-assignment order, so no new id semantics are introduced.
    /// Empty subset is the sentinel 0 (identical to today's empty-fan key).
    fn intern_fan_subset_id(&mut self, subset: &[u64]) -> u64 {
        if subset.is_empty() {
            return 0;
        }
        let mut sum: u64 = 0;
        let mut xor: u64 = 0;
        for leg_id in subset {
            let component = Self::hold_repo_fan_leg_signature_component(*leg_id);
            sum = sum.wrapping_add(component);
            xor ^= component;
        }
        let key = (sum, xor, subset.len());
        if let Some(entries) = self.hold_repo_fan_subset_by_signature.get(&key) {
            for (legs, id) in entries.iter().rev() {
                if legs.as_slice() == subset {
                    return *id;
                }
            }
        }
        let id = self.hold_repo_fan_context_next_id.max(1);
        self.hold_repo_fan_context_next_id = self.hold_repo_fan_context_next_id.wrapping_add(1);
        if self.hold_repo_fan_context_next_id == 0 {
            self.hold_repo_fan_context_next_id = 1;
        }
        self.hold_repo_fan_subset_by_signature
            .entry(key)
            .or_default()
            .push((subset.to_vec(), id));
        id
    }

    /// The scope-precise fan key is the DEFAULT (STEP 5): it is byte-exact on
    /// both kernels (dumb 6894a41d…, roswell fea818a1…) and is what lets the
    /// native hoon-138 arbitrary self-mint COMPLETE (bounded ~13 GB / ~46 s vs
    /// the old never-completing 38-46 GB). Kept as a single source of truth so the
    /// cache helpers and the partition tests share one switch.
    fn scoped_fan_enabled() -> bool {
        true
    }

    /// Scope-precise fan key for a descent op on `scope` (the deepening subject).
    /// Projects the active leg-set onto `reachable_legs(scope)`: legs not reachable
    /// from `scope` can never be tested during its resolution, so they cannot
    /// change the result (correctness §1-2). Collapses to 0 (the empty-fan key
    /// shape, already proven by every kernel) for the ~97% of resolutions whose
    /// intersection is empty. Falls back to the whole-active key when the flag is
    /// off, so the change is inert until enabled.
    fn fan_context_key_scoped(&mut self, scope: &NRc<NTy>) -> Result<u64> {
        if !Self::scoped_fan_enabled() {
            return Ok(self.hold_repo_fan_context_key());
        }
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            return Ok(0);
        }
        let legs = self.reachable_legs(scope)?;
        let inter = Self::intersect_sorted_legs(&self.hold_repo_fan_active_leg_ids, &legs);
        if inter.is_empty() {
            return Ok(0);
        }
        Ok(self.intern_fan_subset_id(&inter))
    }

    /// Scope-precise fan key over the UNION of two scopes' reachable legs (used
    /// by dual-perspective ops like mull, where the fan can be consulted from
    /// either the sut or the dox descent). Byte-safe: any leg that could change
    /// the result is reachable from at least one of the two scopes.
    fn fan_context_key_scoped_pair(&mut self, a: &NRc<NTy>, b: &NRc<NTy>) -> Result<u64> {
        if !Self::scoped_fan_enabled() {
            return Ok(self.hold_repo_fan_context_key());
        }
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            return Ok(0);
        }
        let la = self.reachable_legs(a)?;
        let lb = self.reachable_legs(b)?;
        let legs = Self::merge_sorted_legs(&la, &lb);
        let inter = Self::intersect_sorted_legs(&self.hold_repo_fan_active_leg_ids, &legs);
        if inter.is_empty() {
            return Ok(0);
        }
        Ok(self.intern_fan_subset_id(&inter))
    }

    /// `fan_context_key_scoped` for a noun-keyed cache surface (redo/rest): when
    /// the flag is on and the active set is non-empty, lift the noun subject to
    /// native (content-keyed `native_of_cached`) ONCE and scope on it. Flag-off
    /// and empty-active short-circuit before any conversion, so the noun path is
    /// unchanged when the feature is inert.
    fn fan_context_key_scoped_noun(&mut self, sut: Noun) -> Result<u64> {
        if !Self::scoped_fan_enabled() {
            return Ok(self.hold_repo_fan_context_key());
        }
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            return Ok(0);
        }
        let sut_native = self.native_of_cached(sut)?;
        self.fan_context_key_scoped(&sut_native)
    }

    fn refresh_hold_repo_fan_context_id(&mut self) {
        if self.hold_repo_fan_active_leg_ids.is_empty() {
            self.hold_repo_fan_context_id = 0;
            return;
        }

        let key = (
            self.hold_repo_fan_signature_sum,
            self.hold_repo_fan_signature_xor,
            self.hold_repo_fan_active_leg_ids.len(),
        );
        if let Some(entries) = self.hold_repo_fan_context_by_signature.get(&key) {
            for (legs, id) in entries.iter().rev() {
                if *legs == self.hold_repo_fan_active_leg_ids {
                    self.hold_repo_fan_context_id = *id;
                    return;
                }
            }
        }

        let id = self.hold_repo_fan_context_next_id.max(1);
        self.hold_repo_fan_context_next_id = self.hold_repo_fan_context_next_id.wrapping_add(1);
        if self.hold_repo_fan_context_next_id == 0 {
            self.hold_repo_fan_context_next_id = 1;
        }
        self.hold_repo_fan_context_by_signature
            .entry(key)
            .or_default()
            .push((self.hold_repo_fan_active_leg_ids.clone(), id));
        self.hold_repo_fan_context_id = id;
    }

    fn hold_repo_fan_activate_leg_id(&mut self, leg_id: u64) -> bool {
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

    fn hold_repo_fan_deactivate_leg_id(&mut self, leg_id: u64) {
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
                    hash ^= *axis;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
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

    fn mint_cache_signature(gen: &Hoon) -> Option<u64> {
        // `mint` returns formula nouns with `%spot` hints, so cache keys must include debug spots.
        Sig64::hoon_signature_spot_sensitive(gen)
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
                vec![Limb::Axis(1)],
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
        let mut axe = 6u64;
        for (idx, hoon) in hoons.iter().enumerate() {
            let is_last = idx + 1 == hoons.len();
            let wing_axe = if is_last { axe } else { peg_axis(axe, 2)? };
            let target_wing = vec![Limb::Parent(0, None), Limb::Axis(wing_axe)];
            compiled.push((target_wing, hoon.clone()));
            if !is_last {
                axe = peg_axis(axe, 3)?;
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
        extended_wing.push(Limb::Axis(2));
        let wrapped_pairs = pairs
            .iter()
            .map(|(pair_wing, pair_hoon)| {
                (
                    pair_wing.clone(),
                    Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(pair_hoon.clone())),
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
                    Box::new(Hoon::Axis(3)),
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
                [h] => Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(h.clone())),
                [h, t @ ..] => Hoon::CenCol(
                    Box::new(Hoon::Axis(2)),
                    vec![Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(h.clone())), loop_yex(t)],
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
            Box::new(Hoon::SigGar(term_or_pair.clone(), Box::new(Hoon::Axis(1)))),
            Box::new(q.clone()),
        )
    }

    fn lower_sigfas(chum: &Chum, q: &Hoon) -> Hoon {
        Hoon::SigCen(
            chum.clone(),
            Box::new(Hoon::Axis(7)),
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
            let wing_parent_axis6 = vec![Limb::Parent(0, None), Limb::Axis(6)];
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
                Box::new(Hoon::Axis(1)),
            )),
            Box::new(acc),
        )
    }

    fn prefix_signature(prefix: Option<&str>) -> u32 {
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
        hash
    }

    fn core_mint_cache_key(
        &mut self,
        sut: Noun,
        gol: Noun,
        tomes_map: Noun,
        prefix: &Option<String>,
        poly: Poly,
    ) -> CoreMintBoundaryKey {
        let context = self.cache_context_key();
        let poly_key = match poly {
            Poly::Dry => 0u8,
            Poly::Wet => 1u8,
        };
        (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(gol),
            self.noun_mug_cached(tomes_map) ^ Self::prefix_signature(prefix.as_deref()),
            context.semantic.vet_key,
            poly_key,
            context.semantic.fan_context_key,
            context.memo.arm_epoch_key,
            context.memo.placeholder_context_key,
        )
    }

    /// C-final.1b: native-re-keyed `core_mint` boundary cache. The TYPE
    /// components (sut/gol) are keyed on interned `Rc` pointer identity (==
    /// structural identity for hash-consed flip types), so the deepening subject
    /// is never lowered to a noun here. The non-type semantic fields are carried
    /// VERBATIM from the old `core_mint_cache_key`: `tomes_sig` = mug(tomes_map)
    /// ^ prefix_signature(prefix) (tomes_map is AST-derived, not a deepening
    /// type, so it stays a mug), vet, poly, fan, arm_epoch, placeholder.
    fn core_mint_cache_lookup(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        tomes_map: Noun,
        prefix: &Option<String>,
        poly: Poly,
    ) -> Result<Option<(NRc<NTy>, Noun)>> {
        let context = self.cache_context_key();
        // `mint_core` validates the produced core against `gol` through
        // `nice`/`nest`, so `%hold` fan reachability is a property of the
        // subject-goal pair, not just the deepening subject.
        let fan = self.fan_context_key_scoped_pair(sut, gol)?;
        let tomes_sig =
            (self.noun_mug_cached(tomes_map) ^ Self::prefix_signature(prefix.as_deref())) as u64;
        let poly_key = match poly {
            Poly::Dry => 0u8,
            Poly::Wet => 1u8,
        };
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
        formula: Noun,
    ) -> Result<()> {
        let context = self.cache_context_key();
        let fan = self.fan_context_key_scoped_pair(sut, gol)?;
        let tomes_sig =
            (self.noun_mug_cached(tomes_map) ^ Self::prefix_signature(prefix.as_deref())) as u64;
        let poly_key = match poly {
            Poly::Dry => 0u8,
            Poly::Wet => 1u8,
        };
        native_core_mint_cache_store(
            &mut self.cx, sut, gol, tomes_sig, context.semantic.vet_key, poly_key, fan,
            context.memo.arm_epoch_key, context.memo.placeholder_context_key, core_type, formula,
        );
        Ok(())
    }

    fn arm_placeholder_context_signature(&self) -> u64 {
        if self.arm_placeholder_play_in_progress.is_empty() {
            return 0;
        }
        let mut raws: Vec<u64> = self
            .arm_placeholder_play_in_progress
            .iter()
            .copied()
            .collect();
        raws.sort_unstable();
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for raw in raws {
            hash ^= raw;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^ (self.arm_placeholder_play_in_progress.len() as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }

    fn arm_cache_epoch_key(&self) -> u64 {
        // `arm_epoch` bumps on every arm enter/exit. If we key caches directly on it, we lose
        // cross-arm memoization even when no recursion-sensitive context is active.
        //
        // Keep full epoch sensitivity while recursion/placeholder state is live, but collapse the
        // key to 0 when all such state is empty so caches can be reused in the common steady state.
        if self.arm_in_progress.is_empty()
            && self.arm_goal_in_progress.is_empty()
            && self.arm_placeholder_play_in_progress.is_empty()
        {
            0
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

    fn mint_cache_key(&mut self, sut: Noun, gol: Noun, gen_sig: u64) -> MintBoundaryKey {
        // mint's result depends on the active fan scope (%hold/%rest legs) and
        // on in-progress recursive-arm state, exactly like the sibling
        // core_mint cache — include the full context (fan + arm epoch +
        // placeholder), all of which collapse to 0 in the steady state.
        let context = self.cache_context_key();
        (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(gol),
            context.semantic.vet_key,
            gen_sig,
            context.semantic.fan_context_key,
            context.memo.arm_epoch_key,
            context.memo.placeholder_context_key,
        )
    }

    /// C-final.1b: native-re-keyed `mint` boundary cache. TYPE components
    /// (sut/gol) keyed on interned `Rc` pointer identity; non-type semantic
    /// fields carried VERBATIM from the old `mint_cache_key` (vet, gen_sig, fan,
    /// arm_epoch, placeholder).
    fn mint_cache_lookup(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        gen_sig: u64,
    ) -> Result<Option<(NRc<NTy>, Noun)>> {
        let context = self.cache_context_key();
        // `mint` cache hits bypass the fresh `nice(sut, gol, typ)` check; include
        // goal-reachable `%hold` legs in the scoped fan key so cached success is
        // only reused in a semantically equivalent rest/fan context.
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
        gen_sig: u64,
        ty: NRc<NTy>,
        formula: Noun,
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
        let key = self.mint_cache_key(sut, gol, self.noun_mug_cached(gen) as u64);
        let Some(entries) = self.boundary_memo.mint.get(&key) else {
            return Ok(None);
        };
        let sut_raw = unsafe { sut.as_raw() };
        let gol_raw = unsafe { gol.as_raw() };
        for entry in entries.iter().rev() {
            let sut_match = unsafe { entry.sut.as_raw() } == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let gol_match = unsafe { entry.gol.as_raw() } == gol_raw
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
        let key = self.mint_cache_key(sut, gol, self.noun_mug_cached(gen) as u64);
        let bucket = self
            .boundary_memo
            .mint
            .ensure_key(key, Self::MINT_CACHE_KEY_LIMIT);
        let sut_raw = unsafe { sut.as_raw() };
        let gol_raw = unsafe { gol.as_raw() };
        for entry in bucket.iter() {
            let sut_match = unsafe { entry.sut.as_raw() } == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let gol_match = unsafe { entry.gol.as_raw() } == gol_raw
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

    fn mull_cache_key(&mut self, sut: Noun, gol: Noun, dox: Noun, gen: Noun) -> MullBoundaryKey {
        // Like mint, the dual-perspective wet recheck depends on the active
        // fan scope and in-progress arm state; carry the full context (all
        // steady-state-0). fire_wet_rib is deliberately NOT keyed (it is not
        // steady-state-collapsing); no concrete rib-only divergence is known.
        let context = self.cache_context_key();
        (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(gol),
            self.noun_mug_cached(dox),
            context.semantic.vet_key,
            self.noun_mug_cached(gen) as u64,
            context.semantic.fan_context_key,
            context.memo.arm_epoch_key,
            context.memo.placeholder_context_key,
        )
    }

    /// C-final.1b: native-re-keyed `mull` boundary cache. TYPE components
    /// (sut/gol/dox) keyed on interned `Rc` pointer identity; non-type semantic
    /// fields carried VERBATIM from the old `mull_cache_key` (vet, gen_sig =
    /// mug(gen), fan, arm_epoch, placeholder). gen stays a mug (it is the AST
    /// node, not a deepening type).
    fn mull_cache_lookup(
        &mut self,
        sut: &NRc<NTy>,
        gol: &NRc<NTy>,
        dox: &NRc<NTy>,
        gen: Noun,
    ) -> Result<Option<(NRc<NTy>, NRc<NTy>)>> {
        let context = self.cache_context_key();
        // mull is dual-perspective (sut + dox). The active fan can be consulted
        // from EITHER perspective's %hold descent, so scope on the union of both
        // legsets (legset(sut) ∪ legset(dox)) to stay byte-safe.
        let fan = self.fan_context_key_scoped_pair(sut, dox)?;
        let gen_sig = self.noun_mug_cached(gen) as u64;
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
        gen: Noun,
        p_ty: NRc<NTy>,
        q_ty: NRc<NTy>,
    ) -> Result<()> {
        let context = self.cache_context_key();
        let fan = self.fan_context_key_scoped_pair(sut, dox)?;
        let gen_sig = self.noun_mug_cached(gen) as u64;
        native_mull_cache_store(
            &mut self.cx, sut, gol, dox, context.semantic.vet_key, gen_sig, fan,
            context.memo.arm_epoch_key, context.memo.placeholder_context_key, p_ty, q_ty,
        );
        Ok(())
    }

    fn unary_type_boundary_key(&mut self, sut: Noun, ref_: Noun) -> TypeBinaryBoundaryKey {
        // Shared by crop and fuse; both call repo() on %hold types, whose
        // unfolding depends on the active fan scope.
        let semantic = self.semantic_context_key();
        (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(ref_),
            semantic.vet_key,
            semantic.fan_context_key,
        )
    }

    /// C-final.4: native-re-keyed `crop` boundary cache. TYPE components
    /// (sut/ref) keyed on interned `Rc` pointer identity; the semantic fields
    /// (vet, fan) are carried VERBATIM from the old `unary_type_boundary_key`.
    /// VALUE is the native result type. No `live_to_noun` on store / `native_of`
    /// on hit.
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

    /// C-final.4: native-re-keyed `fuse` boundary cache. TYPE components
    /// (sut/ref) keyed on interned `Rc` pointer identity; the semantic fields
    /// (vet, fan) carried VERBATIM from the old `unary_type_boundary_key`.
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
        let key = (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(ref_),
            semantic.vet_key,
            fan,
        );
        let Some(bucket) = self.boundary_memo.redo.get(&key) else {
            return Ok(None);
        };
        let sut_raw = unsafe { sut.as_raw() };
        let ref_raw = unsafe { ref_.as_raw() };
        for entry in bucket.iter().rev() {
            let sut_match = unsafe { entry.sut.as_raw() } == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let ref_match = unsafe { entry.ref_.as_raw() } == ref_raw
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
        let key = (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(ref_),
            semantic.vet_key,
            fan,
        );
        let bucket = self
            .boundary_memo
            .redo
            .ensure_key(key, Self::REDO_CACHE_KEY_LIMIT);
        let sut_raw = unsafe { sut.as_raw() };
        let ref_raw = unsafe { ref_.as_raw() };
        for entry in bucket.iter() {
            let sut_match = unsafe { entry.sut.as_raw() } == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let ref_match = unsafe { entry.ref_.as_raw() } == ref_raw
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
        let key = (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(legs),
            semantic.vet_key,
            fan,
        );
        let Some(bucket) = self.boundary_memo.rest.get(&key) else {
            return Ok(None);
        };
        let sut_raw = unsafe { sut.as_raw() };
        let legs_raw = unsafe { legs.as_raw() };
        for entry in bucket.iter().rev() {
            let sut_match = unsafe { entry.sut.as_raw() } == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let legs_match = unsafe { entry.legs.as_raw() } == legs_raw
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
        let key = (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(legs),
            semantic.vet_key,
            fan,
        );
        let bucket = self
            .boundary_memo
            .rest
            .ensure_key(key, Self::REST_CACHE_KEY_LIMIT);
        let sut_raw = unsafe { sut.as_raw() };
        let legs_raw = unsafe { legs.as_raw() };
        for entry in bucket.iter() {
            let sut_match = unsafe { entry.sut.as_raw() } == sut_raw
                || noun_eq(entry.sut, sut, &self.slab.noun_space())?;
            let legs_match = unsafe { entry.legs.as_raw() } == legs_raw
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

    /// C-final.4: native-re-keyed `fish` boundary cache. The TYPE component (sut)
    /// is keyed on interned `Rc` pointer identity; (axis, vet, fan) carried
    /// VERBATIM from the old key. VALUE is the NOCK FORMULA (noun) — formula stays
    /// a noun, no `live_to_noun` of the deepening type for the key.
    fn fish_boundary_lookup(&mut self, sut: &NRc<NTy>, axis: u64) -> Result<Option<Noun>> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        Ok(native_fish_cache_lookup(
            &self.cx, sut, axis, semantic.vet_key, fan,
        ))
    }

    fn fish_boundary_store(&mut self, sut: &NRc<NTy>, axis: u64, result: Noun) -> Result<()> {
        let semantic = self.semantic_context_key();
        let fan = self.fan_context_key_scoped(sut)?;
        native_fish_cache_store(&mut self.cx, sut, axis, semantic.vet_key, fan, result);
        Ok(())
    }

    fn noun_mug_cached(&self, noun: Noun) -> u32 {
        // Prefer the mug cached on allocated nouns. This avoids building an ever-growing Rust
        // HashMap while compiling large inputs (hoon-138).
        let space = self.slab.noun_space();
        get_mug(noun, &space).unwrap_or_else(|| slab_mug(noun, &space))
    }

    fn nest_mug_lookup(&mut self, sut: Noun, ref_: Noun) -> Result<Option<bool>> {
        let semantic = self.semantic_context_key();
        let key = (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(ref_),
            semantic.vet_key,
            semantic.fan_context_key,
        );
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

    fn nest_mug_register(&mut self, sut: Noun, ref_: Noun, result: bool) {
        let semantic = self.semantic_context_key();
        let key = (
            self.noun_mug_cached(sut),
            self.noun_mug_cached(ref_),
            semantic.vet_key,
            semantic.fan_context_key,
        );
        let bucket = self
            .boundary_memo
            .nest
            .ensure_key(key, Self::NEST_MUG_KEY_LIMIT);
        if bucket.len() >= Self::NEST_MUG_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(NestCacheEntry { sut, ref_, result });
    }

    pub fn mint(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, gen: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        match self.mint_inner(sut, gol, gen) {
            Ok(value) => Ok(value),
            Err(err) => Err(self.decorate_error(err)),
        }
    }

    /// Noun-bridged `mint` for not-yet-flipped callers (C-final.1a): lift sut/gol
    /// to native, run native mint, lower the type slot (the formula stays noun).
    /// CAUTION: this body's `self.mint(...)` is the NATIVE mint (do NOT rename to
    /// self.mint_noun — that would be infinite self-recursion).
    pub fn mint_noun(&mut self, sut: Noun, gol: Noun, gen: &Hoon) -> Result<(Noun, Noun)> {
        let space = self.slab.noun_space();
        let sut_n = native_of(&mut self.cx, sut, &space)?;
        let gol_n = native_of(&mut self.cx, gol, &space)?;
        let (ty, formula) = self.mint(sut_n, gol_n, gen)?;
        let ty_noun = live_to_noun(&mut self.cx, &ty, self.slab);
        Ok((ty_noun, formula))
    }

    fn mint_inner(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, gen: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        // ATOMIC FLIP (C-final.1a): mint reads/returns the native enum (type slot;
        // the formula slot stays Noun). C-final.1b: the mint boundary cache is now
        // native-re-keyed on the interned (sut, gol) `Rc` pointers, so we no
        // longer lower sut/gol just to compute the cache key. C-final.2: play now
        // takes a native subject too, so mint no longer lowers sut for play.
        let cache_sig = Self::mint_cache_signature(gen);
        if let Some(gen_sig) = cache_sig {
            if let Some(cached) = self.mint_cache_lookup(&sut, &gol, gen_sig)? {
                if let Hoon::Dbug(spot, _) = gen {
                    self.record_semantic_type(spot, &cached.0);
                }
                return Ok(cached);
            }
        }

        // Canonical hoon-138 `++mint` short-circuit:
        // if subject is void (except direct `dbug`), allow only `%lost`/`%zpzp` under `vet`,
        // and return `[void [0 0]]`.
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
            return Ok((cons_void(&mut self.cx), slot_formula_axis(self.slab, 0)));
        }

        // Retain the native (sut, gol) `Rc`s for the post-match cache store; the
        // match below moves `sut`/`gol` into the per-arm recursion (Rc clone is
        // cheap — a refcount bump, NOT a structural copy).
        let cache_sut = sut.clone();
        let cache_gol = gol.clone();
        let result = match gen {
            Hoon::Pair(p, q) => {
                let gol_head = cons_noun(&mut self.cx);
                let (t_head, f_head) = self.mint(sut.clone(), gol_head, p)?;
                let gol_tail = cons_noun(&mut self.cx);
                let (t_tail, f_tail) = self.mint(sut.clone(), gol_tail, q)?;
                let ty = cons_cell(&mut self.cx, t_head, t_tail);
                let ty = self.nice(sut, gol, ty)?;
                let formula = cons(self.slab, f_head, f_tail)?;
                Ok((ty, formula))
            }
            Hoon::Rock(_, expr) | Hoon::Sand(_, expr) => {
                let ty = self.play(sut.clone(), gen)?;
                let ty = self.nice(sut, gol, ty)?;
                let value = noun_expr_to_noun(self.slab, expr);
                let formula = T(self.slab, &[D(1), value]);
                Ok((ty, formula))
            }
            Hoon::ZapZap | Hoon::Eror(_) => {
                let ty = cons_void(&mut self.cx);
                let ty = self.nice(sut, gol, ty)?;
                let formula = crash_formula(self.slab);
                Ok((ty, formula))
            }
            Hoon::Dbug(spot, inner) => self.mint_dbug(sut, gol, spot, inner),
            Hoon::Note(note, inner) => self.mint_note(sut, gol, note, inner),
            Hoon::Lost(_) => self.mint_lost(sut, gol),
            Hoon::TisGar(p, q) => self.mint_tsgr(sut, gol, p, q),
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
                    let formula = T(self.slab, &[D(0), D(0)]);
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
                let ty = self.peek(sut.clone(), Way::Free, *axis)?;
                let ty = self.nice(sut, gol, ty)?;
                let formula = slot_formula_axis(self.slab, *axis);
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
            // C-final.1b: store the native type directly (no lowering).
            self.mint_cache_store(&cache_sut, &cache_gol, gen_sig, result.0.clone(), result.1)?;
        }

        Ok(result)
    }

    pub fn slot_axis(noun: Noun, mut axis: u64, space: &NounSpace) -> Option<Noun> {
        if axis == 0 {
            return None;
        }
        let mut node = noun;
        let mut steps = Vec::new();
        while axis > 1 {
            steps.push((axis & 1) as u8);
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

    const SEMANTIC_FACT_LIMIT: usize = 100_000;

    fn record_semantic_type(&mut self, spot: &Spot, ty: &NRc<NTy>) {
        if !self.semantic_recording || self.semantic_type_facts.len() >= Self::SEMANTIC_FACT_LIMIT {
            return;
        }
        let location = Self::location_from_spot(spot);
        let Some(key) = Self::semantic_location_key(&location) else {
            return;
        };
        if !self.semantic_type_fact_keys.insert(key) {
            return;
        }
        self.semantic_type_facts.push(CompilerSemanticFact {
            location,
            type_summary: Self::semantic_type_summary(ty, 0),
        });
    }

    fn record_semantic_resolution(&mut self, name: &str, definition: Noun) {
        if !self.semantic_recording
            || self.semantic_resolution_facts.len() >= Self::SEMANTIC_FACT_LIMIT
        {
            return;
        }
        let Some(use_location) = self.dbug_locations.last().cloned() else {
            return;
        };
        let Some(use_key) = Self::semantic_location_key(&use_location) else {
            return;
        };
        let Some(definition_ast) = self.hoon_ast_lookup(definition) else {
            return;
        };
        let Some(definition_location) = Self::semantic_definition_location(&definition_ast) else {
            return;
        };
        let Some(definition_key) = Self::semantic_location_key(&definition_location) else {
            return;
        };
        let name = name.to_string();
        if !self
            .semantic_resolution_fact_keys
            .insert((use_key, definition_key, name.clone()))
        {
            return;
        }
        self.semantic_resolution_facts.push(CompilerResolutionFact {
            use_location,
            definition_location,
            name,
        });
    }

    fn semantic_definition_location(hoon: &Hoon) -> Option<CompilerErrorLocation> {
        match hoon {
            Hoon::Dbug(spot, _) => Some(Self::location_from_spot(spot)),
            Hoon::Note(_, inner) => Self::semantic_definition_location(inner),
            _ => None,
        }
    }

    fn semantic_location_key(location: &CompilerErrorLocation) -> Option<SemanticLocationKey> {
        Some((
            location.file.clone()?,
            location.start_line?,
            location.start_col?,
            location.end_line?,
            location.end_col?,
        ))
    }

    fn semantic_type_summary(ty: &NRc<NTy>, depth: usize) -> String {
        if depth >= 4 {
            return "…".to_string();
        }
        match &**ty {
            NTy::Void => "%void".to_string(),
            NTy::Noun => "*".to_string(),
            NTy::Atom { aura, .. } => Self::semantic_leaf_term(aura)
                .filter(|aura| !aura.is_empty())
                .map_or_else(|| "@".to_string(), |aura| format!("@{aura}")),
            NTy::Cell(head, tail) => format!(
                "[{} {}]",
                Self::semantic_type_summary(head, depth + 1),
                Self::semantic_type_summary(tail, depth + 1)
            ),
            NTy::Core { payload, garb, .. } => {
                let payload = Self::semantic_type_summary(payload, depth + 1);
                garb.nym.as_deref().map_or_else(
                    || format!("core({payload})"),
                    |name| format!("{name}({payload})"),
                )
            }
            NTy::Face { tool, inner } => {
                let inner = Self::semantic_type_summary(inner, depth + 1);
                Self::semantic_leaf_term(tool).map_or_else(
                    || format!("face({inner})"),
                    |name| format!("{name}={inner}"),
                )
            }
            NTy::Hint { payload, .. } => Self::semantic_type_summary(payload, depth + 1),
            NTy::Fork { .. } => "%fork".to_string(),
            NTy::Hold { .. } => "%hold".to_string(),
        }
    }

    fn semantic_leaf_term(leaf: &Leaf) -> Option<String> {
        let Leaf::Direct(value) = leaf else {
            return None;
        };
        let bytes = value.to_le_bytes();
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        std::str::from_utf8(&bytes[..end]).ok().map(str::to_string)
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

        if let Ok(cwd) = std::env::current_dir() {
            let cwd_components = cwd
                .components()
                .filter_map(|component| match component {
                    std::path::Component::Normal(segment) => {
                        Some(segment.to_string_lossy().into_owned())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !cwd_components.is_empty()
                && path.len() > cwd_components.len()
                && path.starts_with(cwd_components.as_slice())
            {
                return path[cwd_components.len()..].join("/");
            }
        }

        path.join("/")
    }

    pub fn play(&mut self, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        // Canonical ++play runs with vet disabled for the entire evaluation
        // scope; restore it afterward (safe save/restore — see with_vet_off).
        // ATOMIC FLIP (C-final.2): play TAKES native sut and RETURNS native
        // Rc<Type>. No compile path lowers the deepening subject to a noun for
        // play anymore — the subject threads natively (the O(N^2)->O(N) win).
        self.with_vet_off(|ut| ut.play_inner(sut, gen))
    }

    /// Noun-input/noun-output bridge for external callers that still hold a noun
    /// subject (binary prelude seed, fire, tests): native_of the noun sut, run
    /// native play, lower the native result back to a noun at the boundary.
    pub fn play_noun(&mut self, sut: Noun, gen: &Hoon) -> Result<Noun> {
        let sut = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let r = self.play(sut, gen)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    fn play_inner(&mut self, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        let result = {
            match gen {
                Hoon::Pair(p, q) => {
                    let head = self.play(sut.clone(), p)?;
                    let tail = self.play(sut, q)?;
                    Ok(cons_cell(&mut self.cx, head, tail))
                }
                Hoon::Rock(aura, expr) => Ok(self.play_rock(aura, expr)),
                Hoon::Sand(aura, expr) => self.play_sand(aura, expr),
                Hoon::ZapZap | Hoon::Eror(_) => Ok(cons_void(&mut self.cx)),
                Hoon::Dbug(_, inner) => self.play_dbug(sut, inner),
                Hoon::Note(note, inner) => self.play_note(sut, note, inner),
                Hoon::Lost(_) => Ok(cons_void(&mut self.cx)),
                Hoon::TisGar(p, q) => {
                    let next = self.play(sut, p)?;
                    self.play(next, q)
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
                    // feel is native (C9); thread the native subject directly.
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
                Hoon::Axis(axis) => {
                    // peek is native (C2); thread the native subject directly.
                    self.peek(sut, Way::Free, *axis)
                }
                Hoon::BarCen(prefix, tomes) => self.play_core(sut, prefix, tomes, Poly::Dry),
                Hoon::BarPat(prefix, tomes) => self.play_core(sut, prefix, tomes, Poly::Wet),
                _ => self.play_opened(sut, gen),
            }
        };
        result
    }

    fn mint_tsgr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let goal = cons_noun(&mut self.cx);
        let (p_ty, p_formula) = self.mint(sut, goal, p)?;
        let (q_ty, q_formula) = self.mint(p_ty, gol, q)?;
        let formula = comb(self.slab, p_formula, q_formula)?;
        Ok((q_ty, formula))
    }

    fn mint_brtis(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        match items {
            [] => {
                let zero = D(0);
                let ty = ty_atom_n(&mut self.cx, self.slab, "n", Some(zero)).1;
                let ty = self.nice(sut, gol, ty)?;
                let formula = T(self.slab, &[D(1), zero]);
                Ok((ty, formula))
            }
            [head, tail @ ..] => {
                let goal = cons_noun(&mut self.cx);
                let (head_ty, head_formula) = self.mint(sut.clone(), goal.clone(), head)?;
                let (tail_ty, tail_formula) = self.mint_colsig(sut.clone(), goal, tail)?;
                let ty = cons_cell(&mut self.cx, head_ty, tail_ty);
                let ty = self.nice(sut, gol, ty)?;
                let formula = cons(self.slab, head_formula, tail_formula)?;
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
                // `~%` clue wrapping (`%cofl`, etc.) nests the original arm body under `TisTar`.
                // Hold genes can point at the nested original body, so cache it explicitly.
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
        // gain/lose + play are native (C6/C-final); thread the native subject
        // directly. The branch RESULT types stay native and the fork is built
        // via cons_fork (RT-07 mug ordering preserved; mirrors mint_wtcl). The
        // deepening subject (sut) is never lowered.
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

    // Basically a ternary if-then-else but for Hoon AFAICT
    fn mint_wtcl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
        r: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let (_cond_ty, cond_formula) = self.mint(sut.clone(), bool_ty, p)?;
        // gain/lose are native now (C6); thread the native subject directly.
        let fex = self.gain(sut.clone(), p)?;
        let wux = self.lose(sut.clone(), p)?;
        let fex_void = matches!(&*fex, NTy::Void);
        let wux_void = matches!(&*wux, NTy::Void);
        let (ned, duy) = if fex_void && wux_void {
            // Canonical hoon-138 `%wtcl`: [%void %void] => [ned=0 duy=[%0 0]].
            (false, slot_formula_axis(self.slab, 0))
        } else if fex_void {
            (true, const_bool_formula(self.slab, false))
        } else if wux_void {
            (true, const_bool_formula(self.slab, true))
        } else {
            (false, cond_formula)
        };
        let (q_ty, q_formula) = self.mint(fex, gol.clone(), q)?;
        let (r_ty, r_formula) = self.mint(wux, gol, r)?;
        let fol = cond(self.slab, duy, q_formula, r_formula)?;
        // Native fork build (RT-07 mug ordering preserved via cons_fork).
        let ty = self.cons_fork(vec![q_ty, r_ty])?;
        let formula = if ned {
            let toss_tag = term_to_noun(self.slab, "toss");
            let toss = T(self.slab, &[toss_tag, cond_formula]);
            T(self.slab, &[D(11), toss, fol])
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // busk is native now (C-final.2): the subject threads through as a shared
        // native Rc inside the new %face — no lowering.
        let busked = self.busk(sut, p);
        self.mint(busked, gol, q)
    }

    fn busk(&mut self, sut: NRc<NTy>, gen: &Hoon) -> NRc<NTy> {
        // Canonical ++busk: [%face [~ [gen ~]] sut]. Native now: the subject
        // threads through as a SHARED native Rc inside the new %face (no lowering).
        let gen_noun = self.hoon_noun_for_node(gen);
        self.cache_hoon_ast_for_node(gen);
        let pair = T(self.slab, &[gen_noun, D(0)]);
        let tool = T(self.slab, &[D(0), pair]);
        let tool_leaf = NLeaf::from_noun_raw(tool, &self.slab.noun_space());
        cons_face(&mut self.cx, tool_leaf, sut)
    }

    fn mint_wtpm(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        list: &[Hoon],
    ) -> Result<(NRc<NTy>, Noun)> {
        let lowered = Self::lower_wtpm(list);
        self.mint(sut, gol, &lowered)
    }

    fn mint_wtbr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        list: &[Hoon],
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        let expanded = expand_wutsig(wing, q, r);
        self.mint(sut, gol, &expanded)
    }

    fn mint_wtgl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // play + find are native (C-final.2 / C9). fish takes a native type directly.
        let ref_type = self.play(sut.clone(), p)?;
        let port = self.find(sut.clone(), Way::Read, wing)?;
        let formula = match &port {
            // Match hoon-138 `%fits`: when `find` returns a leg, fish directly at that axis
            // rather than composing through `%7`.
            Port::Palo(palo) if matches!(palo.opal, Opal::Leg(_)) => {
                let axis = tend(&palo.vein)?;
                self.type_test_formula_on_axis(ref_type.clone(), axis)?
            }
            _ => {
                let (_ty, base_formula) = self.fine(&port)?;
                let test = self.type_test_formula_on_axis(ref_type.clone(), 1)?;
                // hoon-138 emits an explicit `%7` in this branch.
                T(self.slab, &[D(7), base_formula, test])
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // fend is native (C9). skin_match_static + skin_test_formula stay noun
        // (skin family): lower the hit type + the subject for them.
        let (hit_ty, axis) = self.fend(sut.clone(), Way::Read, wing)?;
        let hit_ty = live_to_noun(&mut self.cx, &hit_ty, self.slab);
        let static_match = self.skin_match_static(hit_ty, skin)?;
        let formula = if let Some(matches) = static_match {
            const_bool_formula(self.slab, matches)
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

        let head_ty = self.peek_noun(ty, Way::Free, 2)?;
        let tail_ty = self.peek_noun(ty, Way::Free, 3)?;
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // hoon-138 mint expansion:
        //   [%wtts p=spec q=wing] => [%fits ~(example ax p) q]
        let example = self.spec_example_cached(spec);
        self.mint_fits(sut, gol, example.as_ref(), wing)
    }

    fn base_test_formula(&mut self, base: &BaseType, slot: Noun) -> Result<Noun> {
        match base {
            BaseType::NounExpr => Ok(const_bool_formula(self.slab, true)),
            BaseType::Void => Ok(const_bool_formula(self.slab, false)),
            BaseType::Cell => Ok(T(self.slab, &[D(3), slot])),
            BaseType::Atom(_) => {
                let test = T(self.slab, &[D(3), slot]);
                let false_formula = const_bool_formula(self.slab, false);
                let true_formula = const_bool_formula(self.slab, true);
                cond(self.slab, test, false_formula, true_formula)
            }
            BaseType::Null => {
                let zero = T(self.slab, &[D(1), D(0)]);
                Ok(T(self.slab, &[D(5), zero, slot]))
            }
            BaseType::Flag => {
                let zero = T(self.slab, &[D(1), D(0)]);
                let one = T(self.slab, &[D(1), D(1)]);
                let eq_zero = T(self.slab, &[D(5), slot, zero]);
                let eq_one = T(self.slab, &[D(5), slot, one]);
                let atom_test = self.base_test_formula(&BaseType::Atom("$".to_string()), slot)?;
                let flag_value_test = flor_formula(self.slab, eq_zero, eq_one)?;
                flan_formula(self.slab, atom_test, flag_value_test)
            }
        }
    }

    fn type_test_formula_on_axis(&mut self, typ: NRc<NTy>, axis: u64) -> Result<Noun> {
        // ATOMIC FLIP (consumer C7): fish reads the native type enum. The RETURN
        // is a NOCK FORMULA (noun), not a type. C-final.4: the fish boundary cache
        // is native-re-keyed on the interned `Rc` pointer of `typ`, so the
        // deepening subject is no longer lowered to a noun here. Deepening children
        // stay native; leaf-carried parts (atom value, coil via repo) lowered.
        if let Some(cached) = self.fish_boundary_lookup(&typ, axis)? {
            return Ok(cached);
        }
        let mut seen_holds: Vec<NRc<NTy>> = Vec::new();
        let result = self.type_test_formula_on_axis_inner(typ.clone(), axis, &mut seen_holds)?;
        self.fish_boundary_store(&typ, axis, result)?;
        Ok(result)
    }

    fn type_test_formula_on_axis_inner(
        &mut self,
        typ: NRc<NTy>,
        axis: u64,
        seen_holds: &mut Vec<NRc<NTy>>,
    ) -> Result<Noun> {
        match &*typ {
            NTy::Void => Ok(const_bool_formula(self.slab, false)),
            NTy::Noun => Ok(const_bool_formula(self.slab, true)),
            NTy::Atom { .. } => {
                // The atom value is a (small) carried leaf; lower the whole type
                // and decode via the existing noun helper.
                let typ_noun = live_to_noun(&mut self.cx, &typ, self.slab);
                let (_aura, value) = type_atom_parts(typ_noun, &self.slab.noun_space())?;
                let slot = slot_formula_axis(self.slab, axis);
                if let Some(value) = value {
                    let const_value = T(self.slab, &[D(1), value]);
                    Ok(T(self.slab, &[D(5), const_value, slot]))
                } else {
                    // Canonical ++fish: unknown atom narrows via flip([3 [0 axis]]).
                    let is_cell = T(self.slab, &[D(3), slot]);
                    flip_formula(self.slab, is_cell)
                }
            }
            NTy::Cell(head, tail) => {
                let head = head.clone();
                let tail = tail.clone();
                let slot = slot_formula_axis(self.slab, axis);
                let is_cell = T(self.slab, &[D(3), slot]);
                let head_formula =
                    self.type_test_formula_on_axis_inner(head, peg_axis(axis, 2)?, seen_holds)?;
                let tail_formula =
                    self.type_test_formula_on_axis_inner(tail, peg_axis(axis, 3)?, seen_holds)?;
                let both = flan_formula(self.slab, head_formula, tail_formula)?;
                flan_formula(self.slab, is_cell, both)
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

    fn skin_test_formula(&mut self, sut: Noun, axis: u64, skin: &Skin) -> Result<Noun> {
        let ref_type = self.peek_noun(sut, Way::Free, axis)?;
        if let Some(matches) = self.skin_match_static(ref_type, skin)? {
            return Ok(const_bool_formula(self.slab, matches));
        }

        let slot = slot_formula_axis(self.slab, axis);
        match skin {
            Skin::Base(BaseType::Flag) => {
                let atom_skin = Skin::Base(BaseType::Atom("$".to_string()));
                let atom_test = self.skin_test_formula(sut, axis, &atom_skin)?;
                let zero = T(self.slab, &[D(1), D(0)]);
                let one = T(self.slab, &[D(1), D(1)]);
                let eq_zero = T(self.slab, &[D(5), slot, zero]);
                let eq_one = T(self.slab, &[D(5), slot, one]);
                let flag_value_test = flor_formula(self.slab, eq_zero, eq_one)?;
                flan_formula(self.slab, atom_test, flag_value_test)
            }
            Skin::Base(base) => self.base_test_formula(base, slot),
            Skin::Leaf(_aura, atom) => {
                let value = parsed_atom_to_noun(self.slab, atom);
                let const_val = T(self.slab, &[D(1), value]);
                Ok(T(self.slab, &[D(5), const_val, slot]))
            }
            Skin::Cell(head, tail) => {
                let is_cell = T(self.slab, &[D(3), slot]);
                let head_axis = peg_axis(axis, 2)?;
                let tail_axis = peg_axis(axis, 3)?;
                let head_test = self.skin_test_formula(sut, head_axis, head)?;
                let tail_test = self.skin_test_formula(sut, tail_axis, tail)?;
                let both = and_formula(self.slab, head_test, tail_test)?;
                let false_formula = const_bool_formula(self.slab, false);
                cond(self.slab, is_cell, both, false_formula)
            }
            Skin::Dbug(_, inner) => self.skin_test_formula(sut, axis, inner),
            Skin::Help(_, inner) => self.skin_test_formula(sut, axis, inner),
            Skin::Name(_, inner) => self.skin_test_formula(sut, axis, inner),
            Skin::Over(wing, inner) => {
                let rel_axis = self.resolve_wing_axis_noun(sut, wing)?;
                let axis = peg_axis(axis, rel_axis)?;
                self.skin_test_formula(sut, axis, inner)
            }
            Skin::Spec(spec, inner) => {
                let ref_type = self.peek_noun(sut, Way::Free, axis)?;
                let example = self.spec_example_cached(spec);
                let hit = self.play_noun(sut, example.as_ref())?;
                if !self.nest_noun(hit, ref_type)? {
                    return Err(CompilerError::Noun("native mint: wthx spec".to_string()));
                }
                self.skin_test_formula(sut, axis, inner)
            }
            Skin::Wash(_) => Ok(const_bool_formula(self.slab, true)),
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
        axis: u64,
        seen_holds: &mut Vec<NRc<NTy>>,
    ) -> Result<Noun> {
        let Some((head, tail)) = options.split_first() else {
            return Ok(const_bool_formula(self.slab, false));
        };
        let head_formula = self.type_test_formula_on_axis_inner(head.clone(), axis, seen_holds)?;
        let tail_formula = self.type_test_formula_on_axis_fork(tail, axis, seen_holds)?;
        flor_formula(self.slab, head_formula, tail_formula)
    }

    fn mint_wtls(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        default: &Hoon,
        list: &[(Spec, Hoon)],
    ) -> Result<(NRc<NTy>, Noun)> {
        // Match hoon-138 open() lowering:
        //   [%wtls p=wing q=default r=list] => [%wthp p (weld r [[[%base %noun] q] ~])]
        let expanded = expand_wutlus(wing, default, list);
        self.mint(sut, gol, &expanded)
    }

    // Ternary type match conditional
    fn mint_wthp(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
        list: &[(Spec, Hoon)],
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // Canonical hoon-138 open() lowering:
        //   [%kthp p q] => [%ktls ~(example ax p) q]
        let example = self.spec_example_cached(spec);
        let hif = self.play(sut.clone(), example.as_ref())?;
        let hif = self.nice(sut.clone(), gol, hif)?;
        let (_q_ty, q_formula) = self.mint(sut, hif.clone(), q)?;
        Ok((hif, q_formula))
    }

    fn mint_kttr(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, spec: &Spec) -> Result<(NRc<NTy>, Noun)> {
        // Canonical hoon-138 open() lowering:
        //   [%kttr p] => [%ktsg ~(example ax p)]
        let example = self.spec_example_cached(spec);
        self.mint_ktsg(sut, gol, example.as_ref())
    }

    fn mint_ktcl(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, spec: &Spec) -> Result<(NRc<NTy>, Noun)> {
        let opened = self.spec_factory_open_cached(spec);
        self.mint(sut, gol, opened.as_ref())
    }

    fn mint_ktsg(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, p: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        self.blow_ktsg(sut, gol, p)
    }

    fn blow_ktsg(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, gen: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        // Canonical hoon-138:
        //   pro=(mint gol gen)
        //   jon=(apex:musk bran q.pro)
        //   if jon is ~ or %wait => keep q.pro
        //   else rewrite to %1 noun
        let (ty, formula) = self.mint(sut.clone(), gol, gen)?;
        // bran_canonical_semi is native (Phase-2 tail): thread the native subject
        // directly — no `live_to_noun` of the deepening subject.
        let bran = self.bran_canonical_semi(sut)?;
        let fold_key = (self.noun_mug_cached(bran), self.noun_mug_cached(formula));
        if let Some(entries) = self.ktsg_fold_cache.get(&fold_key) {
            let entries: Vec<KtsgFoldCacheEntry> = entries.iter().copied().collect();
            for entry in entries.into_iter().rev() {
                let space = self.slab.noun_space();
                let hit = (unsafe { entry.bran.raw_equals(&bran) }
                    || noun_eq(entry.bran, bran, &space)?)
                    && (unsafe { entry.formula.raw_equals(&formula) }
                        || noun_eq(entry.formula, formula, &space)?);
                if hit {
                    return Ok(match entry.result {
                        Some(noun) => (ty, T(self.slab, &[D(1), noun])),
                        None => (ty, formula),
                    });
                }
            }
        }
        let jon = self.musk_apex_output(bran, formula)?;
        let collapsed = match jon {
            MuskOutput::Done(noun) => Some(noun),
            MuskOutput::Stop | MuskOutput::Wait => None,
        };
        self.ktsg_fold_cache_store(fold_key, bran, formula, collapsed);
        if let Some(noun) = collapsed {
            return Ok((ty, T(self.slab, &[D(1), noun])));
        }
        Ok((ty, formula))
    }

    fn ktsg_fold_cache_store(
        &mut self,
        key: (u32, u32),
        bran: Noun,
        formula: Noun,
        result: Option<Noun>,
    ) {
        let bucket = self
            .ktsg_fold_cache
            .ensure_key(key, Self::MUSK_MACK_CACHE_KEY_LIMIT);
        if bucket.len() >= Self::MUSK_MACK_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(KtsgFoldCacheEntry {
            bran,
            formula,
            result,
        });
    }

    fn semi_mask_full(&mut self, blocks: Noun) -> Noun {
        if noun_is_zero(blocks) {
            return self.semi_mask_full_empty_interned();
        }
        T(self.slab, &[D(SEMI_TAG_FULL), blocks])
    }

    fn semi_mask_half(&mut self, left: Noun, right: Noun) -> Noun {
        T(self.slab, &[D(SEMI_TAG_HALF), left, right])
    }

    fn semi_mask_lazy(&mut self, fragment: u64, resolve: Noun) -> Noun {
        let frag = noun_u64(self.slab, fragment);
        T(self.slab, &[D(SEMI_TAG_LAZY), frag, resolve])
    }

    fn semi_make(&mut self, mask: Noun, data: Noun) -> Noun {
        T(self.slab, &[mask, data])
    }

    /// The `[%full ~]` mask and the fully-blocked seminoun are by far the
    /// most constructed nouns on musk's evaluation path; intern one copy of
    /// each per Ut. Nouns are immutable, so sharing is sound, and the stable
    /// identities improve raw-pointer memo hit rates.
    fn semi_mask_full_empty_interned(&mut self) -> Noun {
        if let Some(mask) = self.semi_mask_full_empty {
            return mask;
        }
        let mask = T(self.slab, &[D(SEMI_TAG_FULL), D(0)]);
        self.semi_mask_full_empty = Some(mask);
        mask
    }

    fn semi_full_complete(&mut self, data: Noun) -> Noun {
        let mask = self.semi_mask_full_empty_interned();
        self.semi_make(mask, data)
    }

    fn semi_blocks_root_blocked(&mut self) -> Noun {
        if let Some(blocks) = self.semi_root_blocked_set {
            return blocks;
        }
        let blocks = T(self.slab, &[D(0), D(0), D(0)]);
        self.semi_root_blocked_set = Some(blocks);
        blocks
    }

    fn semi_full_blocked(&mut self) -> Noun {
        // hoon-138 `*seminoun` and `++complete` use the singleton root
        // block set `[~ ~ ~]` for a fully blocked noun.
        if let Some(semi) = self.semi_full_blocked_interned {
            return semi;
        }
        let blocks = self.semi_blocks_root_blocked();
        let mask = self.semi_mask_full(blocks);
        let semi = self.semi_make(mask, D(0));
        self.semi_full_blocked_interned = Some(semi);
        semi
    }

    fn semi_lazy_root(&mut self, resolver_id: u64) -> Noun {
        let resolve = noun_u64(self.slab, resolver_id);
        let mask = self.semi_mask_lazy(1, resolve);
        self.semi_make(mask, D(0))
    }

    fn lazy_resolver_new_id(&mut self) -> u64 {
        let id = self.lazy_resolver_next_id;
        self.lazy_resolver_next_id = self.lazy_resolver_next_id.wrapping_add(1);
        if self.lazy_resolver_next_id == 0 {
            self.lazy_resolver_next_id = 1;
        }
        id
    }

    /// RT-05 canonical lazy-core identity: return ONE stable resolver id per
    /// structurally-equal recursive core so `cons_core` interns their lazy forms to
    /// a single `Rc` and every pointer-keyed recursion cut (mint_cache,
    /// arm_goal_for_hoon_in_progress, fond hold_path, fish, bran) converges. The
    /// lazy core's structural identity is exactly `(sut, garb, tomes_map)`, i.e.
    /// `(Rc::as_ptr(sut), tomes_sig, poly)` — `garb = f(prefix, poly)` folds into
    /// `tomes_sig` (which already xors `prefix_signature`), and the resolver this id
    /// keys is fully determined by `(core_type, poly, arms-from-tomes)` (the lazy
    /// callback goal is always `%noun`, `vet` is read at resolve time, and `gol` is
    /// not captured). A fresh monotonic id per call made structurally-equal lazy
    /// cores pointer-distinct, so the deepening subject churned and the cuts only
    /// fired at the redo-gil backstop depth instead of the natural settling depth —
    /// O(depth) redundant Type<->Noun boundary work per recursive type. The id is
    /// transient (completed cores carry an id-free `[%full ~]` semi), so reusing it
    /// never changes emitted bytes. `sut` is interned (held by the TypeTable for the
    /// whole compile), so `Rc::as_ptr(sut)` is a stable structural key.
    fn lazy_resolver_canonical_id(&mut self, sut: &NRc<NTy>, tomes_sig: u64, poly: Poly) -> u64 {
        let poly_key = match poly {
            Poly::Dry => 0u8,
            Poly::Wet => 1u8,
        };
        let key = (NRc::as_ptr(sut) as usize, tomes_sig, poly_key);
        if let Some(&id) = self.lazy_resolver_canonical_ids.get(&key) {
            return id;
        }
        let id = self.lazy_resolver_new_id();
        self.lazy_resolver_canonical_ids.insert(key, id);
        id
    }

    fn lazy_resolver_register_context(
        &mut self,
        resolver_id: u64,
        core_type: NRc<NTy>,
        poly: Poly,
        arms_by_axis: HashMap<u64, LazyResolverArmEntry>,
    ) {
        // Lazy resolvers live for the whole compile and are resolved on demand
        // (re-minting an arm against `core_type`), including CROSS-ARM: another
        // core's arm can reference this one. The native deepening `core_type` is a
        // heap `Rc` and the arm AST nouns live for the whole compile in the single
        // slab (no per-arm frame recycles them), so nothing needs relocation here.
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
        resolver_id: u64,
        fragment: u64,
    ) -> Result<Option<Noun>> {
        // hoon-138 `++laze`: the resolver answers exact arm axes only
        // (`(~(get by tal) axe)`); any other fragment — including 1, the
        // whole battery — produces `~` and the caller treats it as blocked.
        // Completed cores never reach here: `mint_core` stores a
        // materialized `[[%full ~] battery]` semi in the result type.
        if let Some(ctx) = self.lazy_resolvers.get(&resolver_id) {
            if let Some(cached) = ctx.cached_formula_by_axis.get(&fragment) {
                return Ok(Some(*cached));
            }
            if ctx.arms_by_axis.contains_key(&fragment) {
                return self.lazy_resolver_compile_arm(resolver_id, fragment);
            }
        }
        Ok(None)
    }

    fn lazy_resolver_compile_arm(
        &mut self,
        resolver_id: u64,
        fragment: u64,
    ) -> Result<Option<Noun>> {
        let Some((core_type, poly, arm_entry)) = ({
            let Some(ctx) = self.lazy_resolvers.get_mut(&resolver_id) else {
                return Ok(None);
            };
            if let Some(cached) = ctx.cached_formula_by_axis.get(&fragment) {
                return Ok(Some(*cached));
            }
            if !ctx.in_progress_axes.insert(fragment) {
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
        // ATOMIC FLIP perf: the lazy callback goal is native `%noun` (== ty_noun);
        // build it natively so it threads straight to mint with no re-lift.
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
                // Cache the formula (in the compile slab) for the whole compile.
                // The cache is read CROSS-ARM (another core resolving this one),
                // and re-minting on a cache miss is NOT safe: it would run in the
                // caller's %hold/fan scope, not this arm's, producing a wrong type.
                // Caching the formula keeps the resolver a pure lookup.
                if let Some(ctx) = self.lazy_resolvers.get_mut(&resolver_id) {
                    ctx.cached_formula_by_axis.insert(fragment, formula);
                }
                Ok(Some(formula))
            }
            Err(err) => Err(err),
        }
    }

    fn semi_parts(&mut self, semi: Noun) -> Result<(Noun, Noun)> {
        let space = self.slab.noun_space();
        let cell = semi
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("seminoun not cell: {err}")))?;
        Ok((cell.head().noun(), cell.tail().noun()))
    }

    fn semi_mask_tag(&mut self, mask: Noun) -> Result<SemiTag> {
        let space = self.slab.noun_space();
        let cell = mask
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("semi mask not cell: {err}")))?;
        let atom = cell
            .head()
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("semi mask tag not atom: {err}")))?;
        Ok(match atom.as_u64() {
            Ok(SEMI_TAG_FULL) => SemiTag::Full,
            Ok(SEMI_TAG_HALF) => SemiTag::Half,
            Ok(SEMI_TAG_LAZY) => SemiTag::Lazy,
            _ => SemiTag::Other,
        })
    }

    fn semi_mask_full_blocks(&mut self, mask: Noun) -> Result<Noun> {
        let space = self.slab.noun_space();
        let cell = mask
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("semi full mask not cell: {err}")))?;
        Ok(cell.tail().noun())
    }

    fn semi_mask_half_parts(&mut self, mask: Noun) -> Result<(Noun, Noun)> {
        let space = self.slab.noun_space();
        let cell = mask
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("semi half mask not cell: {err}")))?;
        let tail = cell
            .tail()
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("semi half mask tail not cell: {err}")))?;
        Ok((tail.head().noun(), tail.tail().noun()))
    }

    fn semi_mask_lazy_parts(&mut self, mask: Noun) -> Result<(u64, Noun)> {
        let space = self.slab.noun_space();
        let cell = mask
            .in_space(&space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("semi lazy mask not cell: {err}")))?;
        let tail = cell
            .tail()
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("semi lazy mask tail not cell: {err}")))?;
        let fragment = tail
            .head()
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("semi lazy fragment not atom: {err}")))?;
        let fragment = fragment
            .as_u64()
            .map_err(|err| CompilerError::Decode(format!("semi lazy fragment decode: {err}")))?;
        Ok((fragment, tail.tail().noun()))
    }

    fn semi_blocks_union_coarse(&mut self, left: Noun, right: Noun) -> Noun {
        if noun_is_zero(left) && noun_is_zero(right) {
            D(0)
        } else {
            self.semi_blocks_root_blocked()
        }
    }

    fn semi_combine(&mut self, hed: Noun, tal: Noun) -> Result<Noun> {
        let (hed_mask, hed_data) = self.semi_parts(hed)?;
        let (tal_mask, tal_data) = self.semi_parts(tal)?;
        let hed_tag = self.semi_mask_tag(hed_mask)?;
        let tal_tag = self.semi_mask_tag(tal_mask)?;
        if hed_tag == SemiTag::Full && tal_tag == SemiTag::Full {
            let hed_blocks = self.semi_mask_full_blocks(hed_mask)?;
            let tal_blocks = self.semi_mask_full_blocks(tal_mask)?;
            let hed_empty = noun_is_zero(hed_blocks);
            let tal_empty = noun_is_zero(tal_blocks);
            if hed_empty == tal_empty {
                if hed_empty {
                    let data = T(self.slab, &[hed_data, tal_data]);
                    return Ok(self.semi_full_complete(data));
                }
                let blocks = self.semi_blocks_union_coarse(hed_blocks, tal_blocks);
                let mask = self.semi_mask_full(blocks);
                return Ok(self.semi_make(mask, D(0)));
            }
        }
        let mask = self.semi_mask_half(hed_mask, tal_mask);
        let data = T(self.slab, &[hed_data, tal_data]);
        Ok(self.semi_make(mask, data))
    }

    fn semi_squash_mask(&mut self, mask: Noun) -> Result<Noun> {
        match self.semi_mask_tag(mask)? {
            SemiTag::Full => self.semi_mask_full_blocks(mask),
            SemiTag::Half => {
                let (left, right) = self.semi_mask_half_parts(mask)?;
                let left_blocks = self.semi_squash_mask(left)?;
                let right_blocks = self.semi_squash_mask(right)?;
                Ok(self.semi_blocks_union_coarse(left_blocks, right_blocks))
            }
            SemiTag::Lazy => {
                let lazy_bus = self.semi_make(mask, D(0));
                let completed = self.semi_complete(lazy_bus)?;
                let (next_mask, _next_data) = self.semi_parts(completed)?;
                self.semi_squash_mask(next_mask)
            }
            _ => Ok(self.semi_blocks_root_blocked()),
        }
    }

    fn semi_complete(&mut self, bus: Noun) -> Result<Noun> {
        let (mask, data) = self.semi_parts(bus)?;
        match self.semi_mask_tag(mask)? {
            SemiTag::Full => Ok(bus),
            SemiTag::Lazy => {
                let (fragment, resolve) = self.semi_mask_lazy_parts(mask)?;
                // hoon-138 `++complete`: "fragment 1 is the whole thing;
                // blocked; we can't get fragment 1 while compiling it."
                if fragment == 1 {
                    return Ok(self.semi_full_blocked());
                }
                let resolver_id = {
                    let space = self.slab.noun_space();
                    match resolve
                        .in_space(&space)
                        .as_atom()
                        .and_then(|atom| atom.as_u64())
                    {
                        Ok(id) => id,
                        Err(_) => return Ok(self.semi_full_blocked()),
                    }
                };
                match self.lazy_resolver_resolve_axis(resolver_id, fragment)? {
                    Some(value) => Ok(self.semi_full_complete(value)),
                    None => Ok(self.semi_full_blocked()),
                }
            }
            SemiTag::Half => {
                let (left_mask, right_mask) = self.semi_mask_half_parts(mask)?;
                let (data_head, data_tail) = {
                    let space = self.slab.noun_space();
                    let data_cell = match data.in_space(&space).as_cell() {
                        Ok(cell) => cell,
                        Err(_) => return Ok(self.semi_full_blocked()),
                    };
                    (data_cell.head().noun(), data_cell.tail().noun())
                };
                let left = self.semi_make(left_mask, data_head);
                let right = self.semi_make(right_mask, data_tail);
                let left = self.semi_complete(left)?;
                let right = self.semi_complete(right)?;
                self.semi_combine(left, right)
            }
            _ => Ok(self.semi_full_blocked()),
        }
    }

    fn semi_fragment(&mut self, axe: u64, bus: Noun) -> Result<Option<Noun>> {
        if axe == 1 {
            return Ok(Some(bus));
        }
        if axe == 0 {
            return Ok(None);
        }
        let (now, lat) = axis_cap_mas(axe)?;
        let (mask, data) = self.semi_parts(bus)?;
        match self.semi_mask_tag(mask)? {
            SemiTag::Lazy => {
                let (fragment, resolve) = self.semi_mask_lazy_parts(mask)?;
                let next_fragment = peg_axis(fragment, axe)?;
                let next_mask = self.semi_mask_lazy(next_fragment, resolve);
                Ok(Some(self.semi_make(next_mask, data)))
            }
            SemiTag::Full => {
                let blocks = self.semi_mask_full_blocks(mask)?;
                if !noun_is_zero(blocks) {
                    return Ok(Some(bus));
                }
                let next_data = {
                    let space = self.slab.noun_space();
                    let data_cell = match data.in_space(&space).as_cell() {
                        Ok(cell) => cell,
                        Err(_) => return Ok(None),
                    };
                    if now == 2 {
                        data_cell.head().noun()
                    } else {
                        data_cell.tail().noun()
                    }
                };
                let next = self.semi_full_complete(next_data);
                self.semi_fragment(lat, next)
            }
            SemiTag::Half => {
                let (left_mask, right_mask) = self.semi_mask_half_parts(mask)?;
                let (next_mask, next_data) = {
                    let space = self.slab.noun_space();
                    let data_cell = match data.in_space(&space).as_cell() {
                        Ok(cell) => cell,
                        Err(_) => return Ok(None),
                    };
                    if now == 2 {
                        (left_mask, data_cell.head().noun())
                    } else {
                        (right_mask, data_cell.tail().noun())
                    }
                };
                let next = self.semi_make(next_mask, next_data);
                self.semi_fragment(lat, next)
            }
            _ => Ok(None),
        }
    }

    fn semi_fragment_big(&mut self, axe: &BigUint, bus: Noun) -> Result<Option<Noun>> {
        if let Ok(small) = u64::try_from(axe) {
            return self.semi_fragment(small, bus);
        }
        if axe == &BigUint::from(0u32) {
            return Ok(None);
        }
        if axe == &BigUint::from(1u32) {
            return Ok(Some(bus));
        }
        let (now, lat) = axis_big_cap_mas(axe)?;
        let (mask, data) = self.semi_parts(bus)?;
        match self.semi_mask_tag(mask)? {
            SemiTag::Lazy => {
                let (fragment, resolve) = self.semi_mask_lazy_parts(mask)?;
                let next_fragment = peg_axis_big_pair(BigUint::from(fragment), axe)?;
                if let Ok(next_fragment) = u64::try_from(&next_fragment) {
                    let next_mask = self.semi_mask_lazy(next_fragment, resolve);
                    Ok(Some(self.semi_make(next_mask, data)))
                } else {
                    // Canonical `++laze` maps only declared battery axes.  A
                    // virtual fragment outside the native axis range cannot be
                    // resolved by the native arm map, so preserve the partial
                    // interpreter contract by treating it as blocked rather
                    // than as a malformed formula.
                    Ok(Some(self.semi_full_blocked()))
                }
            }
            SemiTag::Full => {
                let blocks = self.semi_mask_full_blocks(mask)?;
                if !noun_is_zero(blocks) {
                    return Ok(Some(bus));
                }
                let next_data = {
                    let space = self.slab.noun_space();
                    let data_cell = match data.in_space(&space).as_cell() {
                        Ok(cell) => cell,
                        Err(_) => return Ok(None),
                    };
                    if now == 2 {
                        data_cell.head().noun()
                    } else {
                        data_cell.tail().noun()
                    }
                };
                let next = self.semi_full_complete(next_data);
                self.semi_fragment_big(&lat, next)
            }
            SemiTag::Half => {
                let (left_mask, right_mask) = self.semi_mask_half_parts(mask)?;
                let (next_mask, next_data) = {
                    let space = self.slab.noun_space();
                    let data_cell = match data.in_space(&space).as_cell() {
                        Ok(cell) => cell,
                        Err(_) => return Ok(None),
                    };
                    if now == 2 {
                        (left_mask, data_cell.head().noun())
                    } else {
                        (right_mask, data_cell.tail().noun())
                    }
                };
                let next = self.semi_make(next_mask, next_data);
                self.semi_fragment_big(&lat, next)
            }
            _ => Ok(None),
        }
    }

    fn semi_mutate(&mut self, axe: u64, lit: Noun, big: Noun) -> Result<Option<Noun>> {
        if axe == 0 {
            return Ok(None);
        }
        if axe == 1 {
            return Ok(Some(lit));
        }
        if axe == 2 {
            let Some(tal) = self.semi_fragment(3, big)? else {
                return Ok(None);
            };
            return self.semi_combine(lit, tal).map(Some);
        }
        if axe == 3 {
            let Some(hed) = self.semi_fragment(2, big)? else {
                return Ok(None);
            };
            return self.semi_combine(hed, lit).map(Some);
        }
        let (cap, mor) = axis_cap_mas(axe)?;
        let Some(hed) = self.semi_fragment(2, big)? else {
            return Ok(None);
        };
        let Some(tal) = self.semi_fragment(3, big)? else {
            return Ok(None);
        };
        if cap == 2 {
            let Some(mutated) = self.semi_mutate(mor, lit, hed)? else {
                return Ok(None);
            };
            return self.semi_combine(mutated, tal).map(Some);
        }
        let Some(mutated) = self.semi_mutate(mor, lit, tal)? else {
            return Ok(None);
        };
        self.semi_combine(hed, mutated).map(Some)
    }

    fn semi_mutate_big(&mut self, axe: &BigUint, lit: Noun, big: Noun) -> Result<Option<Noun>> {
        if let Ok(small) = u64::try_from(axe) {
            return self.semi_mutate(small, lit, big);
        }
        if axe == &BigUint::from(0u32) {
            return Ok(None);
        }
        if axe == &BigUint::from(1u32) {
            return Ok(Some(lit));
        }
        let (cap, mor) = axis_big_cap_mas(axe)?;
        let Some(hed) = self.semi_fragment(2, big)? else {
            return Ok(None);
        };
        let Some(tal) = self.semi_fragment(3, big)? else {
            return Ok(None);
        };
        if cap == 2 {
            let Some(mutated) = self.semi_mutate_big(&mor, lit, hed)? else {
                return Ok(None);
            };
            return self.semi_combine(mutated, tal).map(Some);
        }
        let Some(mutated) = self.semi_mutate_big(&mor, lit, tal)? else {
            return Ok(None);
        };
        self.semi_combine(hed, mutated).map(Some)
    }

    fn semi_require<F>(&mut self, noy: Option<Noun>, yen: F) -> Result<Option<Noun>>
    where
        F: FnOnce(&mut Self, Noun) -> Result<Option<Noun>>,
    {
        let Some(noy) = noy else {
            return Ok(None);
        };
        let bus = self.semi_complete(noy)?;
        let (mask, data) = self.semi_parts(bus)?;
        match self.semi_mask_tag(mask)? {
            SemiTag::Half => {
                let blocks = self.semi_squash_mask(mask)?;
                let wait_mask = self.semi_mask_full(blocks);
                Ok(Some(self.semi_make(wait_mask, D(0))))
            }
            SemiTag::Full => {
                let blocks = self.semi_mask_full_blocks(mask)?;
                if !noun_is_zero(blocks) {
                    return Ok(Some(self.semi_make(mask, D(0))));
                }
                yen(self, data)
            }
            SemiTag::Lazy => Ok(Some(self.semi_full_blocked())),
            _ => Ok(None),
        }
    }

    fn musk_araw(
        &mut self,
        bus: Noun,
        fol: Noun,
        memo: &mut FastHashMap<(u64, u64), Noun>,
    ) -> Result<Option<Noun>> {
        let key = unsafe { (bus.as_raw(), fol.as_raw()) };
        if let Some(cached) = memo.get(&key) {
            return Ok(Some(*cached));
        }
        let result = self.musk_araw_uncached(bus, fol, memo)?;
        if let Some(noun) = result {
            memo.insert(key, noun);
        }
        Ok(result)
    }

    fn musk_araw_uncached(
        &mut self,
        bus: Noun,
        fol: Noun,
        memo: &mut FastHashMap<(u64, u64), Noun>,
    ) -> Result<Option<Noun>> {
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
                    ut.musk_araw(lub, ryf, memo)
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
                if let Some(core) = self.semi_full_complete_data(one)? {
                    if let Some(axis) = axis_small {
                        let Some(value) = self.musk_mack_constant_core(core, axis)? else {
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
                let partial = self.semi_require(frag, |ut, ryf| ut.musk_araw(one, ryf, memo))?;
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
        one: Noun,
        axis: u64,
        memo: &mut FastHashMap<(u64, u64), Noun>,
    ) -> Result<Option<Noun>> {
        let frag = self.semi_fragment(axis, one)?;
        let partial = self.semi_require(frag, |ut, ryf| ut.musk_araw(one, ryf, memo))?;
        Ok(partial)
    }

    fn musk_araw_arm_partial_big(
        &mut self,
        one: Noun,
        axis: &BigUint,
        memo: &mut FastHashMap<(u64, u64), Noun>,
    ) -> Result<Option<Noun>> {
        let frag = self.semi_fragment_big(axis, one)?;
        self.semi_require(frag, |ut, ryf| ut.musk_araw(one, ryf, memo))
    }

    fn semi_full_complete_data(&mut self, semi: Noun) -> Result<Option<Noun>> {
        let (mask, data) = self.semi_parts(semi)?;
        if self.semi_mask_tag(mask)? != SemiTag::Full {
            return Ok(None);
        }
        let blocks = self.semi_mask_full_blocks(mask)?;
        if noun_is_zero(blocks) {
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    fn musk_mack_constant_core(&mut self, core: Noun, axis: u64) -> Result<Option<Noun>> {
        if Self::slot_axis(core, axis, &self.slab.noun_space()).is_none() {
            return Ok(None);
        }

        let raw_key = (unsafe { core.as_raw() }, axis);
        if let Some(result) = self.musk.mack_cache_raw.get(&raw_key).copied() {
            return Ok(result);
        }

        let key = (self.noun_mug_cached(core), axis);
        if let Some(entries) = self.musk.mack_cache.buckets.get(&key) {
            let space = self.slab.noun_space();
            for entry in entries.iter().rev() {
                if unsafe { entry.core.raw_equals(&core) } || noun_eq(entry.core, core, &space)? {
                    self.musk.mack_cache_raw.insert(raw_key, entry.result);
                    return Ok(entry.result);
                }
            }
        }

        let result = self.musk_interpret_mack(core, axis);
        self.musk.mack_cache_raw.insert(raw_key, result);
        let bucket = self
            .musk
            .mack_cache
            .ensure_key(key, Self::MUSK_MACK_CACHE_KEY_LIMIT);
        if bucket.len() >= Self::MUSK_MACK_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(MuskMackCacheEntry { core, result });
        Ok(result)
    }

    fn musk_interpret_mack(&mut self, core: Noun, axis: u64) -> Option<Noun> {
        let slab: *mut NounSlab = &mut *self.slab;
        let slab_space = self.slab.noun_space();
        let context = &mut self.musk.context as *mut NockContext;
        unsafe {
            let context = &mut *context;
            let core = self.musk_mack_cached_core_in_context(context, core, &slab_space);
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
        // On success, cold/warm/cache are PRESERVED out of the frame
        // (`with_stack_frame`) rather than rolled back: jet registrations
        // and the pointer unifications performed by their battery
        // comparisons are pure and monotone, and discarding them forced
        // every subsequent mack to redo full structural equality walks
        // against the jet state — quadratic in practice and the dominant
        // cost of fold-heavy kernel compiles.
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
            let core = self.musk_mack_cached_core_in_context(context, core, &slab_space);
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

    fn musk_apex_output(&mut self, bus: Noun, fol: Noun) -> Result<MuskOutput> {
        let mut memo: FastHashMap<(u64, u64), Noun> = Default::default();
        let Some(noy) = self.musk_araw(bus, fol, &mut memo)? else {
            return Ok(MuskOutput::Stop);
        };
        let (mask, data) = self.semi_parts(noy)?;
        let blocks = self.semi_squash_mask(mask)?;
        if noun_is_zero(blocks) {
            Ok(MuskOutput::Done(data))
        } else {
            Ok(MuskOutput::Wait)
        }
    }

    // PHASE-2 TAIL: the bran/seminoun projection now reads the NATIVE type enum
    // (`NRc<NTy>`) directly instead of decoding type nouns + bridging face/hint/
    // hold via `repo_noun`. The deepening subject (payload/cell/face/hint/hold
    // chains) is never lowered: recursion threads the shared native children, the
    // %hold cycle guard + raw-recursion guard key on the interned `Rc` pointer
    // (`NRc::as_ptr`), and the cache re-keys on that pointer (no mug of the
    // deepening subject). The OUTPUT stays a seminoun (semi_* algebra is noun).
    fn bran_canonical_semi(&mut self, sut: NRc<NTy>) -> Result<Noun> {
        let mut seen_holds: Vec<NRc<NTy>> = Vec::new();
        self.bran_canonical_semi_inner(sut, &mut seen_holds)
    }

    fn bran_seen_holds_signature(seen_holds: &[NRc<NTy>]) -> (u64, u64, usize) {
        let mut sum = 0u64;
        let mut xor = 0u64;
        for hold in seen_holds {
            let id = NRc::as_ptr(hold) as u64;
            let component = id.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            sum = sum.wrapping_add(component);
            xor ^= component.rotate_left((id as u32) & 31);
        }
        (sum, xor, seen_holds.len())
    }

    fn bran_semi_cache_key(&self, sut: &NRc<NTy>, seen_holds: &[NRc<NTy>]) -> BranSemiMemoKey {
        let semantic = self.semantic_context_key();
        let (seen_sum, seen_xor, seen_len) = Self::bran_seen_holds_signature(seen_holds);
        (
            NRc::as_ptr(sut) as u64,
            semantic.vet_key,
            semantic.fan_context_key,
            seen_sum,
            seen_xor,
            seen_len,
        )
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
    ) -> Result<Option<Noun>> {
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

    fn bran_semi_cache_store(&mut self, sut: &NRc<NTy>, seen_holds: &[NRc<NTy>], semi: Noun) {
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
    ) -> Result<Noun> {
        // Matches hoon-138 `++bran` exactly: cycles are broken ONLY at `%hold`
        // (via `seen_holds` == hoon's `gil`); every other (shared, non-hold)
        // subtree is re-descended and the result memoized (`bran_semi_cache` ==
        // hoon's `~+`). An earlier global ancestor guard (`seen_raw`) over-blocked
        // here: when a non-hold subtree was re-encountered as an ancestor through
        // a `%hold` expansion, `++bran` re-descends it (blocking only at the inner
        // hold, so the result is a *partial* seminoun), whereas the ancestor guard
        // collapsed the whole subtree to fully-blocked. That produced
        // `[%full [~ ~ ~]]` (the `*seminoun` bunt) where hoonc computes the actual
        // `[%full ~]` complete stencil — the first divergence in the native
        // hoon-138 self-mint vs hoonc. Termination is preserved: the Rc type DAG
        // is acyclic, and the only logical cycles run through `%hold` (whose repo
        // is the lazy back-edge), which `seen_holds` still guards.
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
    ) -> Result<Noun> {
        match &*sut {
            NTy::Noun | NTy::Void => Ok(self.semi_full_blocked()),
            NTy::Atom { .. } => {
                // Atoms are leaf-only (no deepening): lower the small atom type
                // and reuse the existing noun decoder (mirrors atom_nest).
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
                // payload + context are native (recurse native); only the bounded
                // `rest` leaf (battery seminoun + tomes) is lowered to read the
                // coil's seminoun head.
                let payload = payload.clone();
                let rest_noun = live_leaf_to_noun(&mut self.cx, rest, self.slab);
                let space = self.slab.noun_space();
                let rest_cell = rest_noun.in_space(&space).as_cell().map_err(|err| {
                    CompilerError::Decode(format!("bran core rest not cell: {err}"))
                })?;
                let coil_semi = rest_cell.head().noun();
                let payload_semi = self.bran_canonical_semi_inner(payload, seen_holds)?;
                let _ = self.semi_parts(coil_semi)?;
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
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        const HOON_VERSION: u64 = 138;
        let (ty, _formula) = self.mint(sut.clone(), gol, &Hoon::KetTar(Box::new(spec.clone())))?;
        // The minted type is embedded in a nock %12 hint formula (noun): lower it.
        let ty_noun = live_to_noun(&mut self.cx, &ty, self.slab);
        let hint_inner = T(self.slab, &[D(HOON_VERSION), ty_noun]);
        let hint = T(self.slab, &[D(1), hint_inner]);
        let goal = cons_noun(&mut self.cx);
        let (_q_ty, q_formula) = self.mint(sut, goal, q)?;
        let formula = T(self.slab, &[D(12), hint, q_formula]);
        Ok((ty, formula))
    }

    fn mint_dtls(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, p: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        let atom_ty = ty_atom_n(&mut self.cx, self.slab, "$", None).1;
        let (_p_ty, p_formula) = self.mint(sut.clone(), atom_ty.clone(), p)?;
        let formula = T(self.slab, &[D(4), p_formula]);
        let ty = self.nice(sut, gol, atom_ty)?;
        Ok((ty, formula))
    }

    fn mint_dttr(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let noun_ty = cons_noun(&mut self.cx);
        let (_p_ty, p_formula) = self.mint(sut.clone(), noun_ty.clone(), p)?;
        let (_q_ty, q_formula) = self.mint(sut.clone(), noun_ty.clone(), q)?;
        let formula = T(self.slab, &[D(2), p_formula, q_formula]);
        let ty = self.nice(sut, gol, noun_ty)?;
        Ok((ty, formula))
    }

    fn mint_dtts(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let noun_ty = cons_noun(&mut self.cx);
        let (_p_ty, p_formula) = self.mint(sut.clone(), noun_ty.clone(), p)?;
        let (_q_ty, q_formula) = self.mint(sut.clone(), noun_ty, q)?;
        let formula = T(self.slab, &[D(5), p_formula, q_formula]);
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let ty = self.nice(sut, gol, bool_ty)?;
        Ok((ty, formula))
    }

    fn mint_dtwt(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, p: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        let noun_ty = cons_noun(&mut self.cx);
        let (_p_ty, p_formula) = self.mint(sut.clone(), noun_ty, p)?;
        let formula = T(self.slab, &[D(3), p_formula]);
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let ty = self.nice(sut, gol, bool_ty)?;
        Ok((ty, formula))
    }

    fn play_wtts(&mut self, _sut: NRc<NTy>, _spec: &Spec, _wing: &WingType) -> Result<NRc<NTy>> {
        Ok(ty_bool_n(&mut self.cx, self.slab).1)
    }

    fn play_ketvar(&mut self, sut: NRc<NTy>, p: &Hoon, vair: Vair) -> Result<NRc<NTy>> {
        // play + wrap_type are native; thread the native subject/result directly.
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
        // play + hint_type are native; thread the native subject directly.
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

    fn axis_contains(container: u64, contained: u64) -> bool {
        let container_bits = 64 - container.leading_zeros();
        let contained_bits = 64 - contained.leading_zeros();
        if contained_bits < container_bits {
            return false;
        }
        let shift = contained_bits - container_bits;
        (contained >> shift) == container
    }

    fn axis_parent(axis: u64) -> u64 {
        axis >> 1
    }

    fn axis_sibling(axis: u64) -> u64 {
        if axis & 1 == 0 {
            axis + 1
        } else {
            axis - 1
        }
    }

    fn hike_insert(
        &mut self,
        axe: u64,
        fol: Noun,
        rel: &mut std::collections::BTreeMap<u64, Noun>,
    ) -> Result<()> {
        let mut probe = axe;
        while probe != 1 {
            if rel.contains_key(&probe) {
                return Ok(());
            }
            probe = Self::axis_parent(probe);
        }

        let existing: Vec<u64> = rel.keys().copied().collect();
        for key in existing {
            if Self::axis_contains(axe, key) {
                rel.remove(&key);
            }
        }

        let sib = Self::axis_sibling(axe);
        if let Some(sib_fol) = rel.remove(&sib) {
            let parent = Self::axis_parent(sib);
            let merged = if sib > axe {
                cons(self.slab, fol, sib_fol)?
            } else {
                cons(self.slab, sib_fol, fol)?
            };
            self.hike_insert(parent, merged, rel)?;
        } else {
            rel.insert(axe, fol);
        }
        Ok(())
    }

    fn hike_formula(&mut self, root_axis: BigUint, edits: &[(u64, Noun)]) -> Result<Noun> {
        let mut rel: std::collections::BTreeMap<u64, Noun> = std::collections::BTreeMap::new();
        for (axis, formula) in edits {
            self.hike_insert(*axis, *formula, &mut rel)?;
        }
        let mut out = slot_formula_axis_big(self.slab, root_axis);
        // hoon-138 `++hike` sorts with `gth` (descending) and recurses from head to tail:
        //   [%10 [hi ...] [%10 [lo ...] [%0 a]]]
        // To build the same tree iteratively, apply edits in ascending order so larger axes
        // wrap later and become outer nodes.
        let keys: Vec<u64> = rel.keys().copied().collect();
        for axis in keys {
            let edit_formula = rel
                .get(&axis)
                .copied()
                .ok_or_else(|| CompilerError::Noun("missing hike edit".to_string()))?;
            let axis_noun = noun_u64(self.slab, axis);
            let patch_cell = T(self.slab, &[axis_noun, edit_formula]);
            let edit_cell = T(self.slab, &[patch_cell, out]);
            out = T(self.slab, &[D(10), edit_cell]);
        }
        Ok(out)
    }

    fn cnts_tack(
        &mut self,
        sut: NRc<NTy>,
        wing: &WingType,
        mur: NRc<NTy>,
    ) -> Result<(u64, NRc<NTy>)> {
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
    ) -> Result<(u64, Vec<(NRc<NTy>, Noun)>)> {
        let mut axis_match: Option<u64> = None;
        let mut out = Vec::with_capacity(arms.len());
        for (typ, foot) in arms {
            let (axis, new_typ) = self.cnts_tack(typ.clone(), wing, mur.clone())?;
            if let Some(existing) = axis_match {
                if existing != axis {
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
    fn tack(&mut self, sut: NRc<NTy>, hyp: &WingType, mur: NRc<NTy>) -> Result<(u64, NRc<NTy>)> {
        self.cnts_tack(sut, hyp, mur)
    }

    // HOON138:arm=ut:toss lines=10860-10871 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++toss`; full parity review is still in progress
    fn toss(
        &mut self,
        hyp: &WingType,
        mur: NRc<NTy>,
        men: &[(NRc<NTy>, Noun)],
    ) -> Result<(u64, Vec<(NRc<NTy>, Noun)>)> {
        self.cnts_toss(hyp, mur, men)
    }

    fn play_cnts_apply_leg_patches(
        &mut self,
        sut: NRc<NTy>,
        mut typ: NRc<NTy>,
        pairs: &[(WingType, Hoon)],
    ) -> Result<NRc<NTy>> {
        for (sub_wing, expr) in pairs {
            // play is native (C-final): thread the native subject directly.
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
                // fire is native (C-final): arm cores are already `NRc<NTy>`.
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

    fn mint_hand(&mut self, typ: &Type, nock: &Nock) -> Result<(NRc<NTy>, Noun)> {
        let typ_noun = type_to_noun(self.slab, typ)?;
        let typ_native = native_of(&mut self.cx, typ_noun, &self.slab.noun_space())?;
        let nock_noun = nock_to_noun(self.slab, nock);
        Ok((typ_native, nock_noun))
    }

    fn play_tune(&mut self, sut: NRc<NTy>, tune: &TermOrTune) -> Result<NRc<NTy>> {
        // play_tune wraps the WHOLE subject in a %face; native now: the subject
        // threads through as a SHARED native Rc inside the new %face (no lowering).
        let tool = term_or_tune_to_noun(self.slab, tune)?;
        let tool_leaf = NLeaf::from_noun_raw(tool, &self.slab.noun_space());
        Ok(cons_face(&mut self.cx, tool_leaf, sut))
    }

    fn mint_siggar(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        hint: &TermOrPair,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let (q_ty, q_formula) = self.mint(sut.clone(), gol, q)?;
        let hint_noun = match hint {
            TermOrPair::Term(name) => term_to_noun(self.slab, name),
            TermOrPair::Pair(name, hoon) => {
                // hoon-138 `%sggr`: pair hint payload always mints under `%noun`.
                let noun_goal = cons_noun(&mut self.cx);
                let (_ty, formula) = self.mint(sut, noun_goal, hoon)?;
                let name_noun = term_to_noun(self.slab, name);
                T(self.slab, &[name_noun, formula])
            }
        };
        let formula = T(self.slab, &[D(11), hint_noun, q_formula]);
        Ok((q_ty, formula))
    }

    fn mint_sigzap(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let _ = self.play(sut.clone(), p)?;
        self.mint(sut, gol, q)
    }

    fn mint_zpcom(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let ty = self.play(sut.clone(), p)?;
        let ty = self.nice(sut, gol, ty)?;
        let q_noun = hoon_to_noun(self.slab, q);
        let formula = T(self.slab, &[D(1), q_noun]);
        Ok((ty, formula))
    }

    fn mint_zpmc(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        p: &Hoon,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let goal = cons_noun(&mut self.cx);
        let (vos_ty, vos_formula) = self.mint(sut.clone(), goal.clone(), q)?;
        let (ref_ty, _ref_formula) = self.mint(sut.clone(), goal, p)?;
        let cell_ty = cons_cell(&mut self.cx, ref_ty, vos_ty.clone());
        let ty = self.nice(sut, gol, cell_ty)?;
        // burp_type takes/returns a noun: lower vos_ty.
        let vos_ty_noun = live_to_noun(&mut self.cx, &vos_ty, self.slab);
        let burped = self.burp_type(vos_ty_noun)?;
        let head = T(self.slab, &[D(1), burped]);
        let formula = cons(self.slab, head, vos_formula)?;
        Ok((ty, formula))
    }

    fn mint_zpgl(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        spec: &Spec,
        q: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let typ_expr = Hoon::KetTar(Box::new(spec.clone()));
        let typ = self.play(sut.clone(), &typ_expr)?;
        let typ = self.nice(sut.clone(), gol, typ)?;

        // Mirror hoon-138 `%zpgl` mint shape without introducing an extra binding.
        // This keeps the compiled noun aligned with canonical dynock fixtures.
        let target_type = Hoon::TisGar(
            Box::new(Hoon::ZapGar(Box::new(Hoon::KetTar(Box::new(spec.clone()))))),
            Box::new(Hoon::Axis(2)),
        );
        let actual_type = Hoon::TisGar(Box::new(q.clone()), Box::new(Hoon::Axis(2)));
        let cond = Hoon::CenCol(
            Box::new(Hoon::Limb("levi".to_string())),
            vec![target_type, actual_type],
        );
        let yes = Hoon::TisGar(Box::new(q.clone()), Box::new(Hoon::Axis(3)));
        let expanded = Hoon::WutCol(Box::new(cond), Box::new(yes), Box::new(Hoon::ZapZap));
        let goal = cons_noun(&mut self.cx);
        let (_val_ty, val_formula) = self.mint(sut, goal, &expanded)?;
        Ok((typ, val_formula))
    }

    fn mint_zpts(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, p: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        let noun_ty = cons_noun(&mut self.cx);
        let ty = self.nice(sut.clone(), gol, noun_ty.clone())?;
        // hoon-138: `%zpts` compiles inner with `gol=%noun` under `vet=|`.
        self.with_vet_off(|ut| {
            let (_p_ty, p_formula) = ut.mint(sut, noun_ty, p)?;
            let formula = T(ut.slab, &[D(1), p_formula]);
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // feel is native (C9): thread the native subject directly.
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // Canonical hoon-138 open() lowering:
        //   [%tsbr *] => [%tsls ~(example ax p.gen) q.gen]
        // This is NOT `=+ *spec ...`; using `%kttr` would inject `%ktsg` folding that the
        // canonical `%tsbr` path does not perform.
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
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        let expanded = Hoon::TisFas(skin.clone(), Box::new(q.clone()), Box::new(p.clone()));
        self.mint(sut, gol, &expanded)
    }

    fn mint_wing(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        wing: &WingType,
    ) -> Result<(NRc<NTy>, Noun)> {
        // find + fine are native (C9 / C-final): fine returns the typ directly.
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
        let hoon_raw = unsafe { hoon.as_raw() };

        // ATOMIC FLIP perf: core/goal are interned native Rcs, so structural
        // equality is exactly pointer identity (hash-cons guarantees one canonical
        // Rc per type) — Rc::ptr_eq replaces the noun as_raw/noun_eq compare.
        for entry in self.arm_goal_in_progress.iter().rev() {
            let entry_hoon_raw = unsafe { entry.hoon.as_raw() };
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

    /// Noun-bridged `cnts_base_port` for still-noun callers (play_cnts/mint_cnts).
    fn cnts_base_port_noun(&mut self, sut: Noun, wing: &WingType) -> Result<Port> {
        let sut_n = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        self.cnts_base_port(sut_n, wing)
    }

    /// CONTENT-keyed `native_of` for the recursive-type hot path (redo/repo/fire
    /// rebuild structurally-equal nouns at fresh addresses every level). The
    /// address-keyed decode memo misses on those, forcing a full re-walk + re-jam
    /// of the Jammed leaves (fork treaps, batteries) — the dominant decode cost.
    /// Here a fresh-but-equal noun reuses the prior interned `Rc` after one
    /// `noun_eq` verify, skipping the decode entirely. Returns EXACTLY what
    /// `native_of` would (the canonical interned `Rc`), so it is byte-exact; mug
    /// collisions just fall through to a real decode.
    fn native_of_cached(&mut self, noun: Noun) -> Result<NRc<NTy>> {
        let mug = self.noun_mug_cached(noun) as u64;
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // cnts_base_port + tack/toss are native (C9): thread the native subject.
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
        let mut edits: Vec<(u64, Noun)> = Vec::with_capacity(pairs.len());
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
                    // sample edit is `%void`; the subsequent `++fire` is responsible for
                    // rejecting non-core arm subjects.
                    hag = next_hag;
                    edits.push((edit_axis, patch_formula));
                }
                // Match hoon-138 `++ergo` edit accumulation order for arm patches too.
                edits.reverse();

                let hike = self.hike_formula(base_axis, &edits)?;
                let arm_axis_noun = noun_u64(self.slab, arm_axis);
                let formula = T(self.slab, &[D(9), arm_axis_noun, hike]);
                // fire is native (C-final): arm cores are already `NRc<NTy>`.
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
    ) -> Result<(NRc<NTy>, Noun)> {
        self.mint_cnts(sut, gol, hyp, rig)
    }

    fn mint_limb(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, name: &str) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        let tool = term_or_tune_to_noun(self.slab, tune)?;
        // %face over the native subject (collapse-aware cons_face).
        let tool_leaf = NLeaf::from_noun_raw(tool, &self.slab.noun_space());
        let ty = cons_face(&mut self.cx, tool_leaf, sut.clone());
        let ty = self.nice(sut, gol, ty)?;
        let formula = T(self.slab, &[D(0), D(1)]);
        Ok((ty, formula))
    }

    fn hoon_ast_ptr_key(gen: &Hoon) -> usize {
        gen as *const Hoon as usize
    }

    fn cache_hoon_ast_ptr(
        cache: &mut HashMap<usize, (Option<u64>, Noun)>,
        order: &mut VecDeque<usize>,
        ptr: usize,
        raw: Option<u64>,
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

        let hoon_noun = hoon_to_noun(self.slab, gen);
        let hoon_raw = unsafe { hoon_noun.as_raw() };
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
            if unsafe { cached_noun.as_raw() } == hoon_raw {
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
                1,
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
            1,
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
        let Some(sig) = Self::mint_cache_signature(gen) else {
            let opened = open(gen.clone());
            return (&opened != gen).then(|| Arc::new(opened));
        };
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

    fn core_context_from_payload(&mut self, payload: Noun) -> Result<Noun> {
        // Canonical hoon-138 core construction stores the current subject as core context.
        // Keep one path here: context is exactly the payload subject used to build the core.
        Ok(payload)
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // Canonical layered-core construction keeps prior arms in payload/context ancestry.
        // New core tomes contain only the newly declared arms; inherited arms are resolved via
        // `%core` payload traversal in `fond`, not by copying old tomes into the new battery.
        // PHASE 2 (native coil context): mint_core TAKES native sut/gol and the
        // core_mint cache is native-Rc-keyed, so the deepening subject `sut` is
        // NEVER lowered to a noun. `gol` (the goal, not the deepening subject) is
        // lowered once for goal_core_for_mine's noun-side walk. The core payload
        // AND context are both the SHARED native `sut`.
        let gol_noun = live_to_noun(&mut self.cx, &gol, self.slab);
        let tomes_map = self.tomes_map_from_ast(tomes)?;
        if let Some((cached_ty, cached_formula)) =
            self.core_mint_cache_lookup(&sut, &gol, tomes_map, prefix, poly)?
        {
            // C-final.1b: native-re-keyed cache returns the native type directly.
            return Ok((cached_ty, cached_formula));
        }
        let garb = garb_native(prefix.as_deref(), poly, Vair::Gold);
        // Match hoon-138/hoonc layered-core payload layout and formula shape.
        let payload_formula = slot_formula_axis(self.slab, 1);
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

        // Canonical hoon-138 `++mine` compiles arm formulas against a `%lazy`
        // battery seminoun (`++laze`) in a single pass.
        // RT-05: a CONTENT-ADDRESSED resolver id (keyed on the lazy core's
        // structural identity) so structurally-equal recursive cores intern to one
        // `Rc` and the pointer-keyed cuts converge at the natural settling depth.
        let tomes_sig =
            (self.noun_mug_cached(tomes_map) ^ Self::prefix_signature(prefix.as_deref())) as u64;
        let resolver_id = self.lazy_resolver_canonical_id(&sut, tomes_sig, poly);
        let lazy_semi = self.semi_lazy_root(resolver_id);
        let lazy_rest = T(self.slab, &[lazy_semi, tomes_map]);
        // PHASE 2: build the NATIVE lazy core (the deepening site) ONCE, native-only
        // (cons_core), with payload AND context = the SHARED native `sut`. No noun
        // core, no sut lowering. Threading this interned Rc to the battery builder
        // means each of N arms reuses ONE core; the context is a shared Rc (never
        // jammed/copied) — this is the O(N^2) fix. The same Rc is stored in the
        // lazy resolver context so cross-arm in-progress cycle detection compares
        // pointer identity.
        let core_native_lazy = {
            let space = self.slab.noun_space();
            let rest_leaf = NLeaf::from_noun_raw(lazy_rest, &space);
            cons_core(
                &mut self.cx,
                sut.clone(),
                garb.clone(),
                sut.clone(),
                rest_leaf,
            )
        };
        // RT-05: with a content-addressed id the same canonical core can be reached
        // again (e.g. distinct gol/vet but identical lazy-core identity). Register
        // exactly once — the resolver content is determined by the id-key, and
        // re-registering would reset its `cached_formula_by_axis`/`in_progress_axes`,
        // dropping the cross-arm formula cache and the in-progress guard.
        if !self.lazy_resolvers.contains_key(&resolver_id) {
            let mut lazy_arms = HashMap::new();
            self.collect_lazy_resolver_arms_from_tomes_map(tomes_map, 1, &mut lazy_arms)?;
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
        // forever. Removing the context here left dead resolver ids inside
        // long-lived types, turning their batteries permanently blocked and
        // breaking constant folds that hoonc performs.
        let battery = self.build_tomes_battery_from_maps(
            tomes_map, core_native_lazy, poly, goal_for_arms, expected_tomes_map,
        )?;
        let semi_noun = self.semi_noun_full(battery);
        let rest = T(self.slab, &[semi_noun, tomes_map]);
        // PHASE 2: mint_core RETURNS the core's native Rc<Type> — THE
        // subject-deepening site — built native-only via cons_core with payload AND
        // context = the SHARED native `sut`. No noun core is built (nice is native +
        // identity-on-success; the cache is native-keyed) and `sut` is never lowered.
        let core_native = {
            let space = self.slab.noun_space();
            let rest_leaf = NLeaf::from_noun_raw(rest, &space);
            cons_core(
                &mut self.cx,
                sut.clone(),
                garb.clone(),
                sut.clone(),
                rest_leaf,
            )
        };
        let battery_formula = T(self.slab, &[D(1), battery]);
        let formula = cons(self.slab, battery_formula, payload_formula)?;
        // nice is native now (C-final.1a); validate the native core type.
        // nice is identity-on-success, so `ty` == `core_native`; cache the
        // validated native type directly (C-final.1b: no lowering to a noun).
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
    ) -> Result<(NRc<NTy>, Noun)> {
        // ATOMIC FLIP (C-final.1a): mine TAKES native sut/gol. wrap_type/nice are
        // native now; thread directly.
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
        // PHASE 2 (native coil context): play_core TAKES native sut and RETURNS
        // native Rc<Type>. The native core embeds the SHARED native payload AND the
        // SHARED native context (both = sut), so the deepening subject is NEVER
        // lowered to a noun. Only the tiny garb + bounded rest are built as nouns
        // for the carried leaves; cons_core mirrors ty_core's void-collapse.
        let tomes_map = self.tomes_map_from_ast(tomes)?;
        let garb = garb_native(prefix.as_deref(), poly, Vair::Gold);
        // Canonical hoon-138 `%play` builds cores with `*seminoun` (blocked by default).
        let semi_noun = self.semi_noun_blocked();
        let rest = T(self.slab, &[semi_noun, tomes_map]);
        let space = self.slab.noun_space();
        let rest_leaf = NLeaf::from_noun_raw(rest, &space);
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
        seen_holds: &mut HashMap<u32, Vec<Noun>>,
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
        let mut seen_holds: HashMap<u32, Vec<Noun>> = HashMap::new();
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
                // hoon-138 `++get-tomes` deliberately returns `~` for forked goal cores after
                // chapter-count validation, so arm-count/name/type checks are disabled here too.
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
        // ATOMIC FLIP perf: return the per-arm mint goal NATIVELY so it threads
        // straight into mint. The common case is %noun (cons_noun(&mut self.cx) == ty_noun);
        // the Some branch plays the goal-core subject and keeps the native result
        // (goal_for_arms is only native_of'd here, in the rare goal-typed mine —
        // it is NOT the deepening core).
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
        axis: u64,
        out: &mut HashMap<u64, LazyResolverArmEntry>,
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
            axis
        } else {
            peg_axis(axis, 2)?
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
                self.collect_lazy_resolver_arms_from_arms_map(right, peg_axis(axis, 3)?, out)
            }
            (false, true) => {
                self.collect_lazy_resolver_arms_from_arms_map(left, peg_axis(axis, 3)?, out)
            }
            (false, false) => {
                self.collect_lazy_resolver_arms_from_arms_map(left, peg_axis(axis, 6)?, out)?;
                self.collect_lazy_resolver_arms_from_arms_map(right, peg_axis(axis, 7)?, out)
            }
        }
    }

    fn collect_lazy_resolver_arms_from_tomes_map(
        &mut self,
        tomes_map: Noun,
        axis: u64,
        out: &mut HashMap<u64, LazyResolverArmEntry>,
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
            axis
        } else {
            peg_axis(axis, 2)?
        };
        self.collect_lazy_resolver_arms_from_arms_map(arms_map, chapter_axis, out)?;
        match (left_empty, right_empty) {
            (true, true) => Ok(()),
            (true, false) => {
                self.collect_lazy_resolver_arms_from_tomes_map(right, peg_axis(axis, 3)?, out)
            }
            (false, true) => {
                self.collect_lazy_resolver_arms_from_tomes_map(left, peg_axis(axis, 3)?, out)
            }
            (false, false) => {
                self.collect_lazy_resolver_arms_from_tomes_map(left, peg_axis(axis, 6)?, out)?;
                self.collect_lazy_resolver_arms_from_tomes_map(right, peg_axis(axis, 7)?, out)
            }
        }
    }

    fn build_tomes_battery_from_maps(
        &mut self,
        tomes_map: Noun,
        // ATOMIC FLIP perf: the NATIVE deepening core, threaded from mint_core. It
        // is only passed through (to recursion + build_arms_battery_from_map ->
        // mint); never decoded here. One interned Rc shared across all N arms.
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
        // ATOMIC FLIP perf: the NATIVE deepening core and NATIVE goal. Both go
        // straight into native `mint` (no native_of re-lift of the O(N) core per
        // arm — that was the O(N^2) bug). The in-progress dedup keys off the
        // interned Rc pointer identity.
        core_type: NRc<NTy>,
        poly: Poly,
        goal: NRc<NTy>,
        hoon: &Hoon,
        hoon_noun: Noun,
    ) -> Result<Noun> {
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

        // The in-progress dedup id is the interned core Rc's pointer (one canonical
        // Rc per type via hash-cons), replacing the old noun as_raw() identity.
        let core_type_id = NRc::as_ptr(&core_type) as u64;
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
        self.arm_epoch = self.arm_epoch.wrapping_add(1);
        // core_type/goal are the NATIVE deepening core + goal: thread straight to
        // native mint (no per-arm native_of re-lift — the O(N^2) -> O(N) win).
        let result = self.mint(core_type.clone(), goal.clone(), hoon);
        self.vet = prev_vet;
        self.arm_in_progress.remove(&in_progress_key);
        let popped = self.arm_goal_in_progress.pop();
        debug_assert_eq!(
            popped.map(|entry| (
                entry.key,
                NRc::as_ptr(&entry.core),
                unsafe { entry.hoon.as_raw() },
                NRc::as_ptr(&entry.goal),
                entry.vet,
            )),
            Some((
                Arc::clone(&key),
                NRc::as_ptr(&core_type),
                unsafe { hoon_noun.as_raw() },
                NRc::as_ptr(&goal),
                arm_vet,
            ))
        );
        self.arm_epoch = self.arm_epoch.wrapping_add(1);
        let (ty, formula) = match result {
            Ok(ok) => ok,
            Err(err) => {
                return Err(with_arm_context(key.as_ref(), err));
            }
        };
        // prune_recursive_holds takes the arm RESULT type as a noun. `ty` is the
        // native mint result (the arm's output, NOT the deepening core), so
        // lowering it per arm is bounded and acceptable; the deepening core is
        // never lowered.
        let ty_noun = live_to_noun(&mut self.cx, &ty, self.slab);
        let _ty = self.prune_recursive_holds(ty_noun, hoon_noun)?;

        Ok(formula)
    }

    fn build_arms_battery_from_map(
        &mut self,
        arms_map: Noun,
        // ATOMIC FLIP perf: native deepening core, passed through to the per-arm
        // builder -> mint. `goal` stays a noun: it is the goal-core play SUBJECT
        // forwarded to goal_arm_expected_type (not the small per-arm mint goal,
        // which that helper produces natively).
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

    // garb_from_parts (noun garb builder) is superseded by `garb_native`, which
    // builds the native `Garb` struct fed to `cons_core`; the byte-identical noun
    // emission now lives in `Garb::to_noun`.

    fn semi_noun_full(&mut self, noun: Noun) -> Noun {
        let full = term_to_noun(self.slab, "full");
        let blocks = D(0);
        let stencil = T(self.slab, &[full, blocks]);
        T(self.slab, &[stencil, noun])
    }

    fn semi_noun_blocked(&mut self) -> Noun {
        self.semi_full_blocked()
    }

    fn mint_opened(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        gen: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
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
    ) -> Result<(NRc<NTy>, Noun)> {
        let location = Self::location_from_spot(spot);
        self.dbug_locations.push(location);
        let result = self.mint(sut, gol, inner);
        self.dbug_locations.pop();
        let (ty, formula) = result?;
        self.record_semantic_type(spot, &ty);
        let spot_noun = spot_to_noun(self.slab, spot)?;
        let hint_inner = T(self.slab, &[D(1), spot_noun]);
        let spot_tag = term_to_noun(self.slab, "spot");
        let hint = T(self.slab, &[spot_tag, hint_inner]);
        let formula = T(self.slab, &[D(11), hint, formula]);
        Ok((ty, formula))
    }

    fn mint_note(
        &mut self,
        sut: NRc<NTy>,
        gol: NRc<NTy>,
        note: &Note,
        inner: &Hoon,
    ) -> Result<(NRc<NTy>, Noun)> {
        let (payload_ty, formula) = self.mint(sut.clone(), gol, inner)?;
        let note_noun = note_to_noun(self.slab, note)?;
        let hinted = self.hint_type(sut, note_noun, payload_ty)?;
        Ok((hinted, formula))
    }

    fn mint_lost(&mut self, sut: NRc<NTy>, gol: NRc<NTy>) -> Result<(NRc<NTy>, Noun)> {
        if self.vet {
            return Err(CompilerError::Noun("mint-lost".to_string()));
        }
        let ty = cons_void(&mut self.cx);
        let ty = self.nice(sut, gol, ty)?;
        let formula = crash_formula(self.slab);
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
    ) -> Result<(NRc<NTy>, Noun)> {
        let test = Hoon::WutTis(
            Box::new(Spec::Base(BaseType::Atom("$".to_string()))),
            wing.clone(),
        );
        // Match hoon-138 open() lowering: wtkt -> wtcl(wtts atom p, r, q)
        let expanded = Hoon::WutCol(Box::new(test), Box::new(r.clone()), Box::new(q.clone()));
        self.mint(sut, gol, &expanded)
    }

    fn mint_wtzp(&mut self, sut: NRc<NTy>, gol: NRc<NTy>, p: &Hoon) -> Result<(NRc<NTy>, Noun)> {
        let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
        let (_p_ty, p_formula) = self.mint(sut.clone(), bool_ty.clone(), p)?;
        let false_formula = const_bool_formula(self.slab, false);
        let true_formula = const_bool_formula(self.slab, true);
        let formula = cond(self.slab, p_formula, false_formula, true_formula)?;
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
        // ATOMIC FLIP (C-final.1a): nice reads/returns native (mirrors mull_nice,
        // which is the goal-nest check in the mull context). Identity-on-success.
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
        // ATOMIC FLIP (C-final.1a): hint_type reads/returns native. The hint head
        // is the noun pair `[inner_noun note]`; collapse void/noun -> payload.
        match &*payload {
            NTy::Void | NTy::Noun => return Ok(payload),
            _ => {}
        }
        let inner_noun = live_to_noun(&mut self.cx, &inner, self.slab);
        let head = T(self.slab, &[inner_noun, note]);
        let head_leaf = NLeaf::from_noun_raw(head, &self.slab.noun_space());
        Ok(cons_hint(&mut self.cx, head_leaf, payload))
    }

    /// Native-shadow `hint_type` (INC2): on the void/noun collapse hoon-138
    /// returns `payload` itself, so the native is the payload's own native.
    #[allow(dead_code)]
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

        let hoon_raw = unsafe { hoon_noun.as_raw() };
        if let Some(ast) = self.hoon_cache_raw.get(&hoon_raw).cloned() {
            return Some(ast);
        }
        if let Some(ast) = self.decoded_hold_hoon_cache_raw.get(&hoon_raw).cloned() {
            return Some(ast);
        }
        let hoon_mug = self.noun_mug_cached(hoon_noun);
        let bucket = self.hoon_cache_struct.get(&hoon_mug)?;
        for (cached_noun, cached_ast) in bucket.iter().rev() {
            if unsafe { cached_noun.as_raw() } == hoon_raw {
                return Some(Arc::clone(cached_ast));
            }
            if let Ok(true) = noun_eq(*cached_noun, hoon_noun, &self.slab.noun_space()) {
                return Some(Arc::clone(cached_ast));
            }
        }
        None
    }

    fn decode_hold_hoon_ast(&mut self, hoon_noun: Noun) -> std::result::Result<Arc<Hoon>, String> {
        let hoon_raw = unsafe { hoon_noun.as_raw() };
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
        if self.exact_hoon_ast_lookup_enabled {
            if let Some((_raw, noun)) = self
                .decoded_hold_hoon_ptr_cache
                .get(&Self::hoon_ast_ptr_key(gen))
                .copied()
            {
                return noun;
            }
        }
        hoon_to_noun(self.slab, gen)
    }

    fn prune_recursive_holds(&mut self, typ: Noun, hoon_noun: Noun) -> Result<Noun> {
        // This traversal can get very deep (e.g. large recursive molds in hoon-138).  Use an
        // explicit stack to avoid Rust stack overflows in release tests.
        let mut seen: HashSet<u64> = HashSet::new();
        let mut todo: Vec<Noun> = vec![typ];
        let mut post: Vec<Noun> = Vec::new();
        while let Some(node) = todo.pop() {
            let raw = unsafe { node.as_raw() };
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
        let mut memo: HashMap<u64, Noun> = HashMap::with_capacity(post.len().saturating_mul(2));
        for node in post.into_iter().rev() {
            let raw = unsafe { node.as_raw() };
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
                        let opt_raw = unsafe { option.as_raw() };
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
                    let head_raw = unsafe { head.as_raw() };
                    let tail_raw = unsafe { tail.as_raw() };
                    let head = memo.get(&head_raw).copied().unwrap_or(head);
                    let tail = memo.get(&tail_raw).copied().unwrap_or(tail);
                    ty_cell(self.slab, head, tail)
                }
                "face" => {
                    let inner = type_face_inner(node, &self.slab.noun_space())?;
                    let inner_raw = unsafe { inner.as_raw() };
                    let inner = memo.get(&inner_raw).copied().unwrap_or(inner);
                    type_face_with_inner(self.slab, node, inner)?
                }
                "hint" => {
                    let (inner, note, payload) = type_hint_parts(node, &self.slab.noun_space())?;
                    let payload_raw = unsafe { payload.as_raw() };
                    let payload = memo.get(&payload_raw).copied().unwrap_or(payload);
                    ty_hint(self.slab, inner, note, payload)
                }
                "core" => {
                    let (payload, coil) = type_core_parts(node, &self.slab.noun_space())?;
                    let (garb, context, rest) = coil_parts(coil, &self.slab.noun_space())?;
                    let payload_raw = unsafe { payload.as_raw() };
                    let context_raw = unsafe { context.as_raw() };
                    let payload = memo.get(&payload_raw).copied().unwrap_or(payload);
                    let context = memo.get(&context_raw).copied().unwrap_or(context);
                    let new_coil = coil_from_parts(self.slab, garb, context, rest);
                    ty_core(self.slab, payload, new_coil)
                }
                _ => node,
            };
            memo.insert(raw, result);
        }
        let root_raw = unsafe { typ.as_raw() };
        Ok(*memo.get(&root_raw).unwrap_or(&typ))
    }

    fn nest(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<bool> {
        // ATOMIC FLIP (consumer C8): nest reads the native enum directly. The
        // deepening children (cell/face/hint/core payloads) stay native (no
        // lowering — this is the memory win). Leaf-carried parts (coil, fork set,
        // atom aura/bits) are lowered via live_to_noun / live_leaf_to_noun
        // (memoized) and decoded with the existing noun helpers. Type identity for
        // the seen-hold / gil / memo sets uses the interned `Rc` pointer as a
        // canonical id (flip natives are hash-consed), replacing the old noun
        // interner. repo/peek are native (C1/C2); play still takes a noun subject
        // (lowered here) until C-final. The boundary cache is native-keyed
        // (intern.rs) — keying on noun mugs would force lowering the deepening
        // subject per call, which is O(N^2) over the deepening chain.
        let semantic = self.semantic_context_key();
        // nest descends and repos %hold on BOTH sut and ref, so scope the fan on
        // the union of both legsets (legset(sut) ∪ legset(ref)).
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
        // Native exposes only the top-level seg=0,reg=0 case (jet-cacheable).
        let seg_empty = true;
        let reg_empty = true;
        let cacheable = (result && reg_empty) || (!result && seg_empty);
        if cacheable {
            nest_cache_store(&mut self.cx, &sut, &ref_, semantic.vet_key, fan, result);
        }
        Ok(result)
    }

    /// Noun-bridged `nest` for not-yet-flipped callers (C8): lift both type nouns
    /// to native, run native nest. Drops as callers flip (C-final).
    fn nest_noun(&mut self, sut: Noun, ref_: Noun) -> Result<bool> {
        let space = self.slab.noun_space();
        let sut_n = native_of(&mut self.cx, sut, &space)?;
        let ref_n = native_of(&mut self.cx, ref_, &space)?;
        self.nest(sut_n, ref_n)
    }

    /// Decode a native `%fork` set into its native option types (C8): the fork set
    /// is a carried leaf, so lower it and re-lift the options. Forks are small.
    fn fork_options_native(&mut self, fork: &NRc<NTy>) -> Result<Vec<NRc<NTy>>> {
        let fork_noun = live_to_noun(&mut self.cx, fork, self.slab);
        let space = self.slab.noun_space();
        let opts = type_fork_options(fork_noun, &space)?;
        let mut out = Vec::with_capacity(opts.len());
        for opt in opts {
            out.push(native_of(&mut self.cx, opt, &space)?);
        }
        Ok(out)
    }

    /// Native `core_dox` (C8): build the doppelganger context-core from a carried
    /// coil leaf without lowering the (possibly deep) payload. `core_dox` ignores
    /// the payload, so a `%noun` placeholder is byte-identical.
    /// Native `core_dox`: `++dox` rebuilds the core as `core(context, garb', rest)`
    /// where `garb'` forces vair=gold and payload becomes the context. PHASE 2:
    /// built native-only with the SHARED native `context` (no native_of of the
    /// deepening subject). `garb`/`rest` are lowered (tiny/bounded) so garb_with_vair
    /// can rewrite the garb; the new garb leaf round-trips byte-identically.
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
                sut_id: NRc::as_ptr(&sut) as u64,
                ref_id: NRc::as_ptr(&ref_) as u64,
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
                    let sut_id = NRc::as_ptr(&sut) as u64;
                    let ref_id = NRc::as_ptr(&ref_) as u64;
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
                for option in self.fork_options_native(&ref_)? {
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
                let sut_id = NRc::as_ptr(&sut) as u64;
                let ref_id = NRc::as_ptr(&ref_) as u64;
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
        // coil-equal short-circuit: garb/rest leaf-equal AND context ptr-equal
        // (interned natives are canonical, so structural equality == ptr identity).
        if sut_garb == ref_garb
            && NRc::ptr_eq(&sut_context_n, &ref_context_n)
            && sut_rest == ref_rest
        {
            return self.nest_inner(
                sut_payload, ref_payload, depth, seen_sut_holds, seen_ref_holds, gil, memo,
            );
        }
        // PHASE 2: garb is native (direct field access); rest is tiny/bounded —
        // lower to noun for the noun coil decoders. The CONTEXT is already native
        // (the deepening win: no native_of, no lowering of the deepening subject).
        // The native garb/Leaf rest are kept for the native core_dox rebuild below.
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
        let sut_id = NRc::as_ptr(&sut) as u64;
        let ref_id = NRc::as_ptr(&ref_) as u64;
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
                let sut_peek = self.peek(ref_ctx, Way::Rite, 2)?;
                let ref_peek = self.peek(sut_ctx, Way::Rite, 2)?;
                self.nest_inner(
                    sut_peek, ref_peek, depth, seen_sut_holds, seen_ref_holds, gil, memo,
                )
            }
            Vair::Zinc => {
                let sut_peek = self.peek(sut_ctx, Way::Read, 2)?;
                let ref_peek = self.peek(ref_ctx, Way::Read, 2)?;
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
        // ATOMIC FLIP (consumer C3): wrap_type reads + rebuilds the native enum.
        // Branch rebuilds use the collapse-aware cons_* ctors. The core coil and
        // the fork set stay noun in Phase 1 (lowered via to_noun); repo is native.
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
                // garb is native: read + rewrite the vair directly. context
                // (deepening subject) and rest stay native/leaf — no lowering.
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
            NTy::Fork { set } => {
                let set_noun = live_leaf_to_noun(&mut self.cx, set, self.slab);
                let options = fork_set_options(set_noun, &self.slab.noun_space())?;
                let mut wrapped = Vec::with_capacity(options.len());
                for option in options {
                    let opt = native_of(&mut self.cx, option, &self.slab.noun_space())?;
                    let w = self.wrap_type(opt, vair)?;
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

    /// Noun-bridged `wrap_type` for not-yet-flipped callers (C3). Drops as
    /// callers flip.
    fn wrap_type_noun(&mut self, typ: Noun, vair: Vair) -> Result<Noun> {
        let native = native_of(&mut self.cx, typ, &self.slab.noun_space())?;
        let r = self.wrap_type(native, vair)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
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
        let raw = unsafe { typ.as_raw() };
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
                    // Canonical hoon-138 `++burp`: keep a `[%full ~]`-complete coil
                    // seminoun as-is. (The earlier fragment-`%spot` strip here was a
                    // NET REGRESSION: hoonc's seminoun spotting is context-dependent —
                    // it KEEPS the spot on cores embedded in type specs/mold samples
                    // like the `++map` `$|` validator's `(tree (pair))` sample (self-mint
                    // divergence byte 881767) and BARES it on the top-level output coil
                    // cores (byte 1776224). Stripping all moved the first divergence
                    // BACKWARD (881767 < 1776224). honk's natural mint emits all spotted,
                    // which is correct for the type-embedded cores; the remaining bug #2
                    // is honk over-spotting the OUTPUT-coil cores. Apps have no such
                    // cores, so kernels are byte-exact either way.)
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
            // fine is native (C-final): returns the typ directly for fond.
            let (ty, _formula) = self.fine(&port)?;
            current = ty;
        }
        Ok(true)
    }

    /// Noun-bridged `feel` for still-noun callers.
    fn feel_noun(&mut self, sut: Noun, wings: &[WingType]) -> Result<bool> {
        let sut_n = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        self.feel(sut_n, wings)
    }

    fn take<F>(
        &mut self,
        sut: NRc<NTy>,
        vein: &[Option<BigUint>],
        duz: &F,
    ) -> Result<(u64, NRc<NTy>)>
    where
        F: Fn(&mut Self, NRc<NTy>) -> Result<NRc<NTy>>,
    {
        let axis = tend(vein)?;
        let vit: Vec<Option<u64>> = vein
            .iter()
            .rev()
            .map(|step| step.as_ref().map(axis_big_to_u64).transpose())
            .collect::<Result<_>>()?;
        let ty = self.take_inner(sut, &vit, duz)?;
        Ok((axis, ty))
    }

    fn take_inner<F>(&mut self, sut: NRc<NTy>, vit: &[Option<u64>], duz: &F) -> Result<NRc<NTy>>
    where
        F: Fn(&mut Self, NRc<NTy>) -> Result<NRc<NTy>>,
    {
        let Some((head, tail)) = vit.split_first() else {
            return duz(self, sut);
        };
        self.take_inner_head_tail(sut, *head, tail, duz)
    }

    fn take_inner_head_tail<F>(
        &mut self,
        sut: NRc<NTy>,
        head: Option<u64>,
        tail: &[Option<u64>],
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
                let mut vil: HashSet<u64> = HashSet::new();
                self.take_axis(sut, step, tail, duz, &mut vil)
            }
        }
    }

    fn take_axis<F>(
        &mut self,
        sut: NRc<NTy>,
        step: u64,
        tail: &[Option<u64>],
        duz: &F,
        vil: &mut HashSet<u64>,
    ) -> Result<NRc<NTy>>
    where
        F: Fn(&mut Self, NRc<NTy>) -> Result<NRc<NTy>>,
    {
        if step == 1 {
            return self.take_inner(sut, tail, duz);
        }
        let (cap, mas) = axis_cap_mas(step)?;
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
                    let opt_ty = self.take_axis(option, step, tail, duz, vil)?;
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
                let sut_id = NRc::as_ptr(&sut) as u64;
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

    /// Noun-bridged `gain` for still-noun callers (play_wtcl/mint_wtcl).
    fn gain_noun(&mut self, sut: Noun, gen: &Hoon) -> Result<Noun> {
        let sut_n = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let r = self.gain(sut_n, gen)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    /// Noun-bridged `lose` for still-noun callers (play_wtcl/mint_wtcl).
    fn lose_noun(&mut self, sut: Noun, gen: &Hoon) -> Result<Noun> {
        let sut_n = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let r = self.lose(sut_n, gen)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    fn chip(&mut self, how: bool, sut: NRc<NTy>, gen: &Hoon) -> Result<NRc<NTy>> {
        match gen {
            // Source-location/debug wrappers are non-semantic.
            Hoon::Dbug(_, inner) | Hoon::Note(_, inner) => self.chip(how, sut, inner.as_ref()),
            Hoon::WutTis(spec, wing) => {
                let example = self.spec_example_cached(spec);
                // play is native (C-final.2); thread the native subject directly.
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
        // Native identity collapse: interned ptr == structural identity.
        let ty = if NRc::ptr_eq(&ty, &sut) { sut } else { ty };
        Ok(ty)
    }

    fn gain_skin(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>, skin: &Skin) -> Result<NRc<NTy>> {
        let mut seen: HashSet<u64> = HashSet::new();
        self.gain_skin_inner(sut, ref_, skin, &mut seen)
    }

    fn gain_skin_inner(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        skin: &Skin,
        seen: &mut HashSet<u64>,
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
                    let mut seen_ref: HashSet<u64> = HashSet::new();
                    self.gain_atom_skin(sut, ref_, aura, &mut seen_ref)
                }
            },
            Skin::Cell(head, tail) => {
                let mut seen_ref: HashSet<u64> = HashSet::new();
                self.gain_cell_skin(sut, ref_, head, tail, &mut seen_ref)
            }
            Skin::Leaf(aura, atom) => {
                let mut seen_ref: HashSet<u64> = HashSet::new();
                self.gain_leaf_skin(sut, ref_, aura, atom, &mut seen_ref)
            }
            Skin::Dbug(_, inner) => self.gain_skin_inner(sut, ref_, inner, seen),
            Skin::Help(help, inner) => {
                let payload = self.gain_skin_inner(sut.clone(), ref_, inner, seen)?;
                let help_noun = noun_expr_to_noun(self.slab, help);
                let note_noun = tagged1(self.slab, "help", help_noun);
                // hoon-138 `hint_type(sut, note, payload)`: the hint "inner" slot is
                // the subject type itself, so the native head leaf is `[sut note]`.
                // cons_hint preserves the void/noun collapse hint_type applies.
                let sut_noun = live_to_noun(&mut self.cx, &sut, self.slab);
                let head_noun = T(self.slab, &[sut_noun, note_noun]);
                let head_leaf = NLeaf::from_noun_raw(head_noun, &self.slab.noun_space());
                Ok(cons_hint(&mut self.cx, head_leaf, payload))
            }
            Skin::Name(name, inner) => {
                let inner_ty = self.gain_skin_inner(sut, ref_, inner, seen)?;
                let tool_noun = term_to_noun(self.slab, name);
                let tool_leaf = NLeaf::from_noun_raw(tool_noun, &self.slab.noun_space());
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
        seen: &mut HashSet<u64>,
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
                let ref_id = NRc::as_ptr(&ref_) as u64;
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
        seen: &mut HashSet<u64>,
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
                // (`[%cell head %noun]`).  More specific tail skins refine the core as an
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
                let ref_id = NRc::as_ptr(&ref_) as u64;
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
        seen: &mut HashSet<u64>,
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
                let ref_id = NRc::as_ptr(&ref_) as u64;
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
        let mut seen: HashSet<u64> = HashSet::new();
        self.lose_skin_inner(sut, ref_, skin, &mut seen)
    }

    fn lose_skin_inner(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        skin: &Skin,
        seen: &mut HashSet<u64>,
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
                    let mut seen_ref: HashSet<u64> = HashSet::new();
                    self.lose_atom_skin(sut, ref_, aura, &mut seen_ref)
                }
            },
            Skin::Cell(head, tail) => {
                let mut seen_ref: HashSet<u64> = HashSet::new();
                self.lose_cell_skin(sut, ref_, head, tail, &mut seen_ref)
            }
            Skin::Leaf(aura, atom) => {
                let mut seen_ref: HashSet<u64> = HashSet::new();
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
        seen: &mut HashSet<u64>,
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
                let ref_id = NRc::as_ptr(&ref_) as u64;
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
        seen: &mut HashSet<u64>,
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
                // 3-way fork rebuild (RT-07 ordering preserved via cons_fork).
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
                let ref_id = NRc::as_ptr(&ref_) as u64;
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
        seen: &mut HashSet<u64>,
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
                let ref_id = NRc::as_ptr(&ref_) as u64;
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
        // ATOMIC FLIP (consumer C4): native. C-final.4: the fuse boundary cache is
        // native-re-keyed on the interned (sut, ref) `Rc` pointers, so the
        // deepening subject is no longer lowered to a noun here.
        if let Some(cached) = self.fuse_boundary_lookup(&sut, &ref_)? {
            return Ok(cached);
        }
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        let result = self.fuse_inner(sut.clone(), ref_.clone(), &mut seen)?;
        self.fuse_boundary_store(&sut, &ref_, result.clone())?;
        Ok(result)
    }

    /// Noun-bridged `fuse` for not-yet-flipped callers (C4). Drops at C-final.
    fn fuse_noun(&mut self, sut: Noun, ref_: Noun) -> Result<Noun> {
        let sn = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let rn = native_of(&mut self.cx, ref_, &self.slab.noun_space())?;
        let r = self.fuse(sn, rn)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    fn miss(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<bool> {
        // ATOMIC FLIP (consumer C5b): native. seen/memo keyed by native pointer.
        let mut seen: Vec<(u64, u64)> = Vec::new();
        if let Some((stored_epoch, mut memo)) = self.miss_memo_persist.take() {
            let context = self.cache_context_key();
            let epoch = (
                context.semantic.vet_key, context.semantic.fan_context_key,
                context.memo.arm_epoch_key, context.memo.placeholder_context_key,
            );
            if stored_epoch != epoch {
                memo.clear();
            }
            let result = self.miss_dext(sut, ref_, &mut seen, &mut memo);
            self.miss_memo_persist = Some((epoch, memo));
            return result;
        }
        let mut memo: FastHashMap<(u64, u64, u8), bool> = Default::default();
        self.miss_dext(sut, ref_, &mut seen, &mut memo)
    }

    /// Noun-bridged `miss` for not-yet-flipped callers (C5b). Drops at C-final.
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
            // Sentinel epoch (vet_key is only ever 0/1): the first `miss`
            // call stamps the real epoch over an empty map.
            self.miss_memo_persist = Some(((u8::MAX, 0, 0, 0), Default::default()));
        } else if !enabled {
            self.miss_memo_persist = None;
        }
        was_enabled
    }

    #[inline]
    fn miss_memo_key(&self, sut: &NRc<NTy>, ref_: &NRc<NTy>) -> (u64, u64, u8) {
        (
            NRc::as_ptr(sut) as u64,
            NRc::as_ptr(ref_) as u64,
            u8::from(self.vet),
        )
    }
    /// Memo over raw (sut, ref_, vet, rest-context) keys, scoped to one
    /// outer `miss` call. Without it, sibling fork branches re-explore
    /// identical hold expansions and a single outer `miss` over hoon-138
    /// types performs >10^8 recursive calls. The memo is deliberately NOT
    /// persisted across calls: `miss` reaches `repo`/`rest`/`redo`, whose
    /// cached state evolves during a build, and verdicts memoized under
    /// earlier state can flip (observed via shadow validation as
    /// cached=true/fresh=false mismatches, manifesting as redo-match
    /// miscompiles in batch builds). Within one call the state is
    /// consistent and unconditional reuse is validated by the parity corpus.
    fn miss_dext(
        &mut self,
        sut: NRc<NTy>,
        ref_: NRc<NTy>,
        seen: &mut Vec<(u64, u64)>,
        memo: &mut FastHashMap<(u64, u64, u8), bool>,
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
        seen: &mut Vec<(u64, u64)>,
        memo: &mut FastHashMap<(u64, u64, u8), bool>,
    ) -> Result<bool> {
        if NRc::ptr_eq(&sut, &ref_) {
            // C-final.4: nest is native; call it directly on the deepening type
            // instead of lowering to a noun for nest_noun.
            let void = cons_void(&mut self.cx);
            return self.nest(void, sut.clone());
        }
        if matches!(&*ref_, NTy::Void) {
            return Ok(true);
        }
        match &*sut {
            NTy::Void => Ok(true),
            NTy::Noun => {
                // C-final.4: native nest directly (no live_to_noun bridge).
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
            NTy::Fork { set } => {
                let set_noun = live_leaf_to_noun(&mut self.cx, set, self.slab);
                for option in fork_set_options(set_noun, &self.slab.noun_space())? {
                    let opt = native_of(&mut self.cx, option, &self.slab.noun_space())?;
                    if !self.miss_dext(opt, ref_.clone(), seen, memo)? {
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
                let sp = NRc::as_ptr(&sut) as u64;
                let rp = NRc::as_ptr(&ref_) as u64;
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
        seen: &mut Vec<(u64, u64)>,
        memo: &mut FastHashMap<(u64, u64, u8), bool>,
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
        seen: &mut HashSet<(usize, usize)>,
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
            NTy::Fork { set } => {
                let set_noun = live_leaf_to_noun(&mut self.cx, set, self.slab);
                let options = fork_set_options(set_noun, &self.slab.noun_space())?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = native_of(&mut self.cx, option, &self.slab.noun_space())?;
                    let f = self.fuse_inner(opt, ref_.clone(), seen)?;
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
                let key = (NRc::as_ptr(&sut) as usize, NRc::as_ptr(&ref_) as usize);
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
        // ATOMIC FLIP (consumer C5): native. C-final.4: the crop boundary cache is
        // native-re-keyed on the interned (sut, ref) `Rc` pointers, so the
        // deepening subject is no longer lowered to a noun here.
        if let Some(cached) = self.crop_boundary_lookup(&sut, &ref_)? {
            return Ok(cached);
        }
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        let result = self.crop_inner(sut.clone(), ref_.clone(), &mut seen)?;
        self.crop_boundary_store(&sut, &ref_, result.clone())?;
        Ok(result)
    }

    /// Noun-bridged `crop` for not-yet-flipped callers (C5). Drops at C-final.
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
        seen: &mut HashSet<(usize, usize)>,
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
                    // hoon-138 nest(ref_head, sut_head); nest still noun (C8) -> lower.
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
            NTy::Fork { set } => {
                let set_noun = live_leaf_to_noun(&mut self.cx, set, self.slab);
                let options = fork_set_options(set_noun, &self.slab.noun_space())?;
                let mut out = Vec::with_capacity(options.len());
                for option in options {
                    let opt = native_of(&mut self.cx, option, &self.slab.noun_space())?;
                    let c = self.crop_inner(opt, ref_.clone(), seen)?;
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
                let key = (NRc::as_ptr(&sut) as usize, NRc::as_ptr(&ref_) as usize);
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
        seen: &mut HashSet<(usize, usize)>,
    ) -> Result<NRc<NTy>> {
        match &*ref_ {
            NTy::Core { .. } => Ok(sut),
            NTy::Face { .. } | NTy::Hint { .. } | NTy::Hold { .. } => {
                let inner = self.repo(ref_.clone())?;
                self.crop_inner(sut, inner, seen)
            }
            NTy::Fork { set } => {
                let set_noun = live_leaf_to_noun(&mut self.cx, set, self.slab);
                let options = fork_set_options(set_noun, &self.slab.noun_space())?;
                let mut acc = sut;
                for option in options {
                    let opt = native_of(&mut self.cx, option, &self.slab.noun_space())?;
                    acc = self.crop_inner(acc, opt, seen)?;
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

    /// Native-shadow `fork_from_options` (INC2). The boundary fork carries the
    /// mug-ordered set as one opaque leaf (RT-07), and the empty/single collapses
    /// yield void/the single member — `native_of` on the result captures all
    /// three cases byte-exactly without reordering the set.
    #[allow(dead_code)]
    fn fork_from_options_n(&mut self, options: Vec<Noun>) -> Result<(Noun, NRc<NTy>)> {
        let noun = self.fork_from_options(options)?;
        let native = native_of(&mut self.cx, noun, &self.slab.noun_space())?;
        Ok((noun, native))
    }

    /// Native `%fork` constructor — the keystone of the redo/fire/repo native
    /// flip. Takes NATIVE options and returns the canonical interned fork type,
    /// so callers in the type SCC never round-trip the whole fork through a noun
    /// (`live_to_noun(opt); fork_from_options; native_of`). The mug-ordered treap
    /// build is DELEGATED to `fork_from_options` over the per-`Rc`-memoized
    /// lowering of each option, so the emitted `%set` leaf is byte-IDENTICAL by
    /// construction (zero treap re-derivation; RT-07 deferred). All collapse
    /// rules (empty->void, single->bare member, void-drop, nested-fork union)
    /// come free from `fork_from_options`; the single result is content-keyed via
    /// `native_of_cached` so a structurally-equal fork reuses one interned `Rc`.
    fn cons_fork(&mut self, options: Vec<NRc<NTy>>) -> Result<NRc<NTy>> {
        // Memo on the canonical option-pointer SET (sorted+deduped): a fork is
        // mug-ordered and set-valued, so the interned option pointers fully
        // determine the result. Skips the mug-treap rebuild + decode/jam for the
        // forks that recur throughout the recursive-type elaboration. Byte-exact:
        // a hit returns the same interned `Rc` the rebuild would.
        let mut noun_opts = Vec::with_capacity(options.len());
        for opt in &options {
            noun_opts.push(live_to_noun(&mut self.cx, opt, self.slab));
        }
        let fork_noun = self.fork_from_options(noun_opts)?;
        let result = self.native_of_cached(fork_noun)?;
        Ok(result)
    }

    fn atom_nest(&mut self, sut: NRc<NTy>, ref_: NRc<NTy>) -> Result<bool> {
        // ATOMIC FLIP (consumer C8): atoms are small, so lowering the carried
        // aura/bits leaves via the whole-type to_noun is cheap and reuses the
        // existing noun decoder.
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

    fn peek(&mut self, sut: NRc<NTy>, way: Way, axis: u64) -> Result<NRc<NTy>> {
        // ATOMIC FLIP (consumer C2): peek reads the native enum directly. The
        // seen-hold dedup, the core coil, and the fork set stay noun-keyed in
        // Phase 1 (lowered via to_noun); repo is native (C1).
        fn seen_hold(
            ut: &mut Ut<'_>,
            seen: &mut HashMap<u32, Vec<(Noun, u64)>>,
            hold: Noun,
            axis: u64,
        ) -> Result<bool> {
            let axis_noun = noun_u64(ut.slab, axis);
            let mug = ut.noun_mug_cached(hold) ^ slab_mug(axis_noun, &ut.slab.noun_space());
            if let Some(bucket) = seen.get(&mug) {
                for (prior, prior_axis) in bucket {
                    if *prior_axis == axis && noun_eq(*prior, hold, &ut.slab.noun_space())? {
                        return Ok(true);
                    }
                }
            }
            seen.entry(mug).or_default().push((hold, axis));
            Ok(false)
        }

        fn unsee_hold(
            ut: &mut Ut<'_>,
            seen: &mut HashMap<u32, Vec<(Noun, u64)>>,
            hold: Noun,
            axis: u64,
        ) -> Result<()> {
            let axis_noun = noun_u64(ut.slab, axis);
            let mug = ut.noun_mug_cached(hold) ^ slab_mug(axis_noun, &ut.slab.noun_space());
            if let Some(bucket) = seen.get_mut(&mug) {
                let space = ut.slab.noun_space();
                let mut idx = 0;
                while idx < bucket.len() {
                    if bucket[idx].1 == axis && noun_eq(bucket[idx].0, hold, &space)? {
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
            axis: u64,
            seen_holds: &mut HashMap<u32, Vec<(Noun, u64)>>,
        ) -> Result<NRc<NTy>> {
            if axis == 1 {
                return Ok(sut);
            }
            match &*sut {
                NTy::Noun => Ok(cons_noun(&mut ut.cx)),
                NTy::Void => Ok(cons_void(&mut ut.cx)),
                NTy::Atom { .. } => Ok(cons_void(&mut ut.cx)),
                NTy::Cell(head, tail) => {
                    let (cap, mas) = axis_cap_mas(axis)?;
                    let child = if cap == 2 { head.clone() } else { tail.clone() };
                    go(ut, child, way, mas, seen_holds)
                }
                NTy::Core { payload, garb, .. } => {
                    let (cap, mas) = axis_cap_mas(axis)?;
                    if cap != 3 {
                        return Ok(cons_noun(&mut ut.cx));
                    }
                    let payload = payload.clone();
                    // garb is native (direct field access); the context (deepening
                    // subject) is not needed here.
                    let vair = garb.vair;
                    let (sam, con) = peel(way, vair);
                    let tow = if mas == 1 { 1 } else { axis_cap_mas(mas)?.0 };
                    if (sam && con) || (sam && tow == 2) || (con && tow == 3) {
                        return go(ut, payload, way, mas, seen_holds);
                    }
                    if way != Way::Read {
                        return Err(CompilerError::Noun("payload-block".to_string()));
                    }
                    let sam_type = if sam {
                        go(ut, payload.clone(), way, 2, seen_holds)?
                    } else {
                        cons_noun(&mut ut.cx)
                    };
                    let con_type = if con {
                        go(ut, payload.clone(), way, 3, seen_holds)?
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
                    if seen_hold(ut, seen_holds, hold_noun, axis)? {
                        return Ok(cons_void(&mut ut.cx));
                    }
                    let expanded = ut.repo(sut.clone())?;
                    let result = go(ut, expanded, way, axis, seen_holds);
                    unsee_hold(ut, seen_holds, hold_noun, axis)?;
                    result
                }
                NTy::Fork { set } => {
                    let set_noun = live_leaf_to_noun(&mut ut.cx, set, ut.slab);
                    let options = fork_set_options(set_noun, &ut.slab.noun_space())?;
                    let mut peeks = Vec::with_capacity(options.len());
                    for option in options {
                        let opt = native_of(&mut ut.cx, option, &ut.slab.noun_space())?;
                        peeks.push(go(ut, opt, way, axis, seen_holds)?);
                    }
                    let mut peek_nouns = Vec::with_capacity(peeks.len());
                    for p in &peeks {
                        peek_nouns.push(live_to_noun(&mut ut.cx, p, ut.slab));
                    }
                    let fork_noun = ut.fork_from_options(peek_nouns)?;
                    native_of(&mut ut.cx, fork_noun, &ut.slab.noun_space())
                }
            }
        }

        let mut seen_holds: HashMap<u32, Vec<(Noun, u64)>> = HashMap::new();
        go(self, sut, way, axis, &mut seen_holds)
    }

    /// Noun-bridged `peek` for not-yet-flipped callers (C2): lift sut, run native
    /// peek, lower the result. Drops as callers flip.
    fn peek_noun(&mut self, sut: Noun, way: Way, axis: u64) -> Result<Noun> {
        let native = native_of(&mut self.cx, sut, &self.slab.noun_space())?;
        let r = self.peek(native, way, axis)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }

    /// Grow the native stack before recursing into deep type operations
    /// (`redo`, `mull`, `nest`). Without this, deeply nested types overflow
    /// the Rust stack on consumers running with a default 8 MB thread stack.
    /// Constants match the `mint`/`play` guard in `native/mod.rs`.
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
        // ATOMIC FLIP (consumer C7): mull reads/returns the native enum.
        // C-final.1b: the mull boundary cache is native-re-keyed on the interned
        // (sut, gol, dox) `Rc` pointers, so sut/gol/dox are no longer lowered to
        // nouns to build the cache key. gen stays a noun (mug-keyed AST node).
        let gen_noun = self.hoon_noun_for_node(gen);
        if let Some(cached) = self.mull_cache_lookup(&sut, &gol, &dox, gen_noun)? {
            return Ok(cached);
        }
        // hoon-138 pre-check: mull-none if sut is void
        if matches!(&*sut, NTy::Void) {
            return Err(CompilerError::Noun("mull-none".to_string()));
        }

        // Grow the native stack for deep recursion safety (matches mint/play).
        let result =
            self.with_stack_guard(|ut| ut.mull_inner(sut.clone(), gol.clone(), dox.clone(), gen))?;
        // C-final.1b: store the native (p, q) types directly (no lowering).
        self.mull_cache_store(
            &sut,
            &gol,
            &dox,
            gen_noun,
            result.0.clone(),
            result.1.clone(),
        )?;
        Ok(result)
    }

    /// Noun-bridged `mull` for not-yet-flipped callers (C7): lift sut/gol/dox to
    /// native, run native mull, lower both result types. Drops as callers flip.
    /// Currently only exercised by tests (the live noun caller in wet.rs calls
    /// native `mull` directly with native_of'd args); kept for future callers.
    #[allow(dead_code)]
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
                &Hoon::Axis(1),
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
                &Hoon::Axis(1),
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

            // ---- Cast: %ktls ----
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
                let tool_leaf = NLeaf::from_noun_raw(tool, &self.slab.noun_space());
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
                // hint_type is native now (C-final.1a): thread sut/dox + payloads.
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
                // busk is native now (C-final.2): thread the native subjects directly.
                let boc = self.busk(sut, p);
                let nuf = self.busk(dox, p);
                self.mull(boc, gol, nuf, q)
            }

            // ---- Conditional: %wtcl ----
            Hoon::WutCol(p, q, r) => {
                let bool_gol = ty_bool_n(&mut self.cx, self.slab).1;
                let _nor = self.mull(sut.clone(), bool_gol, dox.clone(), p)?;

                // True branch: apply gain to both sut and dox. gain/lose are native
                // now (C6) — thread the native subjects directly.
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

                // Fork both results (RT-07 mug ordering preserved via cons_fork).
                let p_ty = self.cons_fork(vec![hiq.0, ran.0])?;
                let p_ty = self.mull_nice(sut, gol, p_ty)?;
                let q_ty = self.cons_fork(vec![hiq.1, ran.1])?;
                Ok((p_ty, q_ty))
            }

            // ---- Type test: %fits ----
            Hoon::Fits(p, wing) => {
                // Play the pattern from both perspectives. play + mint are native
                // (C-final): thread the native subjects directly.
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
                let pov_p = self.type_test_formula_on_axis(waz_p, syx_p)?;
                let pov_q = self.type_test_formula_on_axis(waz_q, syx_q)?;

                // Assert axes AND fish nock are identical (pov_* are FORMULAS).
                if syx_p != syx_q || !noun_eq(pov_p, pov_q, &self.slab.noun_space())? {
                    return Err(CompilerError::Noun("mull-bonk-a".to_string()));
                }
                let bool_ty = ty_bool_n(&mut self.cx, self.slab).1;
                self.mull_beth(sut, gol, bool_ty)
            }

            // ---- Aura test: %wthx ----
            Hoon::WutHax(_skin, wing) => {
                // fend from both perspectives. fend is native now (C9): thread the
                // native subjects + read native types directly.
                let (new_type, new_axis) = self.fend(sut.clone(), Way::Read, wing)?;
                let (old_type, old_axis) = self.fend(dox.clone(), Way::Read, wing)?;

                // Assert axes match
                if new_axis != old_axis {
                    return Err(CompilerError::Noun("mull-bonk-x".to_string()));
                }
                // Assert old type nests in new (type.new ⊆ type.old)
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
            // hoon-138: (beth %noun) — no recursion on p.gen
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
                // Note: hoon-138 has a comment "XX is this right?" here.
                let kttr = Hoon::KetTar(Box::new(spec.as_ref().clone()));
                let ty = self.play(sut.clone(), &kttr)?;
                self.mull_beth(sut, gol, ty)
            }

            // ---- Conditional compilation: %zppt ----
            Hoon::ZapPat(wings, q, r) => {
                // feel is native now (C9): thread the native subjects directly.
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

            // ---- Sugar forms that lower via open() before mull ----
            // TisLus (=+) lowers to TisGar => handled above
            Hoon::TisLus(p, q) => {
                let lowered = Hoon::TisGar(
                    Box::new(Hoon::Pair(p.clone(), Box::new(Hoon::Axis(1)))),
                    q.clone(),
                );
                self.mull(sut, gol, dox, &lowered)
            }
            // TisHep (=-) is TisLus with args reversed
            Hoon::TisHep(p, q) => {
                let lowered = Hoon::TisGar(
                    Box::new(Hoon::Pair(q.clone(), Box::new(Hoon::Axis(1)))),
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
    fn cove(&self, mut formula: Noun) -> Result<u64> {
        // hoon-138 `++cove` extracts an axis from `[0 axis]`, skipping `%11` hints.
        let space = self.slab.noun_space();
        loop {
            let cell = formula
                .in_space(&space)
                .as_cell()
                .map_err(|_| CompilerError::Noun("cove: not a cell".to_string()))?;
            let head = cell.head().noun();
            let head_val = head
                .as_direct()
                .map_err(|_| CompilerError::Noun("cove: head not direct".to_string()))?;
            match head_val.data() {
                0 => {
                    return cell
                        .tail()
                        .as_atom()
                        .map_err(|_| CompilerError::Noun("cove: tail not atom".to_string()))?
                        .as_u64()
                        .map_err(|_| CompilerError::Noun("cove: axis too large".to_string()))
                }
                11 => {
                    let tail = cell.tail().as_cell().map_err(|_| {
                        CompilerError::Noun("cove: malformed hint formula".to_string())
                    })?;
                    formula = tail.tail().noun();
                }
                other => {
                    return Err(CompilerError::Noun(format!(
                        "cove: not a slot formula (head={other})"
                    )));
                }
            }
        }
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
        // Mull the payload (ruf, typically Hoon::Axis(1))
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
        // PHASE 2: the core payload AND context (sut/dox) are both SHARED native
        // Rc<Type> — never lowered to a noun. Only the tiny garb + bounded rest are
        // built as noun leaves; cons_core mirrors ty_core's void-collapse.
        let tomes_map = self.tomes_map_from_ast(tomes)?;
        // Construct yet = core(sut, [nym hud gold], sut, laze, dom)
        let garb = garb_native(nym, hud, Vair::Gold);
        let semi_noun = self.semi_noun_blocked();
        let rest = T(self.slab, &[semi_noun, tomes_map]);
        let yet = {
            let space = self.slab.noun_space();
            let rest_leaf = NLeaf::from_noun_raw(rest, &space);
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
            let rest_leaf = NLeaf::from_noun_raw(rest, &space);
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
        // find/Port are native (C6+C9): thread the native subjects directly.
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
        // Port carries native types now (C6+C9): read them directly.
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
        // Palo/Opal/tack/toss carry native types now (C6+C9). zil.0/zil.1 (native
        // mull results) thread straight into native tack/toss; fire stays on the
        // NOUN path (arm cores lowered just before the fire call).
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
                // Fire sut-side with vet on, dox-side with vet off. fire is native
                // (C-final): arm cores are already `NRc<NTy>`.
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
    // Delegate to the shared structural equality (moved to `native::noun` so the
    // IR `Leaf` impls can use it too). Same name/signature: callers unchanged.
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

fn slot_formula_axis(slab: &mut NounSlab, axis: u64) -> Noun {
    let axis_noun = noun_u64(slab, axis);
    slot_formula_axis_noun(slab, axis_noun)
}

fn slot_formula_axis_big(slab: &mut NounSlab, axis: BigUint) -> Noun {
    let axis_noun = noun_biguint(slab, axis);
    slot_formula_axis_noun(slab, axis_noun)
}

fn slab_mug(noun: Noun, space: &NounSpace) -> u32 {
    // Delegate to the shared iterative mug (moved to `native::noun` alongside
    // `noun_eq`). Same name/signature: callers unchanged.
    crate::native::noun::slab_mug(noun, space)
}

fn install_musk_cold_state(context: &mut NockContext, raw: &[u8], label: &str) -> Result<()> {
    let cold_noun = <Noun as NounExt>::cue_bytes_slice(&mut context.stack, raw)
        .map_err(|err| CompilerError::Decode(format!("cue {label} cold jam: {err:?}")))?;
    let stack_space = context.stack.noun_space();
    let (battery_to_paths, root_to_paths, path_to_batteries) =
        Cold::from_noun(&mut context.stack, &cold_noun, &stack_space)
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

// garb_poly/garb_vair (noun-path garb field decoders) are superseded by direct
// field access on the native `Garb` struct (the %core garb is no longer a noun
// leaf); `garb_parts` survives for the remaining noun-coil bridge `garb_with_vair`.

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
) -> Result<Option<u64>> {
    let (found, _skip) = find_face_axis_skip_inner(noun, name, skip, space)?;
    Ok(found)
}

#[cfg(test)]
fn find_face_axis_skip_inner(
    noun: Noun,
    name: &str,
    skip: u64,
    space: &NounSpace,
) -> Result<(Option<u64>, u64)> {
    let tag = type_tag(noun, space)?;
    match tag.as_str() {
        "face" => {
            let mut remaining = skip;
            if type_face_name_if_atom(noun, space)?.as_deref() == Some(name) {
                if remaining == 0 {
                    return Ok((Some(1), remaining));
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
                return Ok((Some(peg_axis(2, axis)?), remaining));
            }
            let (found_tail, remaining) = find_face_axis_skip_inner(tail, name, remaining, space)?;
            if let Some(axis) = found_tail {
                return Ok((Some(peg_axis(3, axis)?), remaining));
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
                return Ok((Some(peg_axis(3, axis)?), rem));
            }
            let (found, rem) = find_face_axis_skip_inner(context, name, rem, space)?;
            if let Some(axis) = found {
                return Ok((Some(peg_axis(7, axis)?), rem));
            }
            Ok((None, rem))
        }
        "fork" => {
            let options = type_fork_options(noun, space)?;
            let mut remaining = skip;
            let mut found_axis: Option<u64> = None;
            for option in options {
                let (found, rem) = find_face_axis_skip_inner(option, name, remaining, space)?;
                remaining = rem;
                if let Some(axis) = found {
                    if let Some(existing) = found_axis {
                        if existing != axis {
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

fn peg_axis(a: u64, b: u64) -> Result<u64> {
    peg(a, b).map_err(|err| CompilerError::Decode(format!("peg axis: {err}")))
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

fn axis_big_to_u64(axis: &BigUint) -> Result<u64> {
    let location = std::panic::Location::caller();
    u64::try_from(axis).map_err(|_| {
        CompilerError::Decode(format!(
            "axis exceeds u64 at {}:{}: {axis}",
            location.file(),
            location.line()
        ))
    })
}

fn tend(vein: &[Option<BigUint>]) -> Result<u64> {
    axis_big_to_u64(&tend_big(vein)?)
}

fn tend_big(vein: &[Option<BigUint>]) -> Result<BigUint> {
    let mut axis = BigUint::from(1u32);
    for step in vein.iter().rev() {
        let step = step.clone().unwrap_or_else(|| BigUint::from(1u32));
        axis = peg_axis_big_pair(axis, &step)?;
    }
    Ok(axis)
}

/// Chunked mint of a `=> p1 => … => body` compose chain. Mirrors `mint_tsgr`'s
/// composition exactly — each pre-body layer minted with goal `%noun` against
/// the carried subject, the body with the outer `gol`, formulas folded right via
/// `comb` — but mints each layer in its OWN fresh `Ut`/slab, carrying only the
/// subject type and per-layer formulas back into `out_slab` and dropping each
/// layer's working slab. Peak working memory bounds to one layer + the carried
/// subject instead of the cumulative whole — the basis for a bounded native
/// prelude mint (Step 2).
///
/// Output is byte-identical to monolithic mint when each layer's cores have full
/// batteries (so the per-layer lazy resolvers, dropped with each working slab,
/// are unnecessary for cross-layer name resolution). The
/// `chunked_tisgar_chain_matches_monolithic_mint` test guards that invariant.
///
/// NOTE: `out_slab` still accumulates each carried subject copy (only the latest
/// is live); reclaiming the stale copies (ping-pong out-slabs or
/// checkpoint/rewind around the subject copy) is the memory optimization to add
/// once correctness is established.
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
        // Each layer mints in a FRESH slab. The per-`Ut` `cx: Context::new()`
        // (constructed in `Ut::new` below) gives this layer a fresh native intern
        // table + lowering memos (live_to_noun / the nest cache) — they key on
        // canonical `Rc` pointers but hold slab-bound nouns, and would otherwise
        // hand this layer a noun allocated in a prior layer's (dropped) slab.
        // Cross-layer state crosses via the `subject`/`gol` nouns copied in below
        // and is re-interned fresh within this layer.
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
        formula = comb(out_slab, head, formula)?;
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
// Native-shadow type constructors (native-types migration, INC1).
//
// Each `ty_*_n` builds the SAME noun as its `ty_*` sibling — byte-identical, via
// the exact same code — AND the corresponding interned native `Rc<Type>`, built
// from its children's ALREADY-native `Rc<Type>` (O(n): no re-decode of children;
// the shared Rc is reused even when the noun side rebuilds). Branch constructors
// mirror hoon-138's collapse rules by reading the RESULT noun's tag, so a
// collapsed core/face/hint yields Void/Noun native exactly when the noun does.
// Leaf constructors (atom/fork/bool) capture leaf content via the memoized
// `native_of` (O(1) on a freshly-built leaf noun). All native nodes intern
// through the one shared thread-local table (`live_intern`/`native_of`).
//
// Additive: the noun-only `ty_*` call sites are untouched; callers migrate to
// `_n` incrementally (INC2+). Validated per-node by `assert_native_eq`.
// ---------------------------------------------------------------------------
use std::rc::Rc as NRc;

use crate::native::ir::intern::{
    cons_cell, cons_core, cons_face, cons_hint, cons_noun, cons_void,
    core_mint_cache_lookup as native_core_mint_cache_lookup,
    core_mint_cache_store as native_core_mint_cache_store,
    crop_cache_lookup as native_crop_cache_lookup, crop_cache_store as native_crop_cache_store,
    fish_cache_lookup as native_fish_cache_lookup, fish_cache_store as native_fish_cache_store,
    fuse_cache_lookup as native_fuse_cache_lookup, fuse_cache_store as native_fuse_cache_store,
    legset_memo_lookup, legset_memo_store, live_intern, live_leaf_to_noun, live_to_noun,
    mint_cache_lookup as native_mint_cache_lookup, mint_cache_store as native_mint_cache_store,
    mull_cache_lookup as native_mull_cache_lookup, mull_cache_store as native_mull_cache_store,
    native_of, native_of_mug_candidates, native_of_mug_insert, nest_cache_lookup, nest_cache_store,
    Context,
};
use crate::native::ir::leaf::Leaf as NLeaf;
use crate::native::ir::ty::{garb_native, Garb as NGarb, Type as NTy};

#[allow(dead_code)]
fn ty_noun_n(cx: &mut Context, slab: &mut NounSlab) -> (Noun, NRc<NTy>) {
    (ty_noun(slab), live_intern(cx, NTy::Noun))
}

#[allow(dead_code)]
fn ty_void_n(cx: &mut Context, slab: &mut NounSlab) -> (Noun, NRc<NTy>) {
    (ty_void(slab), live_intern(cx, NTy::Void))
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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
            let tool_leaf = NLeaf::from_noun_raw(tool, &slab.noun_space());
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

#[allow(dead_code)]
fn ty_face_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    name: &str,
    inner: (Noun, NRc<NTy>),
) -> (Noun, NRc<NTy>) {
    let name_noun = term_to_noun(slab, name);
    ty_face_tool_n(cx, slab, name_noun, inner)
}

#[allow(dead_code)]
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
            // noun = [%hint [inner note] payload]; capture the actual [inner note]
            // head ty_hint built (no redundant allocation).
            let head = noun
                .in_space(&space)
                .as_cell()
                .and_then(|c| c.tail().as_cell())
                .map(|c| c.head().noun())
                .expect("ty_hint_n: hint noun shape");
            let head_leaf = NLeaf::from_noun_raw(head, &space);
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

#[allow(dead_code)]
fn ty_hold_n(
    cx: &mut Context,
    slab: &mut NounSlab,
    inner: (Noun, NRc<NTy>),
    hoon: Noun,
) -> (Noun, NRc<NTy>) {
    let noun = ty_hold(slab, inner.0, hoon);
    let gene = NLeaf::from_noun_raw(hoon, &slab.noun_space());
    let native = live_intern(
        cx,
        NTy::Hold {
            subject: inner.1,
            gene,
        },
    );
    (noun, native)
}

#[allow(dead_code)]
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
            let rest_leaf = NLeaf::from_noun_raw(rest, &slab.noun_space());
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

#[allow(dead_code)]
fn ty_fork_n(cx: &mut Context, slab: &mut NounSlab, options: Vec<Noun>) -> (Noun, NRc<NTy>) {
    let noun = ty_fork(slab, options);
    let native = native_of(cx, noun, &slab.noun_space()).expect("ty_fork_n native");
    (noun, native)
}

#[allow(dead_code)]
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
    fn native_ctors_byte_exact_all_tags_and_collapses() {
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
        // richer cell-leaf gene (Jammed cell leaf round-trip: jam->cue->copy)
        let subj2 = ty_atom_n(&mut cx, &mut slab, "ud", None);
        let g_inner = T(&mut slab, &[D(1), D(2)]);
        let gene2 = T(&mut slab, &[g_inner, D(3)]);
        let (n, t) = ty_hold_n(&mut cx, &mut slab, subj2, gene2);
        check(&slab, n, &t);

        // core non-collapse. garb = [nym poly vair]; D(0) is no longer a valid
        // garb noun (the native `Garb` decodes the [nym poly vair] cell shape).
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
        let (n, t) = cell_type_n(&mut cx, &mut slab, h, tl).unwrap();
        check(&slab, n, &t);
        let hv = ty_void_n(&mut cx, &mut slab);
        let tl2 = ty_noun_n(&mut cx, &mut slab);
        let (n, t) = cell_type_n(&mut cx, &mut slab, hv, tl2).unwrap();
        check(&slab, n, &t);
        assert!(
            matches!(&*t, NTy::Void),
            "cell(void,_) must collapse to Void"
        );
        let h2 = ty_noun_n(&mut cx, &mut slab);
        let tv = ty_void_n(&mut cx, &mut slab);
        let (n, t) = cell_type_n(&mut cx, &mut slab, h2, tv).unwrap();
        check(&slab, n, &t);
        assert!(
            matches!(&*t, NTy::Void),
            "cell(_,void) must collapse to Void"
        );
    }

    #[test]
    fn native_method_wrappers_byte_exact() {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);

        // hint_type_n: non-collapse + void/noun collapse (returns payload native)
        let payload = ty_atom_n(&mut ut.cx, ut.slab, "ud", None);
        let note = term_to_noun(ut.slab, "fast");
        let (n, t) = ut.hint_type_n(D(0), note, payload).unwrap();
        assert_native_eq(n, &t, &ut.slab.noun_space());
        let pv = ty_void_n(&mut ut.cx, ut.slab);
        let note2 = term_to_noun(ut.slab, "fast");
        let (n, t) = ut.hint_type_n(D(0), note2, pv).unwrap();
        assert_native_eq(n, &t, &ut.slab.noun_space());
        assert!(matches!(&*t, NTy::Void));
        let pn = ty_noun_n(&mut ut.cx, ut.slab);
        let note3 = term_to_noun(ut.slab, "fast");
        let (n, t) = ut.hint_type_n(D(0), note3, pn).unwrap();
        assert_native_eq(n, &t, &ut.slab.noun_space());
        assert!(matches!(&*t, NTy::Noun));

        // fork_from_options_n: multi-option + single-collapse + empty->void
        let o1 = ty_atom_n(&mut ut.cx, ut.slab, "f", Some(D(0))).0;
        let o2 = ty_atom_n(&mut ut.cx, ut.slab, "f", Some(D(1))).0;
        let (n, t) = ut.fork_from_options_n(vec![o1, o2]).unwrap();
        assert_native_eq(n, &t, &ut.slab.noun_space());
        let single = ty_atom_n(&mut ut.cx, ut.slab, "ud", None).0;
        let (n, t) = ut.fork_from_options_n(vec![single]).unwrap();
        assert_native_eq(n, &t, &ut.slab.noun_space());
        let (n, t) = ut.fork_from_options_n(vec![]).unwrap();
        assert_native_eq(n, &t, &ut.slab.noun_space());
        assert!(matches!(&*t, NTy::Void), "empty fork must collapse to Void");
    }

    // STEP 1 unit: reachable_legs of a Hold-over-Cell DAG returns exactly the
    // hold's leg-id; non-hold types return ∅; and the scoped-fan intersection is 0
    // when the active set is disjoint from the legset (collapses to the empty-fan
    // key). Also checks the legset/intern are pure (memoized) per pointer.
    #[test]
    fn reachable_legs_and_scoped_fan_intersection() {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);

        // A plain cell type has no holds.
        let a = ty_atom(ut.slab, "ud", None);
        let b = ty_atom(ut.slab, "t", None);
        let cell_noun = ty_cell(ut.slab, a, b);
        let cell = ut.native_of_cached(cell_noun).unwrap();
        let legs = ut.reachable_legs(&cell).unwrap();
        assert!(legs.is_empty(), "cell of atoms has no reachable legs");

        // Build [%hold cell gen] over the cell above; legset = {leg_id(hold)}.
        let gen = T(ut.slab, &[D(1), D(2)]); // a trivial gene noun
        let hold_noun = ty_hold(ut.slab, cell_noun, gen);
        let hold = ut.native_of_cached(hold_noun).unwrap();
        let hold_legs = ut.reachable_legs(&hold).unwrap();
        assert_eq!(
            hold_legs.len(),
            1,
            "single hold has exactly one reachable leg"
        );
        let leg_id = ut.hold_repo_fan_leg_id_for_hold_native(&hold).unwrap();
        assert_eq!(hold_legs[0], leg_id, "legset is the hold's own leg-id");

        // Wrap the hold in a cell; legset is still {leg_id}.
        let other = ty_noun(ut.slab);
        let outer_noun = ty_cell(ut.slab, hold_noun, other);
        let outer = ut.native_of_cached(outer_noun).unwrap();
        let outer_legs = ut.reachable_legs(&outer).unwrap();
        assert_eq!(
            outer_legs.as_ref(),
            &[leg_id],
            "cell-wrapped hold keeps the leg"
        );

        // Memoized: second call returns the same Rc-shared slice content.
        let outer_legs2 = ut.reachable_legs(&outer).unwrap();
        assert_eq!(outer_legs.as_ref(), outer_legs2.as_ref());

        // intersect-empty -> intern_fan_subset_id returns the empty sentinel 0.
        assert_eq!(ut.intern_fan_subset_id(&[]), 0);
        let inter = Ut::intersect_sorted_legs(&[leg_id + 7, leg_id + 9], &outer_legs);
        assert!(
            inter.is_empty(),
            "disjoint active set -> empty intersection"
        );

        // intersect-nonempty -> a stable nonzero id, deterministic across calls.
        let inter2 = Ut::intersect_sorted_legs(&[leg_id], &outer_legs);
        assert_eq!(inter2, vec![leg_id]);
        let id1 = ut.intern_fan_subset_id(&inter2);
        let id2 = ut.intern_fan_subset_id(&inter2);
        assert_ne!(id1, 0);
        assert_eq!(id1, id2, "equal subsets map to one stable id");

        // merge/intersect helper sanity.
        assert_eq!(Ut::merge_sorted_legs(&[1, 3], &[2, 3, 4]), vec![1, 2, 3, 4]);
        assert_eq!(
            Ut::intersect_sorted_legs(&[1, 3, 5], &[3, 5, 7]),
            vec![3, 5]
        );
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
    example(spec, 1, &hay, &cox, &bug, &nut, &def)
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
        Limb::Axis(axis) => Ok(slot_formula_axis(slab, *axis)),
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
            let fragment_noun = noun_u64(slab, *fragment);
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
            let axis_noun = noun_u64(slab, *axis);
            T(slab, &[D(10), axis_noun, core_noun])
        }
        Edit((axis, new), core) => {
            let new_noun = nock_to_noun(slab, new);
            let core_noun = nock_to_noun(slab, core);
            let axis_noun = noun_u64(slab, *axis);
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
        AxisSelect(axis) => noun_u64(slab, *axis),
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

fn const_bool_formula(slab: &mut NounSlab, value: bool) -> Noun {
    let atom = if value { 0 } else { 1 };
    T(slab, &[D(1), D(atom)])
}

fn is_const_bool_formula(formula: Noun, value: bool, space: &NounSpace) -> bool {
    let Ok((head, tail)) = noun_pair(formula, space) else {
        return false;
    };
    if !noun_eq_direct(head, 1, space) {
        return false;
    }
    let expected = if value { 0 } else { 1 };
    noun_eq_direct(tail, expected, space)
}

fn is_crash_formula(formula: Noun, space: &NounSpace) -> bool {
    let Ok((head, tail)) = noun_pair(formula, space) else {
        return false;
    };
    noun_eq_direct(head, 0, space) && noun_eq_direct(tail, 0, space)
}

fn flip_formula(slab: &mut NounSlab, dyr: Noun) -> Result<Noun> {
    let space = slab.noun_space();
    if is_const_bool_formula(dyr, true, &space) {
        return Ok(const_bool_formula(slab, false));
    }
    if is_const_bool_formula(dyr, false, &space) {
        return Ok(const_bool_formula(slab, true));
    }
    if is_crash_formula(dyr, &space) {
        return Ok(dyr);
    }
    let false_formula = const_bool_formula(slab, false);
    let true_formula = const_bool_formula(slab, true);
    Ok(T(slab, &[D(6), dyr, false_formula, true_formula]))
}

fn flan_formula(slab: &mut NounSlab, bos: Noun, nif: Noun) -> Result<Noun> {
    let space = slab.noun_space();
    if noun_eq(bos, nif, &space)?
        || is_const_bool_formula(bos, false, &space)
        || is_const_bool_formula(nif, true, &space)
        || is_crash_formula(bos, &space)
    {
        return Ok(bos);
    }
    if is_const_bool_formula(bos, true, &space)
        || is_const_bool_formula(nif, false, &space)
        || is_crash_formula(nif, &space)
    {
        return Ok(nif);
    }
    let false_formula = const_bool_formula(slab, false);
    Ok(T(slab, &[D(6), bos, nif, false_formula]))
}

fn flor_formula(slab: &mut NounSlab, bos: Noun, nif: Noun) -> Result<Noun> {
    let space = slab.noun_space();
    if noun_eq(bos, nif, &space)?
        || is_const_bool_formula(bos, true, &space)
        || is_const_bool_formula(nif, false, &space)
        || is_crash_formula(bos, &space)
    {
        return Ok(bos);
    }
    if is_const_bool_formula(bos, false, &space)
        || is_const_bool_formula(nif, true, &space)
        || is_crash_formula(nif, &space)
    {
        return Ok(nif);
    }
    let true_formula = const_bool_formula(slab, true);
    Ok(T(slab, &[D(6), bos, true_formula, nif]))
}

fn and_formula(slab: &mut NounSlab, left: Noun, right: Noun) -> Result<Noun> {
    let false_formula = const_bool_formula(slab, false);
    cond(slab, left, right, false_formula)
}

fn crash_formula(slab: &mut NounSlab) -> Noun {
    T(slab, &[D(0), D(0)])
}

fn cell_type(slab: &mut NounSlab, head: Noun, tail: Noun) -> Result<Noun> {
    // Native-shadow (INC6, flag-gated): cells are the most common structural type
    // and thread head/tail natives. Under the flag, build via cell_type_n (which
    // mirrors the cell(void,_)/cell(_,void)->void collapse) and assert byte-exact;
    // the shipping path is the unchanged noun-only logic below.
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
    if type_tag(head, &space)? == "void" {
        return Ok(ty_void(slab));
    }
    if type_tag(tail, &space)? == "void" {
        return Ok(ty_void(slab));
    }
    Ok(ty_cell(slab, head, tail))
}

/// Native-shadow `cell_type` (INC2): mirrors the cell(void,_)/cell(_,void)->void
/// collapse, returning the native Void in those cases and `ty_cell_n` otherwise.
#[allow(dead_code)]
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
