//! Circuit-based challenger implementation matching native DuplexChallenger exactly.
//!
//! This module provides [`CircuitChallenger`], which maintains state as coefficient-level
//! targets to ensure exact transcript compatibility with the native `DuplexChallenger`.
//!
//! # Soundness
//!
//! All Poseidon2 permutations in the challenger are CTL-verified against the Poseidon2 AIR table.
//! The circuit builder's `add_poseidon2_perm_for_challenger` / `add_poseidon2_perm_for_challenger_base`
//! (in `p3_circuit`) delegate to the standard Poseidon2 non-primitive op with full input and rate-output CTL exposure,
//! and the executor runs the real permutation so the lookup argument enforces correctness.

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use p3_circuit::ops::{Poseidon1Config, Poseidon2Config, Tip5Config};
use p3_circuit::{CircuitBuilder, CircuitBuilderError, NonPrimitiveOpId};
use p3_field::{ExtensionField, PrimeField64};

use crate::Target;
use crate::challenger_perm::ChallengerPermConfig;
use crate::traits::RecursiveChallenger;

/// Circuit challenger with coefficient-level state management.
///
/// Maintains state as WIDTH base field coefficient targets to exactly match
/// the native `DuplexChallenger<F, P, WIDTH, RATE>` behavior.
///
/// # Type Parameters
/// - `WIDTH`: Sponge state width (16 for Poseidon2)
/// - `RATE`: Sponge rate (8 for typical configuration)
/// - `C`: Challenger permutation config (e.g. [`Poseidon2Config`])
pub struct CircuitChallenger<const WIDTH: usize, const RATE: usize, C: ChallengerPermConfig> {
    /// Permutation config for the challenger (e.g. Poseidon2).
    config: C,

    /// Sponge state: WIDTH base field coefficient targets.
    /// Each target represents a base field element embedded in EF.
    state: Vec<Target>,

    /// Buffered inputs not yet absorbed into state.
    input_buffer: Vec<Target>,

    /// Buffered outputs from last duplexing.
    output_buffer: Vec<Target>,

    /// Whether the challenger has been initialized with zero state.
    initialized: bool,

    /// Whether `duplexing_base` has been called at least once since the last init/clear.
    ///
    /// The first D=1 permutation uses `new_start=true` with rate slots CTL-verified and
    /// capacity `None` (zeros enforced by the compact D=1 AIR). Later permutations use
    /// `new_start=false` with the same capacity pattern and chaining.
    duplexed_once: bool,

    /// Optional diagnostic phase name attached to challenger Tip5 operation ids.
    profile_phase: Option<&'static str>,

    /// Monotone counter used to make diagnostic phase tags unique.
    profile_phase_counter: usize,
}

impl<const WIDTH: usize, const RATE: usize, C: ChallengerPermConfig>
    CircuitChallenger<WIDTH, RATE, C>
{
    /// Create a new uninitialized circuit challenger.
    ///
    /// # Parameters
    /// - `config`: The permutation configuration (e.g. Poseidon2) for the challenger.
    ///
    /// Call `init()` to initialize the state with zeros before use.
    pub const fn new(config: C) -> Self {
        Self {
            config,
            state: Vec::new(),
            input_buffer: Vec::new(),
            output_buffer: Vec::new(),
            initialized: false,
            duplexed_once: false,
            profile_phase: None,
            profile_phase_counter: 0,
        }
    }

    /// Set an optional diagnostic phase label for future challenger Tip5 rows.
    ///
    /// The label is recorded only in circuit metadata via `CircuitBuilder::tag_op`.
    /// It does not affect the transcript state, proof constraints, or witness data.
    pub fn set_profile_phase(&mut self, phase: Option<&'static str>) {
        self.profile_phase = phase;
    }

    fn tag_profile_phase<EF>(
        &mut self,
        circuit: &mut CircuitBuilder<EF>,
        op_id: NonPrimitiveOpId,
    ) where
        EF: p3_field::Field,
    {
        if let Some(phase) = self.profile_phase {
            let tag = format!("challenger_phase/{phase}/{}", self.profile_phase_counter);
            circuit
                .tag_op(op_id, tag)
                .expect("challenger phase profile tags must be unique");
        }
        self.profile_phase_counter += 1;
    }

    /// Initialize the challenger state with zeros.
    ///
    /// This must be called before any observe/sample operations.
    pub fn init<BF, EF>(&mut self, circuit: &mut CircuitBuilder<EF>)
    where
        BF: PrimeField64,
        EF: ExtensionField<BF>,
    {
        if self.initialized {
            return;
        }
        let zero = circuit.define_const(EF::ZERO);
        self.state = vec![zero; WIDTH];
        self.initialized = true;
    }

    /// Perform duplexing: absorb inputs, permute, fill output buffer.
    ///
    /// Matches native `DuplexChallenger::duplexing()` exactly.
    fn duplexing<BF, EF>(&mut self, circuit: &mut CircuitBuilder<EF>)
    where
        BF: PrimeField64,
        EF: ExtensionField<BF>,
    {
        debug_assert!(self.initialized, "Challenger must be initialized");
        debug_assert!(self.input_buffer.len() <= RATE, "Input buffer exceeds RATE");

        let p2_config = self.config.as_poseidon2().copied();
        let p1_config = self.config.as_poseidon1().copied();
        let tip5_config = self.config.as_tip5().copied();

        // 1. Overwrite state[0..n] with inputs (NOT XOR, matches native)
        for (i, val) in self.input_buffer.drain(..).enumerate() {
            self.state[i] = val;
        }

        // Branch by NPO packing (`config.d()`), not `EF::DIMENSION`, so a quintic
        // (or other) challenge field can still use a base width-16 permutation.
        if let Some(cfg) = p2_config {
            if cfg.d() == 1 {
                self.duplexing_base(circuit, cfg);
            } else {
                self.duplexing_ext::<BF, EF>(circuit, cfg);
            }
        } else if let Some(cfg) = p1_config {
            if cfg.d() == 1 {
                self.duplexing_base_p1(circuit, cfg);
            } else {
                self.duplexing_ext_p1::<BF, EF>(circuit, cfg);
            }
        } else if let Some(cfg) = tip5_config {
            // Tip5 is base-field only (D=1) by construction.
            self.duplexing_base_tip5(circuit, cfg);
        } else {
            panic!("unsupported challenger permutation");
        }

        // 5. Fill output buffer from state[0..RATE]
        self.output_buffer.clear();
        self.output_buffer.extend_from_slice(&self.state[..RATE]);
    }

    /// Duplexing for D=1 (base field): permutation operates directly on 16 elements.
    ///
    /// The first call uses `new_start=true`: rate slots are CTL-verified; capacity slots use
    /// `None` (initial state is zero and the compact D=1 AIR asserts zero capacity on sponge
    /// chain starts). Subsequent calls use `new_start=false` with `None` for capacity so the
    /// AIR enforces continuity via the chain constraint.
    fn duplexing_base<EF>(
        &mut self,
        circuit: &mut CircuitBuilder<EF>,
        poseidon2_config: Poseidon2Config,
    ) where
        EF: p3_field::Field,
    {
        let (new_start, inputs) = if !self.duplexed_once {
            // First permutation: CTL-verify rate only; capacity is zero without witness CTL.
            let inputs: [Option<Target>; 16] =
                core::array::from_fn(|i| if i < RATE { Some(self.state[i]) } else { None });
            (true, inputs)
        } else {
            // Subsequent permutations: CTL-verify rate inputs only; capacity via chain.
            let inputs: [Option<Target>; 16] =
                core::array::from_fn(|i| if i < RATE { Some(self.state[i]) } else { None });
            (false, inputs)
        };
        self.duplexed_once = true;

        let outputs = circuit
            .add_poseidon2_perm_for_challenger_base(poseidon2_config, new_start, inputs)
            .expect("poseidon2 base permutation should succeed");

        self.state = outputs.to_vec();
    }

    fn duplexing_ext<BF, EF>(
        &mut self,
        circuit: &mut CircuitBuilder<EF>,
        poseidon2_config: Poseidon2Config,
    ) where
        BF: PrimeField64,
        EF: ExtensionField<BF>,
    {
        let num_ext_limbs = WIDTH / EF::DIMENSION;
        let mut ext_inputs = Vec::with_capacity(num_ext_limbs);
        for i in 0..num_ext_limbs {
            let start = i * EF::DIMENSION;
            let end = start + EF::DIMENSION;
            let ext = circuit
                .recompose_base_coeffs_to_ext::<BF>(&self.state[start..end])
                .expect("recomposition should succeed");
            ext_inputs.push(ext);
        }

        let ext_outputs = circuit
            .add_poseidon2_perm_for_challenger(poseidon2_config, &ext_inputs)
            .expect("poseidon2 permutation should succeed");

        for (limb, &ext_out) in ext_outputs.iter().enumerate() {
            let coeffs = circuit
                .decompose_ext_to_base_coeffs::<BF>(ext_out)
                .expect("decomposition should succeed");
            let start = limb * EF::DIMENSION;
            for (i, coeff) in coeffs.into_iter().enumerate() {
                self.state[start + i] = coeff;
            }
        }
    }

    /// Poseidon1 D=1 duplexing.
    fn duplexing_base_p1<EF>(
        &mut self,
        circuit: &mut CircuitBuilder<EF>,
        poseidon1_config: Poseidon1Config,
    ) where
        EF: p3_field::Field,
    {
        let new_start = !self.duplexed_once;
        let inputs: [Option<Target>; 16] =
            core::array::from_fn(|i| if i < RATE { Some(self.state[i]) } else { None });
        self.duplexed_once = true;

        let outputs = circuit
            .add_poseidon1_perm_for_challenger_base(poseidon1_config, new_start, inputs)
            .expect("poseidon1 base permutation should succeed");

        self.state = outputs.to_vec();
    }

    /// Tip5 D=1 duplexing (Goldilocks, width 16, rate 10). Faithful
    /// mirror of [`Self::duplexing_base_p1`], specialised to the
    /// deployed Tip5 sponge geometry: the first `RATE` (10) slots are
    /// CTL-verified each permutation (so the in-circuit transcript is
    /// bus-bound to the same values the native
    /// `DuplexChallenger<Goldilocks, Tip5Perm, 16, 10>` absorbs /
    /// squeezes); the 6 capacity slots are carried by the Tip5 NPO
    /// executor's sponge chain (`None` here), exactly as Poseidon1
    /// D=1 carries its capacity.
    fn duplexing_base_tip5<EF>(&mut self, circuit: &mut CircuitBuilder<EF>, tip5_config: Tip5Config)
    where
        EF: p3_field::Field,
    {
        let new_start = !self.duplexed_once;
        let inputs: [Option<Target>; 16] =
            core::array::from_fn(|i| if i < RATE { Some(self.state[i]) } else { None });
        self.duplexed_once = true;

        let (op_id, outputs) = circuit
            .add_tip5_perm_for_challenger_base_with_op_id(tip5_config, new_start, inputs)
            .expect("tip5 base permutation should succeed");
        self.tag_profile_phase(circuit, op_id);

        self.state = outputs.to_vec();
    }

    /// Poseidon1 D>=2 duplexing.
    fn duplexing_ext_p1<BF, EF>(
        &mut self,
        circuit: &mut CircuitBuilder<EF>,
        poseidon1_config: Poseidon1Config,
    ) where
        BF: PrimeField64,
        EF: ExtensionField<BF>,
    {
        let num_ext_limbs = WIDTH / EF::DIMENSION;
        let mut ext_inputs = Vec::with_capacity(num_ext_limbs);
        for i in 0..num_ext_limbs {
            let start = i * EF::DIMENSION;
            let end = start + EF::DIMENSION;
            let ext = circuit
                .recompose_base_coeffs_to_ext::<BF>(&self.state[start..end])
                .expect("recomposition should succeed");
            ext_inputs.push(ext);
        }

        let ext_outputs = circuit
            .add_poseidon1_perm_for_challenger(poseidon1_config, &ext_inputs)
            .expect("poseidon1 permutation should succeed");

        for (limb, &ext_out) in ext_outputs.iter().enumerate() {
            let coeffs = circuit
                .decompose_ext_to_base_coeffs::<BF>(ext_out)
                .expect("decomposition should succeed");
            let start = limb * EF::DIMENSION;
            for (i, coeff) in coeffs.into_iter().enumerate() {
                self.state[start + i] = coeff;
            }
        }
    }
}

impl<const WIDTH: usize, const RATE: usize> CircuitChallenger<WIDTH, RATE, Poseidon2Config> {
    /// Create a challenger with BabyBear D4 Width16 configuration (default).
    pub const fn new_babybear() -> Self {
        Self::new(Poseidon2Config::BABY_BEAR_D4_W16)
    }

    /// Create a challenger with BabyBear D1 Width16 configuration (base field challenges).
    pub const fn new_babybear_base() -> Self {
        Self::new(Poseidon2Config::BABY_BEAR_D1_W16)
    }

    /// Create a challenger with KoalaBear D4 Width16 configuration.
    pub const fn new_koalabear() -> Self {
        Self::new(Poseidon2Config::KOALA_BEAR_D4_W16)
    }

    /// Create a challenger with KoalaBear D1 Width16 configuration (base field challenges).
    pub const fn new_koalabear_base() -> Self {
        Self::new(Poseidon2Config::KOALA_BEAR_D1_W16)
    }
}

impl CircuitChallenger<8, 4, Poseidon2Config> {
    /// Create a challenger with Goldilocks D2 Width8 configuration.
    pub const fn new_goldilocks() -> Self {
        Self::new(Poseidon2Config::GOLDILOCKS_D2_W8)
    }
}

impl<const WIDTH: usize, const RATE: usize> CircuitChallenger<WIDTH, RATE, Poseidon1Config> {
    /// Create a Poseidon1 challenger with BabyBear D1 Width16 (base field challenges).
    pub const fn new_babybear_poseidon1_base() -> Self {
        Self::new(Poseidon1Config::BABY_BEAR_D1_W16)
    }

    /// Create a Poseidon1 challenger with KoalaBear D1 Width16 (base field challenges).
    pub const fn new_koalabear_poseidon1_base() -> Self {
        Self::new(Poseidon1Config::KOALA_BEAR_D1_W16)
    }
}

impl CircuitChallenger<8, 4, Poseidon1Config> {
    /// Create a Poseidon1 challenger with Goldilocks D2 Width8 configuration.
    pub const fn new_goldilocks_poseidon1() -> Self {
        Self::new(Poseidon1Config::GOLDILOCKS_D2_W8)
    }
}

impl CircuitChallenger<16, 10, Tip5Config> {
    /// Create a Tip5 challenger with the deployed Goldilocks D=1
    /// width-16 rate-10 configuration. This is the in-circuit
    /// counterpart of the native
    /// `DuplexChallenger<Goldilocks, Tip5Perm, 16, 10>` used by
    /// ai-pow-zk Layer-0 Fiat-Shamir.
    pub const fn new_goldilocks_tip5_base() -> Self {
        Self::new(Tip5Config::GOLDILOCKS_W16)
    }
}

impl<BF, EF, const WIDTH: usize, const RATE: usize, C: ChallengerPermConfig>
    RecursiveChallenger<BF, EF> for CircuitChallenger<WIDTH, RATE, C>
where
    BF: PrimeField64,
    EF: ExtensionField<BF>,
{
    fn observe(&mut self, circuit: &mut CircuitBuilder<EF>, value: Target) {
        // Ensure initialized
        self.init::<BF, EF>(circuit);

        // Any buffered output is now invalid (matches native behavior)
        self.output_buffer.clear();

        self.input_buffer.push(value);

        if self.input_buffer.len() == RATE {
            self.duplexing::<BF, EF>(circuit);
        }
    }

    fn sample(&mut self, circuit: &mut CircuitBuilder<EF>) -> Target {
        // Ensure initialized
        self.init::<BF, EF>(circuit);

        // If we have buffered inputs or ran out of outputs, duplex
        // (matches native DuplexChallenger::sample behavior)
        if !self.input_buffer.is_empty() || self.output_buffer.is_empty() {
            self.duplexing::<BF, EF>(circuit);
        }

        self.output_buffer
            .pop()
            .expect("Output buffer should be non-empty after duplexing")
    }

    fn observe_ext(&mut self, circuit: &mut CircuitBuilder<EF>, value: Target) {
        // Decompose extension element to D base coefficients
        let coeffs = circuit
            .decompose_ext_to_base_coeffs::<BF>(value)
            .expect("decomposition should succeed");

        // Observe each coefficient (matches native observe_algebra_element)
        for coeff in coeffs {
            self.observe(circuit, coeff);
        }
    }

    fn sample_ext(&mut self, circuit: &mut CircuitBuilder<EF>) -> Target {
        // Sample D base elements (matches native sample_algebra_element)
        let coeffs: Vec<_> = (0..EF::DIMENSION).map(|_| self.sample(circuit)).collect();

        // Recompose into extension element
        circuit
            .recompose_base_coeffs_to_ext::<BF>(&coeffs)
            .expect("recomposition should succeed")
    }

    fn sample_bits(
        &mut self,
        circuit: &mut CircuitBuilder<EF>,
        num_bits: usize,
    ) -> Result<Vec<Target>, CircuitBuilderError> {
        // A verifier parameter with `num_bits > BF::bits()` would panic on the
        // `bits[..num_bits]` slice below, so we check and abort early if needed.
        let bf_bits = BF::bits();
        if num_bits > bf_bits {
            return Err(CircuitBuilderError::BinaryDecompositionTooManyBits {
                expected: bf_bits,
                n_bits: num_bits,
            });
        }
        let base_sample = self.sample(circuit);
        // **M-S5b Path B Stage B2 (2026-05-20):** call the
        // low-bits-with-high-witness primitive instead of full
        // 64-bit decomposition. Soundness-equivalent for downstream
        // FRI/PoW consumers (only low bits used); saves
        // (bf_bits − num_bits) bool_checks + (bf_bits − num_bits − 1)
        // mul_adds per call. Design + soundness argument:
        // `crates/ai-pow-zk/docs/2026-05-20_PATH_B_B2_SAMPLE_BITS_DESIGN.md`.
        // For `num_bits == bf_bits` the new primitive falls back to
        // `decompose_to_bits` internally — byte-identical to the
        // previous behavior.
        circuit.decompose_to_low_bits_with_high_witness::<BF>(base_sample, num_bits)
    }

    fn check_pow_witness(
        &mut self,
        circuit: &mut CircuitBuilder<EF>,
        witness_bits: usize,
        witness: Target,
    ) -> Result<(), CircuitBuilderError> {
        // When no PoW is required, keep challenger state unchanged
        if witness_bits == 0 {
            return Ok(());
        }

        // Observe witness as base field element
        self.observe(circuit, witness);

        // Sample and check leading bits are zero
        let bits = self.sample_bits(circuit, witness_bits)?;
        for bit in bits {
            circuit.assert_zero(bit);
        }

        Ok(())
    }

    fn clear(&mut self, circuit: &mut CircuitBuilder<EF>) {
        let zero = circuit.define_const(EF::ZERO);
        self.state = vec![zero; WIDTH];
        self.input_buffer.clear();
        self.output_buffer.clear();
        self.initialized = true;
        self.duplexed_once = false;
    }
}
