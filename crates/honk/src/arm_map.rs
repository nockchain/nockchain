use std::collections::HashMap;

use nockvm::noun::{Noun, NounSpace};

use crate::errors::{CompilerError, Result};
use crate::native::noun::atom_to_string;
use crate::types::TypeNoun;

#[derive(Clone, Debug, Default)]
pub struct ArmMap {
    by_name: HashMap<String, u64>,
}

impl ArmMap {
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }

    pub fn with_pairs(pairs: impl IntoIterator<Item = (String, u64)>) -> Self {
        let mut map = HashMap::new();
        for (name, axis) in pairs {
            map.insert(name, axis);
        }
        Self { by_name: map }
    }

    pub fn insert(&mut self, name: String, axis: u64) {
        self.by_name.insert(name, axis);
    }

    pub fn axis_for(&self, name: &str) -> Option<u64> {
        self.by_name.get(name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &u64)> {
        self.by_name.iter()
    }

    pub fn from_type(ty: &TypeNoun, space: &NounSpace) -> Result<Self> {
        arm_map_from_type(ty, space)
    }
}

pub fn arm_map_from_type(ty: &TypeNoun, space: &NounSpace) -> Result<ArmMap> {
    let Some(coil) = find_core_coil(ty.noun(), space)? else {
        return Ok(ArmMap::default());
    };
    let tomes = coil_tomes(coil, space)?;
    let mut map = ArmMap::new();
    collect_tomes(tomes, 2, &mut map, space)?;
    Ok(map)
}

pub fn arm_map_from_noun_list(noun: Noun, space: &NounSpace) -> Result<ArmMap> {
    let mut map = ArmMap::new();
    let mut cursor = noun;
    loop {
        let cursor_handle = cursor.in_space(space);
        if let Ok(atom) = cursor_handle.as_atom() {
            let value = atom
                .as_u64()
                .map_err(|err| CompilerError::Noun(err.to_string()))?;
            if value == 0 {
                break;
            }
            return Err(CompilerError::Decode(format!(
                "unexpected atom in arm-map list: {value}"
            )));
        }

        let cell = cursor_handle
            .as_cell()
            .map_err(|err| CompilerError::Noun(err.to_string()))?;
        let head = cell.head();
        let tail = cell.tail().noun();

        let pair = head
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("arm-map entry not a cell: {err}")))?;
        let name_atom = pair
            .head()
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("arm-map name not atom: {err}")))?;
        let axis_atom = pair
            .tail()
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("arm-map axis not atom: {err}")))?;
        let name = atom_to_string(name_atom)
            .map_err(|err| CompilerError::Decode(format!("arm-map name decode failed: {err}")))?;
        let axis = axis_atom
            .as_u64()
            .map_err(|err| CompilerError::Decode(format!("arm-map axis decode failed: {err}")))?;
        map.insert(name, axis);
        cursor = tail;
    }
    Ok(map)
}

fn find_core_coil(noun: Noun, space: &NounSpace) -> Result<Option<Noun>> {
    let cell = match noun.in_space(space).as_cell() {
        Ok(cell) => cell,
        Err(_) => return Ok(None),
    };
    let tag_atom = match cell.head().as_atom() {
        Ok(atom) => atom,
        Err(_) => return Ok(None),
    };
    let tag = tag_atom
        .into_string()
        .map_err(|err| CompilerError::Decode(format!("type tag decode failed: {err}")))?;

    match tag.as_str() {
        "core" => {
            let rest = cell
                .tail()
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("core type missing tail: {err}")))?;
            Ok(Some(rest.tail().noun()))
        }
        "face" => {
            let rest = cell
                .tail()
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("face type missing tail: {err}")))?;
            let inner = rest.tail().noun();
            find_core_coil(inner, space)
        }
        "hint" => {
            let rest = cell
                .tail()
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("hint type missing tail: {err}")))?;
            let payload = rest.tail().noun();
            find_core_coil(payload, space)
        }
        "hold" => {
            let rest = cell
                .tail()
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("hold type missing tail: {err}")))?;
            let typ = rest.head().noun();
            find_core_coil(typ, space)
        }
        _ => Ok(None),
    }
}

fn coil_tomes(coil: Noun, space: &NounSpace) -> Result<Noun> {
    let cell = coil
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("core coil not cell: {err}")))?;
    let rest = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("core coil missing tail: {err}")))?;
    let rest = rest
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("core coil missing tomes: {err}")))?;
    Ok(rest.tail().noun())
}

fn collect_tomes(dom: Noun, axe: u64, map: &mut ArmMap, space: &NounSpace) -> Result<()> {
    let Some((node, left, right)) = map_node(dom, space)? else {
        return Ok(());
    };
    let left_empty = left.is_atom();
    let right_empty = right.is_atom();
    let base = if left_empty && right_empty {
        axe
    } else {
        peg_axis(axe, 2)?
    };

    let node_cell = node
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("tome entry not cell: {err}")))?;
    let tome = node_cell.tail();
    let tome_cell = tome
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("tome value not cell: {err}")))?;
    let arms_map = tome_cell.tail().noun();
    collect_arms(arms_map, 1, base, map, space)?;

    if left_empty && right_empty {
        return Ok(());
    }
    if left_empty {
        collect_tomes(right, peg_axis(axe, 3)?, map, space)?;
    } else if right_empty {
        collect_tomes(left, peg_axis(axe, 3)?, map, space)?;
    } else {
        collect_tomes(left, peg_axis(axe, 6)?, map, space)?;
        collect_tomes(right, peg_axis(axe, 7)?, map, space)?;
    }
    Ok(())
}

fn collect_arms(dab: Noun, axe: u64, base: u64, map: &mut ArmMap, space: &NounSpace) -> Result<()> {
    let Some((node, left, right)) = map_node(dab, space)? else {
        return Ok(());
    };
    let left_empty = left.is_atom();
    let right_empty = right.is_atom();
    let arm_axis = if left_empty && right_empty {
        axe
    } else {
        peg_axis(axe, 2)?
    };

    let node_cell = node
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("arm entry not cell: {err}")))?;
    let name = term_from_noun(node_cell.head().noun(), space)?;
    let axis = peg_axis(base, arm_axis)?;
    map.insert(name, axis);

    if left_empty && right_empty {
        return Ok(());
    }
    if left_empty {
        collect_arms(right, peg_axis(axe, 3)?, base, map, space)?;
    } else if right_empty {
        collect_arms(left, peg_axis(axe, 3)?, base, map, space)?;
    } else {
        collect_arms(left, peg_axis(axe, 6)?, base, map, space)?;
        collect_arms(right, peg_axis(axe, 7)?, base, map, space)?;
    }
    Ok(())
}

fn map_node(noun: Noun, space: &NounSpace) -> Result<Option<(Noun, Noun, Noun)>> {
    if noun.is_atom() {
        return Ok(None);
    }
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("map node not cell: {err}")))?;
    let node = cell.head().noun();
    let rest = cell
        .tail()
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("map node missing branches: {err}")))?;
    let left = rest.head().noun();
    let right = rest.tail().noun();
    Ok(Some((node, left, right)))
}

fn term_from_noun(noun: Noun, space: &NounSpace) -> Result<String> {
    let atom = noun
        .in_space(space)
        .as_atom()
        .map_err(|err| CompilerError::Decode(format!("arm name not atom: {err}")))?;
    atom_to_string(atom)
        .map_err(|err| CompilerError::Decode(format!("arm name decode failed: {err}")))
}

fn peg_axis(a: u64, b: u64) -> Result<u64> {
    if a == 0 || b == 0 {
        return Err(CompilerError::Decode("axis must be non-zero".to_string()));
    }
    let b_bits = 64 - b.leading_zeros();
    let shift = (b_bits - 1) as u32;
    let mask = if shift == 0 {
        0u128
    } else {
        (1u128 << shift) - 1
    };
    let out = ((b as u128) & mask) | ((a as u128) << shift);
    if out > u64::MAX as u128 {
        return Err(CompilerError::Decode("axis overflow".to_string()));
    }
    Ok(out as u64)
}
