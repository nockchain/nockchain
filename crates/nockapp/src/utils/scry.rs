use either::{Left, Right};
use nockvm::noun::Noun;

pub enum ScryResult {
    BadPath,    // ~
    Nothing,    // [~ ~]
    Some(Noun), // [~ ~ foo]
    Invalid,    // anything that isn't one of the above
}

impl From<&Noun> for ScryResult {
    fn from(noun: &Noun) -> ScryResult {
        match noun.as_either_atom_cell() {
            Left(atom) => {
                let Ok(direct) = atom.as_direct() else {
                    return ScryResult::Invalid;
                };
                if direct.data() == 0 {
                    return ScryResult::BadPath;
                }
            }
            Right(cell) => {
                let Ok(head) = cell.head().as_direct() else {
                    return ScryResult::Invalid;
                };
                if head.data() == 0 {
                    match cell.tail().as_either_atom_cell() {
                        Left(atom) => {
                            let Ok(direct) = atom.as_direct() else {
                                return ScryResult::Invalid;
                            };
                            if direct.data() == 0 {
                                return ScryResult::Nothing;
                            }
                        }
                        Right(tail) => {
                            let Ok(tail_head) = tail.head().as_direct() else {
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
