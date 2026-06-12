pub mod formula;
pub mod hot;
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
            let (ty, formula) = ut.mint(sut, gol, expr)?;

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
