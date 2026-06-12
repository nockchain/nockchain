use nockvm::noun::{Noun, NounSpace};

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
        self.noun
            .in_space(space)
            .slot(axis)
            .ok()
            .map(|noun| Self::new(noun.noun()))
    }

    pub fn core_slot(&self, axis: u64, space: &NounSpace) -> Option<Self> {
        let core = find_core_coil(self.noun, space)?;
        core.in_space(space)
            .slot(axis)
            .ok()
            .map(|noun| Self::new(noun.noun()))
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
