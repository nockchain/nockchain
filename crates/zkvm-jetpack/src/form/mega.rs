use nockvm_macros::tas;

use crate::form::belt::Belt;

// +$  mega-typ  ?(%var %rnd %dyn %con %com)
#[repr(u64)]
#[derive(Clone, Copy, Debug, strum::FromRepr)]
pub enum MegaTyp {
    Con = 0,
    Var = 1,
    Rnd = 2,
    Dyn = 3,
    Com = 4,
}

impl MegaTyp {
    pub fn to_tas(self) -> u64 {
        match self {
            Self::Con => tas!(b"con"),
            Self::Var => tas!(b"var"),
            Self::Rnd => tas!(b"rnd"),
            Self::Dyn => tas!(b"dyn"),
            Self::Com => tas!(b"com"),
        }
    }
}

impl TryFrom<u64> for MegaTyp {
    type Error = ();

    fn try_from(value: u64) -> std::result::Result<Self, Self::Error> {
        Self::from_repr(value).ok_or(())
    }
}

// ::  bit length of type
// ++  typ-len  3
const TYP_LEN: u64 = 3;
// ::  bit length of index
// ++  idx-len  10
const IDX_LEN: u64 = 10;
// ::  bit length of exponent
// ++  exp-len  30
const EXP_LEN: u64 = 30;

fn mega_typ(term: u64) -> core::result::Result<MegaTyp, ()> {
    // ^-  mega-typ
    // ?+  (cut 0 [0 typ-len] term)  !!
    (term & ((1 << TYP_LEN) - 1)).try_into()
}

fn mega_idx(term: u64) -> usize {
    // ^-  @ud
    // (cut 0 [typ-len idx-len] term)
    ((term & (((1 << IDX_LEN) - 1) << TYP_LEN)) >> TYP_LEN) as usize
}

fn mega_exp(term: u64) -> u64 {
    // ^-  @ud
    // (cut 0 [(add typ-len idx-len) exp-len] term)
    (term & (((1 << EXP_LEN) - 1) << (TYP_LEN + IDX_LEN))) >> (TYP_LEN + IDX_LEN)
}

pub fn brek(ter: Belt) -> (MegaTyp, usize, u64) {
    //  |=  ter=mega-term
    //  ^-  [mega-typ @ @ud]
    //  :+  ~(typ mega ter)
    //    ~(idx mega ter)
    //  ~(exp mega ter)
    (
        mega_typ(ter.0).expect("Invalid term passed"),
        mega_idx(ter.0),
        mega_exp(ter.0),
    )
}
