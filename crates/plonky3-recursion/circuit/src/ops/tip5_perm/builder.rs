//! Inherent `CircuitBuilder` methods for adding Tip5 permutation rows
//! (C2.3).
//!
//! - [`CircuitBuilder::add_tip5_perm`] mirrors
//!   `add_poseidon1_perm_base` (the D=1 base path, used by the
//!   in-circuit challenger and the standalone batch-STARK CTL gate).
//! - [`CircuitBuilder::add_tip5_perm_mmcs`] mirrors
//!   `add_poseidon1_perm` (the merkle / MMCS-path variant), consumed
//!   by the `PermConfig`-generic `add_perm` dispatch so the in-circuit
//!   MMCS (`add_mmcs_verify`) routes Tip5 through the Tip5 NPO.

use alloc::vec;
use alloc::vec::Vec;

use p3_field::Field;

use crate::CircuitBuilderError;
use crate::builder::{CircuitBuilder, NonPrimitiveOpParams};
use crate::ops::NpoTypeId;
use crate::ops::tip5_perm::call::{Tip5PermCall, Tip5PermCallMmcs};
use crate::types::{ExprId, NonPrimitiveOpId};

impl<F: Field> CircuitBuilder<F> {
    /// Add a Tip5 perm row (one 5-round Tip5 permutation), D=1 base
    /// field. Mirrors `add_poseidon1_perm_base`.
    ///
    /// Returns `(op_id, outputs)` where `outputs` is `[Option<ExprId>;
    /// 16]`:
    /// - `outputs[0..10]`: present if `out_ctl[i]` is true
    ///   (CTL-verified, rate elements)
    /// - `outputs[10..16]`: present if `return_all_outputs` is true
    ///   (capacity, not CTL-verified)
    pub fn add_tip5_perm(
        &mut self,
        call: &Tip5PermCall,
    ) -> Result<(NonPrimitiveOpId, [Option<ExprId>; 16]), CircuitBuilderError> {
        let op_type = NpoTypeId::tip5_perm(call.config);
        self.ensure_op_enabled(&op_type)?;

        if call.config.d() != 1 {
            return Err(CircuitBuilderError::Tip5ConfigMismatch {
                expected: "D=1 configuration".into(),
                got: alloc::format!("D={} configuration", call.config.d()),
            });
        }

        let input_exprs: [Vec<ExprId>; 16] = call
            .inputs
            .map(|opt| opt.map_or_else(Vec::new, |v| vec![v]));

        let output_labels: [Option<&'static str>; 16] = core::array::from_fn(|i| match i {
            0..10 if call.out_ctl[i] => Some("tip5_perm_out"),
            10..16 if call.return_all_outputs => Some("tip5_perm_out_capacity"),
            _ => None,
        });

        let (op_id, _call_expr_id, outputs) = self.push_non_primitive_op_with_outputs(
            op_type,
            input_exprs.into(),
            output_labels.into(),
            Some(NonPrimitiveOpParams::Tip5Perm {
                new_start: call.new_start,
                merkle_path: false,
            }),
            "tip5_perm",
        );

        let outputs: [Option<ExprId>; 16] = outputs
            .try_into()
            .expect("push_non_primitive_op_with_outputs must return exactly 16 outputs");
        Ok((op_id, outputs))
    }

    /// Add a Tip5 perm row in the merkle / MMCS-path layout. Faithful
    /// mirror of `add_poseidon1_perm` with Tip5 D=1 numbers
    /// (`width_ext` = 16, `rate_ext` = 10).
    ///
    /// `inputs` are `width_ext` limb slots followed by the
    /// `mmcs_index_sum` and `mmcs_bit` tail slots (Poseidon1 D=1
    /// layout). Returns `(op_id, outputs)` where `outputs` has length
    /// `width_ext`:
    /// - `outputs[0..rate_ext]`: present if `out_ctl[i]` is true
    ///   (CTL-verified rate digest limbs);
    /// - `outputs[rate_ext..]`: present if `return_all_outputs` is
    ///   true (capacity, not CTL-verified).
    pub fn add_tip5_perm_mmcs(
        &mut self,
        call: &Tip5PermCallMmcs,
    ) -> Result<(NonPrimitiveOpId, Vec<Option<ExprId>>), CircuitBuilderError> {
        let op_type = NpoTypeId::tip5_perm(call.config);
        self.ensure_op_enabled(&op_type)?;

        if call.config.d() != 1 {
            return Err(CircuitBuilderError::Tip5ConfigMismatch {
                expected: "D=1 configuration".into(),
                got: alloc::format!("D={} configuration", call.config.d()),
            });
        }
        if call.merkle_path && call.mmcs_bit.is_none() {
            return Err(CircuitBuilderError::Tip5MerkleMissingMmcsBit);
        }
        if !call.merkle_path && call.mmcs_bit.is_some() {
            return Err(CircuitBuilderError::Tip5NonMerkleWithMmcsBit);
        }

        let width_ext = call.config.width_ext();
        let rate_ext = call.config.rate_ext();

        let mut input_exprs: Vec<Vec<ExprId>> = Vec::with_capacity(width_ext + 2);
        for limb in &call.inputs {
            input_exprs.push(limb.map_or_else(Vec::new, |v| vec![v]));
        }
        input_exprs.push(call.mmcs_index_sum.map_or_else(Vec::new, |v| vec![v]));
        input_exprs.push(call.mmcs_bit.map_or_else(Vec::new, |v| vec![v]));

        let mut output_labels: Vec<Option<&'static str>> = Vec::with_capacity(width_ext);
        for i in 0..rate_ext {
            let expose = call.out_ctl.get(i).copied().unwrap_or(false);
            output_labels.push(expose.then_some("tip5_perm_out"));
        }
        for _ in rate_ext..width_ext {
            output_labels.push(
                call.return_all_outputs
                    .then_some("tip5_perm_out_capacity"),
            );
        }

        let (op_id, _call_expr_id, outputs) = self.push_non_primitive_op_with_outputs(
            op_type,
            input_exprs,
            output_labels,
            Some(NonPrimitiveOpParams::Tip5Perm {
                new_start: call.new_start,
                merkle_path: call.merkle_path,
            }),
            "tip5_perm_mmcs",
        );
        Ok((op_id, outputs))
    }
}
