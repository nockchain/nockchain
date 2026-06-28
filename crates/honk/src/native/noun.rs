use hatch::ast::hoon::{NounExpr, ParsedAtom};
use nockapp::noun::slab::NounSlab;
use nockvm::ext::AtomExt;
use nockvm::mug::{calc_atom_mug_u32, calc_cell_mug_u32, get_mug, set_mug};
use nockvm::noun::{Atom, AtomHandle, Noun, NounAllocator, NounSpace, D, DIRECT_MAX, T};

use crate::errors::{CompilerError, Result};
use crate::native::ut::types::FastHashSet;

pub fn tag(noun: Noun, space: &NounSpace) -> Result<String> {
    let noun = noun.in_space(space);
    if let Ok(atom) = noun.as_atom() {
        return atom
            .into_string()
            .map_err(|err| CompilerError::Decode(format!("tag atom decode failed: {err}")));
    }
    let cell = noun
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("tag noun not cell: {err}")))?;
    let atom = cell
        .head()
        .as_atom()
        .map_err(|err| CompilerError::Decode(format!("tag head not atom: {err}")))?;
    atom.into_string()
        .map_err(|err| CompilerError::Decode(format!("tag head decode failed: {err}")))
}

pub fn term_to_noun<A: NounAllocator>(allocator: &mut A, term: &str) -> Noun {
    if term == "$" {
        return D(0);
    }
    let atom = Atom::from_bytes(allocator, term.as_bytes());
    atom.as_noun()
}

pub fn opt_to_noun<A: NounAllocator>(allocator: &mut A, opt: Option<Noun>) -> Noun {
    match opt {
        None => D(0),
        Some(value) => T(allocator, &[D(0), value]),
    }
}

pub fn opt_from_noun(noun: Noun, space: &NounSpace) -> Result<Option<Noun>> {
    let noun = noun.in_space(space);
    if let Ok(atom) = noun.as_atom() {
        let val = atom
            .as_u64()
            .map_err(|err| CompilerError::Decode(format!("opt atom decode failed: {err}")))?;
        if val == 0 {
            return Ok(None);
        }
        return Err(CompilerError::Decode(format!(
            "unexpected opt atom value: {val}"
        )));
    }
    let cell = noun
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("opt noun not cell: {err}")))?;
    let head = cell.head();
    let head_atom = head
        .as_atom()
        .map_err(|err| CompilerError::Decode(format!("opt head not atom: {err}")))?;
    let head_val = head_atom
        .as_u64()
        .map_err(|err| CompilerError::Decode(format!("opt head decode failed: {err}")))?;
    if head_val != 0 {
        return Err(CompilerError::Decode(format!(
            "unexpected opt head value: {head_val}"
        )));
    }
    Ok(Some(cell.tail().noun()))
}

pub fn noun_expr_to_noun(slab: &mut NounSlab, expr: &NounExpr) -> Noun {
    match expr {
        NounExpr::ParsedAtom(atom) => parsed_atom_to_noun(slab, atom),
        NounExpr::Cell(head, tail) => {
            let head = noun_expr_to_noun(slab, head);
            let tail = noun_expr_to_noun(slab, tail);
            T(slab, &[head, tail])
        }
    }
}

pub fn parsed_atom_to_noun<A: NounAllocator>(allocator: &mut A, atom: &ParsedAtom) -> Noun {
    match atom {
        ParsedAtom::Small(n) => {
            if *n <= DIRECT_MAX as u128 {
                D(*n as u64)
            } else {
                let bytes = n.to_le_bytes();
                let trimmed_len = bytes.iter().rev().take_while(|&&b| b == 0).count();
                let trimmed = &bytes[..bytes.len() - trimmed_len];
                let bytes_slice = if trimmed.is_empty() { &[0u8] } else { trimmed };
                Atom::from_bytes(allocator, bytes_slice).as_noun()
            }
        }
        ParsedAtom::Big(b) => {
            let mut bytes = b.to_bytes_le();
            if bytes.is_empty() {
                bytes.push(0);
            }
            Atom::from_bytes(allocator, bytes.as_slice()).as_noun()
        }
    }
}

pub fn vec_to_list<A: NounAllocator>(allocator: &mut A, items: Vec<Noun>) -> Noun {
    let mut out = D(0);
    for item in items.into_iter().rev() {
        out = T(allocator, &[item, out]);
    }
    out
}

pub fn list_to_vec(noun: Noun, space: &NounSpace) -> Result<Vec<Noun>> {
    let mut out = Vec::new();
    let mut cursor = noun;
    loop {
        let cursor_handle = cursor.in_space(space);
        if let Ok(atom) = cursor_handle.as_atom() {
            let val = atom
                .as_u64()
                .map_err(|err| CompilerError::Decode(format!("list atom decode failed: {err}")))?;
            if val == 0 {
                break;
            }
            return Err(CompilerError::Decode(format!(
                "improper list terminator: {val}"
            )));
        }
        let cell = cursor_handle
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("list noun not cell: {err}")))?;
        out.push(cell.head().noun());
        cursor = cell.tail().noun();
    }
    Ok(out)
}

#[track_caller]
pub fn atom_to_string(atom: AtomHandle<'_>) -> Result<String> {
    if let Ok(value) = atom.as_u64() {
        if value == 0 {
            return Ok("$".to_string());
        }
    }
    atom.into_string().map_err(|err| {
        let location = std::panic::Location::caller();
        let bytes = atom.as_ne_bytes();
        let preview_len = bytes.len().min(16);
        let preview = bytes[..preview_len]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        CompilerError::Decode(format!(
            "atom decode failed at {location}: {err}; bytes={preview}"
        ))
    })
}

pub fn noun_eq_direct(noun: Noun, value: u64, space: &NounSpace) -> bool {
    match noun
        .in_space(space)
        .as_atom()
        .ok()
        .and_then(|atom| atom.as_u64().ok())
    {
        Some(val) => val == value,
        None => false,
    }
}

pub fn noun_pair(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("noun not cell: {err}")))?;
    Ok((cell.head().noun(), cell.tail().noun()))
}

pub fn cell_head(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("noun not cell: {err}")))?;
    Ok(cell.head().noun())
}

pub fn cell_tail(noun: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("noun not cell: {err}")))?;
    Ok(cell.tail().noun())
}

/// Structural deep equality of two `space`-resident nouns. Shared (lives here,
/// not in `ut`, so the IR `Leaf` impls can use it) iterative form: a fast
/// pointer-identity check, a mug pre-filter (cheap quick-reject that dominates
/// in memo scans), then an iterative deep compare (no recursion — deep hoon-138
/// type nouns would otherwise blow the Rust stack).
pub fn noun_eq(a: Noun, b: Noun, space: &NounSpace) -> Result<bool> {
    // Pointer-identical nouns are the common case in memo scans; answer before
    // allocating the traversal stack below. Likewise reject on the root mugs
    // before allocating: quick rejects dominate in `miss`/memo scans and must
    // not pay a heap allocation each.
    if unsafe { a.raw_equals(&b) } {
        return Ok(true);
    }
    let a_mug = get_mug(a, space).unwrap_or_else(|| slab_mug(a, space));
    let b_mug = get_mug(b, space).unwrap_or_else(|| slab_mug(b, space));
    if a_mug != b_mug {
        return Ok(false);
    }
    // Avoid recursion: deep type/hoon nouns (hoon-138) can otherwise blow the Rust stack.
    let mut stack: Vec<(Noun, Noun)> = Vec::with_capacity(64);
    let mut seen_cell_pairs: Option<FastHashSet<(u64, u64)>> = None;
    let mut cell_pairs_checked = 0usize;
    stack.push((a, b));
    while let Some((left, right)) = stack.pop() {
        if unsafe { left.raw_equals(&right) } {
            continue;
        }
        let left_mug = get_mug(left, space).unwrap_or_else(|| slab_mug(left, space));
        let right_mug = get_mug(right, space).unwrap_or_else(|| slab_mug(right, space));
        if left_mug != right_mug {
            return Ok(false);
        }
        if left.is_atom() && right.is_atom() {
            let left_atom = left
                .in_space(space)
                .as_atom()
                .map_err(|err| CompilerError::Decode(format!("noun eq atom: {err}")))?;
            let right_atom = right
                .in_space(space)
                .as_atom()
                .map_err(|err| CompilerError::Decode(format!("noun eq atom: {err}")))?;
            if !left_atom.eq_bytes(right_atom.as_ne_bytes()) {
                return Ok(false);
            }
            continue;
        }
        if left.is_cell() && right.is_cell() {
            let left_cell = left
                .in_space(space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("noun eq cell: {err}")))?;
            let right_cell = right
                .in_space(space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("noun eq cell: {err}")))?;
            cell_pairs_checked = cell_pairs_checked.saturating_add(1);
            if cell_pairs_checked > 64 {
                let left_raw = unsafe { left.as_raw() };
                let right_raw = unsafe { right.as_raw() };
                let pair_key = if left_raw <= right_raw {
                    (left_raw, right_raw)
                } else {
                    (right_raw, left_raw)
                };
                let seen = seen_cell_pairs.get_or_insert_with(FastHashSet::default);
                if !seen.insert(pair_key) {
                    continue;
                }
            }
            // Postorder: push tail then head so head is compared first.
            stack.push((left_cell.tail().noun(), right_cell.tail().noun()));
            stack.push((left_cell.head().noun(), right_cell.head().noun()));
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

/// Iterative mug of `noun`, caching mugs on allocated nouns (cells and indirect
/// atoms). Avoids deep recursion and repeatedly re-hashing giant type structures
/// (hoon-138). Shared here so `noun_eq` (and `ut`) can use it.
pub fn slab_mug(noun: Noun, space: &NounSpace) -> u32 {
    let mut stack = vec![noun];
    while let Some(current) = stack.pop() {
        let Ok(mut allocated) = current.as_allocated() else {
            continue; // direct atoms: `get_mug` computes without caching
        };
        if get_mug(current, space).is_some() {
            continue;
        }
        let current_handle = current.in_space(space);
        if current_handle.is_atom() {
            let atom = current_handle
                .as_atom()
                .expect("atom expected when current.is_atom()");
            unsafe {
                set_mug(&mut allocated, calc_atom_mug_u32(atom.atom(), space), space);
            }
            continue;
        }
        let cell = current_handle
            .as_cell()
            .expect("cell expected when current.is_cell()");
        match (
            get_mug(cell.head().noun(), space),
            get_mug(cell.tail().noun(), space),
        ) {
            (Some(head_mug), Some(tail_mug)) => unsafe {
                set_mug(
                    &mut allocated,
                    calc_cell_mug_u32(head_mug, tail_mug, space),
                    space,
                );
            },
            _ => {
                // Postorder: revisit current after children are mugged.
                stack.push(current);
                stack.push(cell.tail().noun());
                stack.push(cell.head().noun());
            }
        }
    }
    get_mug(noun, space).expect("Noun should have a mug once mugged.")
}
