use either::{Left, Right};
use nockvm::interpreter::Context;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, D};

use crate::form::belt::Belt;
use crate::form::felt::{fmul_, Felt};
use crate::utils::vec_to_hoon_list;

#[derive(Copy, Clone)]
pub struct TreeData {
    pub size: Felt,
    pub leaf: Felt,
    pub dyck: Felt,
    pub n: Noun,
}

impl Default for TreeData {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeData {
    pub fn new() -> Self {
        TreeData {
            size: Felt::one(),
            leaf: Felt::zero(),
            dyck: Felt::zero(),
            n: D(0),
        }
    }
}

enum Dyck {
    One,
    Noun(Noun),
}

pub fn build_tree_data(noun: Noun, alf: &Felt) -> Result<TreeData, JetErr> {
    let mut stack: Vec<Dyck> = Vec::<Dyck>::new();
    stack.push(Dyck::Noun(noun));

    let mut leaf: Felt = Felt::zero();
    let mut dyck: Felt = Felt::zero();
    let mut size: Felt = Felt::one();

    let mut cur: Dyck;
    while !stack.is_empty() {
        cur = stack.pop().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        match cur {
            Dyck::One => {
                dyck = fmul_(&dyck, alf);
                dyck.0[0] = dyck.0[0] + Belt::one();
            }
            Dyck::Noun(noun) => match noun.as_either_atom_cell() {
                Right(cell) => {
                    stack.push(Dyck::Noun(cell.tail()));
                    stack.push(Dyck::One);
                    stack.push(Dyck::Noun(cell.head()));
                    dyck = fmul_(&dyck, alf);
                }
                Left(atom) => {
                    size = fmul_(&size, alf);
                    leaf = fmul_(&leaf, alf);
                    leaf.0[0] = leaf.0[0] + Belt(atom.as_u64()?);
                }
            },
        }
    }
    Ok(TreeData {
        size,
        leaf,
        dyck,
        n: noun,
    })
}

pub fn leaf_sequence(context: &mut Context, sample: Noun) -> Result<Noun, JetErr> {
    let mut leaf: Vec<u64> = Vec::<u64>::new();
    do_leaf_sequence(sample, &mut leaf)?;
    Ok(vec_to_hoon_list(&mut context.stack, &leaf))
}

fn do_leaf_sequence(noun: Noun, vec: &mut Vec<u64>) -> Result<(), JetErr> {
    if noun.is_atom() {
        vec.push(noun.as_atom()?.as_u64()?);
        Ok(())
    } else {
        let cell = noun.as_cell()?;
        do_leaf_sequence(cell.head(), vec)?;
        do_leaf_sequence(cell.tail(), vec)?;
        Ok(())
    }
}
