//! Native IR for honk's Hoon type system and Nock output.
//!
//! This is the implemented native-types architecture described by
//! `docs/native-compiler/ARENA-TYPE-IR.md`, `ARENA-FORMULA-IR.md`, and
//! `ARENA-SEMINOUN-IR.md`. Compiler types, formulas, intermediate values, and
//! seminouns are stored in context-owned arenas and referenced by dense IDs.
//! Nouns are retained or materialized only where compiler semantics require a
//! noun boundary. `crate::native::ut` uses these arenas throughout minting,
//! type operations, formula construction, and seminoun completion.

pub mod core;
pub mod formula;
pub mod formula_dag;
pub(crate) mod intern;
pub mod leaf;
pub(crate) mod semi_dag;
pub(crate) mod ty;
pub(crate) mod value_dag;

use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounSpace};

use crate::errors::{CompilerError, Result};

/// Check that the native Formula IR can represent `formula` and re-emit it byte
/// for byte: `from_noun(formula).to_noun() == formula`, compared by jam so
/// internal sharing is irrelevant. The compiler checks every minted formula this
/// way when `HONK_IR_ROUNDTRIP` is set.
pub fn roundtrip_check(formula: Noun, space: &NounSpace) -> Result<()> {
    let parsed = formula::Formula::from_noun(formula, space)?;
    let mut dst: NounSlab = NounSlab::new();
    let rebuilt = parsed.to_noun(&mut dst);
    dst.set_root(rebuilt);
    let rebuilt_jam = dst.jam();

    let mut orig: NounSlab = NounSlab::new();
    orig.copy_into(formula, space);
    let orig_jam = orig.jam();

    if orig_jam == rebuilt_jam {
        Ok(())
    } else {
        Err(CompilerError::Noun(format!(
            "native IR round-trip mismatch: orig={} bytes, rebuilt={} bytes",
            orig_jam.len(),
            rebuilt_jam.len()
        )))
    }
}

/// Measure hash-consing dedup on a real type: parse it into the native IR, intern
/// it bottom-up, and return `(unshared_nodes, distinct_nodes)`. The ratio shows
/// how much structurally equal duplication (from subject deepening) the intern
/// table collapses.
pub fn type_intern_stats(type_noun: Noun, space: &NounSpace) -> Result<(u64, u64)> {
    let parsed = ty::BoundaryType::from_noun(type_noun, space)?;
    let mut table = intern::TypeTable::new();
    let _ = table.intern_boundary(&parsed);
    Ok((table.interned_calls, table.distinct))
}

/// Type analogue of [`roundtrip_check`]: `from_noun(type).to_noun() == type`,
/// compared by jam. Run on real subject types when `HONK_IR_ROUNDTRIP` is set.
pub fn type_roundtrip_check(type_noun: Noun, space: &NounSpace) -> Result<()> {
    let parsed = ty::BoundaryType::from_noun(type_noun, space)?;
    let mut dst: NounSlab = NounSlab::new();
    let rebuilt = parsed.to_noun(&mut dst);
    dst.set_root(rebuilt);
    let rebuilt_jam = dst.jam();

    let mut orig: NounSlab = NounSlab::new();
    orig.copy_into(type_noun, space);
    let orig_jam = orig.jam();

    if orig_jam == rebuilt_jam {
        Ok(())
    } else {
        Err(CompilerError::Noun(format!(
            "native type IR round-trip mismatch: orig={} bytes, rebuilt={} bytes",
            orig_jam.len(),
            rebuilt_jam.len()
        )))
    }
}

/// Emit a native IR node as a byte-exact noun in `dst`.
///
/// Implemented for [`formula::Formula`]. Leaves are copied into `dst` rather than
/// spliced in as foreign pointers; see
/// `docs/native-compiler/PHASE0-PROVENANCE-DESIGN.md`.
pub trait ToNoun {
    fn to_noun(&self, dst: &mut NounSlab) -> Noun;
}
