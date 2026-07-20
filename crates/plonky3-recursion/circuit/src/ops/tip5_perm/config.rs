//! Tip5 configuration types and execution closures (C2.2 / M-S4).
//!
//! Mirrors the [`Poseidon1Config`](crate::ops::Poseidon1Config)
//! *interface* (the methods the recursion challenger / circuit
//! lowering call) but with **recursive Tip5-correct internals**:
//! recursive proving uses the 5-round Goldilocks Tip5 variant from
//! `p3-tip5-circuit-air` / `nockchain_math::tip5::permute_5round`,
//! with sponge **width 16, rate 10, capacity 6, digest 5**. This is not
//! the canonical 7-round Nockchain hash path. The split-and-lookup +
//! `x^7` round structure lives in the constraint system; this is only
//! the parameter bundle / NPO key, kept faithful to the recursive
//! prover's transcript geometry.

use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;

use p3_field::Field;
use serde::{Deserialize, Serialize};

use crate::CircuitBuilderError;
use crate::builder::NpoLoweringContext;
use crate::types::{ExprId, WitnessId};

/// Identifies the base field for a Tip5 configuration.
///
/// Recursive Tip5 is Goldilocks-only; the enum mirrors
/// `Poseidon1FieldId` so a future field is a one-variant addition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Tip5FieldId {
    /// Goldilocks (64-bit, `p = 2^64 − 2^32 + 1`).
    Goldilocks,
}

/// Tip5 configuration: a stable NPO key and faithful parameter source
/// for the recursive 5-round, width-16, rate-10/capacity-6 Goldilocks
/// Tip5 sponge (`PaddingFreeSponge<Tip5Perm,16,10,5>`,
/// `DuplexChallenger<Goldilocks,Tip5Perm,16,10>`,
/// `TruncatedPermutation<Tip5Perm,2,5,16>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Tip5Config {
    /// Target prime field.
    field_id: Tip5FieldId,
    /// Extension degree. Tip5 is base-field only ⇒ always 1.
    d: usize,
    /// Permutation state width (base field elements). Always 16.
    width: usize,
    /// Sponge rate (absorbed/squeezed per permutation). Always 10.
    rate: usize,
    /// Sponge capacity. Always 6 (`width − rate`).
    capacity: usize,
    /// Digest length (squeezed elements for a hash). Always 5.
    digest: usize,
    /// Number of Tip5 rounds. Recursive proving uses paper-spec N=5.
    num_rounds: usize,
    /// Split-and-lookup S-box lanes per round (the rest use `x^7`). 4.
    num_split: usize,
}

impl Tip5Config {
    /// Explicit recursive configuration: Goldilocks, width 16,
    /// rate 10, capacity 6, digest 5, 5 rounds, 4 split lanes.
    pub const GOLDILOCKS_W16_R5: Self = Self {
        field_id: Tip5FieldId::Goldilocks,
        d: 1,
        width: 16,
        rate: 10,
        capacity: 6,
        digest: 5,
        num_rounds: 5,
        num_split: 4,
    };

    /// Backward-compatible name for the single recursive Tip5 config.
    pub const GOLDILOCKS_W16: Self = Self::GOLDILOCKS_W16_R5;

    /// Returns `true` if this configuration targets Goldilocks.
    pub const fn is_goldilocks(self) -> bool {
        matches!(self.field_id, Tip5FieldId::Goldilocks)
    }

    /// Extension degree (always 1 — Tip5 is base-field only).
    pub const fn d(self) -> usize {
        self.d
    }

    /// State width in base field elements (16).
    pub const fn width(self) -> usize {
        self.width
    }

    /// Rate in extension elements. `d == 1` ⇒ equals the base rate (10).
    pub const fn rate_ext(self) -> usize {
        self.rate / self.d
    }

    /// Rate in base field elements (10).
    pub const fn rate(self) -> usize {
        self.rate
    }

    /// Capacity in extension elements. `d == 1` ⇒ base capacity (6).
    pub const fn capacity_ext(self) -> usize {
        self.capacity / self.d
    }

    /// State width in extension elements (`rate_ext + capacity_ext` = 16).
    pub const fn width_ext(self) -> usize {
        self.rate_ext() + self.capacity_ext()
    }

    /// Digest length in base elements (5).
    pub const fn digest(self) -> usize {
        self.digest
    }

    /// MMCS digest length in extension elements. Tip5 is `d == 1`, so
    /// this equals the base digest (5). Native
    /// `PaddingFreeSponge<Tip5Perm,16,10,5>` squeezes 5 and
    /// `TruncatedPermutation<Tip5Perm,2,5,16>` compresses two
    /// 5-element digests — i.e. digest (5) ≠ rate (10), unlike
    /// Poseidon where digest == rate. The `PermConfig`-generic MMCS
    /// uses this so the in-circuit leaf squeeze / sibling-compress
    /// geometry matches native bit-for-bit.
    pub const fn digest_ext(self) -> usize {
        self.digest / self.d
    }

    /// Tip5 round count (5 for recursive proving).
    pub const fn num_rounds(self) -> usize {
        self.num_rounds
    }

    /// Split-and-lookup lanes per round (4).
    pub const fn num_split(self) -> usize {
        self.num_split
    }

    /// Stable variant string for the `NpoTypeId` key.
    pub const fn variant_name(self) -> &'static str {
        match self.field_id {
            Tip5FieldId::Goldilocks => match (self.width, self.num_rounds) {
                (16, 5) => "goldilocks_w16_r5",
                _ => panic!("unknown Goldilocks Tip5 config"),
            },
        }
    }

    /// Parse a `Tip5Config` from a variant name string.
    pub fn from_variant_name(name: &str) -> Option<Self> {
        match name {
            "goldilocks_w16_r5" => Some(Self::GOLDILOCKS_W16),
            _ => None,
        }
    }

    /// Check input/output counts match this config's layout.
    ///
    /// Tip5 is `d == 1`, so this mirrors Poseidon1's D=1 layout with
    /// Tip5 numbers: `add_tip5_perm_base` supplies exactly `width`
    /// (16) inputs; the challenger/MMCS path supplies `width_ext + 2`
    /// (18 = 16 limbs + mmcs index sum + direction bit), required when
    /// `merkle_path`. Outputs are `rate` (10, squeeze) or `width` (16,
    /// full state).
    pub fn validate_io_counts(
        self,
        input_count: usize,
        output_count: usize,
        merkle_path: bool,
    ) -> Result<(), CircuitBuilderError> {
        let inputs_ok = if merkle_path {
            input_count == self.width_ext() + 2
        } else {
            input_count == self.width() || input_count == self.width_ext() + 2
        };
        if !inputs_ok {
            let expected = if merkle_path {
                format!("{} inputs", self.width_ext() + 2)
            } else {
                format!(
                    "{} or {} inputs for Tip5 (d=1)",
                    self.width(),
                    self.width_ext() + 2
                )
            };
            return Err(CircuitBuilderError::NonPrimitiveOpArity {
                op: "Tip5Perm",
                expected,
                got: input_count,
            });
        }

        if output_count != self.rate() && output_count != self.width() {
            return Err(CircuitBuilderError::NonPrimitiveOpArity {
                op: "Tip5Perm",
                expected: format!(
                    "{} or {} outputs for Tip5 (d=1)",
                    self.rate(),
                    self.width()
                ),
                got: output_count,
            });
        }
        Ok(())
    }

    /// Lower input expressions to witness indices per this config's
    /// layout (mirrors Poseidon1 D=1): flat `width` slots for the
    /// base-perm path, else `width_ext` limb slots + mmcs index sum +
    /// direction bit.
    pub fn lower_inputs<F: Field>(
        self,
        input_exprs: &[Vec<ExprId>],
        ctx: &NpoLoweringContext<'_, F>,
        merkle_path: bool,
    ) -> Result<Vec<Vec<WitnessId>>, CircuitBuilderError> {
        if !merkle_path && input_exprs.len() == self.width() {
            return ctx.lower_expr_slots(input_exprs, "Tip5Perm", "d=1 input");
        }

        let width_ext = self.width_ext();
        let mut widx =
            ctx.lower_expr_slots(&input_exprs[..width_ext], "Tip5Perm", "input limb")?;

        let [mmcs_sum] = ctx
            .lower_expr_slots(
                &input_exprs[width_ext..=width_ext],
                "Tip5Perm",
                "mmcs_index_sum",
            )?
            .try_into()
            .expect("single-element slice must yield single-element vec");
        widx.push(mmcs_sum);

        let [mmcs_bit] = ctx
            .lower_expr_slots(
                &input_exprs[width_ext + 1..=width_ext + 1],
                "Tip5Perm",
                "mmcs_bit",
            )?
            .try_into()
            .expect("single-element slice must yield single-element vec");
        widx.push(mmcs_bit);

        Ok(widx)
    }
}

/// Tip5 permutation execution closure: `width` (16) base field
/// elements in, `width` out (the full permuted state).
///
/// Consumed by the C2.3 NPO executor (plugin/builder/trace); declared
/// here with the config so the geometry contract is single-sourced.
pub type Tip5PermExec<F> = Arc<dyn Fn(&[F]) -> Vec<F> + Send + Sync>;

/// Config data stored inside `NpoConfig` for Tip5 operations (C2.3).
pub struct Tip5PermConfigData<F> {
    pub exec: Tip5PermExec<F>,
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use hashbrown::HashMap;
    use p3_test_utils::goldilocks_params::Goldilocks;

    use super::*;
    use crate::builder::NpoLoweringContext;

    type F = Goldilocks;

    #[test]
    fn geometry_matches_recursive_tip5() {
        let c = Tip5Config::GOLDILOCKS_W16;
        assert!(c.is_goldilocks());
        assert_eq!(c.d(), 1);
        assert_eq!(c.width(), 16);
        assert_eq!(c.rate(), 10); // DuplexChallenger<_,_,16,10>
        assert_eq!(c.rate_ext(), 10); // d == 1
        assert_eq!(c.capacity_ext(), 6); // 16 − 10
        assert_eq!(c.width_ext(), 16); // rate_ext + capacity_ext
        assert_eq!(c.digest(), 5); // PaddingFreeSponge<_,16,10,5>
        assert_eq!(c.num_rounds(), 5); // recursive Tip5 is 5-round
        assert_eq!(c.num_split(), 4);
    }

    #[test]
    fn variant_name_roundtrips() {
        let c = Tip5Config::GOLDILOCKS_W16;
        assert_eq!(c.variant_name(), "goldilocks_w16_r5");
        assert_eq!(Tip5Config::from_variant_name(c.variant_name()), Some(c));
        assert_eq!(Tip5Config::from_variant_name("nope"), None);
    }

    #[test]
    fn validate_io_counts_ok_and_errors() {
        let c = Tip5Config::GOLDILOCKS_W16;
        // base-perm path: 16 inputs; squeeze (10) or full state (16) out
        assert!(c.validate_io_counts(16, 10, false).is_ok());
        assert!(c.validate_io_counts(16, 16, false).is_ok());
        // challenger/MMCS path: width_ext+2 = 18 inputs
        assert!(c.validate_io_counts(18, 10, false).is_ok());
        assert!(c.validate_io_counts(18, 16, true).is_ok());
        // merkle requires exactly 18
        let Err(CircuitBuilderError::NonPrimitiveOpArity { op, expected, got }) =
            c.validate_io_counts(16, 10, true)
        else {
            panic!("expected arity error");
        };
        assert_eq!(op, "Tip5Perm");
        assert_eq!(expected, "18 inputs");
        assert_eq!(got, 16);
        // bad output count
        let Err(CircuitBuilderError::NonPrimitiveOpArity { expected, .. }) =
            c.validate_io_counts(16, 7, false)
        else {
            panic!("expected arity error");
        };
        assert_eq!(expected, "10 or 16 outputs for Tip5 (d=1)");
    }

    #[test]
    fn lower_inputs_flat_and_merkle_layouts() {
        let c = Tip5Config::GOLDILOCKS_W16;

        // flat width=16
        let mut map = HashMap::new();
        for i in 0u32..16 {
            map.insert(ExprId(i), WitnessId(i));
        }
        let mut counter = 100u32;
        let mut alloc = |_: usize| {
            let id = WitnessId(counter);
            counter += 1;
            id
        };
        let ctx = NpoLoweringContext::<F>::new(&mut map, &mut alloc);
        let exprs: Vec<Vec<ExprId>> = (0u32..16).map(|i| vec![ExprId(i)]).collect();
        let got = c.lower_inputs(&exprs, &ctx, false).unwrap();
        let want: Vec<Vec<WitnessId>> = (0u32..16).map(|i| vec![WitnessId(i)]).collect();
        assert_eq!(got, want);

        // merkle width_ext+2 = 18
        let mut map2 = HashMap::new();
        for i in 0u32..18 {
            map2.insert(ExprId(i), WitnessId(200 + i));
        }
        let mut counter2 = 400u32;
        let mut alloc2 = |_: usize| {
            let id = WitnessId(counter2);
            counter2 += 1;
            id
        };
        let ctx2 = NpoLoweringContext::<F>::new(&mut map2, &mut alloc2);
        let exprs2: Vec<Vec<ExprId>> = (0u32..18).map(|i| vec![ExprId(i)]).collect();
        let got2 = c.lower_inputs(&exprs2, &ctx2, true).unwrap();
        let want2: Vec<Vec<WitnessId>> = (0u32..18).map(|i| vec![WitnessId(200 + i)]).collect();
        assert_eq!(got2, want2);
    }
}
