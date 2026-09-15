//! Arena-indexed, hash-consed Nock formula DAG.
//!
//! Unlike the historical `Rc<Formula>` shadow, this representation is designed
//! to be the compiler's live formula value. Every child edge is a compact
//! [`FormulaId`], structurally equal nodes share one ID, and a formula is
//! materialized into the compile slab at most once. Quoted constants and hint
//! clues remain opaque leaves: importing `[1 constant]` never walks the
//! potentially large constant noun.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use nockapp::noun::slab::NounSlab;
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, T};
use num_bigint::BigUint;
use smallvec::SmallVec;

use super::leaf::Leaf;
use crate::errors::{CompilerError, Result};
use crate::native::noun::noun_pair;

/// Compile-local identity of one canonical Nock formula node.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct FormulaId(pub(crate) u32);

/// Arbitrary-precision Nock axis.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum Axis {
    Small(u64),
    Big(BigUint),
}

impl Axis {
    fn from_noun(noun: Noun, space: &NounSpace) -> Result<Self> {
        let atom = noun
            .in_space(space)
            .as_atom()
            .map_err(|_| CompilerError::Noun("native formula axis is not an atom".into()))?;
        match atom.as_u64() {
            Ok(value) => Ok(Self::Small(value)),
            Err(_) => Ok(Self::Big(BigUint::from_bytes_le(atom.as_ne_bytes()))),
        }
    }

    fn from_biguint(value: BigUint) -> Self {
        match u64::try_from(&value) {
            Ok(value) => Self::Small(value),
            Err(_) => Self::Big(value),
        }
    }

    fn to_biguint(&self) -> BigUint {
        match self {
            Self::Small(value) => BigUint::from(*value),
            Self::Big(value) => value.clone(),
        }
    }

    fn is_zero(&self) -> bool {
        match self {
            Self::Small(value) => *value == 0,
            Self::Big(value) => value.bits() == 0,
        }
    }

    fn is_one(&self) -> bool {
        match self {
            Self::Small(value) => *value == 1,
            Self::Big(value) => value == &BigUint::from(1u8),
        }
    }

    fn to_noun(&self, slab: &mut NounSlab) -> Noun {
        match self {
            Self::Small(value) => Atom::new(slab, *value).as_noun(),
            Self::Big(value) => Atom::from_bytes(slab, &value.to_bytes_le()).as_noun(),
        }
    }
}

/// Canonical formula node. Opcode variants follow the exact noun shape.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum FormulaNode {
    /// Opaque malformed or extension formula accepted by `%hand`. It is never
    /// inspected by smart constructors and re-emits exactly.
    Raw(Leaf),
    Slot(Axis),
    Quote(Leaf),
    Eval(FormulaId, FormulaId),
    Cell(FormulaId, FormulaId),
    Cond(FormulaId, FormulaId, FormulaId),
    Kick {
        axis: Axis,
        core: FormulaId,
    },
    Edit {
        axis: Axis,
        value: FormulaId,
        target: FormulaId,
    },
    Hint {
        clue: Leaf,
        body: FormulaId,
    },
    Op {
        code: u8,
        args: SmallVec<[FormulaId; 2]>,
    },
}

#[derive(Clone, Debug)]
struct FormulaEntry {
    node: FormulaNode,
    materialized: Option<Noun>,
}

/// Per-compile canonical formula storage.
#[derive(Default)]
pub struct FormulaArena {
    entries: Vec<FormulaEntry>,
    buckets: HashMap<u64, Vec<FormulaId>>,
    imported_by_raw: HashMap<u64, FormulaId>,
    pub requested: u64,
    pub hits: u64,
    pub imports: u64,
    pub materializations: u64,
}

impl FormulaArena {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn entry(&self, id: FormulaId) -> &FormulaEntry {
        &self.entries[id.0 as usize]
    }

    #[inline]
    fn entry_mut(&mut self, id: FormulaId) -> &mut FormulaEntry {
        &mut self.entries[id.0 as usize]
    }

    fn intern_with_materialized(
        &mut self,
        node: FormulaNode,
        materialized: Option<Noun>,
    ) -> FormulaId {
        self.requested += 1;
        let mut hasher = DefaultHasher::new();
        node.hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(bucket) = self.buckets.get(&hash) {
            if let Some(id) = bucket
                .iter()
                .copied()
                .find(|id| self.entry(*id).node == node)
            {
                self.hits += 1;
                if self.entry(id).materialized.is_none() && materialized.is_some() {
                    self.entry_mut(id).materialized = materialized;
                }
                return id;
            }
        }
        let id = FormulaId(
            u32::try_from(self.entries.len())
                .expect("one Honk compile cannot contain more than u32::MAX formula nodes"),
        );
        self.entries.push(FormulaEntry { node, materialized });
        self.buckets.entry(hash).or_default().push(id);
        id
    }

    fn intern(&mut self, node: FormulaNode) -> FormulaId {
        self.intern_with_materialized(node, None)
    }

    pub fn distinct(&self) -> usize {
        self.entries.len()
    }

    pub fn slot(&mut self, axis: BigUint) -> FormulaId {
        self.intern(FormulaNode::Slot(Axis::from_biguint(axis)))
    }

    pub fn slot_u64(&mut self, axis: u64) -> FormulaId {
        self.intern(FormulaNode::Slot(Axis::Small(axis)))
    }

    pub fn quote(&mut self, noun: Noun, space: &NounSpace) -> FormulaId {
        self.intern(FormulaNode::Quote(Leaf::from_noun_raw(noun, space)))
    }

    pub fn eval(&mut self, subject: FormulaId, formula: FormulaId) -> FormulaId {
        self.intern(FormulaNode::Eval(subject, formula))
    }

    pub fn cell(&mut self, head: FormulaId, tail: FormulaId) -> FormulaId {
        self.intern(FormulaNode::Cell(head, tail))
    }

    pub fn conditional(&mut self, test: FormulaId, yes: FormulaId, no: FormulaId) -> FormulaId {
        self.intern(FormulaNode::Cond(test, yes, no))
    }

    pub fn kick(&mut self, axis: BigUint, core: FormulaId) -> FormulaId {
        self.intern(FormulaNode::Kick {
            axis: Axis::from_biguint(axis),
            core,
        })
    }

    pub fn edit(&mut self, axis: BigUint, value: FormulaId, target: FormulaId) -> FormulaId {
        self.intern(FormulaNode::Edit {
            axis: Axis::from_biguint(axis),
            value,
            target,
        })
    }

    pub fn hint(&mut self, clue: Noun, body: FormulaId, space: &NounSpace) -> FormulaId {
        self.intern(FormulaNode::Hint {
            clue: Leaf::from_noun_raw(clue, space),
            body,
        })
    }

    pub fn op(&mut self, code: u8, args: &[FormulaId]) -> FormulaId {
        self.intern(FormulaNode::Op {
            code,
            args: SmallVec::from_slice(args),
        })
    }

    /// Import a legacy formula noun once. Quoted constants and hint clues are
    /// opaque leaves, so this is proportional to formula structure rather than
    /// to the data embedded in it.
    pub fn import(&mut self, noun: Noun, space: &NounSpace) -> Result<FormulaId> {
        if !noun.is_direct() {
            let raw = unsafe { noun.as_raw() };
            if let Some(id) = self.imported_by_raw.get(&raw).copied() {
                return Ok(id);
            }
        }
        self.imports += 1;
        let Ok((head, tail)) = noun_pair(noun, space) else {
            return Ok(self.intern_with_materialized(
                FormulaNode::Raw(Leaf::from_noun_raw(noun, space)),
                Some(noun),
            ));
        };
        let decoded = (|| -> Result<FormulaNode> {
            if head.in_space(space).as_cell().is_ok() {
                return Ok(FormulaNode::Cell(
                    self.import(head, space)?,
                    self.import(tail, space)?,
                ));
            }
            let op = head
                .in_space(space)
                .as_atom()
                .ok()
                .and_then(|atom| atom.as_u64().ok())
                .ok_or_else(|| CompilerError::Noun("native formula opcode is not small".into()))?;
            let pair = |noun| {
                noun_pair(noun, space)
                    .map_err(|_| CompilerError::Noun("native formula has malformed args".into()))
            };
            Ok(match op {
                0 => FormulaNode::Slot(Axis::from_noun(tail, space)?),
                1 => FormulaNode::Quote(Leaf::from_noun_raw(tail, space)),
                2 => {
                    let (subject, formula) = pair(tail)?;
                    FormulaNode::Eval(self.import(subject, space)?, self.import(formula, space)?)
                }
                3 | 4 => FormulaNode::Op {
                    code: op as u8,
                    args: smallvec::smallvec![self.import(tail, space)?],
                },
                5 | 7 | 8 | 12 | 13 => {
                    let (left, right) = pair(tail)?;
                    FormulaNode::Op {
                        code: op as u8,
                        args: smallvec::smallvec![
                            self.import(left, space)?,
                            self.import(right, space)?
                        ],
                    }
                }
                6 => {
                    let (test, branches) = pair(tail)?;
                    let (yes, no) = pair(branches)?;
                    FormulaNode::Cond(
                        self.import(test, space)?,
                        self.import(yes, space)?,
                        self.import(no, space)?,
                    )
                }
                9 => {
                    let (axis, core) = pair(tail)?;
                    FormulaNode::Kick {
                        axis: Axis::from_noun(axis, space)?,
                        core: self.import(core, space)?,
                    }
                }
                10 => {
                    let (edit, target) = pair(tail)?;
                    let (axis, value) = pair(edit)?;
                    FormulaNode::Edit {
                        axis: Axis::from_noun(axis, space)?,
                        value: self.import(value, space)?,
                        target: self.import(target, space)?,
                    }
                }
                11 => {
                    let (clue, body) = pair(tail)?;
                    FormulaNode::Hint {
                        clue: Leaf::from_noun_raw(clue, space),
                        body: self.import(body, space)?,
                    }
                }
                _ => FormulaNode::Raw(Leaf::from_noun_raw(noun, space)),
            })
        })();
        // `%hand` deliberately admits extension and malformed formulas. Treat
        // anything outside the canonical shape as an opaque leaf rather than
        // rejecting source the historical noun compiler accepted.
        let node = decoded.unwrap_or_else(|_| FormulaNode::Raw(Leaf::from_noun_raw(noun, space)));
        let id = self.intern_with_materialized(node, Some(noun));
        if !noun.is_direct() {
            self.imported_by_raw.insert(unsafe { noun.as_raw() }, id);
        }
        Ok(id)
    }

    fn leaf_noun(leaf: &Leaf, slab: &mut NounSlab) -> Noun {
        match leaf {
            // `Leaf::Direct` means "fits u64", which is wider than Nock's
            // tagged direct-atom range. Let the allocator choose direct versus
            // indirect representation instead of calling `D` unconditionally.
            Leaf::Direct(value) => Atom::new(slab, *value).as_noun(),
            Leaf::Noun(noun, _) => *noun,
            Leaf::Jammed(_, _) => leaf.to_noun(slab),
        }
    }

    /// Materialize a canonical formula into `slab`. Every arena node is emitted
    /// at most once, preserving DAG sharing in the resulting noun.
    pub fn materialize(&mut self, id: FormulaId, slab: &mut NounSlab) -> Noun {
        if let Some(noun) = self.entry(id).materialized {
            return noun;
        }
        let node = self.entry(id).node.clone();
        let noun = match node {
            FormulaNode::Raw(leaf) => Self::leaf_noun(&leaf, slab),
            FormulaNode::Slot(axis) => {
                let axis = axis.to_noun(slab);
                T(slab, &[D(0), axis])
            }
            FormulaNode::Quote(leaf) => {
                let value = Self::leaf_noun(&leaf, slab);
                T(slab, &[D(1), value])
            }
            FormulaNode::Eval(subject, formula) => {
                let subject = self.materialize(subject, slab);
                let formula = self.materialize(formula, slab);
                T(slab, &[D(2), subject, formula])
            }
            FormulaNode::Cell(head, tail) => {
                let head = self.materialize(head, slab);
                let tail = self.materialize(tail, slab);
                T(slab, &[head, tail])
            }
            FormulaNode::Cond(test, yes, no) => {
                let test = self.materialize(test, slab);
                let yes = self.materialize(yes, slab);
                let no = self.materialize(no, slab);
                T(slab, &[D(6), test, yes, no])
            }
            FormulaNode::Kick { axis, core } => {
                let axis = axis.to_noun(slab);
                let core = self.materialize(core, slab);
                T(slab, &[D(9), axis, core])
            }
            FormulaNode::Edit {
                axis,
                value,
                target,
            } => {
                let axis = axis.to_noun(slab);
                let value = self.materialize(value, slab);
                let edit = T(slab, &[axis, value]);
                let target = self.materialize(target, slab);
                T(slab, &[D(10), edit, target])
            }
            FormulaNode::Hint { clue, body } => {
                let clue = Self::leaf_noun(&clue, slab);
                let body = self.materialize(body, slab);
                T(slab, &[D(11), clue, body])
            }
            FormulaNode::Op { code, args } => match args.as_slice() {
                [arg] => {
                    let arg = self.materialize(*arg, slab);
                    T(slab, &[D(u64::from(code)), arg])
                }
                [left, right] => {
                    let left = self.materialize(*left, slab);
                    let right = self.materialize(*right, slab);
                    T(slab, &[D(u64::from(code)), left, right])
                }
                _ => unreachable!("native formula opcodes have arity one or two"),
            },
        };
        self.entry_mut(id).materialized = Some(noun);
        self.materializations += 1;
        noun
    }

    fn slot_axis(&self, id: FormulaId) -> Option<&Axis> {
        match &self.entry(id).node {
            FormulaNode::Slot(axis) => Some(axis),
            _ => None,
        }
    }

    fn quote_leaf(&self, id: FormulaId) -> Option<&Leaf> {
        match &self.entry(id).node {
            FormulaNode::Quote(leaf) => Some(leaf),
            _ => None,
        }
    }

    fn quote_direct(&self, id: FormulaId) -> Option<u64> {
        match self.quote_leaf(id) {
            Some(Leaf::Direct(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn is_slot(&self, id: FormulaId, axis: u64) -> bool {
        self.slot_axis(id)
            .is_some_and(|value| value.to_biguint() == BigUint::from(axis))
    }

    pub fn is_crash(&self, id: FormulaId) -> bool {
        self.is_slot(id, 0)
    }

    pub fn is_const_bool(&self, id: FormulaId, value: bool) -> bool {
        self.quote_direct(id) == Some(if value { 0 } else { 1 })
    }

    pub fn equal(&self, left: FormulaId, right: FormulaId) -> bool {
        left == right
    }

    pub fn cove(&self, mut formula: FormulaId) -> Result<BigUint> {
        loop {
            match &self.entry(formula).node {
                FormulaNode::Slot(axis) => return Ok(axis.to_biguint()),
                FormulaNode::Hint { body, .. } => formula = *body,
                _ => {
                    return Err(CompilerError::Noun("cove: not a slot formula".to_string()));
                }
            }
        }
    }

    /// `++cons`: collapse two constants to one quoted pair; otherwise autocons.
    pub fn cons(&mut self, slab: &mut NounSlab, head: FormulaId, tail: FormulaId) -> FormulaId {
        let constants = match (
            self.quote_leaf(head).cloned(),
            self.quote_leaf(tail).cloned(),
        ) {
            (Some(head), Some(tail)) => Some((head, tail)),
            _ => None,
        };
        if let Some((head, tail)) = constants {
            let head = Self::leaf_noun(&head, slab);
            let tail = Self::leaf_noun(&tail, slab);
            let pair = T(slab, &[head, tail]);
            return self.quote(pair, &slab.noun_space());
        }
        self.cell(head, tail)
    }

    /// `++comb`, preserving the noun implementation's check order.
    pub fn comb(&mut self, mal: FormulaId, buz: FormulaId) -> FormulaId {
        if let Some(axis) = self.slot_axis(mal).cloned() {
            if !axis.is_zero() {
                if let Some(other) = self.slot_axis(buz).cloned() {
                    if !other.is_zero() {
                        return self.intern(FormulaNode::Slot(Self::peg(&axis, &other)));
                    }
                }
                if let FormulaNode::Eval(left, right) = self.entry(buz).node {
                    if let (Some(left_axis), Some(right_axis)) = (
                        self.slot_axis(left).cloned(),
                        self.slot_axis(right).cloned(),
                    ) {
                        if !left_axis.is_zero() && !right_axis.is_zero() {
                            let left = self.intern(FormulaNode::Slot(Self::peg(&axis, &left_axis)));
                            let right =
                                self.intern(FormulaNode::Slot(Self::peg(&axis, &right_axis)));
                            return self.eval(left, right);
                        }
                    }
                }
                return self.op(7, &[mal, buz]);
            }
        }
        if let FormulaNode::Cell(head, tail) = self.entry(mal).node {
            if self.slot_axis(tail).is_some_and(Axis::is_one) {
                return self.op(8, &[head, buz]);
            }
        }
        if self.slot_axis(buz).is_some_and(Axis::is_one) {
            return mal;
        }
        self.op(7, &[mal, buz])
    }

    /// `++cond`, preserving constant-folding order.
    pub fn cond(&mut self, test: FormulaId, yes: FormulaId, no: FormulaId) -> FormulaId {
        match self.quote_direct(test) {
            Some(0) => return yes,
            Some(1) => return no,
            _ => {}
        }
        if self.slot_axis(test).is_some_and(Axis::is_zero) {
            return test;
        }
        self.conditional(test, yes, no)
    }

    pub fn flip(&mut self, formula: FormulaId) -> FormulaId {
        if self.is_const_bool(formula, true) {
            return self.intern(FormulaNode::Quote(Leaf::Direct(1)));
        }
        if self.is_const_bool(formula, false) {
            return self.intern(FormulaNode::Quote(Leaf::Direct(0)));
        }
        if self.is_crash(formula) {
            return formula;
        }
        let false_ = self.intern(FormulaNode::Quote(Leaf::Direct(1)));
        let true_ = self.intern(FormulaNode::Quote(Leaf::Direct(0)));
        self.conditional(formula, false_, true_)
    }

    pub fn flan(&mut self, left: FormulaId, right: FormulaId) -> FormulaId {
        if left == right
            || self.is_const_bool(left, false)
            || self.is_const_bool(right, true)
            || self.is_crash(left)
        {
            return left;
        }
        if self.is_const_bool(left, true)
            || self.is_const_bool(right, false)
            || self.is_crash(right)
        {
            return right;
        }
        let false_ = self.intern(FormulaNode::Quote(Leaf::Direct(1)));
        self.conditional(left, right, false_)
    }

    pub fn flor(&mut self, left: FormulaId, right: FormulaId) -> FormulaId {
        if left == right
            || self.is_const_bool(left, true)
            || self.is_const_bool(right, false)
            || self.is_crash(left)
        {
            return left;
        }
        if self.is_const_bool(left, false)
            || self.is_const_bool(right, true)
            || self.is_crash(right)
        {
            return right;
        }
        let true_ = self.intern(FormulaNode::Quote(Leaf::Direct(0)));
        self.conditional(left, true_, right)
    }

    pub fn and(&mut self, left: FormulaId, right: FormulaId) -> FormulaId {
        let false_ = self.intern(FormulaNode::Quote(Leaf::Direct(1)));
        self.cond(left, right, false_)
    }

    fn peg(left: &Axis, right: &Axis) -> Axis {
        let left = left.to_biguint();
        let right = right.to_biguint();
        let right_width = right.bits() - 1;
        let right_prefix = BigUint::from(1u8) << right_width;
        Axis::from_biguint((left << right_width) + (right - right_prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::formula as noun_formula;

    fn jam(mut slab: NounSlab, noun: Noun) -> Vec<u8> {
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    #[test]
    fn import_and_materialize_roundtrip_without_descending_into_quote() {
        let mut slab = NounSlab::new();
        let deep_tail = T(&mut slab, &[D(43), D(44)]);
        let deep_constant = T(&mut slab, &[D(42), deep_tail]);
        let noun = T(&mut slab, &[D(1), deep_constant]);
        let mut arena = FormulaArena::new();
        let id = arena.import(noun, &slab.noun_space()).unwrap();
        assert_eq!(arena.distinct(), 1);
        assert!(unsafe { arena.materialize(id, &mut slab).raw_equals(&noun) });
    }

    #[test]
    fn hash_consing_collapses_equal_formula_nodes() {
        let mut arena = FormulaArena::new();
        let one_a = arena.slot_u64(1);
        let one_b = arena.slot_u64(1);
        let composed_a = arena.op(7, &[one_a, one_b]);
        let composed_b = arena.op(7, &[one_b, one_a]);
        assert_eq!(one_a, one_b);
        assert_eq!(composed_a, composed_b);
        assert_eq!(arena.distinct(), 2);
        assert_eq!(arena.hits, 2);
    }

    #[test]
    fn import_preserves_noncanonical_hand_formula_as_opaque() {
        let mut slab = NounSlab::new();
        // `%hand`'s historical AST encoder uses this noncanonical `%10`
        // shape. It is source data we must preserve, not reject or reinterpret.
        let core = T(&mut slab, &[D(0), D(1)]);
        let noun = T(&mut slab, &[D(10), D(2), core]);
        let mut arena = FormulaArena::new();
        let id = arena.import(noun, &slab.noun_space()).unwrap();
        let emitted = arena.materialize(id, &mut slab);
        assert!(unsafe { emitted.raw_equals(&noun) });
    }

    #[test]
    fn materialize_u64_leaf_outside_direct_atom_range() {
        let mut slab = NounSlab::new();
        let value = Atom::new(&mut slab, u64::MAX).as_noun();
        let mut arena = FormulaArena::new();
        let id = arena.quote(value, &slab.noun_space());
        let formula = arena.materialize(id, &mut slab);
        let (_, emitted) = noun_pair(formula, &slab.noun_space()).unwrap();
        assert_eq!(
            emitted
                .in_space(&slab.noun_space())
                .as_atom()
                .unwrap()
                .as_u64()
                .unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn smart_constructors_match_legacy_nouns() {
        let mut old = NounSlab::new();
        let old_a = T(&mut old, &[D(0), D(6)]);
        let old_b = T(&mut old, &[D(0), D(7)]);
        let old_comb = noun_formula::comb(&mut old, old_a, old_b).unwrap();
        let old_true = T(&mut old, &[D(1), D(0)]);
        let old_cond = noun_formula::cond(&mut old, old_true, old_comb, old_b).unwrap();

        let mut new = NounSlab::new();
        let mut arena = FormulaArena::new();
        let a = arena.slot_u64(6);
        let b = arena.slot_u64(7);
        let combined = arena.comb(a, b);
        let true_ = arena.quote(D(0), &new.noun_space());
        let condition = arena.cond(true_, combined, b);
        let emitted = arena.materialize(condition, &mut new);

        assert_eq!(jam(old, old_cond), jam(new, emitted));
    }
}
