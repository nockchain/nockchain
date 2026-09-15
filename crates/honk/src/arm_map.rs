use std::collections::HashMap;

use nockvm::noun::{Noun, NounSpace};
use num_bigint::BigUint;

use crate::errors::{CompilerError, Result};
use crate::native::noun::atom_to_string;
use crate::types::TypeNoun;

#[derive(Clone, Debug, Default)]
pub struct ArmMap {
    by_name: HashMap<String, BigUint>,
}

impl ArmMap {
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }

    pub fn with_pairs<A: Into<BigUint>>(pairs: impl IntoIterator<Item = (String, A)>) -> Self {
        let mut map = HashMap::new();
        for (name, axis) in pairs {
            map.insert(name, axis.into());
        }
        Self { by_name: map }
    }

    pub fn insert<A: Into<BigUint>>(&mut self, name: String, axis: A) {
        self.by_name.insert(name, axis.into());
    }

    pub fn axis_for(&self, name: &str) -> Option<BigUint> {
        self.by_name.get(name).cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &BigUint)> {
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
    collect_tomes(tomes, BigUint::from(2u32), &mut map, space)?;
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
        let axis = BigUint::from_bytes_le(axis_atom.as_ne_bytes());
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

fn collect_tomes(dom: Noun, axe: BigUint, map: &mut ArmMap, space: &NounSpace) -> Result<()> {
    let Some((node, left, right)) = map_node(dom, space)? else {
        return Ok(());
    };
    let left_empty = left.is_atom();
    let right_empty = right.is_atom();
    let base = if left_empty && right_empty {
        axe.clone()
    } else {
        peg_axis(&axe, &BigUint::from(2u32))?
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
    collect_arms(arms_map, BigUint::from(1u32), base.clone(), map, space)?;

    if left_empty && right_empty {
        return Ok(());
    }
    if left_empty {
        collect_tomes(right, peg_axis(&axe, &BigUint::from(3u32))?, map, space)?;
    } else if right_empty {
        collect_tomes(left, peg_axis(&axe, &BigUint::from(3u32))?, map, space)?;
    } else {
        collect_tomes(left, peg_axis(&axe, &BigUint::from(6u32))?, map, space)?;
        collect_tomes(right, peg_axis(&axe, &BigUint::from(7u32))?, map, space)?;
    }
    Ok(())
}

fn collect_arms(
    dab: Noun,
    axe: BigUint,
    base: BigUint,
    map: &mut ArmMap,
    space: &NounSpace,
) -> Result<()> {
    let Some((node, left, right)) = map_node(dab, space)? else {
        return Ok(());
    };
    let left_empty = left.is_atom();
    let right_empty = right.is_atom();
    let arm_axis = if left_empty && right_empty {
        axe.clone()
    } else {
        peg_axis(&axe, &BigUint::from(2u32))?
    };

    let node_cell = node
        .in_space(space)
        .as_cell()
        .map_err(|err| CompilerError::Decode(format!("arm entry not cell: {err}")))?;
    let name = term_from_noun(node_cell.head().noun(), space)?;
    let axis = peg_axis(&base, &arm_axis)?;
    map.insert(name, axis);

    if left_empty && right_empty {
        return Ok(());
    }
    if left_empty {
        collect_arms(
            right,
            peg_axis(&axe, &BigUint::from(3u32))?,
            base,
            map,
            space,
        )?;
    } else if right_empty {
        collect_arms(
            left,
            peg_axis(&axe, &BigUint::from(3u32))?,
            base,
            map,
            space,
        )?;
    } else {
        collect_arms(
            left,
            peg_axis(&axe, &BigUint::from(6u32))?,
            base.clone(),
            map,
            space,
        )?;
        collect_arms(
            right,
            peg_axis(&axe, &BigUint::from(7u32))?,
            base,
            map,
            space,
        )?;
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

fn peg_axis(a: &BigUint, b: &BigUint) -> Result<BigUint> {
    if a == &BigUint::from(0u32) || b == &BigUint::from(0u32) {
        return Err(CompilerError::Decode("axis must be non-zero".to_string()));
    }
    let shift = usize::try_from(b.bits() - 1)
        .map_err(|_| CompilerError::Decode("axis shift exceeds usize".to_string()))?;
    let base = BigUint::from(1u32) << shift;
    Ok((a << shift) + (b - base))
}
