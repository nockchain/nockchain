//! kick: fire a honk/hoonc build trap and jam the product.
//!
//! `honk --arbitrary` emits a deferred vase trap; nothing downstream of the
//! compiler could previously fire it, because the Python and vere
//! interpreters lack the formula-keyed jets the deferred build relies on
//! (unjetted `+ut` work is the cold-hoonc regime). kick closes that loop by
//! reusing honk's own evaluation environment — see `honk::eval` — to cue the
//! jam, interpret `[9 2 0 1]`, and jam the result (optionally the subnoun at
//! an axis, e.g. 3 for a vase tail).
//!
//! First consumer: the Jock corpus harness (sigilante/jock tools/vex.sh),
//! where a 648KB build trap kicks in 0.14s.
//!
//! Build & install:
//!
//!     cargo build --release -p honk --bin kick
//!     cp target/release/kick ~/.cargo/bin/honk-kick
//!
//! Example (value of an --arbitrary build; axis 3 for the vase tail):
//!
//!     honk --new --arbitrary --output out.jam --prelude hoon-138.hoon entry.hoon deps/
//!     honk-kick out.jam value.jam

use std::path::PathBuf;
use std::time::Duration;
use std::{env, fs, process, thread};

use honk::eval::{catch_nock_panic, create_eval_context_with, load_cold_state};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockvm::ext::{AtomExt, NounExt};
use nockvm::interpreter::{interpret, Context};
use nockvm::jets::JetDispatchMode;
use nockvm::noun::{Atom, Noun, D, T};
use num_bigint::BigUint;

/// Bad arguments. Matches honk's own usage-error code.
const EXIT_USAGE: i32 = 2;
/// The trap could not be read, cued, fired, or written.
const EXIT_FAILURE: i32 = 1;
/// `--timeout` elapsed. Matches timeout(1), which build systems already read.
const EXIT_TIMEOUT: i32 = 124;

const USAGE: &str = "\
usage: kick <trap.jam> <out.jam> [axis] [--timeout <seconds>]

Fires the trap in <trap.jam> ([9 2 0 1]) under honk's evaluation
environment and writes the jam of the product to <out.jam>.

  <trap.jam>            a jammed trap: a core whose arm at axis 2 is fired
  <out.jam>             where to write the jammed product
  [axis]                write the subnoun at this axis of the product
                        instead of the whole product. Arbitrary precision;
                        the product itself is axis 1, and 0 is not an axis.
  --timeout <seconds>   give up after <seconds> and exit 124
  -h, --help            print this message

exit: 0 success, 1 failure, 2 bad arguments, 124 timed out
";

struct Args {
    input: PathBuf,
    output: PathBuf,
    /// An axis is a path through a noun and has no width bound, so it is
    /// carried as a bignum rather than parsed into a `u64`: a deep artifact
    /// can legitimately name an axis past 2^64. It becomes an atom only once
    /// there is a stack to intern it on.
    axis: Option<BigUint>,
    timeout: Option<Duration>,
}

fn parse_args(raw: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut timeout: Option<Duration> = None;
    let mut args = raw.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "-t" | "--timeout" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} wants a number of seconds"))?;
                let seconds: f64 = value
                    .parse()
                    .map_err(|_| format!("{arg}: {value:?} is not a number of seconds"))?;
                if !(seconds.is_finite() && seconds > 0.0) {
                    return Err(format!("{arg}: {value:?} is not a positive duration"));
                }
                timeout = Some(Duration::from_secs_f64(seconds));
            }
            flag if flag.starts_with('-') && flag != "-" => {
                return Err(format!("unknown flag {flag:?}"));
            }
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    let input = positional.next().ok_or("missing <trap.jam>")?;
    let output = positional.next().ok_or("missing <out.jam>")?;
    let axis = positional.next().map(parse_axis).transpose()?;
    if let Some(extra) = positional.next() {
        return Err(format!("unexpected argument {extra:?}"));
    }
    Ok(Some(Args {
        input: PathBuf::from(input),
        output: PathBuf::from(output),
        axis,
        timeout,
    }))
}

/// Read an axis argument.
///
/// Zero is rejected here rather than left to `slot_atom`, both for the
/// diagnostic and because it is the one axis a bit-walking descent silently
/// mistakes for the root.
fn parse_axis(raw: String) -> Result<BigUint, String> {
    let axis = BigUint::parse_bytes(raw.as_bytes(), 10)
        .ok_or_else(|| format!("axis: {raw:?} is not a decimal number"))?;
    if axis == BigUint::ZERO {
        return Err("axis: 0 is not an axis; the whole product is axis 1".to_string());
    }
    Ok(axis)
}

/// Cue the trap, fire it, and jam the requested part of the product.
fn fire(
    context: &mut Context,
    jam: &[u8],
    label: &str,
    axis: Option<&BigUint>,
) -> Result<Vec<u8>, String> {
    // Cued outside the eval frame so the trap outlives the interpretation.
    let trap = <Noun as NounExt>::cue_bytes_slice(&mut context.stack, jam)
        .map_err(|err| format!("{label} is not a valid jam: {err:?}"))?;
    let product = unsafe {
        context.with_stack_frame(0, |context: &mut Context| {
            let kick = T(&mut context.stack, &[D(9), D(2), D(0), D(1)]);
            interpret(context, trap, kick)
        })
    }
    .map_err(|err| format!("{label} crashed: {err:?}"))?;

    let value = match axis {
        None => product,
        Some(axis) => {
            let atom = <Atom as AtomExt>::from_bytes(&mut context.stack, &axis.to_bytes_le());
            let space = context.stack.noun_space();
            product
                .in_space(&space)
                .slot_atom(atom)
                .map_err(|err| format!("axis {axis} of the product: {err:?}"))?
                .noun()
        }
    };

    let space = context.stack.noun_space();
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let root = slab.copy_into(value, &space);
    slab.set_root(root);
    Ok(slab.jam().to_vec())
}

fn run(args: &Args) -> Result<(), String> {
    let label = args.input.display().to_string();
    let jam = fs::read(&args.input).map_err(|err| format!("cannot read {label}: {err}"))?;
    //  `Exact` dispatch, not honk's `HintBlind`: hint-blind matching exists
    //  to bridge an embedded cold state against cores a caller reconstructs
    //  NATIVELY, and kick reconstructs nothing --- it fires a trap built in
    //  honk's process, so the batteries it queries are the ones honk froze,
    //  matchable as they stand.  The walk that mode pays on every warm miss
    //  is not free:  3.3x the wall clock on jock's corpus for byte-identical
    //  jet behaviour.  See create_eval_context_with.
    let mut context = create_eval_context_with(JetDispatchMode::Exact);
    load_canonical_cold_state(&mut context)?;
    let bytes = catch_nock_panic(format!("firing {label}"), || {
        fire(&mut context, &jam, &label, args.axis.as_ref())
    })?;
    fs::write(&args.output, &bytes)
        .map_err(|err| format!("cannot write {}: {err}", args.output.display()))
}

/// Register the kernel's batteries, so the hot state is reachable at all.
///
/// kick fires a PREBUILT trap. Its cores were minted in honk's process, not
/// this one, so no `%fast` hint fires here, and over an empty cold state the
/// hot state is unreachable: every kernel arm interprets, `+add` and `+dec`
/// included. That is the difference between 0.56s and 72.64s on a measured
/// Jock program, because an unjetted `+add` is quadratic in its left operand
/// (`honk::eval::load_cold_state` has the mechanism).
///
/// honk freezes the cold state of its canonical hoon-138 build into
/// `honc-cold-138.jam` for precisely this reason. kick loads the same asset,
/// and a hit is correct by construction --- the map is keyed by the battery
/// noun, so a match is identical code.
///
/// `KICK_NO_COLD_STATE` skips it, which is how the unjetted cost above is
/// measured rather than guessed.
fn load_canonical_cold_state(context: &mut Context) -> Result<(), String> {
    if env::var_os("KICK_NO_COLD_STATE").is_some() {
        return Ok(());
    }
    if honc_cold_138::HONC_COLD_138_JAM.is_empty() {
        //  the bootstrap placeholder stands in when the asset was never
        //  built;  say so, because the symptom is otherwise just slowness
        eprintln!(
            "kick: no embedded cold state, so every kernel arm will \
             interpret (build the honk assets to populate it)"
        );
        return Ok(());
    }
    load_cold_state(context, honc_cold_138::HONC_COLD_138_JAM, "canonical honc")
}

/// Abandon the process once `limit` has elapsed.
///
/// The interpreter has no interrupt, so a runaway trap cannot be unwound;
/// for a one-shot CLI, exiting is the honest way to bound it. Nothing is
/// lost by skipping destructors — the output file is written in one call at
/// the very end, and the NockStack is an anonymous mapping the kernel
/// reclaims.
fn spawn_watchdog(limit: Duration, label: String) {
    thread::spawn(move || {
        thread::sleep(limit);
        eprintln!(
            "kick: timed out after {}s firing {label}",
            limit.as_secs_f64()
        );
        process::exit(EXIT_TIMEOUT);
    });
}

fn main() {
    let args = match parse_args(env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return;
        }
        Err(err) => {
            eprintln!("kick: {err}");
            eprint!("{USAGE}");
            process::exit(EXIT_USAGE);
        }
    };
    if let Some(limit) = args.timeout {
        spawn_watchdog(limit, args.input.display().to_string());
    }
    if let Err(err) = run(&args) {
        eprintln!("kick: {err}");
        process::exit(EXIT_FAILURE);
    }
}
