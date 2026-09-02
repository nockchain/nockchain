//! The evaluation environment honk's binaries share.
//!
//! Everything honk fires — constant folds during a compile, and the build
//! traps `kick` runs afterwards — must run under the same interpreter setup:
//! the native hot state with formula-keyed jet dispatch (`HintBlind`). A
//! second copy of that setup would be a silent correctness hazard, because a
//! divergence shows up not as a build failure but as a *different noun*.
//! Both binaries call in here instead.

use std::env;
use std::panic::{self, AssertUnwindSafe};

use nockapp::utils::{create_context, NOCK_STACK_SIZE_MEDIUM};
use nockvm::interpreter::Context;
use nockvm::jets::cold::Cold;
use nockvm::jets::JetDispatchMode;
use nockvm::mem::{AllocationError, NockStack};

/// Words of `NockStack` reserved for evaluation, absent an override.
///
/// Constant-fold evaluation interns shared mack-core copies on the eval
/// stack for the lifetime of each entry compile; 16GB fills up on
/// fold-heavy kernels (tx-engine digests), and exhaustion silently degrades
/// folds to step evaluation. The reservation is virtual, so size it well
/// above observed peaks.
pub const HONK_EVAL_STACK_SIZE: usize = NOCK_STACK_SIZE_MEDIUM; // 16GB

/// Words of `NockStack` to reserve, honouring `HONK_EVAL_STACK_WORDS`.
///
/// The default reservation is virtual and therefore free on a machine that
/// overcommits, but a process that only fires a small trap does not need it,
/// and several such processes at once (the `kick` test suite) can exceed what
/// a heuristic-overcommit kernel will hand out. The override is in *words*,
/// matching `NockStack::new`, so that it cannot be confused with the
/// byte-denominated `HONK_WORKER_STACK_BYTES`.
pub fn eval_stack_size() -> usize {
    env::var("HONK_EVAL_STACK_WORDS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(HONK_EVAL_STACK_SIZE)
}

/// A fresh interpreter context: native hot state, empty cold state, jets
/// dispatched on formulas rather than hints.
pub fn create_eval_context() -> Context {
    let mut stack = NockStack::new(eval_stack_size(), 0);
    let cold = Cold::new(&mut stack);
    create_context(
        stack,
        crate::native::hot::native_hot_state(),
        cold,
        None,
        vec![],
        JetDispatchMode::HintBlind,
    )
}

/// Run `f`, converting a nock-stack panic into an ordinary error.
///
/// The interpreter reports arena exhaustion and its own invariant violations
/// by panicking, which for a CLI means an abort and a backtrace where the
/// user wanted a diagnostic. `label` names the work in progress ("firing
/// build.jam") and prefixes whatever the panic carried.
pub fn catch_nock_panic<T, E>(
    label: impl Into<String>,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E>
where
    E: From<String>,
{
    let label = label.into();
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<AllocationError>()
                .map(|err| format!(": {err}"))
                .or_else(|| {
                    payload
                        .downcast_ref::<String>()
                        .map(|err| format!(": {err}"))
                })
                .or_else(|| {
                    payload
                        .downcast_ref::<&'static str>()
                        .map(|err| format!(": {err}"))
                })
                .unwrap_or_default();
            Err(format!("{label}: nock stack panic{detail}").into())
        }
    }
}
