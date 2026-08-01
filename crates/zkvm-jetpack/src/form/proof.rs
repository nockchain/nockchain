use std::collections::BTreeMap;
use std::convert::TryFrom;

use nockchain_math::belt::Belt;
use nockchain_math::felt::Felt;
use nockchain_math::mary::Mary;
use nockchain_math::noun_ext::{AtomMathExt, NounMathExt};
use nockchain_math::poly::{BPolySlice, BPolyVec, FPolySlice, FPolyVec};
use nockchain_math::shape::do_leaf_sequence;
use nockchain_math::structs::{HoonList, HoonMapIter};
use nockvm::ext::AtomExt;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, T};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofData {
    MRoot {
        p: [u64; 5],
    },
    Puzzle {
        com: [u64; 5],
        nonce: [u64; 5],
        len: u64,
        leaf: Vec<u64>,
        dyck: Vec<u64>,
    },
    Codeword(FPolyVec),
    Terms(BPolyVec),
    MPath(ProofPath),
    MPathBf(ProofPathBf),
    CompM {
        p: [u64; 5],
        num: u64,
    },
    Evals(FPolyVec),
    Heights(Vec<u64>),
    Poly(BPolyVec),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct ProofPath {
    pub leaf: FPolyVec,
    pub path: Vec<[u64; 5]>,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct ProofPathBf {
    pub leaf: BPolyVec,
    pub path: Vec<[u64; 5]>,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct Proof {
    pub version: ProofVersion,
    pub objects: Vec<ProofData>,
    pub hashes: Vec<[u64; 5]>,
    pub read_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofVersion {
    V0,
    V1,
    V2,
    V3,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct MerkHeap {
    pub height: u32,
    pub root: [u64; 5],
    pub m: Mary,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct CodewordCommitment {
    pub polys: Vec<Mary>,
    pub codewords: Mary,
    pub merk_heap: MerkHeap,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct TableHeader {
    pub name: String,
    pub field: u64,
    pub base_width: u32,
    pub ext_width: u32,
    pub mega_ext_width: u32,
    pub full_width: u32,
    pub num_randomizers: u32,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct TableMary {
    pub header: TableHeader,
    pub mary: Mary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotNoun {
    Atom(Vec<u8>),
    Cell(Box<SnapshotNoun>, Box<SnapshotNoun>),
}

#[derive(Clone, PartialEq)]
pub struct ProofSnapshot {
    pub format: u64,
    pub extension_commitment: CodewordCommitment,
    pub transcript: Proof,
    pub table_count: u32,
    pub base_commitment: CodewordCommitment,
    pub tables: Vec<TableMary>,
    pub first_round_challenges: Vec<Belt>,
    pub subject: SnapshotNoun,
    pub formula: SnapshotNoun,
    pub computation_result: SnapshotNoun,
    pub heights: Vec<u64>,
    pub extra_constraint_count: u32,
    pub proof_version: ProofVersion,
    pub constraints: SnapshotNoun,
    pub constraint_counts: SnapshotNoun,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct ProofStreamRange {
    pub start: u64,
    pub end: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct ProofStreamContext {
    pub total: u64,
    pub digest: [u64; 5],
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct ProofStreamWindow {
    pub format: u64,
    pub proof_version: ProofVersion,
    pub range: ProofStreamRange,
    pub objects: Vec<ProofData>,
    pub context: ProofStreamContext,
}

pub struct Constraints(pub ProofMap<usize, MPDenseConstraints>);
pub struct ConstraintsSlice<'a>(pub ProofMap<usize, MPDenseConstraintsSlice<'a>>);
pub struct IndexBPolyMap<'a>(pub ProofMap<usize, &'a [Belt]>);
pub struct IndexFPolyMap<'a>(pub ProofMap<usize, &'a [Felt]>);
pub struct IndexBeltMap(pub ProofMap<usize, Belt>);
pub struct IndexFeltMap(pub ProofMap<usize, Felt>);
pub struct MPDenseConstraints {
    pub boundary: Vec<ConstraintData>,
    pub row: Vec<ConstraintData>,
    pub transition: Vec<ConstraintData>,
    pub terminal: Vec<ConstraintData>,
    pub extra: Vec<ConstraintData>,
}

pub struct MPDenseConstraintsSlice<'a> {
    pub boundary: Vec<ConstraintDataSlice<'a>>,
    pub row: Vec<ConstraintDataSlice<'a>>,
    pub transition: Vec<ConstraintDataSlice<'a>>,
    pub terminal: Vec<ConstraintDataSlice<'a>>,
    pub extra: Vec<ConstraintDataSlice<'a>>,
}

impl Constraints {
    pub fn to_slice(&self) -> ConstraintsSlice<'_> {
        ConstraintsSlice(self.0.iter().map(|(k, v)| (*k, v.to_slice())).collect())
    }
}

impl MPDenseConstraints {
    pub fn to_slice(&self) -> MPDenseConstraintsSlice<'_> {
        MPDenseConstraintsSlice {
            boundary: self
                .boundary
                .iter()
                .map(|x| x.to_slice())
                .collect::<Vec<_>>(),
            row: self.row.iter().map(|x| x.to_slice()).collect::<Vec<_>>(),
            transition: self
                .transition
                .iter()
                .map(|x| x.to_slice())
                .collect::<Vec<_>>(),
            terminal: self
                .terminal
                .iter()
                .map(|x| x.to_slice())
                .collect::<Vec<_>>(),
            extra: self.extra.iter().map(|x| x.to_slice()).collect::<Vec<_>>(),
        }
    }
}

pub struct ConstraintData {
    pub constraint: MPUltra,
    pub degs: Vec<u64>,
}

pub struct ConstraintDataSlice<'a> {
    pub constraint: MPUltraSlice<'a>,
    pub degs: Vec<u64>,
}

impl ConstraintData {
    pub fn to_slice(&self) -> ConstraintDataSlice<'_> {
        ConstraintDataSlice {
            constraint: self.constraint.to_slice(),
            degs: self.degs.clone(),
        }
    }
}

pub struct Counts {
    pub boundary: usize,
    pub row: usize,
    pub transition: usize,
    pub terminal: usize,
    pub extra: usize,
}

pub type ProofMap<K, V> = BTreeMap<K, V>;
// pub type ProofMap<K, V> = std::collections::HashMap<K, V>;

pub struct CountMap(pub ProofMap<usize, Counts>);

const TYP_LEN: usize = 3;
const IDX_LEN: usize = 10;

pub enum MPUltraSlice<'a> {
    Mega(MPMegaSlice<'a>),
    Comp(MPCompSlice<'a>),
}
pub enum MPUltra {
    Mega(MPMega),
    Comp(MPComp),
}

impl MPUltra {
    pub fn to_slice(&self) -> MPUltraSlice<'_> {
        match self {
            MPUltra::Mega(mega) => MPUltraSlice::Mega(mega.to_slice()),
            MPUltra::Comp(comp) => MPUltraSlice::Comp(comp.to_slice()),
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum ConstraintMegaTyp {
    CON = 0,
    VAR = 1,
    RND = 2,
    DYN = 3,
    COM = 4,
}

#[derive(Debug)]
pub struct Mega {
    pub typ: ConstraintMegaTyp,
    pub idx: usize,
    pub exp: u64,
}

#[derive(Clone)]
pub struct MPMegaSlice<'a>(pub ProofMap<&'a [Belt], Belt>);
pub struct MPMega(pub ProofMap<BPolyVec, Belt>);

impl MPMega {
    pub fn to_slice(&self) -> MPMegaSlice<'_> {
        MPMegaSlice::new(self.0.iter().map(|(k, v)| (&k.0[..], *v)).collect())
    }
}

impl MPMegaSlice<'_> {
    pub fn new<'a>(proof_map: ProofMap<&'a [Belt], Belt>) -> MPMegaSlice<'a> {
        MPMegaSlice(proof_map)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&[Belt], Belt)> {
        self.0.iter().map(|(k, v)| (*k, *v))
    }
}

pub struct MPCompSlice<'a> {
    pub dep: Vec<MPMegaSlice<'a>>,
    pub com: Vec<MPMegaSlice<'a>>,
}

pub struct MPComp {
    pub dep: Vec<MPMega>,
    pub com: Vec<MPMega>,
}

impl MPComp {
    pub fn to_slice(&self) -> MPCompSlice<'_> {
        MPCompSlice {
            dep: self.dep.iter().map(|m| m.to_slice()).collect(),
            com: self.com.iter().map(|m| m.to_slice()).collect(),
        }
    }
}

impl<'a> MPUltraSlice<'a> {
    pub fn try_from(mp_ultra: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let mp_ultra_cell = mp_ultra.in_space(space).as_cell().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        match mp_ultra_cell
            .head()
            .as_atom()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .as_u64()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }) {
            tas!(b"mega") => Ok(MPUltraSlice::Mega(MPMegaSlice::try_from(
                mp_ultra_cell.tail().noun(),
                space,
            )?)),
            tas!(b"comp") => Ok(MPUltraSlice::Comp(MPCompSlice::try_from(
                mp_ultra_cell.tail().noun(),
                space,
            )?)),
            _ => panic!("Invalid MPUltra type"),
        }
    }
}

impl MPUltra {
    pub fn try_from(mp_ultra: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let mp_ultra_cell = mp_ultra.in_space(space).as_cell().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        match mp_ultra_cell
            .head()
            .as_atom()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .as_u64()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }) {
            tas!(b"mega") => Ok(MPUltra::Mega(MPMega::try_from(
                mp_ultra_cell.tail().noun(),
                space,
            )?)),
            tas!(b"comp") => Ok(MPUltra::Comp(MPComp::try_from(
                mp_ultra_cell.tail().noun(),
                space,
            )?)),
            _ => panic!("Invalid MPUltra type"),
        }
    }
}

impl TryFrom<&Belt> for Mega {
    type Error = JetErr;
    fn try_from(belt: &Belt) -> Result<Self, Self::Error> {
        let typ = match belt.0 & (2u64.pow(TYP_LEN as u32) - 1) {
            0 => ConstraintMegaTyp::CON,
            1 => ConstraintMegaTyp::VAR,
            2 => ConstraintMegaTyp::RND,
            3 => ConstraintMegaTyp::DYN,
            4 => ConstraintMegaTyp::COM,
            _ => return Err(BAIL_FAIL),
        };
        let idx = ((belt.0 >> TYP_LEN) & (2u64.pow(IDX_LEN as u32) - 1)) as usize;
        let exp = belt.0 >> (TYP_LEN + IDX_LEN);
        Ok(Mega { typ, idx, exp })
    }
}

impl<'a> IndexBPolyMap<'a> {
    pub fn try_from(hoon_map: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let mut composition_chals = ProofMap::<usize, &[Belt]>::new();
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, &[Belt]) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    BPolySlice::try_from(term_cell.tail().noun(), space)
                        .unwrap_or_else(|err| {
                            panic!(
                                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        })
                        .0,
                )
            };
            composition_chals.insert(k, v);
        }
        Ok(IndexBPolyMap(composition_chals))
    }
}

impl<'a> IndexFPolyMap<'a> {
    pub fn try_from(hoon_map: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let mut composition_chals = ProofMap::<usize, &[Felt]>::new();
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, &[Felt]) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    FPolySlice::try_from(term_cell.tail().noun(), space)
                        .unwrap_or_else(|err| {
                            panic!(
                                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        })
                        .0,
                )
            };
            composition_chals.insert(k, v);
        }
        Ok(IndexFPolyMap(composition_chals))
    }
}

impl NounDecode for Constraints {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        Constraints::try_from(*noun, space).map_err(|_| NounDecodeError::ConstraintsDecodeError)
    }
}

impl Constraints {
    pub fn try_from(hoon_map: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);
        let mut constraints = ProofMap::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, MPDenseConstraints) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    MPDenseConstraints::try_from(term_cell.tail().noun(), space)?,
                )
            };

            constraints.insert(k, v);
        }
        Ok(Constraints(constraints))
    }
}

impl<'a> ConstraintsSlice<'a> {
    pub fn try_from(hoon_map: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);
        let mut constraints = ProofMap::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, MPDenseConstraintsSlice<'a>) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    MPDenseConstraintsSlice::try_from(term_cell.tail().noun(), space)?,
                )
            };

            constraints.insert(k, v);
        }
        Ok(ConstraintsSlice(constraints))
    }
}

impl IndexBeltMap {
    pub fn try_from(hoon_map: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);
        let mut map = ProofMap::<usize, Belt>::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, Belt) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    term_cell.tail().as_atom()?.atom().as_belt(space)?,
                )
            };

            map.insert(k, v);
        }
        Ok(IndexBeltMap(map))
    }
}

impl IndexFeltMap {
    pub fn try_from(hoon_map: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);
        let mut map = ProofMap::<usize, Felt>::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, Felt) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    *term_cell.tail().as_atom()?.atom().as_felt(space)?,
                )
            };

            map.insert(k, v);
        }
        Ok(IndexFeltMap(map))
    }
}

impl<'a> ConstraintDataSlice<'a> {
    pub fn try_from(noun: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let cell = noun.in_space(space).as_cell()?;
        let cs = MPUltraSlice::try_from(cell.head().noun(), space)?;
        let degs: Vec<u64> = HoonList::try_from(cell.tail().noun(), space)?
            .map(|n| n.in_space(space).as_atom()?.as_u64())
            .collect::<Result<Vec<u64>, _>>()?;
        Ok(ConstraintDataSlice {
            constraint: cs,
            degs,
        })
    }
}

impl ConstraintData {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let cell = noun.in_space(space).as_cell()?;
        let cs = MPUltra::try_from(cell.head().noun(), space)?;
        let degs: Vec<u64> = HoonList::try_from(cell.tail().noun(), space)?
            .map(|n| n.in_space(space).as_atom()?.as_u64())
            .collect::<Result<Vec<u64>, _>>()?;
        Ok(ConstraintData {
            constraint: cs,
            degs,
        })
    }
}

impl<'a> MPDenseConstraintsSlice<'a> {
    pub fn try_from(noun: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let [boundary, row, transition, terminal, extra] = noun.uncell(space)?;

        let boundary: Vec<ConstraintDataSlice<'a>> = HoonList::try_from(boundary, space)?
            .map(|n| ConstraintDataSlice::try_from(n, space))
            .collect::<Result<Vec<ConstraintDataSlice<'a>>, _>>()?;
        let row: Vec<ConstraintDataSlice<'a>> = HoonList::try_from(row, space)?
            .map(|n| ConstraintDataSlice::try_from(n, space))
            .collect::<Result<Vec<ConstraintDataSlice<'a>>, _>>()?;
        let transition: Vec<ConstraintDataSlice<'a>> = HoonList::try_from(transition, space)?
            .map(|n| ConstraintDataSlice::try_from(n, space))
            .collect::<Result<Vec<ConstraintDataSlice<'a>>, _>>()?;
        let terminal: Vec<ConstraintDataSlice<'a>> = HoonList::try_from(terminal, space)?
            .map(|n| ConstraintDataSlice::try_from(n, space))
            .collect::<Result<Vec<ConstraintDataSlice<'a>>, _>>()?;
        let extra: Vec<ConstraintDataSlice<'a>> = HoonList::try_from(extra, space)?
            .map(|n| ConstraintDataSlice::try_from(n, space))
            .collect::<Result<Vec<ConstraintDataSlice<'a>>, _>>()?;

        Ok(MPDenseConstraintsSlice {
            boundary,
            row,
            transition,
            terminal,
            extra,
        })
    }
}

impl MPDenseConstraints {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let [boundary, row, transition, terminal, extra] = noun.uncell(space)?;

        let boundary: Vec<ConstraintData> = HoonList::try_from(boundary, space)?
            .map(|n| ConstraintData::try_from(n, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let row: Vec<ConstraintData> = HoonList::try_from(row, space)?
            .map(|n| ConstraintData::try_from(n, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let transition: Vec<ConstraintData> = HoonList::try_from(transition, space)?
            .map(|n| ConstraintData::try_from(n, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let terminal: Vec<ConstraintData> = HoonList::try_from(terminal, space)?
            .map(|n| ConstraintData::try_from(n, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let extra: Vec<ConstraintData> = HoonList::try_from(extra, space)?
            .map(|n| ConstraintData::try_from(n, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;

        Ok(MPDenseConstraints {
            boundary,
            row,
            transition,
            terminal,
            extra,
        })
    }
}

// #[inline(always)]
// unsafe fn bpoly_from_cell_unchecked<'a>(cell: nockvm::noun::Cell) -> &'a [Belt] {
//     let len = cell.head().as_direct().unwrap_unchecked().data() as usize;
//     let ptr = cell.tail().as_indirect().unwrap_unchecked().data_pointer();
//     std::slice::from_raw_parts(ptr as *const Belt, len)
// }

impl<'a> MPMegaSlice<'a> {
    pub fn try_from(noun: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let noun_handle = noun.in_space(space);
        let hoon_map = HoonMapIter::new(&noun_handle);
        let mut mega_map = ProofMap::<&[Belt], Belt>::new();
        for term_noun in hoon_map.into_iter() {
            let (k, v): (&[Belt], Belt) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    BPolySlice::try_from(term_cell.head().noun(), space)
                        .unwrap_or_else(|err| {
                            panic!(
                                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        })
                        .0,
                    Belt(term_cell.tail().as_atom()?.as_u64()?),
                )
            };
            mega_map.insert(k, v);
        }

        Ok(MPMegaSlice(mega_map))
    }
}

impl<'a> MPCompSlice<'a> {
    pub fn try_from(noun: Noun, space: &'a NounSpace) -> Result<Self, JetErr> {
        let dep_list = HoonList::try_from(slot(noun, 2, space)?, space)?;
        let com_list = HoonList::try_from(slot(noun, 3, space)?, space)?;

        let mut dep = Vec::with_capacity(dep_list.count());
        let mut com = Vec::with_capacity(com_list.count());

        for dep_noun in dep_list {
            dep.push(MPMegaSlice::try_from(dep_noun, space)?);
        }

        for com_noun in com_list {
            com.push(MPMegaSlice::try_from(com_noun, space)?);
        }

        Ok(MPCompSlice { dep, com })
    }
}

impl MPMega {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let noun_handle = noun.in_space(space);
        let hoon_map = HoonMapIter::new(&noun_handle);
        let mut mega_map = ProofMap::<BPolyVec, Belt>::new();
        for term_noun in hoon_map.into_iter() {
            let (k, v): (BPolyVec, Belt) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    BPolyVec::try_from(term_cell.head().noun(), space).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    }),
                    Belt(term_cell.tail().as_atom()?.as_u64()?),
                )
            };
            mega_map.insert(k, v);
        }

        Ok(MPMega(mega_map))
    }
}

impl MPComp {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let dep_list = HoonList::try_from(slot(noun, 2, space)?, space)?;
        let com_list = HoonList::try_from(slot(noun, 3, space)?, space)?;

        let mut dep = Vec::with_capacity(dep_list.count());
        let mut com = Vec::with_capacity(com_list.count());

        for dep_noun in dep_list {
            dep.push(MPMega::try_from(dep_noun, space)?);
        }

        for com_noun in com_list {
            com.push(MPMega::try_from(com_noun, space)?);
        }

        Ok(MPComp { dep, com })
    }
}
impl NounDecode for CountMap {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        CountMap::try_from(*noun, space).map_err(|_| NounDecodeError::ConstraintsDecodeError)
    }
}

impl CountMap {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let noun_handle = noun.in_space(space);
        let counts = HoonMapIter::new(&noun_handle);

        let mut outer = ProofMap::<usize, Counts>::new();

        for term_noun in counts.into_iter() {
            let (k, v): (usize, Counts) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (term_cell.head().as_atom()?.as_u64()? as usize, {
                    let tail = term_cell.tail().noun();
                    Counts {
                        boundary: slot(tail, 2, space)?.in_space(space).as_atom()?.as_u64()?
                            as usize,
                        row: slot(tail, 6, space)?.in_space(space).as_atom()?.as_u64()? as usize,
                        transition: slot(tail, 14, space)?.in_space(space).as_atom()?.as_u64()?
                            as usize,
                        terminal: slot(tail, 30, space)?.in_space(space).as_atom()?.as_u64()?
                            as usize,
                        extra: slot(tail, 31, space)?.in_space(space).as_atom()?.as_u64()? as usize,
                    }
                })
            };
            outer.insert(k, v);
        }
        Ok(CountMap(outer))
    }
}

#[derive(Debug)]
pub enum ProofStreamError {
    NonEmptyHashes(usize),
    NonZeroReadIndex(u32),
    Exhausted,
}

impl std::fmt::Display for ProofStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProofStreamError::NonEmptyHashes(len) => {
                write!(f, "proof stream expects empty hashes, found {len}")
            }
            ProofStreamError::NonZeroReadIndex(index) => {
                write!(f, "proof stream expects read_index 0, found {index}")
            }
            ProofStreamError::Exhausted => f.write_str("proof stream exhausted"),
        }
    }
}

impl std::error::Error for ProofStreamError {}

pub struct ProofStream<'a> {
    proof: &'a mut Proof,
    cursor: usize,
    committed: usize,
}

impl<'a> ProofStream<'a> {
    pub fn new(proof: &'a mut Proof) -> Result<Self, ProofStreamError> {
        if !proof.hashes.is_empty() {
            return Err(ProofStreamError::NonEmptyHashes(proof.hashes.len()));
        }
        if proof.read_index != 0 {
            return Err(ProofStreamError::NonZeroReadIndex(proof.read_index));
        }
        Ok(Self {
            proof,
            cursor: 0,
            committed: 0,
        })
    }

    pub fn pull(&mut self) -> Result<&ProofData, ProofStreamError> {
        let item = self
            .proof
            .objects
            .get(self.cursor)
            .ok_or(ProofStreamError::Exhausted)?;
        self.cursor += 1;
        self.proof.read_index = self.cursor as u32;
        Ok(item)
    }

    pub(crate) fn commit_consumed(&mut self) {
        while self.committed < self.cursor {
            let object = &self.proof.objects[self.committed];
            self.proof
                .hashes
                .push(crate::form::tog::hash_proof_data(object));
            self.committed += 1;
        }
    }

    pub fn transcript_rng(&mut self) -> Result<crate::form::tog::Tog, JetErr> {
        self.commit_consumed();
        crate::form::tog::verifier_fiat_shamir(self.proof)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReassembleNounError {
    InvalidDyck,
    InvalidLeafOrDyckWord,
}

impl NounDecode for SnapshotNoun {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        if let Ok(atom) = noun.in_space(space).as_atom() {
            return Ok(Self::Atom(atom.as_ne_bytes().to_vec()));
        }
        let cell = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        Ok(Self::Cell(
            Box::new(Self::from_noun(&cell.head().noun(), space)?),
            Box::new(Self::from_noun(&cell.tail().noun(), space)?),
        ))
    }
}

impl NounEncode for SnapshotNoun {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            SnapshotNoun::Atom(bytes) => Atom::from_bytes(allocator, bytes).as_noun(),
            SnapshotNoun::Cell(head, tail) => {
                let head = head.to_noun(allocator);
                let tail = tail.to_noun(allocator);
                T(allocator, &[head, tail])
            }
        }
    }
}

impl NounDecode for ProofSnapshot {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let [format, extension_commitment, transcript, table_count, base_commitment, tables, first_round_challenges, subject, formula, computation_result, heights, extra_constraint_count, proof_version, constraints, constraint_counts] =
            noun.uncell(space)?;

        Ok(Self {
            format: u64::from_noun(&format, space)?,
            extension_commitment: CodewordCommitment::from_noun(&extension_commitment, space)?,
            transcript: Proof::from_noun(&transcript, space)?,
            table_count: u32::from_noun(&table_count, space)?,
            base_commitment: CodewordCommitment::from_noun(&base_commitment, space)?,
            tables: Vec::<TableMary>::from_noun(&tables, space)?,
            first_round_challenges: Vec::<Belt>::from_noun(&first_round_challenges, space)?,
            subject: SnapshotNoun::from_noun(&subject, space)?,
            formula: SnapshotNoun::from_noun(&formula, space)?,
            computation_result: SnapshotNoun::from_noun(&computation_result, space)?,
            heights: Vec::<u64>::from_noun(&heights, space)?,
            extra_constraint_count: u32::from_noun(&extra_constraint_count, space)?,
            proof_version: ProofVersion::from_noun(&proof_version, space)?,
            constraints: SnapshotNoun::from_noun(&constraints, space)?,
            constraint_counts: SnapshotNoun::from_noun(&constraint_counts, space)?,
        })
    }
}

impl NounEncode for ProofSnapshot {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let format = self.format.to_noun(allocator);
        let extension_commitment = self.extension_commitment.to_noun(allocator);
        let transcript = self.transcript.to_noun(allocator);
        let table_count = self.table_count.to_noun(allocator);
        let base_commitment = self.base_commitment.to_noun(allocator);
        let tables = self.tables.to_noun(allocator);
        let first_round_challenges = self.first_round_challenges.to_noun(allocator);
        let subject = self.subject.to_noun(allocator);
        let formula = self.formula.to_noun(allocator);
        let computation_result = self.computation_result.to_noun(allocator);
        let heights = self.heights.to_noun(allocator);
        let extra_constraint_count = self.extra_constraint_count.to_noun(allocator);
        let proof_version = self.proof_version.to_noun(allocator);
        let constraints = self.constraints.to_noun(allocator);
        let constraint_counts = self.constraint_counts.to_noun(allocator);
        T(
            allocator,
            &[
                format, extension_commitment, transcript, table_count, base_commitment, tables,
                first_round_challenges, subject, formula, computation_result, heights,
                extra_constraint_count, proof_version, constraints, constraint_counts,
            ],
        )
    }
}

impl NounDecode for ProofData {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let noun = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let tail = noun.tail().noun();
        match noun
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::ExpectedAtom)?
            .as_u64()?
        {
            tas!(b"m-root") => {
                let p = <[u64; 5]>::from_noun(&tail, space)?;
                Ok(ProofData::MRoot { p })
            }
            tas!(b"puzzle") => {
                let [com, nonce, len, p] = tail.uncell(space)?;
                let com = <[u64; 5]>::from_noun(&com, space)?;
                let nonce = <[u64; 5]>::from_noun(&nonce, space)?;
                let len = len.in_space(space).as_atom()?.as_u64()?;

                let mut leaf = Vec::new();
                do_leaf_sequence(p, &mut leaf, space)
                    .map_err(|_| NounDecodeError::Custom("leaf sequence failed".to_string()))?;
                let dyck = dyck_word(p, space);

                Ok(ProofData::Puzzle {
                    com,
                    nonce,
                    len,
                    leaf,
                    dyck,
                })
            }
            tas!(b"codeword") => Ok(ProofData::Codeword(FPolyVec::from_noun(&tail, space)?)),
            tas!(b"terms") => Ok(ProofData::Terms(BPolyVec::from_noun(&tail, space)?)),
            tas!(b"m-path") => Ok(ProofData::MPath(ProofPath::from_noun(&tail, space)?)),
            tas!(b"m-pathbf") => Ok(ProofData::MPathBf(ProofPathBf::from_noun(&tail, space)?)),
            tas!(b"comp-m") => {
                let [p, num] = tail.uncell(space)?;
                let p = <[u64; 5]>::from_noun(&p, space)?;
                let num = num.in_space(space).as_atom()?.as_u64()?;
                Ok(ProofData::CompM { p, num })
            }
            tas!(b"evals") => Ok(ProofData::Evals(FPolyVec::from_noun(&tail, space)?)),
            tas!(b"heights") => Ok(ProofData::Heights(Vec::<u64>::from_noun(&tail, space)?)),
            tas!(b"poly") => Ok(ProofData::Poly(BPolyVec::from_noun(&tail, space)?)),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

impl NounEncode for ProofData {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            ProofData::MRoot { p } => {
                let tag = Atom::new(allocator, tas!(b"m-root")).as_noun();
                let data = p.to_noun(allocator);
                T(allocator, &[tag, data])
            }
            ProofData::Puzzle {
                com,
                nonce,
                len,
                leaf,
                dyck,
            } => {
                let tag = Atom::new(allocator, tas!(b"puzzle")).as_noun();
                let com_noun = com.to_noun(allocator);
                let nonce_noun = nonce.to_noun(allocator);
                let len_noun = Atom::new(allocator, *len).as_noun();
                let p_noun = reassemble_noun(allocator, leaf, dyck)
                    .expect("invalid puzzle leaf/dyck word in proof");
                T(allocator, &[tag, com_noun, nonce_noun, len_noun, p_noun])
            }
            ProofData::Codeword(codeword) => {
                let tag = Atom::new(allocator, tas!(b"codeword")).as_noun();
                let data = codeword.to_noun(allocator);
                T(allocator, &[tag, data])
            }
            ProofData::Terms(terms) => {
                let tag = Atom::new(allocator, tas!(b"terms")).as_noun();
                let data = terms.to_noun(allocator);
                T(allocator, &[tag, data])
            }
            ProofData::MPath(path) => {
                let tag = Atom::new(allocator, tas!(b"m-path")).as_noun();
                let data = path.to_noun(allocator);
                T(allocator, &[tag, data])
            }
            ProofData::MPathBf(path) => {
                let tag = Atom::new(allocator, tas!(b"m-pathbf")).as_noun();
                let data = path.to_noun(allocator);
                T(allocator, &[tag, data])
            }
            ProofData::CompM { p, num } => {
                let tag = Atom::new(allocator, tas!(b"comp-m")).as_noun();
                let p_noun = p.to_noun(allocator);
                let num_noun = Atom::new(allocator, *num).as_noun();
                T(allocator, &[tag, p_noun, num_noun])
            }
            ProofData::Evals(evals) => {
                let tag = Atom::new(allocator, tas!(b"evals")).as_noun();
                let data = evals.to_noun(allocator);
                T(allocator, &[tag, data])
            }
            ProofData::Heights(heights) => {
                let tag = Atom::new(allocator, tas!(b"heights")).as_noun();
                let data = heights.to_noun(allocator);
                T(allocator, &[tag, data])
            }
            ProofData::Poly(poly) => {
                let tag = Atom::new(allocator, tas!(b"poly")).as_noun();
                let data = poly.to_noun(allocator);
                T(allocator, &[tag, data])
            }
        }
    }
}

impl NounDecode for ProofVersion {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let atom = noun.in_space(space).as_atom()?;
        if noun.is_cell() || atom.atom().is_indirect() {
            return Err(NounDecodeError::ExpectedCell);
        }
        match atom.as_u64()? {
            0 => Ok(ProofVersion::V0),
            1 => Ok(ProofVersion::V1),
            2 => Ok(ProofVersion::V2),
            3 => Ok(ProofVersion::V3),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

impl NounEncode for ProofVersion {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            ProofVersion::V0 => Atom::new(allocator, 0).as_noun(),
            ProofVersion::V1 => Atom::new(allocator, 1).as_noun(),
            ProofVersion::V2 => Atom::new(allocator, 2).as_noun(),
            ProofVersion::V3 => Atom::new(allocator, 3).as_noun(),
        }
    }
}

pub fn dyck_word(noun: Noun, space: &NounSpace) -> Vec<u64> {
    let mut dyck = Vec::new();
    do_dyck_word(noun, &mut dyck, space);
    dyck
}

fn do_dyck_word(noun: Noun, dyck: &mut Vec<u64>, space: &NounSpace) {
    if let Ok(cell) = noun.in_space(space).as_cell() {
        dyck.push(0);
        do_dyck_word(cell.head().noun(), dyck, space);
        dyck.push(1);
        do_dyck_word(cell.tail().noun(), dyck, space);
    }
}

pub fn reassemble_noun<A: NounAllocator>(
    allocator: &mut A,
    leaf: &[u64],
    dyck: &[u64],
) -> Result<Noun, ReassembleNounError> {
    fn rec<A: NounAllocator>(
        allocator: &mut A,
        leaf: &[u64],
        dyck: &[u64],
        leaf_index: usize,
        dyck_index: usize,
    ) -> Result<(Noun, usize, usize), ReassembleNounError> {
        match dyck.get(dyck_index) {
            Some(0) => {
                let (head, leaf_index, dyck_index) =
                    rec(allocator, leaf, dyck, leaf_index, dyck_index + 1)?;
                if dyck.get(dyck_index) != Some(&1) {
                    return Err(ReassembleNounError::InvalidDyck);
                }
                let (tail, leaf_index, dyck_index) =
                    rec(allocator, leaf, dyck, leaf_index, dyck_index + 1)?;
                Ok((T(allocator, &[head, tail]), leaf_index, dyck_index))
            }
            Some(1) | None => {
                let atom = leaf
                    .get(leaf_index)
                    .ok_or(ReassembleNounError::InvalidLeafOrDyckWord)?;
                Ok((
                    Atom::new(allocator, *atom).as_noun(),
                    leaf_index + 1,
                    dyck_index,
                ))
            }
            Some(_) => Err(ReassembleNounError::InvalidDyck),
        }
    }

    let (noun, leaf_index, dyck_index) = rec(allocator, leaf, dyck, 0, 0)?;
    if leaf_index != leaf.len() || dyck_index != dyck.len() {
        return Err(ReassembleNounError::InvalidLeafOrDyckWord);
    }
    Ok(noun)
}

#[cfg(test)]
mod tests {
    use nockvm::ext::NounExt;
    use nockvm::mem::{NockStack, NOCK_STACK_SIZE};
    use noun_serde::{NounDecode, NounEncode};

    use super::{Proof, ProofVersion};

    #[test]
    fn decodes_and_reencodes_public_fixtures() {
        for (bytes, version) in [
            (
                include_bytes!("../../../roswell/tests/fixtures/proof-v0-len1.jam").as_slice(),
                ProofVersion::V0,
            ),
            (
                include_bytes!("../../../roswell/tests/fixtures/proof-v1-len1.jam").as_slice(),
                ProofVersion::V1,
            ),
            (
                include_bytes!("../../../roswell/tests/fixtures/proof-v2-len1.jam").as_slice(),
                ProofVersion::V2,
            ),
        ] {
            let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
            let noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, bytes)
                .expect("fixture should cue");
            let space = stack.noun_space();
            let proof = Proof::from_noun(&noun, &space).expect("fixture should decode");
            assert_eq!(proof.version, version);
            assert!(!proof.objects.is_empty());

            let encoded = proof.to_noun(&mut stack);
            let reparsed_space = stack.noun_space();
            let reparsed =
                Proof::from_noun(&encoded, &reparsed_space).expect("encoded proof decodes");
            assert_eq!(proof, reparsed);
            let reencoded = encoded.jam_self(&mut stack);
            assert_eq!(bytes, reencoded.0.as_ref());
        }
    }

    #[test]
    fn proof_version_v3_round_trips() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let encoded = ProofVersion::V3.to_noun(&mut stack);
        let space = stack.noun_space();
        let decoded = ProofVersion::from_noun(&encoded, &space).expect("v3 should decode");
        assert_eq!(decoded, ProofVersion::V3);
    }
}
