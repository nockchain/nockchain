#![allow(
    clippy::collapsible_if, clippy::derivable_impls, clippy::for_kv_map,
    clippy::manual_is_ascii_check, clippy::manual_is_multiple_of,
    clippy::manual_pattern_char_comparison, clippy::match_like_matches_macro,
    clippy::nonminimal_bool, clippy::only_used_in_recursion, clippy::overly_complex_bool_expr,
    clippy::result_large_err, clippy::too_many_arguments, clippy::type_complexity,
    clippy::unnecessary_cast, clippy::unnecessary_sort_by, clippy::unused_enumerate_index
)]

pub mod arm_map;
pub mod artifact;
pub mod build_cache;
pub mod errors;
pub mod nasm_bridge;
pub mod native;
pub mod pipeline;
pub mod types;

use hatch::ast::hoon::Hoon;
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounAllocator, NounSpace, D, T};

use crate::arm_map::ArmMap;
use crate::errors::Result;
pub use crate::errors::{CompilerErrorKind, CompilerErrorLocation, CompilerErrorMetadata};
use crate::native::NativeCompiler;
use crate::types::TypeNoun;

pub struct Compiler {
    native: NativeCompiler,
}

impl Compiler {
    pub async fn new() -> Result<Self> {
        let native = NativeCompiler::new().await?;
        Ok(Self { native })
    }

    pub fn compile_expr(&mut self, expr: &Hoon) -> Result<Compiled> {
        let compiled = self.native.compile_expr(expr)?;
        Ok(Compiled {
            slab: compiled.slab,
            formula: compiled.formula,
            ty: compiled.ty,
            arm_map: compiled.arm_map,
        })
    }

    pub fn compile_expr_with_vet(&mut self, expr: &Hoon, vet: bool) -> Result<Compiled> {
        let compiled = self.native.compile_expr_with_vet(expr, vet)?;
        Ok(Compiled {
            slab: compiled.slab,
            formula: compiled.formula,
            ty: compiled.ty,
            arm_map: compiled.arm_map,
        })
    }
}

pub struct Compiled {
    slab: NounSlab,
    formula: Noun,
    ty: TypeNoun,
    arm_map: ArmMap,
}

impl Compiled {
    pub fn ty(&self) -> &TypeNoun {
        &self.ty
    }

    pub fn noun_space(&self) -> NounSpace {
        self.slab.noun_space()
    }

    // Note: no public `formula() -> Noun` accessor. Returning the raw formula
    // noun unbound from `self.slab` is the "alien noun" hazard the provenance
    // audit warns about (a Noun whose validity depends on this Compiled's
    // private slab outliving every use). Callers get the formula only through
    // the slab-scoped `jam*` methods below, which keep the owner alive.

    pub fn arm_map(&self) -> &ArmMap {
        &self.arm_map
    }

    pub fn jam(&mut self) -> Vec<u8> {
        self.slab.set_root(self.formula);
        self.slab.jam().to_vec()
    }

    /// Return the compiled output in dynock format: `[type (trap nock)]`.
    ///
    /// This minimal dynock mode uses `%noun` as the type header for stable parity with hoonc.
    ///
    /// Kicking the trap yields the compiled nock as a constant noun, without evaluating it.
    pub fn jam_dynock(&mut self) -> Vec<u8> {
        let trap = wrap_formula_as_dynock_trap(&mut self.slab, self.formula);
        let noun_ty = crate::native::ut::ty_noun(&mut self.slab);
        let dynock = T(&mut self.slab, &[noun_ty, trap]);
        self.slab.set_root(dynock);
        self.slab.jam().to_vec()
    }

    /// Return typed dynock output: `[inferred-type (trap nock)]`.
    ///
    /// This retains the full inferred type tree in the header.
    pub fn jam_dynock_typed(&mut self) -> Vec<u8> {
        let trap = wrap_formula_as_dynock_trap(&mut self.slab, self.formula);
        let dynock = T(&mut self.slab, &[self.ty.noun(), trap]);
        self.slab.set_root(dynock);
        self.slab.jam().to_vec()
    }
}

/// Wrap a compiled nock formula in a trap that returns the formula as a constant.
///
/// This is the trap payload used by dynock output `[type (trap nock)]`.
pub fn wrap_formula_as_dynock_trap(slab: &mut NounSlab, formula: Noun) -> Noun {
    let battery = T(slab, &[D(1), formula]);
    T(slab, &[battery, D(0)])
}
