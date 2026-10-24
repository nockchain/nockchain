use either::{Left, Right};
use nockvm::noun::{Noun, NounHandle, NounSpace};

pub enum ScryResult<'a> {
    BadPath,              // ~
    Nothing,              // [~ ~]
    Some(NounHandle<'a>), // [~ ~ foo]
    Invalid,              // anything that isn't one of the above
}

impl<'a> ScryResult<'a> {
    pub fn from_noun(noun: &Noun, space: &'a NounSpace) -> ScryResult<'a> {
        match noun.in_space(space).as_either_atom_cell() {
            Left(atom) => {
                let Ok(direct) = atom.atom().as_direct() else {
                    return ScryResult::Invalid;
                };
                if direct.data() == 0 {
                    return ScryResult::BadPath;
                }
            }
            Right(cell) => {
                let Ok(head) = cell.head().noun().as_direct() else {
                    return ScryResult::Invalid;
                };
                if head.data() == 0 {
                    match cell.tail().as_either_atom_cell() {
                        Left(atom) => {
                            let Ok(direct) = atom.atom().as_direct() else {
                                return ScryResult::Invalid;
                            };
                            if direct.data() == 0 {
                                return ScryResult::Nothing;
                            }
                        }
                        Right(tail) => {
                            let Ok(tail_head) = tail.head().noun().as_direct() else {
                                return ScryResult::Invalid;
                            };
                            if tail_head.data() == 0 {
                                return ScryResult::Some(tail.tail());
                            }
                        }
                    }
                }
            }
        }
        ScryResult::Invalid
    }
}
