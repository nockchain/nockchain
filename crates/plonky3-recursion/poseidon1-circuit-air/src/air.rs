use alloc::vec::Vec;
use core::borrow::Borrow;
use core::mem::MaybeUninit;

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::{Field, PrimeCharacteristicRing, PrimeField};
use p3_lookup::builder::InteractionBuilder;
use p3_matrix::dense::RowMajorMatrix;
use p3_maybe_rayon::prelude::*;
use p3_poseidon1_air::{
    FullRoundConstants, PartialRoundConstants, Poseidon1Air, Poseidon1Cols,
    generate_trace_rows_for_perm,
};
use p3_uni_stark::SubAirBuilder;
use tracing::instrument;

use crate::columns::{
    Poseidon1CircuitRow, Poseidon1PrepInputLimb, Poseidon1PrepOutputLimb, Poseidon1PreprocessedRow,
    poseidon1_d1_compact_preprocessed_header_cols, poseidon1_preprocessed_row_width_for_air,
    poseidon1_uses_compact_d1_preprocessed,
};
use crate::{Poseidon1CircuitCols, num_cols};

/// Poseidon1 circuit AIR for recursive proof composition.
///
/// Wraps the upstream permutation AIR and adds four groups of constraints:
///
/// - Sponge chaining.
/// - Merkle-path chaining.
/// - MMCS leaf-index accumulator.
/// - Cross-table lookup interactions.
///
/// # Const Generic Parameters
///
/// - **D** — extension degree.
///   Number of base-field elements per extension-field element.
///
/// - **WIDTH** — state width in base-field elements.
///   Equals the rate plus capacity, counted in base-field elements.
///
/// - **WIDTH_EXT** — state width in extension-field elements.
///   Must satisfy `WIDTH_EXT × D = WIDTH`.
///
/// - **RATE_EXT / CAPACITY_EXT** — rate and capacity in extension elements.
///   Their sum must equal WIDTH_EXT.
///
/// - **SBOX_DEGREE** — algebraic degree of the S-box polynomial.
///   For example, 7 for BabyBear, 3 for KoalaBear.
///
/// - **SBOX_REGISTERS** — number of intermediate registers.
///   Used to decompose the high-degree S-box into lower-degree steps.
///
/// - **HALF_FULL_ROUNDS** — full rounds per half.
///   Applied at the beginning and again at the end of the permutation.
///
/// - **PARTIAL_ROUNDS** — number of partial rounds.
///   The S-box is applied to only one state element in each partial round.
///
/// # Invariants
///
/// Checked at compile time during construction:
///
/// ```text
///     WIDTH_EXT × D = WIDTH
///     RATE_EXT + CAPACITY_EXT = WIDTH_EXT
/// ```
#[derive(Debug)]
pub struct Poseidon1CircuitAir<
    F: PrimeCharacteristicRing,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
> {
    /// The inner permutation AIR.
    ///
    /// Stores the round constants.
    ///
    /// Enforces the core constraint:
    /// - The output state must equal the Poseidon1 permutation of the input state.
    ///
    /// All circuit-level constraints (chaining, accumulator, cross-table
    /// lookups) are layered on top by this crate.
    p3_poseidon1:
        Poseidon1Air<F, WIDTH, SBOX_DEGREE, SBOX_REGISTERS, HALF_FULL_ROUNDS, PARTIAL_ROUNDS>,

    /// Flat preprocessed trace data in row-major order.
    ///
    /// Only needed by the prover.
    ///
    /// The verifier works with the committed digest instead, so this
    /// vector may be empty for verification-only instances.
    preprocessed: Vec<F>,

    /// Minimum trace height for FRI compatibility.
    ///
    /// Some FRI configurations require a minimum domain size.
    ///
    /// The actual height is the maximum of the natural row count (rounded
    /// up to a power of two) and this value.
    min_height: usize,
}

impl<
    F: PrimeField,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
> Clone
    for Poseidon1CircuitAir<
        F,
        D,
        WIDTH,
        WIDTH_EXT,
        RATE_EXT,
        CAPACITY_EXT,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
        WITNESS_EXT_D,
    >
{
    fn clone(&self) -> Self {
        Self {
            p3_poseidon1: self.p3_poseidon1.clone(),
            preprocessed: self.preprocessed.clone(),
            min_height: self.min_height,
        }
    }
}

impl<
    F: PrimeField,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
>
    Poseidon1CircuitAir<
        F,
        D,
        WIDTH,
        WIDTH_EXT,
        RATE_EXT,
        CAPACITY_EXT,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
        WITNESS_EXT_D,
    >
{
    /// Create a new AIR with the given round constants.
    ///
    /// The preprocessed trace starts empty.
    ///
    /// You can supply it later via the preprocessed constructor variant,
    /// or by building it from circuit rows with the extraction helper.
    ///
    /// Two compile-time assertions fire if the generic invariants are violated:
    /// - The rate plus capacity must equal the extension width,
    /// - The extension width times the degree must equal the state width.
    pub const fn new(
        full_constants: FullRoundConstants<F, WIDTH>,
        partial_constants: PartialRoundConstants<F, WIDTH>,
    ) -> Self {
        const {
            assert!(CAPACITY_EXT + RATE_EXT == WIDTH_EXT);
            assert!(WIDTH_EXT * D == WIDTH);
            assert!(WITNESS_EXT_D >= D);
        }

        Self {
            p3_poseidon1: Poseidon1Air::new(full_constants, partial_constants),
            preprocessed: Vec::new(),
            min_height: 1,
        }
    }

    /// Set the minimum trace height.
    ///
    /// The value is rounded up to a power of two.
    ///
    /// Use this when FRI requires a domain larger than the natural number
    /// of permutation rows.
    pub fn with_min_height(mut self, min_height: usize) -> Self {
        self.min_height = min_height.next_power_of_two().max(1);
        self
    }

    /// Create a new AIR with pre-populated preprocessed trace data.
    ///
    /// The preprocessed vector must be flat and row-major.
    ///
    /// Its length must be a multiple of the preprocessed width.
    ///
    /// For verification-only instances an empty vector is fine — the
    /// verifier only needs the committed digest.
    ///
    /// The same compile-time invariant checks apply as for the basic
    /// constructor.
    pub const fn new_with_preprocessed(
        full_constants: FullRoundConstants<F, WIDTH>,
        partial_constants: PartialRoundConstants<F, WIDTH>,
        preprocessed: Vec<F>,
    ) -> Self {
        const {
            assert!(CAPACITY_EXT + RATE_EXT == WIDTH_EXT);
            assert!(WIDTH_EXT * D == WIDTH);
            assert!(WITNESS_EXT_D >= D);
        }

        Self {
            p3_poseidon1: Poseidon1Air::new(full_constants, partial_constants),
            preprocessed,
            min_height: 1,
        }
    }

    /// Return the number of preprocessed columns per row.
    pub const fn preprocessed_width() -> usize {
        poseidon1_preprocessed_row_width_for_air(D, WIDTH_EXT, RATE_EXT)
    }

    /// Generate the execution trace matrix from a sequence of circuit rows.
    ///
    /// # Two-Pass Strategy
    ///
    /// ```text
    ///     Pass 1 (sequential)
    ///         Write the direction bit, the MMCS accumulator, and the
    ///         Poseidon1 input state into uninitialized trace memory.
    ///
    ///     Pass 2 (parallel)
    ///         Read the inputs back.
    ///         Compute the full Poseidon1 permutation for every row.
    /// ```
    ///
    /// Pass 1 is sequential because the MMCS accumulator depends on the previous row.
    ///
    /// Pass 2 is parallel because each permutation is independent.
    ///
    /// # Panics
    ///
    /// - If the number of rows is not a power of two.
    /// - If any row's input state has the wrong number of elements.
    #[instrument(skip_all, name = "Poseidon1CircuitAir::build_trace")]
    pub fn generate_trace_rows(
        &self,
        sponge_ops: &[Poseidon1CircuitRow<F>],
        full_constants: &FullRoundConstants<F, WIDTH>,
        partial_constants: &PartialRoundConstants<F, WIDTH>,
        extra_capacity_bits: usize,
    ) -> RowMajorMatrix<F> {
        let n = sponge_ops.len();
        assert!(
            n.is_power_of_two(),
            "Callers expected to pad inputs to a power of two"
        );

        // Each row has two segments:
        //
        //     [ --- permutation columns --- | direction bit | accumulator ]
        //
        // The permutation segment holds the full Poseidon1 state.
        //
        // That includes the 16 input elements, all intermediate values
        // produced during the rounds, and the 16 output elements.
        //
        // After the permutation block come two circuit-specific columns.
        //
        // The direction bit says whether the current node is a left or
        // right child in a Merkle tree (only meaningful in Merkle mode).
        //
        // The accumulator reconstructs the Merkle leaf index one bit at
        // a time as the circuit walks up the authentication path.

        let p1_ncols = p3_poseidon1_air::num_cols::<
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >();
        let ncols = self.width();
        let circuit_ncols = ncols - p1_ncols;

        // Allocate the final trace as uninitialized memory.
        //
        // We use uninitialized memory because both passes will write
        // every element before it is read.
        //
        // The extra capacity bits enlarge only the permutation segment.
        //
        // The circuit columns are always two wide (direction bit and accumulator).
        let mut trace_vec: Vec<F> =
            Vec::with_capacity(n * ((p1_ncols << extra_capacity_bits) + circuit_ncols));
        let trace_slice = trace_vec.spare_capacity_mut();

        // Pass 1: Sequential
        //
        // This pass must run row by row because each row's accumulator
        // depends on the previous row's value.
        //
        // For each row we write three things into the uninitialized memory:
        //
        //   1. The Poseidon1 input state — the 16 field elements that will
        //      be fed into the permutation. These come directly from the
        //      operation struct provided by the caller.
        //
        //   2. The direction bit — zero or one, indicating left or right
        //      child in a Merkle tree.
        //
        //   3. The MMCS accumulator — a running value that reconstructs
        //      the Merkle leaf index from the direction bits.
        //
        // The accumulator follows a simple recurrence:
        //
        //     next_sum = current_sum × 2 + next_bit
        //
        // This is just binary-to-integer conversion built up one bit at
        // a time. For example, leaf index 5 (binary 101) is reconstructed
        // as: 1 → 1×2+0=2 → 2×2+1=5.
        //
        // On chain boundaries or non-Merkle rows the accumulator resets
        // to whatever value the operation struct carries.

        // Tracks the accumulator value from the previous row.
        //
        // Starts at zero. Updated on each iteration.
        let mut prev_mmcs_index_sum = F::ZERO;

        // View the flat allocation as individual rows, each with the
        // right number of columns.
        let rows = trace_slice[..n * ncols].chunks_exact_mut(ncols);

        for (row_index, (op, row)) in sponge_ops.iter().zip(rows).enumerate() {
            let Poseidon1CircuitRow {
                new_start,
                merkle_path,
                mmcs_bit,
                mmcs_index_sum,
                input_values,
                ..
            } = op;

            assert_eq!(
                input_values.len(),
                WIDTH,
                "Trace row input_values must have length WIDTH"
            );

            // Update the accumulator.
            //
            // If this is a Merkle row that continues a chain (not the
            // first row, not a chain boundary), apply the recurrence:
            //
            //     new_value = old_value × 2 + direction_bit
            //
            // Otherwise reset the accumulator. This happens on:
            //   - The very first row (no previous value exists).
            //   - Chain boundaries (a new Merkle proof starts).
            //   - Non-Merkle rows (sponge mode, no index to track).
            if row_index > 0 && *merkle_path && !*new_start {
                prev_mmcs_index_sum = prev_mmcs_index_sum.double() + F::from_bool(*mmcs_bit);
            } else {
                prev_mmcs_index_sum = *mmcs_index_sum;
            }

            // Write the 16 input field elements into the first 16 slots
            // of the row. These will be read back in pass 2 when the
            // permutation is computed.
            for (i, &val) in input_values.iter().enumerate() {
                row[i].write(val);
            }

            // Write the two circuit columns at the end of the row.
            //
            // First circuit column: the direction bit (0 = left, 1 = right).
            //
            // Second circuit column: the running accumulator value.
            let (_p2_part, circuit_part) = row.split_at_mut(p1_ncols);
            circuit_part[0].write(F::from_bool(*mmcs_bit));
            circuit_part[1].write(prev_mmcs_index_sum);
        }

        // Pass 2: Parallel
        //
        // Each row's permutation is independent of every other row.
        //
        // That means we can compute all of them in parallel.
        //
        // For each row:
        //
        //   1. Read back the 16 input elements written during pass 1.
        //
        //   2. Run the full Poseidon1 permutation: external rounds (S-box
        //      applied to all 16 elements), then partial rounds (S-box
        //      applied to just one element), then external rounds again.
        //
        //   3. Write all intermediate round states and the 16 output
        //      elements into the remaining permutation columns.

        // First column of the circulant MDS matrix, derived from the dense form.
        let circ_col: [F; WIDTH] = core::array::from_fn(|i| full_constants.dense_mds[i][0]);

        trace_slice[..n * ncols]
            .par_chunks_exact_mut(ncols)
            .for_each(|row| {
                // Split the row into permutation columns and circuit columns.
                //
                // We only need the permutation part here. The circuit
                // columns were already finalized in pass 1.
                let (p1_part, _circuit_part) = row.split_at_mut(p1_ncols);

                // Read back the 16 input elements that pass 1 wrote.
                //
                // SAFETY: Pass 1 initialized exactly these positions.
                let input: [F; WIDTH] =
                    core::array::from_fn(|i| unsafe { p1_part[i].assume_init() });

                // Reinterpret the flat slice as the typed permutation column struct.
                //
                // This is a zero-copy cast. The struct is `#[repr(C)]`
                // and the slice has exactly the right number of elements.
                let (prefix, p1_cols, suffix) = unsafe {
                    p1_part.align_to_mut::<Poseidon1Cols<
                        MaybeUninit<F>,
                        WIDTH,
                        SBOX_DEGREE,
                        SBOX_REGISTERS,
                        HALF_FULL_ROUNDS,
                        PARTIAL_ROUNDS,
                    >>()
                };

                // Verify the cast produced exactly one struct with no
                // leftover bytes on either side.
                debug_assert!(prefix.is_empty(), "Alignment mismatch");
                debug_assert!(suffix.is_empty(), "Alignment mismatch");
                debug_assert_eq!(p1_cols.len(), 1);

                // Run the Poseidon1 permutation on the input.
                //
                // This fills in every column of the permutation struct:
                // - the beginning full rounds,
                // - the partial rounds,
                // - the ending full rounds,
                // - the final output state.
                generate_trace_rows_for_perm::<
                    F,
                    WIDTH,
                    SBOX_DEGREE,
                    SBOX_REGISTERS,
                    HALF_FULL_ROUNDS,
                    PARTIAL_ROUNDS,
                >(
                    &mut p1_cols[0],
                    input,
                    full_constants,
                    partial_constants,
                    &circ_col,
                );
            });

        // SAFETY: At this point every element has been initialized.
        //
        // Pass 1 wrote the input state, direction bit, and accumulator.
        //
        // Pass 2 wrote all intermediate round states and the output.
        //
        // We can now safely tell the allocator that these bytes are live.
        unsafe {
            trace_vec.set_len(n * ncols);
        }

        RowMajorMatrix::new(trace_vec, ncols)
    }
}

impl<
    F: PrimeField + Sync,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
> BaseAir<F>
    for Poseidon1CircuitAir<
        F,
        D,
        WIDTH,
        WIDTH_EXT,
        RATE_EXT,
        CAPACITY_EXT,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
        WITNESS_EXT_D,
    >
{
    /// Total number of value columns per row.
    ///
    /// Includes all Poseidon1 permutation columns (input, round
    /// intermediates, output).
    ///
    /// Also includes the two circuit-specific columns:
    /// - The direction bit,
    /// - The MMCS accumulator.
    fn width(&self) -> usize {
        num_cols::<
            Poseidon1Cols<u8, WIDTH, SBOX_DEGREE, SBOX_REGISTERS, HALF_FULL_ROUNDS, PARTIAL_ROUNDS>,
        >()
    }

    fn preprocessed_width(&self) -> usize {
        Self::preprocessed_width()
    }

    /// Build the preprocessed trace matrix.
    ///
    /// Pads to a power-of-two height.
    ///
    /// # Padding Strategy
    ///
    /// ```text
    ///     Row 0 .. n-1        actual preprocessed data
    ///     Row n (first pad)   chain boundary flag = 1, rest zero
    ///     Row n+1 .. end      all zeros
    /// ```
    ///
    /// The first padding row marks a chain boundary.
    ///
    /// This prevents chaining constraints from firing across the
    /// real-to-padding boundary.
    ///
    /// All subsequent padding rows are fully zero. Every selector is
    /// inactive, so every constraint is trivially satisfied.
    ///
    /// The chain boundary flag is the second-to-last field in each
    /// preprocessed row.
    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<F>> {
        let width = Self::preprocessed_width();
        let len = self.preprocessed.len();

        debug_assert!(
            len.is_multiple_of(width),
            "Preprocessed trace length {len} is not a multiple of preprocessed width {width}."
        );

        let natural_rows = len / width;

        // The minimum height is already rounded to a power of two in
        // the builder method, so we can use it directly here.
        let padded_rows = natural_rows.next_power_of_two().max(self.min_height);

        // Clone the existing preprocessed data.
        let mut data = self.preprocessed.clone();

        // Pad with zeros up to the required power-of-two height.
        //
        // All-zero padding rows have every selector inactive, so
        // every constraint is trivially satisfied on those rows.
        data.resize(padded_rows * width, F::ZERO);

        // Mark the first padding row as a chain boundary.
        //
        // Without this, the chaining constraint would try to connect
        // the last real row to the first padding row. Setting the
        // chain-start flag to one disables that connection.
        //
        // The flag is the second-to-last field in each row.
        if padded_rows > natural_rows {
            data[len + width - 2] = F::ONE;
        }

        Some(RowMajorMatrix::new(data, width))
    }
}

/// Build the preprocessed trace from a sequence of circuit operations.
///
/// Each operation becomes one preprocessed row. The results are flattened
/// into a single vector in row-major order.
///
/// # Index Scaling
///
/// All witness indices are multiplied by the extension degree.
///
/// This way the CTL keys directly index into the flattened witness table.
/// For example, with extension degree 4 and logical index 5, the stored
/// value is 20.
///
/// # Grouped D=1 CTL
///
/// When `poseidon_extension_degree == 1` (base-field Poseidon1 slots) and `witness_bus_value_slots`
/// divides `IL` / `OL`, each pack uses **all-or-nothing** `in_ctl` / `out_ctl`, and witness indices
/// within a pack must be consecutive. `witness_bus_value_slots` is the AIR's `WITNESS_EXT_D`
/// (e.g. 5 for WitnessBus5). `d` scales stored indices and must match the circuit's extension degree.
///
/// # Compact D=1 width-16 / rate-8 layout
///
/// When `IL == 16`, `OL == 8`, and `poseidon_extension_degree == 1`, rows use a compact layout:
/// `OL` per-rate-limb `in_ctl`, `cap_in_ctl` (always zero; column retained for layout), `cap_chain_enable`,
/// `OL` sponge chain helpers `(1 − new_start)(1 − merkle_path)(1 − in_ctl_i)`, `OL` Merkle chain helpers
/// `(1 − new_start)(merkle_path)(1 − in_ctl_i)`, then `IL` input indices, `OL` output indices,
/// `OL` per-limb `out_ctl`, then four tail flags. Rate `in_ctl` may vary (e.g. partial-chunk overwrite).
/// Capacity inputs are not CTL-verified; sponge `new_start` rows enforce zero capacity in `eval`.
///
/// # Precomputed Selectors
///
/// Several boolean products are precomputed here to keep the constraint
/// degree at 3 during evaluation.
///
/// - The **sponge chain selector** is true when the row is not a chain
///   boundary, not a Merkle row, and the limb is not looked up via CTL.
///
/// - The **Merkle chain selector** is true when the row is not a chain
///   boundary, is a Merkle row, and the limb is not looked up via CTL.
///
/// - The **MMCS Merkle flag** is true when MMCS CTL is enabled and the
///   row is a Merkle row.
///
/// Computing these products at setup time avoids degree-4 expressions
/// in the constraint polynomial.
pub fn extract_preprocessed_from_operations<
    const IL: usize,
    const OL: usize,
    F: Field,
    OF: Field,
>(
    operations: &[Poseidon1CircuitRow<OF>],
    d: u32,
    poseidon_extension_degree: usize,
) -> Vec<F> {
    let row_width = poseidon1_preprocessed_row_width_for_air(poseidon_extension_degree, IL, OL);
    let mut preprocessed = Vec::with_capacity(operations.len() * row_width);

    let compact_d1 = poseidon1_uses_compact_d1_preprocessed(poseidon_extension_degree, IL, OL);

    for operation in operations {
        let Poseidon1CircuitRow {
            in_ctl,
            input_indices,
            out_ctl,
            output_indices,
            mmcs_index_sum_idx,
            mmcs_ctl_enabled,
            new_start,
            merkle_path,
            ..
        } = operation;

        debug_assert_eq!(in_ctl.len(), IL);
        debug_assert_eq!(input_indices.len(), IL);
        debug_assert_eq!(out_ctl.len(), OL);
        debug_assert_eq!(output_indices.len(), OL);

        if compact_d1 {
            for ctl in in_ctl.iter().take(OL) {
                preprocessed.push(F::from_bool(*ctl));
            }
            if !*merkle_path {
                for ctl in in_ctl.iter().take(IL).skip(OL) {
                    debug_assert!(
                        !ctl,
                        "compact D=1 Poseidon1: capacity must not be witness-fed on sponge rows"
                    );
                }
            }
            let cap_chain_enable = !*new_start;
            preprocessed.push(F::ZERO);
            preprocessed.push(F::from_bool(cap_chain_enable));
            for ctl in in_ctl.iter().take(OL) {
                preprocessed.push(F::from_bool(!*new_start && !*merkle_path && !ctl));
            }
            for ctl in in_ctl.iter().take(OL) {
                preprocessed.push(F::from_bool(!*new_start && *merkle_path && !ctl));
            }
            for input_index in input_indices.iter().take(IL) {
                preprocessed.push(F::from_u32(input_index * d));
            }
            for output_index in output_indices.iter().take(OL) {
                preprocessed.push(F::from_u32(output_index * d));
            }
            for ctl in out_ctl.iter().take(OL) {
                preprocessed.push(F::from_bool(*ctl));
            }
            preprocessed.push(F::from_u64(*mmcs_index_sum_idx as u64 * d as u64));
            preprocessed.push(F::from_bool(*mmcs_ctl_enabled && *merkle_path));
            preprocessed.push(F::from_bool(*new_start));
            preprocessed.push(F::from_bool(*merkle_path));
        } else {
            let row = Poseidon1PreprocessedRow::<IL, OL, F> {
                input_limbs: core::array::from_fn(|i| {
                    let ctl = in_ctl[i];
                    Poseidon1PrepInputLimb {
                        idx: F::from_u32(input_indices[i] * d),
                        in_ctl: F::from_bool(ctl),
                        normal_chain_sel: F::from_bool(!*new_start && !*merkle_path && !ctl),
                        merkle_chain_sel: F::from_bool(!*new_start && *merkle_path && !ctl),
                    }
                }),

                output_limbs: core::array::from_fn(|i| Poseidon1PrepOutputLimb {
                    idx: F::from_u32(output_indices[i] * d),
                    out_ctl: F::from_bool(out_ctl[i]),
                }),

                mmcs_index_sum_ctl_idx: F::from_u64(*mmcs_index_sum_idx as u64 * d as u64),

                mmcs_merkle_flag: F::from_bool(*mmcs_ctl_enabled && *merkle_path),

                new_start: F::from_bool(*new_start),

                merkle_path: F::from_bool(*merkle_path),
            };
            row.write_into(&mut preprocessed);
        }
    }

    preprocessed
}

/// Evaluate all circuit-level constraints for one pair of adjacent rows.
///
/// This is the core constraint function.
///
/// It enforces five groups of constraints on the builder.
///
/// 1. **Boolean** — the direction bit must be 0 or 1.
///
/// 2. **Sponge chaining** — when the sponge chain selector is active,
///    the next row's input equals the current row's output for that limb.
///    Checked element by element across the extension degree.
///
/// 3. **Merkle-path chaining** — when the Merkle chain selector is
///    active, the chaining direction depends on the direction bit:
///
///    ```text
///        bit = 0 (left child)    next input limbs 0-1 ← current output 0-1
///        bit = 1 (right child)   next input limbs 2-3 ← current output 0-1
///    ```
///
///    Only the first two limbs carry the Merkle selector.
///    Limbs 2-3 reuse the same selectors gated on the opposite direction.
///
/// 4. **MMCS accumulator** — on Merkle rows that are not chain boundaries,
///    the next accumulator equals twice the current plus the next bit.
///
/// 5. **Poseidon1 permutation** — delegated to the inner permutation AIR
///    via a sub-builder restricted to the permutation columns.
///    Unconditional on every row.
///
/// Chain selectors and the Merkle flag are preprocessed columns. They are
/// known to the verifier and do not need boolean assertions.
///
/// The direction bit is a prover-supplied value column. It must be
/// explicitly constrained to be boolean.
#[unroll::unroll_for_loops]
pub(crate) fn eval<
    AB: AirBuilder,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
>(
    air: &Poseidon1CircuitAir<
        AB::F,
        D,
        WIDTH,
        WIDTH_EXT,
        RATE_EXT,
        CAPACITY_EXT,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
        WITNESS_EXT_D,
    >,
    builder: &mut AB,
    local: &Poseidon1CircuitCols<
        AB::Var,
        Poseidon1Cols<
            AB::Var,
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >,
    >,
    next: &Poseidon1CircuitCols<
        AB::Var,
        Poseidon1Cols<
            AB::Var,
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >,
    >,
    next_preprocessed: &[AB::Var],
) {
    // Extract the three things we'll reference repeatedly:
    //
    //   - The direction bit from the next row (left vs right child).
    //
    //   - The current row's output state — the 16 field elements
    //     produced by the Poseidon1 permutation on this row. Located
    //     in the last full-round's post-state.
    //
    //   - The next row's input state — the 16 field elements that
    //     will be fed into the next permutation. Chaining constraints
    //     tie these to the current output.
    let next_bit = next.mmcs_bit;
    let local_out = &local.poseidon1.ending_full_rounds[HALF_FULL_ROUNDS - 1].post;
    let next_in = &next.poseidon1.inputs;

    // Boolean constraint
    //
    // The direction bit is a value column filled by the prover at
    // runtime. A cheating prover could put any field element here.
    //
    // We constrain it to be 0 or 1 by asserting:
    //
    //     bit × (1 − bit) = 0
    //
    // Preprocessed flags don't need this check — they were committed
    // at setup time and cannot be changed.

    builder.assert_bool(local.mmcs_bit);

    if poseidon1_uses_compact_d1_preprocessed(D, WIDTH_EXT, RATE_EXT) {
        let hdr = poseidon1_d1_compact_preprocessed_header_cols(RATE_EXT);
        debug_assert_eq!(
            next_preprocessed.len(),
            hdr + WIDTH_EXT + RATE_EXT + RATE_EXT + 4
        );
        let s = next_preprocessed;
        let cap_chain_enable = s[RATE_EXT + 1];
        let rate_sponge_base = RATE_EXT + 2;
        let rate_merkle_base = rate_sponge_base + RATE_EXT;
        let tail = hdr + WIDTH_EXT + RATE_EXT + RATE_EXT;
        let next_new_start = s[tail + 2];
        let next_merkle_path = s[tail + 3];
        let not_next_new_start = AB::Expr::ONE - next_new_start.into();

        // Sponge chaining (compact): rate uses precomputed `(1−ns)(1−merkle)(1−ctl_i)`; capacity shares `cap_chain_enable`.
        for limb in 0..RATE_EXT {
            let chain_en = s[rate_sponge_base + limb];
            for d in 0..D {
                builder
                    .when_transition()
                    .when(chain_en)
                    .assert_zero(next_in[limb * D + d] - local_out[limb * D + d]);
            }
        }
        let not_merkle = AB::Expr::ONE - next_merkle_path.into();
        for limb in RATE_EXT..WIDTH_EXT {
            let chain_en = cap_chain_enable * not_merkle.clone();
            for d in 0..D {
                builder
                    .when_transition()
                    .when(chain_en.clone())
                    .assert_zero(next_in[limb * D + d] - local_out[limb * D + d]);
            }
        }

        // Merkle-path chaining (compact): precomputed `(1−ns)(merkle)(1−ctl_i)` × direction bit (degree 3).
        let is_left = AB::Expr::ONE - next_bit.into();
        for i in 0..RATE_EXT {
            let merkle_chain_i = s[rate_merkle_base + i];
            let gate_left_i = merkle_chain_i * is_left.clone();
            let gate_right_i = merkle_chain_i * next_bit;

            for d in 0..D {
                builder
                    .when_transition()
                    .when(gate_left_i.clone())
                    .assert_zero(next_in[i * D + d] - local_out[i * D + d]);

                builder
                    .when_transition()
                    .when(gate_right_i.clone())
                    .assert_zero(next_in[(RATE_EXT + i) * D + d] - local_out[i * D + d]);
            }
        }

        // Sponge chain starts (next row new_start, not Merkle): capacity is never witness-fed;
        // replacing the former zero-constant CTL sends for slots RATE_EXT..WIDTH_EXT.
        for slot in RATE_EXT..WIDTH_EXT {
            for d in 0..D {
                builder
                    .when_transition()
                    .when(next_new_start)
                    .when(not_merkle.clone())
                    .assert_zero(next_in[slot * D + d]);
            }
        }

        builder
            .when_transition()
            .when(not_next_new_start)
            .when(next_merkle_path)
            .assert_zero(
                next.mmcs_index_sum - (local.mmcs_index_sum * AB::Expr::TWO + next.mmcs_bit.into()),
            );
    } else {
        let next_prep: &Poseidon1PreprocessedRow<WIDTH_EXT, RATE_EXT, AB::Var> =
            next_preprocessed.borrow();

        // Sponge chaining
        //
        // In sponge mode the output of one permutation feeds directly
        // into the input of the next permutation.
        //
        // For example, if row 0 outputs [a, b, c, ...] then row 1 must
        // have input [a, b, c, ...].
        //
        // We check this element by element. Each limb has D base-field
        // elements (the extension degree), so we loop over all of them.
        //
        // The sponge chain selector gates the constraint. It is only
        // active on continuation rows in sponge mode. On chain boundaries,
        // Merkle rows, or CTL-loaded limbs, the selector is zero and the
        // constraint is trivially satisfied.

        for limb in 0..WIDTH_EXT {
            for d in 0..D {
                let gate = next_prep.input_limbs[limb].normal_chain_sel;
                builder
                    .when_transition()
                    .when(gate)
                    .assert_zero(next_in[limb * D + d] - local_out[limb * D + d]);
            }
        }

        // Merkle-path chaining: first `RATE_EXT` logical limbs of the output
        // form our digest; the sibling occupies the next `RATE_EXT` limbs of
        // the next row's input. The direction bit selects left vs right placement.
        let is_left = AB::Expr::ONE - next_bit.into();

        for i in 0..RATE_EXT {
            let gate_left_i = next_prep.input_limbs[i].merkle_chain_sel * is_left.clone();
            for d in 0..D {
                builder
                    .when_transition()
                    .when(gate_left_i.clone())
                    .assert_zero(next_in[i * D + d] - local_out[i * D + d]);
            }
        }
        for i in 0..RATE_EXT {
            let gate_right_i = next_prep.input_limbs[i].merkle_chain_sel * next_bit;
            for d in 0..D {
                builder
                    .when_transition()
                    .when(gate_right_i.clone())
                    .assert_zero(next_in[(RATE_EXT + i) * D + d] - local_out[i * D + d]);
            }
        }

        // MMCS accumulator
        //
        // As the circuit walks up a Merkle tree, it sees one direction bit
        // per level. These bits form the binary representation of the leaf
        // index being authenticated.
        //
        // The accumulator reconstructs that index with the recurrence:
        //
        //     next_sum = current_sum × 2 + next_bit
        //
        // For example, authenticating leaf 5 (binary 101):
        //
        //     row 0:  acc = 1            (first bit)
        //     row 1:  acc = 1×2 + 0 = 2  (second bit)
        //     row 2:  acc = 2×2 + 1 = 5  (third bit → final index)
        //
        // The constraint only fires when the next row is a Merkle row
        // that is not a chain boundary. On chain boundaries the
        // accumulator resets, and on non-Merkle rows it is unused.

        // Compute (1 − next_new_start). This is 1 when the next row
        // continues a chain, 0 when it starts a new one.
        let not_next_new_start = AB::Expr::ONE - next_prep.new_start.into();

        // The constraint:
        //
        //     next_accumulator = current_accumulator × 2 + next_direction_bit
        //
        // Rearranged for assert_zero:
        //
        //     next_acc − (current_acc × 2 + next_bit) = 0
        //
        // Gated on: not a chain boundary AND is a Merkle row.
        builder
            .when_transition()
            .when(not_next_new_start)
            .when(next_prep.merkle_path)
            .assert_zero(
                next.mmcs_index_sum - (local.mmcs_index_sum * AB::Expr::TWO + next.mmcs_bit.into()),
            );
    }

    // Poseidon1 permutation
    //
    // Every row must satisfy the Poseidon1 permutation constraint:
    // the output state must be the correct hash of the input state.
    //
    // This is unconditional — it applies regardless of whether the
    // row is sponge, Merkle, padding, or anything else.
    //
    // The permutation constraint is handled by a separate AIR. We
    // give it a sub-builder that only sees the permutation columns
    // (not the two circuit columns at the end of the row).

    let p3_poseidon1_num_cols = p3_poseidon1_air::num_cols::<
        WIDTH,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
    >();
    let mut sub_builder = SubAirBuilder::<
        AB,
        Poseidon1Air<AB::F, WIDTH, SBOX_DEGREE, SBOX_REGISTERS, HALF_FULL_ROUNDS, PARTIAL_ROUNDS>,
        AB::Var,
    >::new(builder, 0..p3_poseidon1_num_cols);

    air.p3_poseidon1.eval(&mut sub_builder);
}

/// Unchecked constraint evaluation with a concrete builder type.
///
/// Exists to support the batch prover.
///
/// In the batch prover the constraint evaluation dispatch erases the
/// concrete builder type behind a trait object. The caller provides two
/// builder types: the erased one and a concrete one that carries the
/// required field bounds.
///
/// At runtime both must be the same type with the same field.
///
/// All five arguments are transmuted from the erased types to the
/// concrete types before calling the main evaluation function.
///
/// This is sound only if the types are truly identical at runtime.
///
/// # Safety
///
/// The caller must guarantee:
///
/// - The AIR's field type, the erased builder's field type, and the
///   concrete builder's field type are all the same.
///
/// - The erased and concrete builder types have identical memory layout.
#[allow(clippy::missing_transmute_annotations)]
pub unsafe fn eval_unchecked_with_concrete<
    F: PrimeField,
    AB: AirBuilder,
    ABConcrete: AirBuilder,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
>(
    air: &Poseidon1CircuitAir<
        F,
        D,
        WIDTH,
        WIDTH_EXT,
        RATE_EXT,
        CAPACITY_EXT,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
        WITNESS_EXT_D,
    >,
    builder: &mut AB,
    local: &Poseidon1CircuitCols<
        AB::Var,
        Poseidon1Cols<
            AB::Var,
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >,
    >,
    next: &Poseidon1CircuitCols<
        AB::Var,
        Poseidon1Cols<
            AB::Var,
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >,
    >,
    next_preprocessed: &[AB::Var],
) where
    ABConcrete::F: PrimeField,
{
    // SAFETY: The caller guarantees all erased types are identical to
    // their concrete counterparts at runtime.
    //
    // Each transmute reinterprets the same memory under the concrete
    // type so the main evaluation function can be called with proper
    // trait bounds.
    unsafe {
        let builder_c = core::mem::transmute(builder);
        let local_c = core::mem::transmute(local);
        let next_c = core::mem::transmute(next);
        let next_preprocessed_c = core::mem::transmute(next_preprocessed);
        let air_c = core::mem::transmute(air);
        eval::<
            ABConcrete,
            D,
            WIDTH,
            WIDTH_EXT,
            RATE_EXT,
            CAPACITY_EXT,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
            WITNESS_EXT_D,
        >(air_c, builder_c, local_c, next_c, next_preprocessed_c);
    }
}

/// Unchecked constraint evaluation with a field type mismatch.
///
/// The AIR's field type may differ from the builder's field type at
/// compile time. At runtime they must be the same.
///
/// This function transmutes the AIR reference so its field matches the
/// builder, then calls the main evaluation function.
///
/// Simpler than the concrete-builder variant above: only the AIR needs
/// to be transmuted. The builder already has the correct associated types.
///
/// # Safety
///
/// The caller must guarantee that the AIR's field type and the builder's
/// field type are the same at runtime.
///
/// Violating this leads to undefined behavior.
#[allow(clippy::missing_transmute_annotations)]
pub unsafe fn eval_unchecked<
    F: PrimeField,
    AB: AirBuilder,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
>(
    air: &Poseidon1CircuitAir<
        F,
        D,
        WIDTH,
        WIDTH_EXT,
        RATE_EXT,
        CAPACITY_EXT,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
        WITNESS_EXT_D,
    >,
    builder: &mut AB,
    local: &Poseidon1CircuitCols<
        AB::Var,
        Poseidon1Cols<
            AB::Var,
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >,
    >,
    next: &Poseidon1CircuitCols<
        AB::Var,
        Poseidon1Cols<
            AB::Var,
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >,
    >,
    next_preprocessed: &[AB::Var],
) where
    AB::F: PrimeField,
{
    // SAFETY: The caller guarantees the two field types are identical at
    // runtime, so the AIR struct has the same memory layout under both.
    unsafe {
        let air_transmuted = core::mem::transmute(air);

        eval::<
            AB,
            D,
            WIDTH,
            WIDTH_EXT,
            RATE_EXT,
            CAPACITY_EXT,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
            WITNESS_EXT_D,
        >(air_transmuted, builder, local, next, next_preprocessed);
    }
}

impl<
    AB: AirBuilder + InteractionBuilder,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
> Air<AB>
    for Poseidon1CircuitAir<
        AB::F,
        D,
        WIDTH,
        WIDTH_EXT,
        RATE_EXT,
        CAPACITY_EXT,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
        WITNESS_EXT_D,
    >
where
    AB::F: PrimeField,
{
    #[inline]
    fn eval(&self, builder: &mut AB) {
        // Get the main trace window.
        //
        // It provides the current row and the next row as flat slices.
        let main = builder.main();

        // Reinterpret the flat slices as typed column structs.
        //
        // This is a zero-copy cast enabled by the `#[repr(C)]` layout
        // and the `Borrow` implementations in the columns module.
        let local = main.current_slice().borrow();
        let next = main.next_slice().borrow();

        // Get the preprocessed trace window and extract both rows.
        //
        // The clone here copies a small window struct (two slice
        // pointers), not the full preprocessed matrix.
        let preprocessed = builder.preprocessed().clone();
        let local_preprocessed = preprocessed.current_slice();
        let next_preprocessed = preprocessed.next_slice();

        // Record cross-table interactions on the WitnessChecks bus before the
        // borrow checker locks `builder` to the constraint methods.
        eval_interactions::<
            AB,
            D,
            WIDTH,
            WIDTH_EXT,
            RATE_EXT,
            CAPACITY_EXT,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
            WITNESS_EXT_D,
        >(builder, local, local_preprocessed, next_preprocessed);

        // Delegate to the core constraint function, which enforces all
        // five constraint groups.
        eval::<
            _,
            D,
            WIDTH,
            WIDTH_EXT,
            RATE_EXT,
            CAPACITY_EXT,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
            WITNESS_EXT_D,
        >(self, builder, local, next, next_preprocessed);
    }
}

/// Push the WitnessChecks CTL interactions for one row of the Poseidon1 circuit AIR: input
/// limb sends, output limb receives, and the MMCS-accumulator send at the end of each
/// Merkle chain.
fn eval_interactions<
    AB: AirBuilder + InteractionBuilder,
    const D: usize,
    const WIDTH: usize,
    const WIDTH_EXT: usize,
    const RATE_EXT: usize,
    const CAPACITY_EXT: usize,
    const SBOX_DEGREE: u64,
    const SBOX_REGISTERS: usize,
    const HALF_FULL_ROUNDS: usize,
    const PARTIAL_ROUNDS: usize,
    const WITNESS_EXT_D: usize,
>(
    builder: &mut AB,
    local: &Poseidon1CircuitCols<
        AB::Var,
        Poseidon1Cols<
            AB::Var,
            WIDTH,
            SBOX_DEGREE,
            SBOX_REGISTERS,
            HALF_FULL_ROUNDS,
            PARTIAL_ROUNDS,
        >,
    >,
    local_preprocessed: &[AB::Var],
    next_preprocessed: &[AB::Var],
) where
    AB::F: PrimeField,
{
    let compact_d1 = poseidon1_uses_compact_d1_preprocessed(D, WIDTH_EXT, RATE_EXT);

    if compact_d1 {
        let hdr = poseidon1_d1_compact_preprocessed_header_cols(RATE_EXT);
        let tail = hdr + WIDTH_EXT + RATE_EXT + RATE_EXT;
        debug_assert_eq!(local_preprocessed.len(), tail + 4);
        debug_assert_eq!(next_preprocessed.len(), tail + 4);

        let merkle_path_p: AB::Expr = local_preprocessed[tail + 3].into();
        let not_merkle = AB::Expr::ONE - merkle_path_p;
        let idx_base = hdr;

        // Input limb sends (rate only; capacity is zero-asserted in eval)
        for limb_idx in 0..RATE_EXT {
            let idx: AB::Expr = local_preprocessed[idx_base + limb_idx].into();
            let in_ctl: AB::Expr = local_preprocessed[limb_idx].into();
            let mut input_idx_limb: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
            input_idx_limb.push(idx);
            for d in 0..D {
                input_idx_limb.push(local.poseidon1.inputs[limb_idx * D + d].into());
            }
            for _ in 0..(WITNESS_EXT_D - D) {
                input_idx_limb.push(AB::Expr::ZERO);
            }
            let mult = in_ctl * not_merkle.clone();
            builder.push_interaction("WitnessChecks", input_idx_limb, -mult, 1);
        }

        // Output limb receives
        let out_idx_base = idx_base + WIDTH_EXT;
        let out_ctl_base = out_idx_base + RATE_EXT;
        for limb_idx in 0..RATE_EXT {
            let idx: AB::Expr = local_preprocessed[out_idx_base + limb_idx].into();
            let out_ctl: AB::Expr = local_preprocessed[out_ctl_base + limb_idx].into();
            let mut output_idx_limb: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
            output_idx_limb.push(idx);
            for d in 0..D {
                output_idx_limb.push(
                    local.poseidon1.ending_full_rounds[HALF_FULL_ROUNDS - 1].post[limb_idx * D + d]
                        .into(),
                );
            }
            for _ in 0..(WITNESS_EXT_D - D) {
                output_idx_limb.push(AB::Expr::ZERO);
            }
            builder.push_interaction("WitnessChecks", output_idx_limb, out_ctl, 1);
        }

        // MMCS accumulator send.
        let mult_a: AB::Expr = local_preprocessed[tail + 1].into();
        let mult_b: AB::Expr = next_preprocessed[tail + 2].into();
        let multiplicity = mult_a * mult_b;
        let mut mmcs_index_sum_lookup: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
        mmcs_index_sum_lookup.push(local_preprocessed[tail].into());
        mmcs_index_sum_lookup.push(local.mmcs_index_sum.into());
        for _ in 0..(WITNESS_EXT_D - 1) {
            mmcs_index_sum_lookup.push(AB::Expr::ZERO);
        }
        builder.push_interaction("WitnessChecks", mmcs_index_sum_lookup, -multiplicity, 1);
    } else {
        let local_pre: &Poseidon1PreprocessedRow<WIDTH_EXT, RATE_EXT, AB::Var> =
            local_preprocessed.borrow();
        let next_pre: &Poseidon1PreprocessedRow<WIDTH_EXT, RATE_EXT, AB::Var> =
            next_preprocessed.borrow();

        // Input limb sends; disabled on Merkle rows (mult zero) so degree stays ≤ 3.
        let not_merkle = AB::Expr::ONE - local_pre.merkle_path.into();

        for limb_idx in 0..WIDTH_EXT {
            let limb = &local_pre.input_limbs[limb_idx];

            let mut input_idx_limb: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
            input_idx_limb.push(limb.idx.into());
            for d in 0..D {
                input_idx_limb.push(local.poseidon1.inputs[limb_idx * D + d].into());
            }
            for _ in 0..(WITNESS_EXT_D - D) {
                input_idx_limb.push(AB::Expr::ZERO);
            }

            let in_ctl: AB::Expr = limb.in_ctl.into();
            let mult = in_ctl * not_merkle.clone();
            builder.push_interaction("WitnessChecks", input_idx_limb, -mult, 1);
        }

        // Output limb receives.
        for limb_idx in 0..RATE_EXT {
            let limb = &local_pre.output_limbs[limb_idx];

            let mut output_idx_limb: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
            output_idx_limb.push(limb.idx.into());
            for d in 0..D {
                output_idx_limb.push(
                    local.poseidon1.ending_full_rounds[HALF_FULL_ROUNDS - 1].post[limb_idx * D + d]
                        .into(),
                );
            }
            for _ in 0..(WITNESS_EXT_D - D) {
                output_idx_limb.push(AB::Expr::ZERO);
            }

            builder.push_interaction("WitnessChecks", output_idx_limb, limb.out_ctl.into(), 1);
        }

        // MMCS accumulator send.
        let mmf: AB::Expr = local_pre.mmcs_merkle_flag.into();
        let next_ns: AB::Expr = next_pre.new_start.into();
        let multiplicity = mmf * next_ns;

        let mut mmcs_index_sum_lookup: Vec<AB::Expr> = Vec::with_capacity(WITNESS_EXT_D + 1);
        mmcs_index_sum_lookup.push(local_pre.mmcs_index_sum_ctl_idx.into());
        mmcs_index_sum_lookup.push(local.mmcs_index_sum.into());
        for _ in 0..(WITNESS_EXT_D - 1) {
            mmcs_index_sum_lookup.push(AB::Expr::ZERO);
        }
        builder.push_interaction("WitnessChecks", mmcs_index_sum_lookup, -multiplicity, 1);
    }
}

#[cfg(test)]
mod test {
    use alloc::vec;

    use p3_baby_bear::{BabyBear, Poseidon1BabyBear, default_babybear_poseidon1_16};
    use p3_field::extension::BinomialExtensionField;
    use p3_matrix::Matrix;
    use p3_symmetric::Permutation;
    use p3_test_utils::air_satisfaction::assert_air_satisfies;
    use rand::rngs::SmallRng;
    use rand::{RngExt, SeedableRng};

    use super::*;
    use crate::columns::{POSEIDON2_LIMBS, POSEIDON2_PUBLIC_OUTPUT_LIMBS};
    use crate::{BabyBearD4Width16, OptimizedConstants, Poseidon1CircuitAirBabyBearD4Width16};

    const WIDTH: usize = 16;
    type Val = BabyBear;
    type EF = BinomialExtensionField<Val, 4>;

    /// Canonical BabyBear Poseidon1 constants plus the matching native permutation; the same
    /// constants drive both the AIR and the reference outputs.
    fn make_constants_and_perm() -> (OptimizedConstants<Val, WIDTH>, Poseidon1BabyBear<WIDTH>) {
        (
            BabyBearD4Width16::round_constants(),
            default_babybear_poseidon1_16(),
        )
    }

    /// Pad a row list to `1 << degree_bits` rows with `new_start=true` fillers so the chaining
    /// constraints don't fire across the boundary, then build the AIR with its preprocessed
    /// trace and the materialized main trace, and assert constraint satisfaction.
    fn check_rows(rows: Vec<Poseidon1CircuitRow<Val>>, constants: &OptimizedConstants<Val, WIDTH>) {
        let degree_bits = 5;
        let target_rows = 1usize << degree_bits;
        let mut padded = rows;
        if padded.len() < target_rows {
            let filler = Poseidon1CircuitRow {
                new_start: true,
                merkle_path: false,
                mmcs_bit: false,
                mmcs_index_sum: Val::ZERO,
                input_values: Val::zero_vec(WIDTH),
                in_ctl: vec![false; POSEIDON2_LIMBS],
                input_indices: vec![0; POSEIDON2_LIMBS],
                out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
                output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
                mmcs_index_sum_idx: 0,
                mmcs_ctl_enabled: false,
            };
            padded.resize(target_rows, filler);
        }

        let preprocessed = extract_preprocessed_from_operations::<4, 2, Val, Val>(&padded, 4, 4);
        let (full, partial) = constants;
        let air = Poseidon1CircuitAirBabyBearD4Width16::new_with_preprocessed(
            full.clone(),
            partial.clone(),
            preprocessed,
        );
        let trace = air.generate_trace_rows(&padded, full, partial, 0);
        assert_air_satisfies::<Val, EF, _>(&air, &trace);
    }

    #[test]
    fn satisfies_poseidon1_sponge() {
        let mut rng = SmallRng::seed_from_u64(1);
        let (constants, perm) = make_constants_and_perm();

        // Row A: new_start=true, sponge mode - use random initial state.
        let state_a: [Val; WIDTH] = core::array::from_fn(|_| rng.random());
        let output_a = perm.permute(state_a);
        let sponge_a = Poseidon1CircuitRow {
            new_start: true,
            merkle_path: false,
            mmcs_bit: false,
            mmcs_index_sum: Val::ZERO,
            input_values: state_a.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: false,
        };

        // Row B: new_start=false, sponge mode - chain from output_a.
        let state_b = output_a;
        let output_b = perm.permute(state_b);
        let sponge_b = Poseidon1CircuitRow {
            new_start: false,
            merkle_path: false,
            mmcs_bit: true,
            mmcs_index_sum: Val::ZERO,
            input_values: state_b.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: false,
        };

        // Row C: merkle mode, mmcs_bit=false. Prev digest chains into limbs 0..1; sibling
        // (limbs 2..3) zeros are fine.
        const D: usize = 4;
        let mut state_c = [Val::ZERO; WIDTH];
        state_c[0..2 * D].copy_from_slice(&output_b[0..2 * D]);
        let output_c = perm.permute(state_c);
        let sponge_c = Poseidon1CircuitRow {
            new_start: false,
            merkle_path: true,
            mmcs_bit: false,
            mmcs_index_sum: Val::ZERO,
            input_values: state_c.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: false,
        };

        // Row D: sponge mode chaining from output_c.
        let state_d = output_c;
        let sponge_d = Poseidon1CircuitRow {
            new_start: false,
            merkle_path: false,
            mmcs_bit: false,
            mmcs_index_sum: Val::ZERO,
            input_values: state_d.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: false,
        };

        check_rows(vec![sponge_a, sponge_b, sponge_c, sponge_d], &constants);
    }

    #[test]
    fn satisfies_poseidon1_merkle_right() {
        const D: usize = 4;
        let mut rng = SmallRng::seed_from_u64(42);
        let (constants, perm) = make_constants_and_perm();

        // Row A: new_start, random state.
        let state_a: [Val; WIDTH] = core::array::from_fn(|_| rng.random());
        let output_a = perm.permute(state_a);
        let row_a = Poseidon1CircuitRow {
            new_start: true,
            merkle_path: false,
            mmcs_bit: false,
            mmcs_index_sum: Val::ZERO,
            input_values: state_a.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: false,
        };

        // Row B: merkle mode, mmcs_bit=true (right child).
        // With mmcs_bit=1: prev output[0..D] → input[2D..3D], output[D..2D] → input[3D..4D];
        // limbs 0-1 hold the sibling.
        let sibling: [Val; 2 * D] = core::array::from_fn(|_| rng.random());
        let mut state_b = [Val::ZERO; WIDTH];
        state_b[0..2 * D].copy_from_slice(&sibling);
        state_b[2 * D..3 * D].copy_from_slice(&output_a[0..D]);
        state_b[3 * D..4 * D].copy_from_slice(&output_a[D..2 * D]);
        let output_b = perm.permute(state_b);
        let row_b = Poseidon1CircuitRow {
            new_start: false,
            merkle_path: true,
            mmcs_bit: true,
            mmcs_index_sum: Val::ZERO,
            input_values: state_b.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: false,
        };

        // Row C: sponge chaining from output_b.
        let row_c = Poseidon1CircuitRow {
            new_start: false,
            merkle_path: false,
            mmcs_bit: false,
            mmcs_index_sum: Val::ZERO,
            input_values: output_b.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: false,
        };

        check_rows(vec![row_a, row_b, row_c], &constants);
    }

    #[test]
    fn satisfies_poseidon1_mmcs_accumulator() {
        const D: usize = 4;
        let mut rng = SmallRng::seed_from_u64(99);
        let (constants, perm) = make_constants_and_perm();

        // 3-row Merkle chain that exercises the MMCS accumulator.
        //   Row 0: new_start=true,  merkle, mmcs_bit=1 → mmcs_index_sum resets to 0
        //   Row 1: new_start=false, merkle, mmcs_bit=0 → mmcs_index_sum = 0*2 + 0 = 0
        //   Row 2: new_start=false, merkle, mmcs_bit=1 → mmcs_index_sum = 0*2 + 1 = 1
        let bits = [true, false, true];

        let state_0: [Val; WIDTH] = core::array::from_fn(|_| rng.random());
        let output_0 = perm.permute(state_0);
        let row_0 = Poseidon1CircuitRow {
            new_start: true,
            merkle_path: true,
            mmcs_bit: bits[0],
            mmcs_index_sum: Val::ZERO,
            input_values: state_0.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: true,
        };

        // Row 1: left child (mmcs_bit=0), chain output[0..2D] → input[0..2D].
        let mut state_1 = [Val::ZERO; WIDTH];
        state_1[0..D].copy_from_slice(&output_0[0..D]);
        state_1[D..2 * D].copy_from_slice(&output_0[D..2 * D]);
        let output_1 = perm.permute(state_1);
        let row_1 = Poseidon1CircuitRow {
            new_start: false,
            merkle_path: true,
            mmcs_bit: bits[1],
            mmcs_index_sum: Val::ZERO, // will be computed by generate_trace_rows
            input_values: state_1.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: true,
        };

        // Row 2: right child (mmcs_bit=1), output[0..D]→input[2D..3D], output[D..2D]→input[3D..4D].
        let sibling: [Val; 2 * D] = core::array::from_fn(|_| rng.random());
        let mut state_2 = [Val::ZERO; WIDTH];
        state_2[0..2 * D].copy_from_slice(&sibling);
        state_2[2 * D..3 * D].copy_from_slice(&output_1[0..D]);
        state_2[3 * D..4 * D].copy_from_slice(&output_1[D..2 * D]);
        let _output_2 = perm.permute(state_2);
        let row_2 = Poseidon1CircuitRow {
            new_start: false,
            merkle_path: true,
            mmcs_bit: bits[2],
            mmcs_index_sum: Val::ZERO,
            input_values: state_2.to_vec(),
            in_ctl: vec![false; POSEIDON2_LIMBS],
            input_indices: vec![0; POSEIDON2_LIMBS],
            out_ctl: vec![false; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            output_indices: vec![0; POSEIDON2_PUBLIC_OUTPUT_LIMBS],
            mmcs_index_sum_idx: 0,
            mmcs_ctl_enabled: true,
        };

        check_rows(vec![row_0, row_1, row_2], &constants);
    }

    #[test]
    fn test_air_constraint_degree() {
        let (full, partial) = BabyBearD4Width16::round_constants();
        let air = Poseidon1CircuitAirBabyBearD4Width16::new(full, partial);
        p3_test_utils::assert_air_constraint_degree!(air, "Poseidon1CircuitAir");
    }

    /// Build an AIR with the given preprocessed data and optional minimum
    /// height, then return the materialized preprocessed trace matrix.
    fn build_preprocessed_trace(
        preprocessed: Vec<BabyBear>,
        min_height: usize,
    ) -> RowMajorMatrix<BabyBear> {
        let (full, partial) = BabyBearD4Width16::round_constants();
        Poseidon1CircuitAirBabyBearD4Width16::new_with_preprocessed(full, partial, preprocessed)
            .with_min_height(min_height)
            .preprocessed_trace()
            .expect("preprocessed_trace returned None")
    }

    /// The preprocessed width for BabyBear width-16 is 24 columns.
    ///
    /// 4 input limbs × 4 fields each  = 16
    /// 2 output limbs × 2 fields each =  4
    /// 4 scalar fields                =  4
    ///                           total = 24
    const PREP_WIDTH: usize = 24;

    #[test]
    fn preprocessed_trace_pads_to_power_of_two() {
        // Feed 3 rows of data. That's 3 × 24 = 72 field elements.
        //
        // 3 is not a power of two, so the method must round up to 4 rows.
        let three_rows = vec![BabyBear::ONE; 3 * PREP_WIDTH];
        let trace = build_preprocessed_trace(three_rows, 1);

        // The matrix should have 4 rows and 24 columns.
        assert_eq!(trace.height(), 4);
        assert_eq!(trace.width(), PREP_WIDTH);
    }

    #[test]
    fn preprocessed_trace_respects_min_height() {
        // Feed 2 rows of data (already a power of two).
        //
        // But request a minimum height of 8.
        //
        // The result must have 8 rows, not 2.
        let two_rows = vec![BabyBear::ONE; 2 * PREP_WIDTH];
        let trace = build_preprocessed_trace(two_rows, 8);

        assert_eq!(trace.height(), 8);
        assert_eq!(trace.width(), PREP_WIDTH);
    }

    #[test]
    fn preprocessed_trace_preserves_original_data() {
        // Fill one row with all-twos.
        //
        // After padding the result must still have those values in the
        // first row.
        let one_row = vec![BabyBear::TWO; PREP_WIDTH];
        let trace = build_preprocessed_trace(one_row.clone(), 1);

        // The first row should be exactly what we put in.
        let values = trace.values.as_slice();
        assert_eq!(&values[..PREP_WIDTH], &one_row[..]);
    }

    #[test]
    fn preprocessed_trace_sets_chain_boundary_on_first_padding_row() {
        // Feed 3 rows. The method pads to 4 rows (next power of two).
        //
        // The first padding row (row index 3) must have its chain-start
        // flag set to one. That flag is the second-to-last column.
        //
        // This prevents the chaining constraint from connecting the
        // last real row to the first padding row.
        let three_rows = vec![BabyBear::ZERO; 3 * PREP_WIDTH];
        let trace = build_preprocessed_trace(three_rows, 1);

        assert_eq!(trace.height(), 4);

        // Row 3 (first padding row): second-to-last column = 1.
        let values = trace.values.as_slice();
        let padding_row = &values[3 * PREP_WIDTH..4 * PREP_WIDTH];
        let chain_start_flag = padding_row[PREP_WIDTH - 2];
        assert_eq!(chain_start_flag, BabyBear::ONE);

        // All other columns in the padding row should be zero.
        for (i, &val) in padding_row.iter().enumerate() {
            if i != PREP_WIDTH - 2 {
                assert_eq!(val, BabyBear::ZERO, "padding row column {i} should be zero");
            }
        }
    }

    #[test]
    fn preprocessed_trace_no_padding_when_exact_power_of_two() {
        // Feed exactly 4 rows. Already a power of two.
        //
        // No padding should occur, so no chain-boundary flag is set on
        // any extra row.
        let four_rows = vec![BabyBear::ONE; 4 * PREP_WIDTH];
        let trace = build_preprocessed_trace(four_rows, 1);

        // Height should be exactly 4 — no extra rows.
        assert_eq!(trace.height(), 4);

        // All 4 rows should contain the original data (all ones).
        let values = trace.values.as_slice();
        for row_idx in 0..4 {
            let start = row_idx * PREP_WIDTH;
            let row = &values[start..start + PREP_WIDTH];
            assert!(
                row.iter().all(|&v| v == BabyBear::ONE),
                "row {row_idx} should be all ones"
            );
        }
    }

    #[test]
    fn preprocessed_trace_padding_rows_beyond_first_are_all_zero() {
        // Feed 1 row. Request minimum height of 8.
        //
        // Rows 1..8 are padding. Row 1 has the chain-boundary flag.
        // Rows 2..8 should be entirely zero.
        let one_row = vec![BabyBear::ONE; PREP_WIDTH];
        let trace = build_preprocessed_trace(one_row, 8);

        assert_eq!(trace.height(), 8);

        // Rows 2 through 7 should be completely zero.
        let values = trace.values.as_slice();
        for row_idx in 2..8 {
            let start = row_idx * PREP_WIDTH;
            let row = &values[start..start + PREP_WIDTH];
            assert!(
                row.iter().all(|&v| v == BabyBear::ZERO),
                "padding row {row_idx} should be all zeros"
            );
        }
    }
}
