use nockvm::noun::{Noun, NounSpace};
use num_bigint::BigUint;

#[derive(Clone, Copy, Debug)]
pub struct TypeNoun {
    noun: Noun,
}

impl TypeNoun {
    pub fn new(noun: Noun) -> Self {
        Self { noun }
    }

    pub fn noun(&self) -> Noun {
        self.noun
    }

    pub fn slot(&self, axis: u64, space: &NounSpace) -> Option<Self> {
        self.slot_big(&BigUint::from(axis), space)
    }

    pub fn slot_big(&self, axis: &BigUint, space: &NounSpace) -> Option<Self> {
        noun_at_axis(self.noun, axis, space).map(Self::new)
    }

    pub fn core_slot(&self, axis: u64, space: &NounSpace) -> Option<Self> {
        self.core_slot_big(&BigUint::from(axis), space)
    }

    pub fn core_slot_big(&self, axis: &BigUint, space: &NounSpace) -> Option<Self> {
        let core = find_core_coil(self.noun, space)?;
        noun_at_axis(core, axis, space).map(Self::new)
    }

    pub fn tag(&self, space: &NounSpace) -> Option<String> {
        let noun = self.noun.in_space(space);
        if let Ok(atom) = noun.as_atom() {
            return atom.into_string().ok();
        }
        let cell = noun.as_cell().ok()?;
        cell.head().as_atom().ok()?.into_string().ok()
    }
}

fn noun_at_axis(noun: Noun, axis: &BigUint, space: &NounSpace) -> Option<Noun> {
    if axis == &BigUint::from(0u8) {
        return None;
    }
    let mut axis = axis.clone();
    let mut steps = Vec::new();
    while axis > BigUint::from(1u8) {
        steps.push(if (&axis & BigUint::from(1u8)) == BigUint::from(0u8) {
            0
        } else {
            1
        });
        axis >>= 1;
    }
    let mut node = noun;
    while let Some(step) = steps.pop() {
        let cell = node.in_space(space).as_cell().ok()?;
        node = if step == 0 {
            cell.head().noun()
        } else {
            cell.tail().noun()
        };
    }
    Some(node)
}

fn find_core_coil(noun: Noun, space: &NounSpace) -> Option<Noun> {
    let cell = noun.in_space(space).as_cell().ok()?;
    let tag_atom = cell.head().as_atom().ok()?;
    let tag = tag_atom.into_string().ok()?;
    match tag.as_str() {
        "core" => {
            let rest = cell.tail().as_cell().ok()?;
            Some(rest.tail().noun())
        }
        "face" => {
            let rest = cell.tail().as_cell().ok()?;
            find_core_coil(rest.tail().noun(), space)
        }
        "hint" => {
            let rest = cell.tail().as_cell().ok()?;
            find_core_coil(rest.tail().noun(), space)
        }
        "hold" => {
            let rest = cell.tail().as_cell().ok()?;
            find_core_coil(rest.head().noun(), space)
        }
        _ => None,
    }
}
