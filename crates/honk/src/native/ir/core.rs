//! Unused model of cores, batteries, lazy batteries, holds, and forks.
//!
//! The compiler uses `ty::Type` and integer `LazyResolverId`s instead.
//! Constraints on a lazy battery shared by `Rc` identity: it must outlive every
//! type, formula, or fold that references it, its per-arm formula cache must use
//! the defining fan scope, and it must not be evicted while live. A `%hold` is a
//! finite lazy node (subject + gene), never a cyclic `Rc`, since cycles leak.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hatch::ast::hoon::Hoon;
use num_bigint::BigUint;

use super::formula::Formula;
use super::ty::Type;

/// A `%core` type.
pub struct Core {
    pub payload: Rc<Type>,
    pub garb: Garb,
    pub battery: Battery,
}

/// Core variance (`%gold`/`%iron`/`%lead`/`%zinc`).
#[derive(Clone, Copy)]
pub enum Garb {
    Gold,
    Iron,
    Lead,
    Zinc,
}

/// A core battery: fully resolved, or lazily resolved on demand.
pub enum Battery {
    Full(Rc<Formula>),
    Lazy(Rc<LazyBattery>),
}

/// On-demand arm compilation, shared by `Rc` identity.
pub struct LazyBattery {
    /// The core type arms are minted against.
    pub context: Rc<Type>,
    /// The arm sources as native AST, keyed by term.
    pub arms: Rc<ArmMap>,
    /// Per-arm compiled formulas, memoized for the whole compile. Resolution
    /// must use the defining fan scope, not the caller's; there is no scope
    /// field yet.
    pub cache: RefCell<HashMap<BigUint, Rc<Formula>>>,
}

/// Native arm map: term → native AST gene.
pub struct ArmMap {
    pub arms: HashMap<Rc<str>, Rc<Hoon>>,
}

/// A `%hold` recursive type: a finite node expanded on demand by repo/rest,
/// memoized on `Rc<Hold>` identity. Never a cyclic `Rc`.
pub struct Hold {
    pub subject: Rc<Type>,
    pub gene: Rc<Hoon>,
}

/// A `%fork` option set, stored as a `Vec`.
pub struct ForkSet {
    pub options: Vec<Rc<Type>>,
}
