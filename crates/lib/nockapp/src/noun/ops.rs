use std::sync::Arc;

use nockvm::interpreter::{interpret, Context};
use nockvm::noun::{Noun, D, T};
use tracing::{span, Level};

use crate::metrics::NockAppMetrics;
use crate::utils::Result;
use crate::CrownError;

/// Slams (applies) a gate at a specific axis of the supplied kernel.
///
/// # Arguments
/// * `context` - The interpreter cotnext.
/// * `arvo` - The kernel.
/// * `axis` - The axis to slam.
/// * `ovo` - The sample noun.
///
/// # Returns
///
/// Result containing the slammed result or an error.
#[tracing::instrument(skip(context, arvo, axis, ovo, metrics))]
pub fn slam(
    context: &mut Context,
    arvo: Noun,
    axis: u64,
    ovo: Noun,
    metrics: Option<Arc<NockAppMetrics>>,
) -> Result<Noun> {
    let stack = &mut context.stack;
    let pul = T(stack, &[D(9), D(axis), D(0), D(2)]);
    let sam = T(stack, &[D(6), D(0), D(7)]);
    let fol = T(stack, &[D(8), pul, D(9), D(2), D(10), sam, D(0), D(2)]);
    let sub = T(stack, &[arvo, ovo]);

    span!(Level::DEBUG, "interpret").in_scope(|| {
        let res = interpret(context, sub, fol).map_err(CrownError::from);
        let _ = metrics.map(|m| {
            m.least_free_space_seen_in_slam
                .swap((context.stack.least_space() * 8) as f64)
        }); // least_space is in 8-byte words
        res
    })
}
