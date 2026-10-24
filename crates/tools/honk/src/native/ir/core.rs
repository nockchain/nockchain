//! Cores, batteries, lazy batteries, holds, forks (plan §3.4–§3.6).
//!
//! Lazy cores replace the noun seminoun + integer `lazy_resolver_next_id`:
//! sharing is by `Rc<LazyBattery>` identity. But laziness is a lifetime/scope
//! contract (RT-05): a `LazyBattery` must outlive every type/formula/fold that
//! references it, its per-arm formula cache must preserve the **defining** fan
//! scope, and it must not be evicted while live. `%hold` is a FINITE lazy node
//! (subject + native gene), never a cyclic `Rc` (cycles leak — plan §3.6).
#![allow(dead_code)]

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

/// Core variance/metadata (`%gold`/`%iron`/`%lead`/`%zinc`); modeled concretely
/// in Phase 2.
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

/// On-demand arm compilation. Shared by `Rc` identity (no integer resolver id).
pub struct LazyBattery {
    /// The core type arms are minted against.
    pub context: Rc<Type>,
    /// The arm sources, keyed by term (native AST — no noun round-trip, RT-13).
    pub arms: Rc<ArmMap>,
    /// Per-arm compiled formulas, memoized for the whole compile. Resolution
    /// must use the DEFINING fan scope, not the caller's (RT-05) — the scope
    /// field is added with the native fan scope in Phase 3.
    pub cache: RefCell<HashMap<BigUint, Rc<Formula>>>,
    // pub fan_scope: FanScope,  // defining scope — Phase 3 (plan §3.5)
}

/// Native arm/tome map: term → native AST gene (replaces noun map values,
/// RT-13).
pub struct ArmMap {
    pub arms: HashMap<Rc<str>, Rc<Hoon>>,
}

/// A `%hold` recursive type — a FINITE node expanded on demand by repo/rest,
/// memoized on `Rc<Hold>` identity. Never a cyclic `Rc`.
pub struct Hold {
    pub subject: Rc<Type>,
    pub gene: Rc<Hoon>,
}

/// A `%fork` option set. Internally a canonical-ordered set (the skeleton uses a
/// `Vec`); the real canonical set + the exact Hoon-treap output serialization
/// land in Phase 2/5 (plan §3.4, RT-07).
pub struct ForkSet {
    pub options: Vec<Rc<Type>>,
}
