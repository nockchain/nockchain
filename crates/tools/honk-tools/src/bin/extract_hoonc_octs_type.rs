use std::error::Error;
use std::fs;
use std::path::PathBuf;

use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::utils::NOCK_STACK_SIZE_MEDIUM;
use nockvm::ext::NounExt;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, NounSpace};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const PROBE_FACE: &str = "octs-probe";

fn usage(program: &str) -> String {
    format!("Usage: {program} <data-import-typed-dynock.jam> <hoonc-octs-type-138.jam>")
}

fn main() -> Result<()> {
    let mut args = std::env::args();
    let program = args
        .next()
        .unwrap_or_else(|| "extract-hoonc-octs-type".to_string());
    let input = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| usage(&program))?;
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| usage(&program))?;
    if args.next().is_some() {
        return Err(usage(&program).into());
    }

    let input_bytes = fs::read(&input)?;
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let root = <Noun as NounExt>::cue_bytes_slice(&mut stack, input_bytes.as_slice())
        .map_err(|err| format!("failed to cue {}: {err:?}", input.display()))?;
    let space = stack.noun_space();
    let octs_type = strip_named_face(typed_dynock_type(root, &space)?, PROBE_FACE, &space)?;
    ensure_type_noun(octs_type, &space)?;

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let octs_type = slab.copy_into(octs_type, &space);
    slab.set_root(octs_type);

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, slab.jam())?;
    Ok(())
}

fn typed_dynock_type(root: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = root
        .in_space(space)
        .as_cell()
        .map_err(|err| format!("typed dynock jam root is not a cell: {err:?}"))?;
    Ok(cell.head().noun())
}

fn strip_named_face(noun: Noun, name: &str, space: &NounSpace) -> Result<Noun> {
    if type_tag(noun, space).as_deref() != Some("face") {
        return Ok(noun);
    }
    let tail = noun_tail(noun, "face type", space)?;
    let tool = noun_head(tail, "face type payload", space)?;
    if atom_text(tool, space).as_deref() == Some(name) {
        return noun_tail(tail, "face type payload", space);
    }
    Ok(noun)
}

fn ensure_type_noun(noun: Noun, space: &NounSpace) -> Result<()> {
    let Some(tag) = type_tag(noun, space) else {
        return Err("inferred octs probe type is not a Hoon type noun".into());
    };
    match tag.as_str() {
        "atom" | "bull" | "cell" | "core" | "cube" | "face" | "fine" | "fork" | "hint" | "hold"
        | "noun" | "void" => Ok(()),
        _ => Err(format!("inferred octs probe type has unexpected tag %{tag}").into()),
    }
}

fn type_tag(noun: Noun, space: &NounSpace) -> Option<String> {
    if let Some(text) = atom_text(noun, space) {
        return Some(text);
    }
    let cell = noun.in_space(space).as_cell().ok()?;
    atom_text(cell.head().noun(), space)
}

fn noun_head(noun: Noun, label: &str, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| format!("{label} is not a cell: {err:?}"))?;
    Ok(cell.head().noun())
}

fn noun_tail(noun: Noun, label: &str, space: &NounSpace) -> Result<Noun> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| format!("{label} is not a cell: {err:?}"))?;
    Ok(cell.tail().noun())
}

fn atom_text(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    if atom.as_u64().ok() == Some(0) {
        return Some("$".to_string());
    }
    atom.into_string().ok()
}
