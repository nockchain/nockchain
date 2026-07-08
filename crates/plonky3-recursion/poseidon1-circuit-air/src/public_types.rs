//! Field-specific configurations and type aliases for the Poseidon1 circuit AIR.
//!
//! ```text
//!     Field       Extension degree   State width   Partial rounds
//!     ─────────   ────────────────   ───────────   ──────────────
//!     BabyBear    1                  16            13
//!     BabyBear    4                  16            13
//!     BabyBear    4                  24            21
//!     KoalaBear   1                  16            20
//!     KoalaBear   4                  16            20
//!     KoalaBear   4                  24            23
//!     Goldilocks  2                   8            22
//! ```

extern crate alloc;

use alloc::vec::Vec;

use p3_baby_bear::{
    BABYBEAR_POSEIDON1_HALF_FULL_ROUNDS, BABYBEAR_POSEIDON1_PARTIAL_ROUNDS_16,
    BABYBEAR_POSEIDON1_PARTIAL_ROUNDS_24, BABYBEAR_POSEIDON1_RC_16, BABYBEAR_POSEIDON1_RC_24,
    BabyBear, MDSBabyBearData,
};
use p3_field::PrimeField;
use p3_goldilocks::poseidon1::{
    GOLDILOCKS_POSEIDON_HALF_FULL_ROUNDS, GOLDILOCKS_POSEIDON_PARTIAL_ROUNDS_8,
    GOLDILOCKS_POSEIDON1_RC_8,
};
use p3_goldilocks::{Goldilocks, MATRIX_CIRC_MDS_8_COL};
use p3_koala_bear::{
    KOALABEAR_POSEIDON_HALF_FULL_ROUNDS, KOALABEAR_POSEIDON_PARTIAL_ROUNDS_16,
    KOALABEAR_POSEIDON_PARTIAL_ROUNDS_24, KOALABEAR_POSEIDON1_RC_16, KOALABEAR_POSEIDON1_RC_24,
    KoalaBear, MDSKoalaBearData,
};
use p3_monty_31::MDSUtils;
use p3_poseidon1::Poseidon1Constants;
use p3_poseidon1_air::{FullRoundConstants, PartialRoundConstants};

use crate::Poseidon1CircuitAir;

/// Geometry of a supported Poseidon1 circuit configuration.
///
/// Carries the const-generic parameters of [`Poseidon1CircuitAir`] for one
/// field/width/extension-degree combination.
pub trait Poseidon1Params {
    type BaseField: PrimeField;
    /// Challenge extension degree.
    const D: usize;
    /// State width in base-field elements.
    const WIDTH: usize;
    /// State width in extension-field limbs (`WIDTH / D`).
    const WIDTH_EXT: usize;
    /// Rate in extension-field limbs.
    const RATE_EXT: usize;
    /// Capacity in extension-field limbs.
    const CAPACITY_EXT: usize;
    /// S-box polynomial degree.
    const SBOX_DEGREE: u64;
    /// Number of S-box intermediate registers.
    const SBOX_REGISTERS: usize;
    /// Number of full rounds per half.
    const HALF_FULL_ROUNDS: usize;
    /// Number of partial rounds.
    const PARTIAL_ROUNDS: usize;
}

/// Optimized (sparse-form) round constants for a Poseidon1 instance.
pub type OptimizedConstants<F, const W: usize> =
    (FullRoundConstants<F, W>, PartialRoundConstants<F, W>);

/// Build the AIR round constants from raw Poseidon1 parameters.
fn optimized_constants<F: PrimeField, const W: usize>(
    half_full_rounds: usize,
    partial_rounds: usize,
    mds_circ_col: [i64; W],
    round_constants: Vec<[F; W]>,
) -> OptimizedConstants<F, W> {
    Poseidon1Constants {
        rounds_f: 2 * half_full_rounds,
        rounds_p: partial_rounds,
        mds_circ_col,
        round_constants,
    }
    .to_optimized()
}

/// BabyBear, base-field (`D=1`) challenges, 16-element state.
pub struct BabyBearD1Width16;

impl Poseidon1Params for BabyBearD1Width16 {
    type BaseField = BabyBear;
    const D: usize = 1;
    const WIDTH: usize = 16;
    const WIDTH_EXT: usize = 16;
    const RATE_EXT: usize = 8;
    const CAPACITY_EXT: usize = 8;
    const SBOX_DEGREE: u64 = 7;
    const SBOX_REGISTERS: usize = 1;
    const HALF_FULL_ROUNDS: usize = 4;
    const PARTIAL_ROUNDS: usize = 13;
}

impl BabyBearD1Width16 {
    pub fn round_constants() -> OptimizedConstants<BabyBear, 16> {
        optimized_constants(
            BABYBEAR_POSEIDON1_HALF_FULL_ROUNDS,
            BABYBEAR_POSEIDON1_PARTIAL_ROUNDS_16,
            MDSBabyBearData::MATRIX_CIRC_MDS_16_COL,
            BABYBEAR_POSEIDON1_RC_16.to_vec(),
        )
    }

    pub fn default_air() -> Poseidon1CircuitAirBabyBearD1Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirBabyBearD1Width16::new(full, partial)
    }

    pub fn default_air_with_preprocessed(
        preprocessed: Vec<BabyBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirBabyBearD1Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirBabyBearD1Width16::new_with_preprocessed(full, partial, preprocessed)
            .with_min_height(min_height)
    }

    pub fn default_air_with_preprocessed_witness_bus5(
        preprocessed: Vec<BabyBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirBabyBearD1Width16WitnessBus5 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirBabyBearD1Width16WitnessBus5::new_with_preprocessed(
            full,
            partial,
            preprocessed,
        )
        .with_min_height(min_height)
    }
}

/// BabyBear, quartic extension, 16-element state.
pub struct BabyBearD4Width16;

impl Poseidon1Params for BabyBearD4Width16 {
    type BaseField = BabyBear;
    const D: usize = 4;
    const WIDTH: usize = 16;
    const WIDTH_EXT: usize = 4;
    const RATE_EXT: usize = 2;
    const CAPACITY_EXT: usize = 2;
    const SBOX_DEGREE: u64 = 7;
    const SBOX_REGISTERS: usize = 1;
    const HALF_FULL_ROUNDS: usize = 4;
    const PARTIAL_ROUNDS: usize = 13;
}

impl BabyBearD4Width16 {
    pub fn round_constants() -> OptimizedConstants<BabyBear, 16> {
        optimized_constants(
            BABYBEAR_POSEIDON1_HALF_FULL_ROUNDS,
            BABYBEAR_POSEIDON1_PARTIAL_ROUNDS_16,
            MDSBabyBearData::MATRIX_CIRC_MDS_16_COL,
            BABYBEAR_POSEIDON1_RC_16.to_vec(),
        )
    }

    pub fn default_air() -> Poseidon1CircuitAirBabyBearD4Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirBabyBearD4Width16::new(full, partial)
    }

    pub fn default_air_with_preprocessed(
        preprocessed: Vec<BabyBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirBabyBearD4Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirBabyBearD4Width16::new_with_preprocessed(full, partial, preprocessed)
            .with_min_height(min_height)
    }
}

/// BabyBear, quartic extension, 24-element state.
pub struct BabyBearD4Width24;

impl Poseidon1Params for BabyBearD4Width24 {
    type BaseField = BabyBear;
    const D: usize = 4;
    const WIDTH: usize = 24;
    const WIDTH_EXT: usize = 6;
    const RATE_EXT: usize = 4;
    const CAPACITY_EXT: usize = 2;
    const SBOX_DEGREE: u64 = 7;
    const SBOX_REGISTERS: usize = 1;
    const HALF_FULL_ROUNDS: usize = 4;
    const PARTIAL_ROUNDS: usize = 21;
}

impl BabyBearD4Width24 {
    pub fn round_constants() -> OptimizedConstants<BabyBear, 24> {
        optimized_constants(
            BABYBEAR_POSEIDON1_HALF_FULL_ROUNDS,
            BABYBEAR_POSEIDON1_PARTIAL_ROUNDS_24,
            MDSBabyBearData::MATRIX_CIRC_MDS_24_COL,
            BABYBEAR_POSEIDON1_RC_24.to_vec(),
        )
    }

    pub fn default_air() -> Poseidon1CircuitAirBabyBearD4Width24 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirBabyBearD4Width24::new(full, partial)
    }

    pub fn default_air_with_preprocessed(
        preprocessed: Vec<BabyBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirBabyBearD4Width24 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirBabyBearD4Width24::new_with_preprocessed(full, partial, preprocessed)
            .with_min_height(min_height)
    }
}

/// KoalaBear, base-field (`D=1`) challenges, 16-element state.
pub struct KoalaBearD1Width16;

impl Poseidon1Params for KoalaBearD1Width16 {
    type BaseField = KoalaBear;
    const D: usize = 1;
    const WIDTH: usize = 16;
    const WIDTH_EXT: usize = 16;
    const RATE_EXT: usize = 8;
    const CAPACITY_EXT: usize = 8;
    const SBOX_DEGREE: u64 = 3;
    const SBOX_REGISTERS: usize = 0;
    const HALF_FULL_ROUNDS: usize = 4;
    const PARTIAL_ROUNDS: usize = 20;
}

impl KoalaBearD1Width16 {
    pub fn round_constants() -> OptimizedConstants<KoalaBear, 16> {
        optimized_constants(
            KOALABEAR_POSEIDON_HALF_FULL_ROUNDS,
            KOALABEAR_POSEIDON_PARTIAL_ROUNDS_16,
            MDSKoalaBearData::MATRIX_CIRC_MDS_16_COL,
            KOALABEAR_POSEIDON1_RC_16.to_vec(),
        )
    }

    pub fn default_air() -> Poseidon1CircuitAirKoalaBearD1Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirKoalaBearD1Width16::new(full, partial)
    }

    pub fn default_air_with_preprocessed(
        preprocessed: Vec<KoalaBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirKoalaBearD1Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirKoalaBearD1Width16::new_with_preprocessed(full, partial, preprocessed)
            .with_min_height(min_height)
    }

    pub fn default_air_with_preprocessed_witness_bus5(
        preprocessed: Vec<KoalaBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirKoalaBearD1Width16WitnessBus5 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirKoalaBearD1Width16WitnessBus5::new_with_preprocessed(
            full,
            partial,
            preprocessed,
        )
        .with_min_height(min_height)
    }
}

/// KoalaBear, quartic extension, 16-element state.
pub struct KoalaBearD4Width16;

impl Poseidon1Params for KoalaBearD4Width16 {
    type BaseField = KoalaBear;
    const D: usize = 4;
    const WIDTH: usize = 16;
    const WIDTH_EXT: usize = 4;
    const RATE_EXT: usize = 2;
    const CAPACITY_EXT: usize = 2;
    const SBOX_DEGREE: u64 = 3;
    const SBOX_REGISTERS: usize = 0;
    const HALF_FULL_ROUNDS: usize = 4;
    const PARTIAL_ROUNDS: usize = 20;
}

impl KoalaBearD4Width16 {
    pub fn round_constants() -> OptimizedConstants<KoalaBear, 16> {
        optimized_constants(
            KOALABEAR_POSEIDON_HALF_FULL_ROUNDS,
            KOALABEAR_POSEIDON_PARTIAL_ROUNDS_16,
            MDSKoalaBearData::MATRIX_CIRC_MDS_16_COL,
            KOALABEAR_POSEIDON1_RC_16.to_vec(),
        )
    }

    pub fn default_air() -> Poseidon1CircuitAirKoalaBearD4Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirKoalaBearD4Width16::new(full, partial)
    }

    pub fn default_air_with_preprocessed(
        preprocessed: Vec<KoalaBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirKoalaBearD4Width16 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirKoalaBearD4Width16::new_with_preprocessed(full, partial, preprocessed)
            .with_min_height(min_height)
    }
}

/// KoalaBear, quartic extension, 24-element state.
pub struct KoalaBearD4Width24;

impl Poseidon1Params for KoalaBearD4Width24 {
    type BaseField = KoalaBear;
    const D: usize = 4;
    const WIDTH: usize = 24;
    const WIDTH_EXT: usize = 6;
    const RATE_EXT: usize = 4;
    const CAPACITY_EXT: usize = 2;
    const SBOX_DEGREE: u64 = 3;
    const SBOX_REGISTERS: usize = 0;
    const HALF_FULL_ROUNDS: usize = 4;
    const PARTIAL_ROUNDS: usize = 23;
}

impl KoalaBearD4Width24 {
    pub fn round_constants() -> OptimizedConstants<KoalaBear, 24> {
        optimized_constants(
            KOALABEAR_POSEIDON_HALF_FULL_ROUNDS,
            KOALABEAR_POSEIDON_PARTIAL_ROUNDS_24,
            MDSKoalaBearData::MATRIX_CIRC_MDS_24_COL,
            KOALABEAR_POSEIDON1_RC_24.to_vec(),
        )
    }

    pub fn default_air() -> Poseidon1CircuitAirKoalaBearD4Width24 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirKoalaBearD4Width24::new(full, partial)
    }

    pub fn default_air_with_preprocessed(
        preprocessed: Vec<KoalaBear>,
        min_height: usize,
    ) -> Poseidon1CircuitAirKoalaBearD4Width24 {
        let (full, partial) = Self::round_constants();
        Poseidon1CircuitAirKoalaBearD4Width24::new_with_preprocessed(full, partial, preprocessed)
            .with_min_height(min_height)
    }
}

/// BabyBear Poseidon1 circuit AIR with `D=1` and 16-element state.
pub type Poseidon1CircuitAirBabyBearD1Width16 = Poseidon1CircuitAir<
    BabyBear,
    { BabyBearD1Width16::D },
    { BabyBearD1Width16::WIDTH },
    { BabyBearD1Width16::WIDTH_EXT },
    { BabyBearD1Width16::RATE_EXT },
    { BabyBearD1Width16::CAPACITY_EXT },
    { BabyBearD1Width16::SBOX_DEGREE },
    { BabyBearD1Width16::SBOX_REGISTERS },
    { BabyBearD1Width16::HALF_FULL_ROUNDS },
    { BabyBearD1Width16::PARTIAL_ROUNDS },
    { BabyBearD1Width16::D },
>;

/// [`BabyBearD1Width16`] with witness-bus keys padded to quintic width.
pub type Poseidon1CircuitAirBabyBearD1Width16WitnessBus5 = Poseidon1CircuitAir<
    BabyBear,
    { BabyBearD1Width16::D },
    { BabyBearD1Width16::WIDTH },
    { BabyBearD1Width16::WIDTH_EXT },
    { BabyBearD1Width16::RATE_EXT },
    { BabyBearD1Width16::CAPACITY_EXT },
    { BabyBearD1Width16::SBOX_DEGREE },
    { BabyBearD1Width16::SBOX_REGISTERS },
    { BabyBearD1Width16::HALF_FULL_ROUNDS },
    { BabyBearD1Width16::PARTIAL_ROUNDS },
    5,
>;

/// BabyBear Poseidon1 circuit AIR with quartic extension and 16-element state.
pub type Poseidon1CircuitAirBabyBearD4Width16 = Poseidon1CircuitAir<
    BabyBear,
    { BabyBearD4Width16::D },
    { BabyBearD4Width16::WIDTH },
    { BabyBearD4Width16::WIDTH_EXT },
    { BabyBearD4Width16::RATE_EXT },
    { BabyBearD4Width16::CAPACITY_EXT },
    { BabyBearD4Width16::SBOX_DEGREE },
    { BabyBearD4Width16::SBOX_REGISTERS },
    { BabyBearD4Width16::HALF_FULL_ROUNDS },
    { BabyBearD4Width16::PARTIAL_ROUNDS },
    { BabyBearD4Width16::D },
>;

/// BabyBear Poseidon1 circuit AIR with quartic extension and 24-element state.
pub type Poseidon1CircuitAirBabyBearD4Width24 = Poseidon1CircuitAir<
    BabyBear,
    { BabyBearD4Width24::D },
    { BabyBearD4Width24::WIDTH },
    { BabyBearD4Width24::WIDTH_EXT },
    { BabyBearD4Width24::RATE_EXT },
    { BabyBearD4Width24::CAPACITY_EXT },
    { BabyBearD4Width24::SBOX_DEGREE },
    { BabyBearD4Width24::SBOX_REGISTERS },
    { BabyBearD4Width24::HALF_FULL_ROUNDS },
    { BabyBearD4Width24::PARTIAL_ROUNDS },
    { BabyBearD4Width24::D },
>;

/// KoalaBear Poseidon1 circuit AIR with `D=1` and 16-element state.
pub type Poseidon1CircuitAirKoalaBearD1Width16 = Poseidon1CircuitAir<
    KoalaBear,
    { KoalaBearD1Width16::D },
    { KoalaBearD1Width16::WIDTH },
    { KoalaBearD1Width16::WIDTH_EXT },
    { KoalaBearD1Width16::RATE_EXT },
    { KoalaBearD1Width16::CAPACITY_EXT },
    { KoalaBearD1Width16::SBOX_DEGREE },
    { KoalaBearD1Width16::SBOX_REGISTERS },
    { KoalaBearD1Width16::HALF_FULL_ROUNDS },
    { KoalaBearD1Width16::PARTIAL_ROUNDS },
    { KoalaBearD1Width16::D },
>;

/// [`KoalaBearD1Width16`] with witness-bus keys padded to quintic width.
pub type Poseidon1CircuitAirKoalaBearD1Width16WitnessBus5 = Poseidon1CircuitAir<
    KoalaBear,
    { KoalaBearD1Width16::D },
    { KoalaBearD1Width16::WIDTH },
    { KoalaBearD1Width16::WIDTH_EXT },
    { KoalaBearD1Width16::RATE_EXT },
    { KoalaBearD1Width16::CAPACITY_EXT },
    { KoalaBearD1Width16::SBOX_DEGREE },
    { KoalaBearD1Width16::SBOX_REGISTERS },
    { KoalaBearD1Width16::HALF_FULL_ROUNDS },
    { KoalaBearD1Width16::PARTIAL_ROUNDS },
    5,
>;

/// KoalaBear Poseidon1 circuit AIR with quartic extension and 16-element state.
pub type Poseidon1CircuitAirKoalaBearD4Width16 = Poseidon1CircuitAir<
    KoalaBear,
    { KoalaBearD4Width16::D },
    { KoalaBearD4Width16::WIDTH },
    { KoalaBearD4Width16::WIDTH_EXT },
    { KoalaBearD4Width16::RATE_EXT },
    { KoalaBearD4Width16::CAPACITY_EXT },
    { KoalaBearD4Width16::SBOX_DEGREE },
    { KoalaBearD4Width16::SBOX_REGISTERS },
    { KoalaBearD4Width16::HALF_FULL_ROUNDS },
    { KoalaBearD4Width16::PARTIAL_ROUNDS },
    { KoalaBearD4Width16::D },
>;

/// KoalaBear Poseidon1 circuit AIR with quartic extension and 24-element state.
pub type Poseidon1CircuitAirKoalaBearD4Width24 = Poseidon1CircuitAir<
    KoalaBear,
    { KoalaBearD4Width24::D },
    { KoalaBearD4Width24::WIDTH },
    { KoalaBearD4Width24::WIDTH_EXT },
    { KoalaBearD4Width24::RATE_EXT },
    { KoalaBearD4Width24::CAPACITY_EXT },
    { KoalaBearD4Width24::SBOX_DEGREE },
    { KoalaBearD4Width24::SBOX_REGISTERS },
    { KoalaBearD4Width24::HALF_FULL_ROUNDS },
    { KoalaBearD4Width24::PARTIAL_ROUNDS },
    { KoalaBearD4Width24::D },
>;

/// Goldilocks, quadratic extension, 8-element state.
pub struct GoldilocksD2Width8;

impl Poseidon1Params for GoldilocksD2Width8 {
    type BaseField = Goldilocks;
    const D: usize = 2;
    const WIDTH: usize = 8;
    const WIDTH_EXT: usize = 4;
    const RATE_EXT: usize = 2;
    const CAPACITY_EXT: usize = 2;
    const SBOX_DEGREE: u64 = 7;
    const SBOX_REGISTERS: usize = 1;
    const HALF_FULL_ROUNDS: usize = 4;
    const PARTIAL_ROUNDS: usize = 22;
}

/// Goldilocks Poseidon1 circuit AIR with quadratic extension and 8-element state.
pub type Poseidon1CircuitAirGoldilocksD2Width8 = Poseidon1CircuitAir<
    Goldilocks,
    { GoldilocksD2Width8::D },
    { GoldilocksD2Width8::WIDTH },
    { GoldilocksD2Width8::WIDTH_EXT },
    { GoldilocksD2Width8::RATE_EXT },
    { GoldilocksD2Width8::CAPACITY_EXT },
    { GoldilocksD2Width8::SBOX_DEGREE },
    { GoldilocksD2Width8::SBOX_REGISTERS },
    { GoldilocksD2Width8::HALF_FULL_ROUNDS },
    { GoldilocksD2Width8::PARTIAL_ROUNDS },
    { GoldilocksD2Width8::D },
>;

/// Round constants for the Goldilocks width-8 configuration.
pub fn goldilocks_d2_width8_round_constants() -> OptimizedConstants<Goldilocks, 8> {
    optimized_constants(
        GOLDILOCKS_POSEIDON_HALF_FULL_ROUNDS,
        GOLDILOCKS_POSEIDON_PARTIAL_ROUNDS_8,
        MATRIX_CIRC_MDS_8_COL,
        GOLDILOCKS_POSEIDON1_RC_8.to_vec(),
    )
}

/// Goldilocks width-8 AIR with an empty preprocessed trace.
pub fn goldilocks_d2_width8_default_air() -> Poseidon1CircuitAirGoldilocksD2Width8 {
    let (full, partial) = goldilocks_d2_width8_round_constants();
    Poseidon1CircuitAirGoldilocksD2Width8::new(full, partial)
}

/// Goldilocks width-8 AIR with pre-populated preprocessed data.
pub fn goldilocks_d2_width8_default_air_with_preprocessed(
    preprocessed: Vec<Goldilocks>,
    min_height: usize,
) -> Poseidon1CircuitAirGoldilocksD2Width8 {
    let (full, partial) = goldilocks_d2_width8_round_constants();
    Poseidon1CircuitAirGoldilocksD2Width8::new_with_preprocessed(full, partial, preprocessed)
        .with_min_height(min_height)
}
