//! Editor type summaries: one short line per located node for hover and
//! signature lenses, printed while type-checking.
//!
//! Holds are the interesting case. `++fire` (hoon-138.hoon:9529) types every
//! arm reference and gate call as `[%hold dox gene]`: `gene` is the arm's
//! body and `dox` the core it will be played against, so nearly every
//! non-literal product is a hold at recording time. These are named without
//! forcing them — no `+play`/`+repo` — from syntax alone: the expression the
//! node was compiled from (`^-  tape`, `$:kernel-state`, `(list @)`), the
//! arm the hold fires (`add`, `kernel-state`), or the arm body's own cast
//! (`^-  @`). A hold none of those describe stays `%hold`.

use super::*;

impl<'a> Ut<'a> {
    pub(super) const SEMANTIC_FACT_LIMIT: usize = 100_000;

    /// Depth past which the printer elides structure. Named holds count as
    /// leaves, so a mold name never costs its reader more than one level.
    const SEMANTIC_SUMMARY_DEPTH: usize = 4;

    /// Depth and width past which a spec printed from source elides. Specs
    /// are finite and usually small — `$@(~ [i=item t=(list item)])` is
    /// four deep — so they get more room than inferred structure.
    const SEMANTIC_SPEC_DEPTH: usize = 6;
    const SEMANTIC_SPEC_WIDTH: usize = 8;

    pub(super) fn record_semantic_type(&mut self, spot: &Spot, ty: &NRc<NTy>, gen: &Hoon) {
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
        let type_summary = self.semantic_type_summary(ty, Some(gen), 0);
        self.semantic_type_facts.push(CompilerSemanticFact {
            location,
            type_summary,
        });
    }

    /// Print `ty` for the editor. `gen` is the expression `ty` was inferred
    /// for, when known; it walks down alongside the type through faces and
    /// pairs so that a hold can be named by the syntax that produced it.
    fn semantic_type_summary(&mut self, ty: &NRc<NTy>, gen: Option<&Hoon>, depth: usize) -> String {
        if depth >= Self::SEMANTIC_SUMMARY_DEPTH {
            return "…".to_string();
        }
        let gen = gen.map(peel_hoon);
        match &**ty {
            NTy::Void => "%void".to_string(),
            NTy::Noun => "*".to_string(),
            NTy::Atom { aura, .. } => semantic_leaf_term(aura)
                .filter(|aura| !aura.is_empty())
                .map_or_else(|| "@".to_string(), |aura| format!("@{aura}")),
            NTy::Cell(head, tail) => {
                let (head, tail) = (head.clone(), tail.clone());
                let (head_gen, tail_gen) = match gen {
                    Some(Hoon::Pair(p, q)) | Some(Hoon::ColHep(p, q)) => {
                        (Some(p.as_ref()), Some(q.as_ref()))
                    }
                    _ => (None, None),
                };
                format!(
                    "[{} {}]",
                    self.semantic_type_summary(&head, head_gen, depth + 1),
                    self.semantic_type_summary(&tail, tail_gen, depth + 1)
                )
            }
            NTy::Core { payload, garb, .. } => {
                let (payload, name) = (payload.clone(), garb.nym.clone());
                let payload = self.semantic_type_summary(&payload, None, depth + 1);
                name.map_or_else(
                    || format!("core({payload})"),
                    |name| format!("{name}({payload})"),
                )
            }
            NTy::Face { tool, inner } => {
                let (tool, inner) = (tool.clone(), inner.clone());
                let inner_gen = match gen {
                    Some(Hoon::KetTis(_, inner)) => Some(inner.as_ref()),
                    _ => None,
                };
                let inner = self.semantic_type_summary(&inner, inner_gen, depth + 1);
                semantic_leaf_term(&tool).map_or_else(
                    || format!("face({inner})"),
                    |name| format!("{name}={inner}"),
                )
            }
            NTy::Hint { payload, .. } => {
                let payload = payload.clone();
                self.semantic_type_summary(&payload, gen, depth + 1)
            }
            NTy::Fork { .. } => "%fork".to_string(),
            NTy::Hold { .. } => self.semantic_hold_summary(ty, gen),
        }
    }

    /// Name a hold, or fall back to `%hold`.
    ///
    /// The expression it was inferred for is tried first when it reads as a
    /// type (`^-  tape`, `$:kernel-state` from a `%like` spec, `(list @)`
    /// from a `%make` spec): that is free and exact. Otherwise the hold's own
    /// structure names it, memoized per interned hold since the same hold
    /// recurs at every use of an arm. Only then does a call's head serve as
    /// the name — the node's own call (`(moat …)`) before the one in the arm
    /// body it fires.
    fn semantic_hold_summary(&mut self, hold: &NRc<NTy>, gen: Option<&Hoon>) -> String {
        if let Some(name) = gen.and_then(|gen| hoon_type_name(gen, 0)) {
            return name;
        }
        let id = hold.arena_id().0;
        let (named, called) = match self.semantic_hold_names.get(&id) {
            Some(cached) => cached.clone(),
            None => {
                let cached = self.semantic_hold_name(hold);
                self.semantic_hold_names.insert(id, cached.clone());
                cached
            }
        };
        named
            .map(|name| name.to_string())
            .or_else(|| gen.and_then(hoon_call_name))
            .or_else(|| called.map(|name| name.to_string()))
            .unwrap_or_else(|| "%hold".to_string())
    }

    /// Name a hold from its structure alone: a proper name, and separately
    /// the head of the call its arm body makes, for when nothing better is
    /// known.
    ///
    /// A hold fires one arm of its core. When that core is the normalizing
    /// gate of a mold this compiler built, the mold's name was noted at
    /// construction ([`Self::note_factory_mold`]); that is what a sample or
    /// cast written as `tape` or `kernel-state` produces. Otherwise the arm's
    /// name is the type's name (`add`, `kernel-state`), found by the gene's
    /// identity in the core's tomes. A `$` arm is a gate or trap and only a
    /// named core says anything, so its body is read instead: the cast it
    /// ends in (`^-  @`), or the `%made` note `++relative` leaves in a
    /// normalizer whose gate this compiler did not build (a cued subject).
    /// Decoding the gene is what `hoon_ast_lookup` already caches for every
    /// arm body the compiler has seen, so no fact pays for more than a cache
    /// probe.
    fn semantic_hold_name(&mut self, hold: &NRc<NTy>) -> SemanticHoldName {
        let NTy::Hold { subject, gene } = &**hold else {
            return (None, None);
        };
        let (subject, gene) = (subject.clone(), gene.clone());
        let gene_noun = live_leaf_to_noun(&mut self.cx, &gene, self.slab);
        if let Some(mold) = self
            .semantic_factory_molds
            .get(&unsafe { gene_noun.as_raw() })
        {
            return (Some(Arc::clone(mold)), None);
        }
        if let NTy::Core { garb, rest, .. } = &*subject {
            let (nym, rest) = (garb.nym.clone(), rest.clone());
            let rest_noun = live_leaf_to_noun(&mut self.cx, &rest, self.slab);
            match self.semantic_arm_name(rest_noun, gene_noun) {
                Some(name) if &*name != "$" => return (Some(name), None),
                Some(_) => {
                    if let Some(nym) = nym {
                        return (Some(Arc::from(nym)), None);
                    }
                }
                None => {}
            }
        }
        let Some(body) = self.hoon_ast_lookup(gene_noun) else {
            return (None, None);
        };
        let named = factory_mold_name(&body).or_else(|| hoon_type_name(&body, 0));
        let called = if named.is_none() {
            hoon_call_name(&body)
        } else {
            None
        };
        (named.map(Arc::from), called.map(Arc::from))
    }

    /// Remember the mold a normalizing gate was built for, keyed by the
    /// gate's `$` arm body: the gene of every hold that fires it.
    ///
    /// `++factory` builds the gate anonymously, so `+$  tape  (list @tD)`
    /// is nowhere in its type; only the `%ktcl` that minted it knew the spec.
    /// Runs on every mold construction, not just recorded checks, because the
    /// prelude's molds are built once per compiler and typed against long
    /// after: one arm lookup and a map insert per distinct mold.
    pub(super) fn note_factory_mold(&mut self, spec: &Spec, gate: &NRc<NTy>) {
        let NTy::Core { rest, .. } = &**gate else {
            return;
        };
        let rest = rest.clone();
        let rest_noun = live_leaf_to_noun(&mut self.cx, &rest, self.slab);
        let space = self.slab.noun_space();
        let Ok(tomes) = coil_tomes(rest_noun, &space) else {
            return;
        };
        let cog = term_to_noun(self.slab, "$");
        let Ok(Some((_, body))) = self.loot(cog, tomes) else {
            return;
        };
        let body_raw = unsafe { body.as_raw() };
        if self.semantic_factory_molds.contains_key(&body_raw) {
            return;
        }
        if let Some(name) = factory_spec_name(spec) {
            self.semantic_factory_molds
                .insert(body_raw, Arc::from(name));
        }
    }

    /// The name of the arm in `rest`'s tomes whose body is `gene`.
    ///
    /// `++fire` stores the arm body noun verbatim as the gene, so most arms
    /// match by identity; a hold that went through a noun round trip (a wet
    /// door's redone payload, a merged fork) carries a copy, which the mug
    /// comparison in `noun_eq` still settles in constant time per arm since
    /// mugs are cached on the cells.
    fn semantic_arm_name(&mut self, rest: Noun, gene: Noun) -> Option<Arc<str>> {
        let space = self.slab.noun_space();
        let tomes = coil_tomes(rest, &space).ok()?;
        let mut tome_nodes = vec![tomes];
        while let Some(current) = tome_nodes.pop() {
            let Ok(Some((node, left, right))) = map_node(current, &space) else {
                continue;
            };
            let arms = node
                .in_space(&space)
                .as_cell()
                .ok()?
                .tail()
                .as_cell()
                .ok()?
                .tail()
                .noun();
            let mut arm_nodes = vec![arms];
            while let Some(current) = arm_nodes.pop() {
                let Ok(Some((node, left, right))) = map_node(current, &space) else {
                    continue;
                };
                let entry = node.in_space(&space).as_cell().ok()?;
                if noun_eq(entry.tail().noun(), gene, &space).unwrap_or(false) {
                    return self.arm_key_term(entry.head().noun()).ok();
                }
                arm_nodes.push(left);
                arm_nodes.push(right);
            }
            tome_nodes.push(left);
            tome_nodes.push(right);
        }
        None
    }
}

pub(super) fn semantic_leaf_term(leaf: &Leaf) -> Option<String> {
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

/// Strip the wrappers that carry no type: debug spots, notes, and hints.
fn peel_hoon(mut hoon: &Hoon) -> &Hoon {
    loop {
        hoon = match hoon {
            Hoon::Dbug(_, inner)
            | Hoon::Note(_, inner)
            | Hoon::SigBar(_, inner)
            | Hoon::SigBuc(_, inner)
            | Hoon::SigCab(_, inner)
            | Hoon::SigFas(_, inner)
            | Hoon::SigGal(_, inner)
            | Hoon::SigGar(_, inner)
            | Hoon::SigLus(_, inner)
            | Hoon::SigTis(_, inner)
            | Hoon::SigZap(_, inner)
            | Hoon::SigCen(_, _, _, inner)
            | Hoon::SigPam(_, _, inner)
            | Hoon::SigWut(_, _, _, inner) => inner,
            _ => return hoon,
        };
    }
}

/// The expression `hoon` produces, when it reads as a type name: a cast, a
/// `$:mold` reference, a mold call, or one of those under the binding runes
/// (`=/`, `=+`, `?>`, …) that leave the product type alone. A bare wing is
/// not a type name here — `t` says nothing about what `t` holds — only
/// where syntax marks it as a mold (see [`hoon_mold_name`]).
fn hoon_type_name(hoon: &Hoon, depth: usize) -> Option<String> {
    if depth >= Ut::SEMANTIC_SPEC_DEPTH {
        return None;
    }
    match peel_hoon(hoon) {
        // `$:mold` is how a `%like`/`%make` spec asks for a mold's type.
        Hoon::TisGal(head, tail) if is_buc_limb(head) => hoon_mold_name(tail, depth),
        // `=<  body  subject`: the product is the body's.
        Hoon::TisGal(body, _) => hoon_type_name(body, depth),
        Hoon::KetHep(spec, _) | Hoon::KetTar(spec) => spec_text(spec, depth),
        Hoon::KetLus(example, _) => hoon_type_name(example, depth),
        // The example of a `^+`: the irregular `` `@t`x `` casts to `^+(`@t`0 …)`.
        Hoon::Rock(aura, _) | Hoon::Sand(aura, _) => Some(base_text(&BaseType::Atom(aura.clone()))),
        Hoon::Base(base) | Hoon::Bust(base) => Some(base_text(base)),
        // A string literal is a tape by definition (`%knit`, hoon-138.hoon:8344).
        Hoon::Knit(_) => Some("tape".to_string()),
        Hoon::KetTis(skin, inner) => Some(format!(
            "{}={}",
            skin_text(skin)?,
            hoon_type_name(inner, depth + 1)?
        )),
        // `(list @)`, as `++unfold` builds it from a `%make` spec or as written.
        Hoon::CenCol(head, args) if args.iter().all(is_spec_argument) => {
            let head = hoon_mold_name(head, depth)?;
            let mut text = format!("({head}");
            for arg in args {
                text.push(' ');
                text.push_str(&spec_argument_text(arg, depth + 1)?);
            }
            text.push(')');
            Some(text)
        }
        Hoon::KetSig(inner)
        | Hoon::KetBar(inner)
        | Hoon::KetPam(inner)
        | Hoon::KetWut(inner)
        | Hoon::TisGar(_, inner)
        | Hoon::TisLus(_, inner)
        | Hoon::TisBar(_, inner)
        | Hoon::TisCol(_, inner)
        | Hoon::TisCom(_, inner)
        | Hoon::TisHep(inner, _)
        | Hoon::TisDot(_, _, inner)
        | Hoon::TisFas(_, _, inner)
        | Hoon::TisMic(_, inner, _)
        | Hoon::TisTar(_, _, inner)
        | Hoon::TisKet(_, _, _, inner)
        | Hoon::TisWut(_, _, _, inner)
        | Hoon::WutGal(_, inner)
        | Hoon::WutGar(_, inner) => hoon_type_name(inner, depth),
        Hoon::TisSig(steps) => hoon_type_name(steps.last()?, depth),
        _ => None,
    }
}

/// A mold reference: a wing (`kernel-state`, `typ:typ`) where syntax says it
/// names a mold — under `$:`, as a call head, in `$_` — or any type name.
fn hoon_mold_name(hoon: &Hoon, depth: usize) -> Option<String> {
    match peel_hoon(hoon) {
        Hoon::Wing(wing) => Some(wing_text(wing)),
        Hoon::Limb(name) => Some(name.clone()),
        _ if depth >= Ut::SEMANTIC_SPEC_DEPTH => None,
        // `++home` re-roots a mold reference inside a normalizer: `=>(+7 tape)`.
        Hoon::TisGar(_, inner) => hoon_mold_name(inner, depth + 1),
        Hoon::TisGal(head, tail) if !is_buc_limb(head) => Some(format!(
            "{}:{}",
            hoon_mold_name(head, depth + 1)?,
            hoon_mold_name(tail, depth + 1)?
        )),
        other => hoon_type_name(other, depth),
    }
}

/// Whether a call argument is a spec, making the call a mold call rather than
/// a gate call: `^:` as `++unfold` wraps `%make` arguments, or a written
/// base like `@` in `(list @)`.
fn is_spec_argument(hoon: &Hoon) -> bool {
    matches!(
        peel_hoon(hoon),
        Hoon::KetCol(_) | Hoon::KetTar(_) | Hoon::Base(_) | Hoon::Bust(_)
    )
}

fn spec_argument_text(hoon: &Hoon, depth: usize) -> Option<String> {
    match peel_hoon(hoon) {
        Hoon::KetCol(spec) | Hoon::KetTar(spec) => spec_text(spec, depth),
        Hoon::Base(base) | Hoon::Bust(base) => Some(base_text(base)),
        _ => None,
    }
}

/// A call's head, for a hold whose only description is the call that made
/// it: `(add …)`, `bump:door`.
fn hoon_call_name(hoon: &Hoon) -> Option<String> {
    match peel_hoon(hoon) {
        Hoon::CenCol(head, _)
        | Hoon::CenHep(head, _)
        | Hoon::CenDot(_, head)
        | Hoon::CenLus(head, _, _)
        | Hoon::CenKet(head, _, _, _)
        | Hoon::MicCol(head, _) => Some(format!("({} …)", hoon_mold_name(head, 0)?)),
        Hoon::CenSig(wing, core, _) => Some(format!(
            "{}:{}",
            wing_text(wing),
            hoon_mold_name(core, 0).unwrap_or_else(|| "…".to_string())
        )),
        _ => None,
    }
}

/// What to call the mold a normalizing gate is built for: the name `+$`
/// gave it, else the spec itself when it has a short spelling.
fn factory_spec_name(spec: &Spec) -> Option<String> {
    match spec {
        Spec::Name(name, _) | Spec::Made((name, _), _) => Some(name.clone()),
        Spec::Dbug(_, inner) | Spec::Gist(_, inner) => factory_spec_name(inner),
        other => spec_text(other, 0),
    }
}

/// The mold name carried by the body of a normalizing gate.
///
/// `++factory` (hoon-138.hoon:7808) builds a `+$` mold's gate as
/// `|:(^~(spore) =+(relative =+(=(+14 +2) +6)))`, and `++relative` wraps its
/// normalizer in the `%made` note that `+$  name` attached to the spec. The
/// gate's `$` arm is otherwise anonymous, so that note is the mold's name.
fn factory_mold_name(body: &Hoon) -> Option<String> {
    let Hoon::TisLus(relative, check) = body else {
        return None;
    };
    let Hoon::TisLus(test, product) = check.as_ref() else {
        return None;
    };
    let Hoon::DotTis(left, right) = test.as_ref() else {
        return None;
    };
    let factory_shape = matches!(left.as_ref(), Hoon::Axis(axis) if axis.as_biguint() == &BigUint::from(14u32))
        && matches!(right.as_ref(), Hoon::Axis(axis) if axis.as_biguint() == &BigUint::from(2u32))
        && matches!(product.as_ref(), Hoon::Axis(axis) if axis.as_biguint() == &BigUint::from(6u32));
    if !factory_shape {
        return None;
    }
    // `++relative` decorates most specs directly; `$=` puts the note under
    // the face and `$&`/`$|` push the decorated normalizer with `=+`.
    let mut relative = relative.as_ref();
    loop {
        relative = match relative {
            Hoon::Note(Note::Made(name, _), _) => return Some(name.clone()),
            Hoon::Note(_, inner)
            | Hoon::Dbug(_, inner)
            | Hoon::KetTis(_, inner)
            | Hoon::TisLus(inner, _) => inner,
            _ => return None,
        };
    }
}

fn is_buc_limb(hoon: &Hoon) -> bool {
    match hoon {
        Hoon::Limb(name) => name == "$",
        Hoon::Wing(wing) => matches!(wing.as_slice(), [Limb::Term(name)] if name == "$"),
        _ => false,
    }
}

fn wing_text(wing: &WingType) -> String {
    wing.iter()
        .map(|limb| match limb {
            Limb::Term(name) => name.clone(),
            Limb::Axis(axis) => format!("+{axis}"),
            Limb::Parent(skip, name) => {
                let mut text = "^".repeat(usize::try_from(*skip).unwrap_or(0));
                text.push_str(name.as_deref().unwrap_or("."));
                text
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Print a spec as the reader wrote it, eliding past the depth cap. `None`
/// when a part has no short spelling (core interfaces, `$;`, unusual skins).
fn spec_text(spec: &Spec, depth: usize) -> Option<String> {
    if depth >= Ut::SEMANTIC_SPEC_DEPTH {
        return Some("…".to_string());
    }
    let list = |items: Vec<&Spec>, open: &str, close: &str| -> Option<String> {
        let mut text = open.to_string();
        for (index, item) in items.into_iter().enumerate() {
            if index > 0 {
                text.push(' ');
            }
            if index >= Ut::SEMANTIC_SPEC_WIDTH {
                text.push('…');
                break;
            }
            text.push_str(&spec_text(item, depth + 1)?);
        }
        text.push_str(close);
        Some(text)
    };
    let pair = |rune: &str, p: &Spec, q: &Spec| list(vec![p, q], &format!("{rune}("), ")");
    match spec {
        Spec::Base(base) => Some(base_text(base)),
        Spec::Leaf(aura, atom) => leaf_text(aura, atom),
        Spec::Like(wing, rest) => Some(
            std::iter::once(wing)
                .chain(rest)
                .map(wing_text)
                .collect::<Vec<_>>()
                .join(":"),
        ),
        Spec::Loop(name) => Some(name.clone()),
        Spec::Make(head, args) => {
            let head = hoon_mold_name(head, depth)?;
            let mut text = format!("({head}");
            for arg in args {
                text.push(' ');
                text.push_str(&spec_text(arg, depth + 1)?);
            }
            text.push(')');
            Some(text)
        }
        Spec::Dbug(_, inner)
        | Spec::Gist(_, inner)
        | Spec::Made(_, inner)
        | Spec::Name(_, inner)
        | Spec::Over(_, inner)
        | Spec::BucLus(_, inner)
        | Spec::BucSig(_, inner)
        | Spec::BucBuc(inner, _)
        | Spec::BucBar(inner, _)
        | Spec::BucPam(inner, _) => spec_text(inner, depth),
        // `$_` of an expression with no mold name — `(list _?>(?=(^ a) (b i.a)))`
        // in `++turn` — elides to the argument, not the whole spec.
        Spec::BucCab(hoon) => Some(
            hoon_mold_name(hoon, depth).map_or_else(|| "…".to_string(), |name| format!("_{name}")),
        ),
        Spec::BucCol(head, tail) => list(
            std::iter::once(head.as_ref()).chain(tail).collect(),
            "[",
            "]",
        ),
        Spec::BucTis(skin, inner) => Some(format!(
            "{}={}",
            skin_text(skin)?,
            spec_text(inner, depth + 1)?
        )),
        Spec::BucWut(head, tail) => list(
            std::iter::once(head.as_ref()).chain(tail).collect(),
            "?(",
            ")",
        ),
        Spec::BucCen(head, tail) => list(
            std::iter::once(head.as_ref()).chain(tail).collect(),
            "$%(",
            ")",
        ),
        Spec::BucPat(p, q) => pair("$@", p, q),
        Spec::BucKet(p, q) => pair("$^", p, q),
        Spec::BucHep(p, q) => pair("$-", p, q),
        Spec::BucGal(p, q) => pair("$<", p, q),
        Spec::BucGar(p, q) => pair("$>", p, q),
        Spec::BucMic(_)
        | Spec::BucDot(..)
        | Spec::BucFas(..)
        | Spec::BucTic(..)
        | Spec::BucZap(..) => None,
    }
}

fn base_text(base: &BaseType) -> String {
    match base {
        BaseType::NounExpr => "*".to_string(),
        BaseType::Cell => "^".to_string(),
        BaseType::Flag => "?".to_string(),
        BaseType::Null => "~".to_string(),
        BaseType::Void => "!!".to_string(),
        // The empty term is the aura of a bare `@`.
        BaseType::Atom(aura) if aura == "$" => "@".to_string(),
        BaseType::Atom(aura) => format!("@{aura}"),
    }
}

/// A constant spec: `%state`, `%1`, `~`, `%.y`, `'text'`.
fn leaf_text(aura: &str, atom: &ParsedAtom) -> Option<String> {
    let ParsedAtom::Small(value) = atom else {
        return None;
    };
    match aura {
        "n" if *value == 0 => Some("~".to_string()),
        "f" if *value == 0 => Some("%.y".to_string()),
        "f" if *value == 1 => Some("%.n".to_string()),
        "tas" | "t" | "ta" => {
            let bytes = value.to_le_bytes();
            let end = bytes
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(bytes.len());
            let text = std::str::from_utf8(&bytes[..end]).ok()?;
            if aura == "tas" {
                Some(format!("%{text}"))
            } else {
                Some(format!("'{text}'"))
            }
        }
        _ => Some(format!("%{value}")),
    }
}

fn skin_text(skin: &Skin) -> Option<String> {
    match skin {
        Skin::Term(name) => Some(name.clone()),
        Skin::Name(name, inner) if matches!(inner.as_ref(), Skin::Base(BaseType::NounExpr)) => {
            Some(name.clone())
        }
        Skin::Dbug(_, inner) | Skin::Help(_, inner) => skin_text(inner),
        _ => None,
    }
}
