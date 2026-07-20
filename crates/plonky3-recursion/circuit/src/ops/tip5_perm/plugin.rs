//! Tip5 circuit plugin — [`NpoCircuitPlugin`] implementation (C2.3).
//!
//! Faithful mechanical mirror of `Poseidon1CircuitPlugin` for the D=1
//! path: it lowers both the non-merkle base layout and the merkle /
//! MMCS layout (`config.lower_inputs(.., merkle)` chooses the flat
//! `width` slots vs the `width_ext` limb slots + mmcs_index_sum +
//! mmcs_bit tail), constructing the executor with the resolved
//! `merkle_path` flag from the per-op params.

use alloc::boxed::Box;

use p3_field::Field;

use crate::CircuitBuilderError;
use crate::builder::{NpoCircuitPlugin, NpoLoweringContext};
use crate::ops::tip5_perm::config::{Tip5Config, Tip5PermConfigData, Tip5PermExec};
use crate::ops::tip5_perm::executor::Tip5PermExecutor;
use crate::ops::{NpoConfig, NpoTypeId, Op};
use crate::tables::TraceGeneratorFn;
use crate::types::ExprId;

/// Circuit-layer plugin for Tip5 non-primitive operations.
pub struct Tip5CircuitPlugin<F: Field> {
    type_id: NpoTypeId,
    tip5_config: Tip5Config,
    npo_config: NpoConfig,
    trace_gen: TraceGeneratorFn<F>,
}

impl<F: Field> Tip5CircuitPlugin<F> {
    pub fn new(
        tip5_config: Tip5Config,
        exec: Tip5PermExec<F>,
        trace_gen: TraceGeneratorFn<F>,
    ) -> Self {
        Self {
            type_id: NpoTypeId::tip5_perm(tip5_config),
            tip5_config,
            npo_config: NpoConfig::new(Tip5PermConfigData { exec }),
            trace_gen,
        }
    }
}

impl<F: Field> NpoCircuitPlugin<F> for Tip5CircuitPlugin<F> {
    fn type_id(&self) -> NpoTypeId {
        self.type_id.clone()
    }

    fn lower(
        &self,
        data: &crate::builder::NonPrimitiveOperationData<F>,
        output_exprs: &[(u32, ExprId)],
        ctx: &mut NpoLoweringContext<'_, F>,
    ) -> Result<Op<F>, CircuitBuilderError> {
        for (_output_idx, expr_id) in output_exprs {
            ctx.ensure_witness_id(*expr_id);
        }

        let (new_start, merkle_path) = data
            .params
            .as_ref()
            .and_then(|p| p.as_tip5_perm())
            .ok_or_else(|| CircuitBuilderError::InvalidNonPrimitiveOpConfiguration {
                op: data.op_type.clone(),
            })?;

        let config = self.tip5_config;
        config.validate_io_counts(
            data.input_exprs.len(),
            data.output_exprs.len(),
            merkle_path,
        )?;

        let inputs_widx = config.lower_inputs(&data.input_exprs, ctx, merkle_path)?;
        let outputs_widx = ctx.lower_expr_slots(&data.output_exprs, "Tip5Perm", "output")?;

        Ok(Op::NonPrimitiveOpWithExecutor {
            inputs: inputs_widx,
            outputs: outputs_widx,
            executor: Box::new(Tip5PermExecutor::new(
                data.op_type.clone(),
                config,
                new_start,
                merkle_path,
            )),
            op_id: data.op_id,
        })
    }

    fn trace_generator(&self) -> TraceGeneratorFn<F> {
        self.trace_gen
    }

    fn config(&self) -> crate::ops::NpoConfig {
        self.npo_config.clone()
    }
}
