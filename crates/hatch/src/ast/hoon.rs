use std::collections::*;
use std::ops::BitOr;

use num_bigint::BigUint;
use num_traits::Zero;
use serde::Serialize;

#[derive(serde::Serialize, Hash, Eq, PartialEq, Debug, Clone)]
pub enum NounExpr {
    ParsedAtom(ParsedAtom),
    Cell(Box<NounExpr>, Box<NounExpr>),
}

#[derive(serde::Serialize, Hash, Eq, PartialEq, Debug, Clone)]
pub enum ParsedAtom {
    Small(u128),
    #[serde(serialize_with = "serialize_biguint_decimal")]
    Big(BigUint),
}

fn serialize_biguint_decimal<S>(value: &BigUint, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

impl From<u16> for ParsedAtom {
    fn from(x: u16) -> Self {
        ParsedAtom::Small(x as u128)
    }
}

impl From<u32> for ParsedAtom {
    fn from(x: u32) -> Self {
        ParsedAtom::Small(x as u128)
    }
}

impl From<u64> for ParsedAtom {
    fn from(x: u64) -> Self {
        ParsedAtom::Small(x as u128)
    }
}

impl ParsedAtom {
    pub fn to_u8(&self) -> Option<u8> {
        match self {
            ParsedAtom::Small(n) => (*n as u128).try_into().ok(),
            ParsedAtom::Big(b) => b.try_into().ok(),
        }
    }
    pub fn to_u32(&self) -> Option<u32> {
        match self {
            ParsedAtom::Small(n) => Some(*n as u32),
            ParsedAtom::Big(b) => b.try_into().ok(),
        }
    }
    pub fn to_u128(&self) -> Option<u128> {
        match self {
            ParsedAtom::Small(n) => Some(*n as u128),
            ParsedAtom::Big(b) => b.try_into().ok(),
        }
    }

    pub fn to_biguint(&self) -> BigUint {
        match self {
            ParsedAtom::Small(n) => (*n).into(),
            ParsedAtom::Big(b) => b.clone(),
        }
    }

    pub fn from_biguint(b: BigUint) -> Self {
        if let Ok(n) = u128::try_from(&b) {
            ParsedAtom::Small(n)
        } else {
            ParsedAtom::Big(b)
        }
    }

    pub fn is_zero(&self) -> bool {
        match self {
            ParsedAtom::Small(n) => *n == 0,
            ParsedAtom::Big(n) => BigUint::is_zero(n),
        }
    }

    pub fn zero() -> Self {
        ParsedAtom::Small(0)
    }

    pub fn to_u64_lossy(&self) -> u64 {
        match self {
            ParsedAtom::Small(n) => *n as u64,
            ParsedAtom::Big(b) => {
                // truncate safely — only used where input < 2^16
                let bytes = b.to_bytes_le();
                let mut out = 0u64;
                for (i, &byte) in bytes.iter().take(8).enumerate() {
                    out |= (byte as u64) << (i * 8);
                }
                out
            }
        }
    }

    pub fn to_u8_lossy(&self) -> u8 {
        (self.to_u64_lossy() & 0xFF) as u8
    }

    pub fn to_u16_lossy(&self) -> u16 {
        (self.to_u64_lossy() & 0xFFFF) as u16
    }

    pub fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (ParsedAtom::Small(a), ParsedAtom::Small(b)) => a.cmp(b),
            (ParsedAtom::Small(a), ParsedAtom::Big(b)) => {
                let a_big = BigUint::from(*a);
                a_big.cmp(b)
            }
            (ParsedAtom::Big(a), ParsedAtom::Small(b)) => {
                let b_big = BigUint::from(*b);
                a.cmp(&b_big)
            }
            (ParsedAtom::Big(a), ParsedAtom::Big(b)) => a.cmp(b),
        }
    }

    pub fn lt(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Less
    }
    pub fn le(&self, other: &Self) -> bool {
        self.cmp(other) != std::cmp::Ordering::Greater
    }
    pub fn gt(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Greater
    }
    pub fn ge(&self, other: &Self) -> bool {
        self.cmp(other) != std::cmp::Ordering::Less
    }
    pub fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl From<&str> for ParsedAtom {
    fn from(s: &str) -> Self {
        // UTF-8 bytes, little-endian @
        let bytes: Vec<u8> = s.bytes().collect();
        let mut acc = BigUint::from(0u32);
        for &b in bytes.iter().rev() {
            // little-endian: first char = lowest byte
            acc = (acc << 8) + BigUint::from(b);
        }
        if let Ok(small) = acc.clone().try_into() {
            ParsedAtom::Small(small)
        } else {
            ParsedAtom::Big(acc)
        }
    }
}

impl BitOr for ParsedAtom {
    type Output = ParsedAtom;

    fn bitor(self, rhs: ParsedAtom) -> ParsedAtom {
        match (self, rhs) {
            (ParsedAtom::Small(a), ParsedAtom::Small(b)) => ParsedAtom::Small(a | b),

            (ParsedAtom::Small(a), ParsedAtom::Big(b)) => ParsedAtom::Big(BigUint::from(a) | b),

            (ParsedAtom::Big(a), ParsedAtom::Small(b)) => ParsedAtom::Big(a | BigUint::from(b)),

            (ParsedAtom::Big(a), ParsedAtom::Big(b)) => ParsedAtom::Big(a | b),
        }
    }
}

// (-1)^s * a * 10^e
//  +dn
#[derive(Clone, Debug)]
pub enum DecimalFloat {
    Finite {
        sign: bool,
        exp: u128,
        mant: BigUint,
    },
    Infinity {
        sign: bool,
    },
    NaN,
}

//  (-1)^s * a * 2^e
//  +fn
#[derive(Clone, Debug)]
pub enum BinaryFloat {
    Finite {
        sign: bool,
        exp: u128,
        mant: BigUint,
    },
    Infinity {
        sign: bool,
    },
    NaN,
}

impl BinaryFloat {
    pub fn sign(&self) -> bool {
        match self {
            BinaryFloat::Finite { sign, .. } => *sign,
            BinaryFloat::Infinity { sign } => *sign,
            BinaryFloat::NaN => false, // irrelevant
        }
    }
}
pub type What = Option<NounExpr>;
pub type Tome = (What, HashMap<Term, Hoon>);
pub type Tune = (HashMap<String, Option<Hoon>>, Vec<Hoon>);
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum TermOrTune {
    Term(Term),
    Tune(Tune),
}
pub type Help = String;
pub type Knot = String;
pub type Cord = String;

// TODO: should be vec<u8>, or maybe just String
pub type Tape = Vec<String>;
pub type Path = Vec<Knot>;
pub type Tyre = Vec<(Term, Hoon)>;
pub type Axis = u64;

pub type SemiNounExpr = (Stencil, NounExpr);

pub type Gate = (Box<Spec>, Box<Spec>);

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Stencil {
    Half {
        left: Box<Stencil>,
        rite: Box<Stencil>,
    },
    Full {
        blocks: Vec<Block>,
    }, // change to set?
    Lazy {
        fragment: Axis,
        resolve: Gate,
    },
}

pub type Block = Vec<Path>;

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Beer {
    Char(Cord),
    Hoon(Hoon),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Woof {
    ParsedAtom(ParsedAtom),
    Hoon(Hoon),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Mane {
    Tag(String),
    TagSpace(String, String),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub struct Manx {
    pub g: Marx,
    pub c: Marl,
}

pub type Marl = Vec<Tuna>;

pub type Mart = Vec<(Mane, Vec<Beer>)>;

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub struct Marx {
    pub n: Mane,
    pub a: Mart,
}

#[derive(Debug, Clone)]
pub enum Mare {
    Manx(Manx),
    Marl(Marl),
}

#[derive(Debug, Clone)]
pub enum Maru {
    Tuna(Tuna),
    Marl(Marl),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Tuna {
    Manx(Manx),
    TunaTail(TunaTail),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum TunaTail {
    Tape(Hoon),
    Manx(Hoon),
    Marl(Hoon),
    Call(Hoon),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Chum {
    Lef(String),
    StdKel(String, ParsedAtom),
    VenProKel(String, String, ParsedAtom),
    VenProVerKel(String, String, ParsedAtom, ParsedAtom),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Coin {
    Dime(String, ParsedAtom),
    Blob(NounExpr),
    Many(Vec<Coin>),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub struct Pint {
    pub p: (u64, u64),
    pub q: (u64, u64),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub struct Spot {
    pub p: Path,
    pub q: Pint,
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Limb {
    Term(String),
    Axis(u64),
    Parent(u64, Option<String>),
}

pub type WingType = Vec<Limb>;
pub type Term = String;

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Spec {
    Base(BaseType),
    Dbug(Spot, Box<Spec>),
    Gist(NounExpr, Box<Spec>),
    Leaf(String, ParsedAtom),
    Like(WingType, Vec<WingType>),
    Loop(String),
    Made((String, Vec<String>), Box<Spec>),
    Make(Hoon, Vec<Spec>),
    Name(String, Box<Spec>),
    Over(WingType, Box<Spec>),
    BucGar(Box<Spec>, Box<Spec>),
    BucBuc(Box<Spec>, HashMap<String, Spec>),
    BucBar(Box<Spec>, Hoon),
    BucCab(Hoon),
    BucCol(Box<Spec>, Vec<Spec>),
    BucCen(Box<Spec>, Vec<Spec>),
    BucDot(Box<Spec>, HashMap<String, Spec>),
    BucGal(Box<Spec>, Box<Spec>),
    BucHep(Box<Spec>, Box<Spec>),
    BucKet(Box<Spec>, Box<Spec>),
    BucLus(String, Box<Spec>),
    BucFas(Box<Spec>, HashMap<String, Spec>),
    BucMic(Hoon),
    BucPam(Box<Spec>, Hoon),
    BucSig(Hoon, Box<Spec>),
    BucTic(Box<Spec>, HashMap<String, Spec>),
    BucTis(Skin, Box<Spec>),
    BucPat(Box<Spec>, Box<Spec>),
    BucWut(Box<Spec>, Vec<Spec>),
    BucZap(Box<Spec>, HashMap<String, Spec>),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Nock {
    Pair(Box<Nock>, Box<Nock>),
    Const(NounExpr),
    Compose(Box<Nock>, Box<Nock>),
    CellTest(Box<Nock>),
    Increment(Box<Nock>),
    Equality(Box<Nock>, Box<Nock>),
    IfThenElse(Box<Nock>, Box<Nock>, Box<Nock>),
    SerialCompose(Box<Nock>, Box<Nock>),
    PushSubject(Box<Nock>, Box<Nock>),
    SelectArm(u64, Box<Nock>),
    Edit((u64, Box<Nock>), Box<Nock>),
    Hint(NockHint, Box<Nock>),
    GrabData(Box<Nock>, Box<Nock>),
    AxisSelect(u64),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum NockHint {
    ParsedAtom(u64),
    Pair(u64, Box<Nock>),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Note {
    Help(NounExpr),
    Know(String),
    Made(String, Option<Vec<WingType>>),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub struct Coil {
    pub p: Garb,
    pub q: Type,
    pub r: (SemiNounExpr, HashMap<String, Tome>),
}

#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct Garb {
    pub name: Option<String>,
    pub poly: Poly,
    pub vair: Vair,
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Poly {
    Wet,
    Dry,
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Vair {
    Gold,
    Iron,
    Lead,
    Zinc,
}

pub type Aura = String; // @ta

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub enum BaseType {
    NounExpr,
    Cell,
    Flag,
    Null,
    Void,
    Atom(Aura), // Aura
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Tiki {
    Wing((Option<String>, WingType)),
    Hoon((Option<String>, Box<Hoon>)),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Skin {
    Term(String),
    Base(BaseType),
    Cell(Box<Skin>, Box<Skin>),
    Dbug(Spot, Box<Skin>),
    Help(NounExpr, Box<Skin>),
    Leaf(String, ParsedAtom),
    Name(String, Box<Skin>),
    Over(WingType, Box<Skin>),
    Spec(Box<Spec>, Box<Skin>),
    Wash(u64),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Type {
    NounExpr,
    Void,
    ParsedAtom(String, Option<u64>),
    Cell(Box<Type>, Box<Type>),
    Core(Box<Type>, Box<Coil>),
    Face(FaceType, Box<Type>),
    Fork(Vec<Type>), // change to set?
    Hint((Box<Type>, Note), Box<Type>),
    Hold(Box<Type>, Hoon),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum FaceType {
    Term(String),
    Tune(Tune),
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum ZpwtArg {
    ParsedAtom(String),
    Pair(String, String),
}

pub type Alas = Vec<(String, Hoon)>;

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum TermOrPair {
    Term(String),
    Pair(String, Box<Hoon>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tarp {
    pub d: u64,
    pub h: u64,
    pub m: u64,
    pub s: u64,
    pub f: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Date {
    pub era: bool, // a=? — true = AD, false = BC
    pub y: u64,    // year (1-based; year 0 = 1 BC, year -1 = 2 BC, etc.)
    pub m: u64,    // month (1–12)
    pub t: Tarp,   // time-of-day + day-of-month in tarp.d
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
pub enum Hoon {
    // Tuple pair
    Pair(Box<Hoon>, Box<Hoon>),
    // Hoon's version of a panic/crash
    ZapZap,
    Axis(u64),
    Base(BaseType),
    Bust(BaseType),
    Dbug(Spot, Box<Hoon>),
    Eror(String),
    Hand(Box<Type>, Nock),
    Note(Note, Box<Hoon>),
    Fits(Box<Hoon>, WingType),
    Knit(Vec<Woof>),
    Leaf(String, ParsedAtom),
    Limb(String),
    Lost(Box<Hoon>),
    Rock(String, NounExpr),
    Sand(String, NounExpr),
    Tell(Vec<Hoon>),
    Tune(TermOrTune),
    Wing(WingType),
    Yell(Vec<Hoon>),
    Xray(Manx),
    BarBuc(Vec<String>, Box<Spec>),
    BarCab(Box<Spec>, Alas, HashMap<String, Tome>),
    BarCol(Box<Hoon>, Box<Hoon>),
    BarCen(Option<String>, HashMap<String, Tome>),
    BarDot(Box<Hoon>),
    BarKet(Box<Hoon>, HashMap<String, Tome>),
    BarHep(Box<Hoon>),
    BarSig(Box<Spec>, Box<Hoon>),
    BarTar(Box<Spec>, Box<Hoon>),
    BarTis(Box<Spec>, Box<Hoon>),
    BarPat(Option<String>, HashMap<String, Tome>),
    BarWut(Box<Hoon>),
    ColCab(Box<Hoon>, Box<Hoon>),
    ColKet(Box<Hoon>, Box<Hoon>, Box<Hoon>, Box<Hoon>),
    ColHep(Box<Hoon>, Box<Hoon>),
    ColLus(Box<Hoon>, Box<Hoon>, Box<Hoon>),
    ColSig(Vec<Hoon>),
    ColTar(Vec<Hoon>),
    CenCab(WingType, Vec<(WingType, Hoon)>),
    CenDot(Box<Hoon>, Box<Hoon>),
    CenHep(Box<Hoon>, Box<Hoon>),
    CenCol(Box<Hoon>, Vec<Hoon>),
    CenTar(WingType, Box<Hoon>, Vec<(WingType, Hoon)>),
    CenKet(Box<Hoon>, Box<Hoon>, Box<Hoon>, Box<Hoon>),
    CenLus(Box<Hoon>, Box<Hoon>, Box<Hoon>),
    CenSig(WingType, Box<Hoon>, Vec<Hoon>),
    CenTis(WingType, Vec<(WingType, Hoon)>),
    DotKet(Box<Spec>, Box<Hoon>),
    DotLus(Box<Hoon>),
    DotTar(Box<Hoon>, Box<Hoon>),
    DotTis(Box<Hoon>, Box<Hoon>),
    DotWut(Box<Hoon>),
    KetBar(Box<Hoon>),
    KetDot(Box<Hoon>, Box<Hoon>),
    KetLus(Box<Hoon>, Box<Hoon>),
    KetHep(Box<Spec>, Box<Hoon>),
    KetPam(Box<Hoon>),
    KetSig(Box<Hoon>),
    KetTis(Skin, Box<Hoon>),
    KetWut(Box<Hoon>),
    KetTar(Box<Spec>),
    KetCol(Box<Spec>),
    SigBar(Box<Hoon>, Box<Hoon>),
    SigCab(Box<Hoon>, Box<Hoon>),
    SigCen(Chum, Box<Hoon>, Tyre, Box<Hoon>),
    SigFas(Chum, Box<Hoon>),
    SigGal(TermOrPair, Box<Hoon>),
    SigGar(TermOrPair, Box<Hoon>),
    SigBuc(String, Box<Hoon>),
    SigLus(u64, Box<Hoon>),
    SigPam(u64, Box<Hoon>, Box<Hoon>),
    SigTis(Box<Hoon>, Box<Hoon>),
    SigWut(u64, Box<Hoon>, Box<Hoon>, Box<Hoon>),
    SigZap(Box<Hoon>, Box<Hoon>),
    MicTis(Marl),
    MicCol(Box<Hoon>, Vec<Hoon>),
    MicFas(Box<Hoon>),
    MicGal(Box<Spec>, Box<Hoon>, Box<Hoon>, Box<Hoon>),
    MicSig(Box<Hoon>, Vec<Hoon>),
    MicMic(Box<Spec>, Box<Hoon>),
    TisBar(Box<Spec>, Box<Hoon>),
    TisCol(Vec<(WingType, Hoon)>, Box<Hoon>),
    TisFas(Skin, Box<Hoon>, Box<Hoon>),
    TisMic(Skin, Box<Hoon>, Box<Hoon>),
    TisDot(WingType, Box<Hoon>, Box<Hoon>),
    TisWut(WingType, Box<Hoon>, Box<Hoon>, Box<Hoon>),
    TisGal(Box<Hoon>, Box<Hoon>),
    TisHep(Box<Hoon>, Box<Hoon>),
    TisGar(Box<Hoon>, Box<Hoon>),
    TisKet(Skin, WingType, Box<Hoon>, Box<Hoon>),
    TisLus(Box<Hoon>, Box<Hoon>),
    TisSig(Vec<Hoon>),
    TisTar((String, Option<Box<Spec>>), Box<Hoon>, Box<Hoon>),
    TisCom(Box<Hoon>, Box<Hoon>),
    WutBar(Vec<Hoon>),
    WutHep(WingType, Vec<(Spec, Hoon)>),
    WutCol(Box<Hoon>, Box<Hoon>, Box<Hoon>),
    WutDot(Box<Hoon>, Box<Hoon>, Box<Hoon>),
    WutKet(WingType, Box<Hoon>, Box<Hoon>),
    WutGal(Box<Hoon>, Box<Hoon>),
    WutGar(Box<Hoon>, Box<Hoon>),
    WutLus(WingType, Box<Hoon>, Vec<(Spec, Hoon)>),
    WutPam(Vec<Hoon>),
    WutPat(WingType, Box<Hoon>, Box<Hoon>),
    WutSig(WingType, Box<Hoon>, Box<Hoon>),
    WutHax(Skin, WingType),
    // Yields a loobean (boolean) from a type match
    WutTis(Box<Spec>, WingType),
    WutZap(Box<Hoon>),
    ZapCom(Box<Hoon>, Box<Hoon>),
    ZapGar(Box<Hoon>),
    ZapGal(Box<Spec>, Box<Hoon>),
    ZapMic(Box<Hoon>, Box<Hoon>),
    ZapTis(Box<Hoon>),
    ZapPat(Vec<WingType>, Box<Hoon>, Box<Hoon>),
    ZapWut(ZpwtArg, Box<Hoon>),
}
