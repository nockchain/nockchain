use hatch::ast::hoon::{NounExpr, ParsedAtom};
use nockapp::noun::slab::NounSlab;
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, AtomHandle, Noun, NounAllocator, NounSpace, D, DIRECT_MAX, T};

use crate::errors::{CompilerError, Result};

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
