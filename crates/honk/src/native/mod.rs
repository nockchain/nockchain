// The arena migration kept the former `Rc<Type>` call shape by aliasing the
// one-word, `Copy` `TypeRef` handle as `Rc` inside native compiler modules.
// Existing `.clone()` calls are therefore zero-cost handle copies. Removing
// hundreds of them mechanically would obscure the semantic arena change; keep
// that cosmetic cleanup separate from the migration review.
#![allow(clippy::clone_on_copy)]

pub mod formula;
pub mod hot;
// Native compiler IR; see docs/native-compiler for the migration and
// performance-validation record.
pub mod ir;
pub mod noun;
pub mod ut;

use hatch::ast::hoon::Hoon;
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounAllocator};

use crate::arm_map::ArmMap;
use crate::errors::Result;
use crate::native::ut::Ut;
use crate::types::TypeNoun;

pub struct NativeCompiler;

impl NativeCompiler {
    fn with_large_stack<R>(f: impl FnOnce() -> Result<R>) -> Result<R> {
        stacker::maybe_grow(32 * 1024, 64 * 1024 * 1024, f)
    }

    pub async fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn compile_expr(&mut self, expr: &Hoon) -> Result<CompiledNative> {
        self.compile_expr_with_options(expr, true)
    }

    pub fn compile_expr_with_vet(&mut self, expr: &Hoon, vet: bool) -> Result<CompiledNative> {
        self.compile_expr_with_options(expr, vet)
    }

    pub fn compile_expr_with_options(&mut self, expr: &Hoon, vet: bool) -> Result<CompiledNative> {
        Self::with_large_stack(|| {
            let mut slab = NounSlab::new();
            let sut = crate::native::ut::ty_noun(&mut slab);
            let gol = crate::native::ut::ty_noun(&mut slab);
            let mut ut = Ut::new(&mut slab);
            ut.set_vet(vet);
            // mint is native now (C-final.1a); route the noun sut/gol through the
            // mint_noun bridge, which returns a noun type for CompiledNative/TypeNoun.
            let (ty, formula) = ut.mint_noun(sut, gol, expr)?;

            // Native-types migration Phase 1: flag-gated IR-completeness
            // invariant. When HONK_IR_ROUNDTRIP is set, assert the native
            // Formula IR can represent and re-emit every minted formula
            // byte-for-byte. Default-off → zero impact on the shipping path.
            if std::env::var_os("HONK_IR_ROUNDTRIP").is_some() {
                crate::native::ir::roundtrip_check(formula, &slab.noun_space())?;
            }

            let ty_noun = TypeNoun::new(ty);
            let space = slab.noun_space();
            let arm_map = ArmMap::from_type(&ty_noun, &space)?;
            Ok(CompiledNative {
                slab,
                ty: ty_noun,
                formula,
                arm_map,
            })
        })
    }
}

pub struct CompiledNative {
    pub slab: NounSlab,
    pub ty: TypeNoun,
    pub formula: Noun,
    pub arm_map: ArmMap,
}
