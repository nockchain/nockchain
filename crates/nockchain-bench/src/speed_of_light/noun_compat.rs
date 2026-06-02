//! Private bench-local PMA noun helpers.
//!
//! PMA noun access uses the bridge pattern:
//! `noun.in_space(space)` -> handle -> handle-side `as_cell()`, `head()`, `tail()`, `uncell()`.

use nockapp::noun::slab::NounSlab;
use nockchain_math::structs::{HoonList, HoonMapIter};
pub(crate) use nockvm::noun::NounSpace;
use nockvm::noun::{Noun, NounAllocator};
use noun_serde::{NounDecode, NounDecodeError};

pub(crate) fn space_for_slab<J>(slab: &NounSlab<J>) -> NounSpace {
    slab.noun_space()
}

pub(crate) fn decode_with_space<T: NounDecode>(
    noun: &Noun,
    space: &NounSpace,
) -> Result<T, NounDecodeError> {
    T::from_noun(noun, space)
}

pub(crate) fn atom_is_zero(noun: &Noun, space: &NounSpace) -> Result<bool, NounDecodeError> {
    Ok(noun.in_space(space).as_atom()?.as_u64()? == 0)
}

pub(crate) fn hoon_list_items(noun: Noun, space: &NounSpace) -> Result<Vec<Noun>, NounDecodeError> {
    Ok(HoonList::try_from(noun, space)?.collect())
}

pub(crate) fn hoon_map_entries(noun: Noun, space: &NounSpace) -> Vec<Noun> {
    HoonMapIter::new(&noun.in_space(space))
        .map(|entry| entry.noun())
        .collect()
}

pub(crate) fn noun_head(noun: Noun, space: &NounSpace) -> Result<Noun, NounDecodeError> {
    Ok(noun.in_space(space).as_cell()?.head().noun())
}

pub(crate) fn noun_tail(noun: Noun, space: &NounSpace) -> Result<Noun, NounDecodeError> {
    Ok(noun.in_space(space).as_cell()?.tail().noun())
}
