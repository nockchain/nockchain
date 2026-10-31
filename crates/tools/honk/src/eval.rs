//! The evaluation environment honk's binaries share.
//!
//! Everything honk fires — constant folds during a compile, and the build
//! traps `kick` runs afterwards — must run under the same interpreter setup:
//! the native hot state over an empty cold state. A second copy of that
//! setup would be a silent correctness hazard, because a divergence shows up
//! not as a build failure but as a *different noun*. Both binaries call in
//! here instead.
//!
//! Jet DISPATCH is the one thing a caller chooses, because it is not part of
//! that setup: it follows which cold state the caller then loads, and it
//! cannot change the noun either way. See `create_eval_context_with`.

use std::env;
use std::panic::{self, AssertUnwindSafe};

use nockapp::utils::{create_context, NOCK_STACK_SIZE_MEDIUM};
use nockvm::ext::NounExt;
use nockvm::interpreter::Context;
use nockvm::jets::cold::{cold_from_noun_resident, Cold};
use nockvm::jets::warm::Warm;
use nockvm::jets::JetDispatchMode;
use nockvm::mem::{AllocationError, NockStack};
use nockvm::noun::Noun;

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
    create_eval_context_with(JetDispatchMode::HintBlind)
}

/// The same context at a caller-chosen dispatch mode.
///
/// The mode follows the COLD STATE, not the semantics. Hint-blind matching
/// exists to bridge an *embedded, pre-serialized* cold state against cores
/// reconstructed natively: honk's compiler cues `honc-cold-138.jam` into
/// this context and then mints cores whose `%spot` hints put them at a
/// different noun than the frozen batteries, so without it those jets never
/// fire. A caller that loads no such asset has nothing to be blind about ---
/// its registrations and its queries come from the same evaluation, which is
/// the condition the mode's own documentation names as not needing it.
///
/// It is a performance choice and not a semantic one: a jet computes what
/// its Nock computes (the runtime's jet-test mode is what enforces that), so
/// neither mode can change the answer.  What it changes is whether every
/// warm MISS pays a formula normalization walk, and that walk is not
/// sharing-aware -- it visits every path to a shared subnoun rather than
/// every subnoun -- so on a formula with real internal sharing it is
/// exponential in the sharing depth.  Measured on jock's 1147-vector
/// corpus, which `kick` evaluates:
///
/// ```text
///              hits        misses       time
///   Exact      31,716,141  165,117,378   66s
///   HintBlind  31,716,141  165,117,378  220s
/// ```
///
/// Byte-identical jet behaviour --- the hint-blind fallback converts not one
/// of those 165 million misses into a hit --- at 3.3x the wall clock.  That
/// is what a mode costs when its premise does not hold.
pub fn create_eval_context_with(dispatch: JetDispatchMode) -> Context {
    let mut stack = NockStack::new(eval_stack_size(), 0);
    let cold = Cold::new(&mut stack);
    create_context(
        stack,
        crate::native::hot::native_hot_state(),
        cold,
        None,
        vec![],
        dispatch,
    )
}

/// Load a serialized cold state into `context`, replacing what it has.
///
/// WHY A CALLER WANTS THIS. A cold state maps a BATTERY to the label path
/// the hot state is keyed by, and `%fast` is the only thing that fills it.
/// An artifact whose cores were built in some *other* process therefore
/// arrives with its batteries unregistered: nothing evaluates here to
/// re-fire those hints, so the hot state is unreachable and the kernel's own
/// arms interpret. That is not a modest slowdown. Hoon's `+add` loops on its
/// LEFT operand and calls `+dec` each turn, and an unjetted `+dec` counts up
/// from zero, so one unjetted addition is quadratic in that operand: swapping
/// the arguments of `add` changed a measured Jock program from 72.64s to
/// 0.56s. Loading the cold state the kernel was frozen with is what makes
/// the hot state reachable at all.
///
/// A HIT IS CORRECT BY CONSTRUCTION, so there is no mismatch to guard
/// against. The map is keyed by the battery NOUN: a match means
/// byte-identical code, and identical code has the same jet. The only
/// failure mode is an ABSENT entry, which costs speed and never an answer.
///
/// `Warm` is rebuilt because it IS the cold-by-hot join --- a cold state on
/// its own dispatches nothing.
pub fn load_cold_state(context: &mut Context, raw: &[u8], label: &str) -> Result<(), String> {
    let cold_noun = <Noun as NounExt>::cue_bytes_slice(&mut context.stack, raw)
        .map_err(|err| format!("cue {label} cold jam: {err:?}"))?;
    let space = context.stack.noun_space();
    //  The noun was cued into this same stack just above, so the resident
    //  decode borrows batteries and paths in place rather than deep-copying
    //  every core a second time.
    let (battery_to_paths, root_to_paths, path_to_batteries) =
        cold_from_noun_resident(&mut context.stack, &cold_noun, &space)
            .map_err(|err| format!("decode {label} cold state: {err:?}"))?;
    context.cold = Cold::from_vecs(
        &mut context.stack,
        battery_to_paths,
        root_to_paths,
        path_to_batteries,
    );
    context.warm = Warm::init(
        &mut context.stack,
        &mut context.cold,
        &context.hot,
        &context.test_jets,
        context.jet_dispatch,
    );
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The dispatch split is deliberate and must stay visible: honk's
    /// compiler cues an embedded cold state and needs hint-blind matching to
    /// find its jets in it, while `kick` loads none and must not pay the
    /// normalization walk. Collapsing these two back into one constructor
    /// would silently hand one caller the other's cost (or, the other
    /// direction, silently drop honk's jets), and neither shows up as a
    /// failure --- only as a slower build or a different noun.
    #[test]
    fn the_default_is_hint_blind_and_the_choice_is_a_choice() {
        assert_eq!(
            create_eval_context().jet_dispatch,
            JetDispatchMode::HintBlind,
            "honk's compiler reads an embedded cold state"
        );
        assert_eq!(
            create_eval_context_with(JetDispatchMode::Exact).jet_dispatch,
            JetDispatchMode::Exact,
            "a caller with no embedded cold state can opt out"
        );
    }
}
