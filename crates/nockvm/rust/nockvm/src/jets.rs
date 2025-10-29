pub mod cold;
pub mod hot;
pub mod warm;

pub mod bits;
pub mod form;
pub mod hash;
pub mod list;
pub mod lock;
pub mod lute;
pub mod math;
pub mod nock;
pub mod parse;
pub mod serial;
pub mod set;
pub mod sort;
pub mod tree;

use cold::FromNounError;
use nockvm_macros::tas;
use util::BAIL_EXIT;

use crate::flog;
use crate::interpreter::{Context, Error, Mote};
use crate::jets::bits::*;
use crate::jets::cold::Cold;
use crate::jets::form::*;
use crate::jets::hash::*;
use crate::jets::hot::{Hot, URBIT_HOT_STATE};
use crate::jets::list::*;
use crate::jets::lock::aes::*;
use crate::jets::lock::ed::*;
use crate::jets::lock::sha::*;
use crate::jets::lute::*;
use crate::jets::math::*;
use crate::jets::nock::*;
use crate::jets::parse::*;
use crate::jets::serial::*;
//use crate::jets::set::*;
use crate::jets::sort::*;
use crate::jets::tree::*;
use crate::jets::warm::Warm;
use crate::mem::{NockStack, Preserve};
use crate::noun::{self, Noun, Slots};

crate::gdb!();

/// Return Err if the computation crashed or should punt to Nock
pub type Result = std::result::Result<Noun, JetErr>;
pub type Jet = fn(&mut Context, Noun) -> Result;

/**
 * Only return a deterministic error if the Nock would have deterministically
 * crashed.
 */
#[derive(Clone, Copy, Debug)]
pub enum JetErr {
    Punt,        // Retry with the raw nock
    Fail(Error), // Error; do not retry
}
impl std::fmt::Display for JetErr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            JetErr::Punt => write!(f, "Punt"),
            JetErr::Fail(err) => write!(f, "Fail({})", err),
        }
    }
}
impl std::error::Error for JetErr {}

impl Preserve for JetErr {
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        match self {
            JetErr::Punt => {}
            JetErr::Fail(ref mut err) => err.preserve(stack),
        }
    }

    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        match self {
            JetErr::Punt => {}
            JetErr::Fail(ref err) => err.assert_in_stack(stack),
        }
    }
}

impl From<Error> for JetErr {
    fn from(err: Error) -> Self {
        Self::Fail(err)
    }
}

// TODO: move this out so that err message can be surfaced
impl From<FromNounError> for JetErr {
    fn from(_err: FromNounError) -> Self {
        BAIL_EXIT
    }
}

impl From<noun::Error> for JetErr {
    fn from(_err: noun::Error) -> Self {
        BAIL_EXIT
    }
}

impl From<JetErr> for () {
    fn from(_: JetErr) -> Self {}
}

impl From<JetErr> for Error {
    fn from(e: JetErr) -> Self {
        match e {
            JetErr::Fail(f) => f,
            JetErr::Punt => panic!("unhandled JetErr::Punt"),
        }
    }
}

pub fn get_jet(context: &mut Context, jet_name: Noun) -> Option<Jet> {
    match jet_name.as_direct().ok()?.data() {
        tas!(b"add") => Some(jet_add),
        tas!(b"dec") => Some(jet_dec),
        tas!(b"div") => Some(jet_div),
        tas!(b"dvr") => Some(jet_dvr),
        tas!(b"gte") => Some(jet_gte),
        tas!(b"gth") => Some(jet_gth),
        tas!(b"lte") => Some(jet_lte),
        tas!(b"lth") => Some(jet_lth),
        tas!(b"mod") => Some(jet_mod),
        tas!(b"mul") => Some(jet_mul),
        tas!(b"sub") => Some(jet_sub),
        //
        tas!(b"cap") => Some(jet_cap),
        tas!(b"mas") => Some(jet_mas),
        //
        tas!(b"flop") => Some(jet_flop),
        tas!(b"lent") => Some(jet_lent),
        //
        tas!(b"bex") => Some(jet_bex),
        tas!(b"can") => Some(jet_can),
        tas!(b"cat") => Some(jet_cat),
        tas!(b"cut") => Some(jet_cut),
        tas!(b"end") => Some(jet_end),
        tas!(b"lsh") => Some(jet_lsh),
        tas!(b"met") => Some(jet_met),
        tas!(b"rap") => Some(jet_rap),
        tas!(b"rep") => Some(jet_rep),
        tas!(b"rev") => Some(jet_rev),
        tas!(b"rip") => Some(jet_rip),
        tas!(b"rsh") => Some(jet_rsh),
        tas!(b"xeb") => Some(jet_xeb),
        //
        tas!(b"con") => Some(jet_con),
        tas!(b"dis") => Some(jet_dis),
        tas!(b"mix") => Some(jet_mix),
        //
        tas!(b"mug") => Some(jet_mug),
        //
        tas!(b"dor") => Some(jet_dor),
        tas!(b"gor") => Some(jet_gor),
        tas!(b"mor") => Some(jet_mor),
        //
        tas!(b"cue") => Some(jet_cue),
        tas!(b"jam") => Some(jet_jam),
        //
        tas!(b"shas") => Some(jet_shas),
        tas!(b"shax") => Some(jet_shax),
        tas!(b"shay") => Some(jet_shay),
        tas!(b"shal") => Some(jet_shal),
        tas!(b"sha1") => Some(jet_sha1),
        //
        tas!(b"scow") => Some(jet_scow),
        //
        tas!(b"mink") => Some(jet_mink),
        //
        tas!(b"puck") => Some(jet_puck),
        tas!(b"shar") => Some(jet_shar),
        tas!(b"sign") => Some(jet_sign),
        tas!(b"veri") => Some(jet_veri),
        //
        tas!(b"siva_en") => Some(jet_siva_en),
        tas!(b"siva_de") => Some(jet_siva_de),
        tas!(b"sivb_en") => Some(jet_sivb_en),
        tas!(b"sivb_de") => Some(jet_sivb_de),
        tas!(b"sivc_en") => Some(jet_sivc_en),
        tas!(b"sivc_de") => Some(jet_sivc_de),
        //
        _ => {
            flog!(context, "unknown jet: {:?}", jet_name);
            None
        }
    }
}

pub fn get_jet_test_mode(_jet_name: Noun) -> bool {
    /*
    match jet_name.as_direct().unwrap_or_else(|| panic!("Panicked at {}:{} (git sha: {:?})", file!(), line!(), option_env!("GIT_SHA"))).data() {
        tas!(b"cut") => true,
        _ => false,
    }
    */
    false
}

pub mod util {
    use std::result;

    use bitvec::prelude::{BitSlice, Lsb0};

    use super::*;
    use crate::interpreter::interpret;
    use crate::noun::{Noun, D, T};

    pub const BAIL_EXIT: JetErr = JetErr::Fail(Error::Deterministic(Mote::Exit, D(0)));
    pub const BAIL_FAIL: JetErr = JetErr::Fail(Error::NonDeterministic(Mote::Fail, D(0)));

    /**
     * Address-based size checks.
     * Currently, only addresses indexable by the first 48 bits are reachable by
     * modern 64-bit CPUs.
     */
    const MAX_BIT_LENGTH: usize = (1 << 47) - 1;

    /// Performs addition that returns None on Noun size overflow
    pub fn checked_add(a: usize, b: usize) -> result::Result<usize, JetErr> {
        a.checked_add(b)
            .filter(|x| x <= &MAX_BIT_LENGTH)
            .ok_or(BAIL_FAIL)
    }

    /// Performs subtraction that returns None on Noun size overflow
    pub fn checked_sub(a: usize, b: usize) -> result::Result<usize, JetErr> {
        a.checked_sub(b).ok_or(BAIL_FAIL)
    }

    pub fn checked_left_shift(bloq: usize, a: usize) -> result::Result<usize, JetErr> {
        let res = a << bloq;

        // Catch overflow
        if (res >> bloq) < a || res > MAX_BIT_LENGTH {
            Err(BAIL_FAIL)
        } else {
            Ok(res)
        }
    }

    /// Convert length in bits to length in 64-bit words
    pub fn bits_to_word(a: usize) -> result::Result<usize, JetErr> {
        checked_add(a, 63).map(|x| x >> 6)
    }

    /// Convert length as bite to length in 64-bit words
    pub fn bite_to_word(bloq: usize, step: usize) -> result::Result<usize, JetErr> {
        bits_to_word(checked_left_shift(bloq, step)?)
    }

    pub fn slot(noun: Noun, axis: u64) -> Result {
        noun.slot(axis).map_err(|_e| BAIL_EXIT)
    }

    /// Extract a bloq and check that it's computable by the current system
    pub fn bloq(a: Noun) -> result::Result<usize, JetErr> {
        let bloq = a.as_direct()?.data() as usize;
        if bloq >= 47 {
            Err(BAIL_FAIL)
        } else {
            Ok(bloq)
        }
    }

    /// Extract the bloq and step from a bite
    pub fn bite(a: Noun) -> result::Result<(usize, usize), JetErr> {
        if let Ok(cell) = a.as_cell() {
            let bloq = bloq(cell.head())?;
            let step = cell.tail().as_direct()?.data() as usize;
            Ok((bloq, step))
        } else {
            bloq(a).map(|x| (x, 1_usize))
        }
    }

    /** In a bloq space, copy from `from` for a span of `step`, to position `to`.
     *
     * Note: unlike the vere version, this sets the bits instead of XORing
     * them.  If we need the XOR version, we could use ^=.
     */
    pub unsafe fn chop(
        bloq: usize,
        from: usize,
        step: usize,
        to: usize,
        dest: &mut BitSlice<u64, Lsb0>,
        source: &BitSlice<u64, Lsb0>,
    ) -> result::Result<(), JetErr> {
        let from_b = checked_left_shift(bloq, from)?;
        let to_b = checked_left_shift(bloq, to)?;
        let mut step_b = checked_left_shift(bloq, step)?;
        let end_b = checked_add(from_b, step_b)?;

        if from_b >= source.len() {
            return Ok(());
        }

        if end_b > source.len() {
            step_b -= end_b - source.len();
        }

        dest[to_b..to_b + step_b].copy_from_bitslice(&source[from_b..from_b + step_b]);
        Ok(())
    }

    pub fn kick(context: &mut Context, core: Noun, axis: Noun) -> result::Result<Noun, JetErr> {
        let formula: Noun = T(&mut context.stack, &[D(9), axis, D(0), D(1)]);
        interpret(context, core, formula).map_err(JetErr::Fail)
    }

    pub fn slam(context: &mut Context, gate: Noun, sample: Noun) -> result::Result<Noun, JetErr> {
        let core: Noun = T(
            &mut context.stack,
            &[gate.as_cell()?.head(), sample, gate.as_cell()?.tail().as_cell()?.tail()],
        );
        kick(context, core, D(2))
    }

    pub mod test {
        use std::sync::atomic::AtomicIsize;
        use std::sync::Arc;

        use ibig::UBig;

        use super::*;
        use crate::hamt::Hamt;
        use crate::interpreter::{NockCancelToken, Slogger};
        use crate::mem::NockStack;
        use crate::noun::{Atom, Noun, D, T};
        use crate::unifying_equality::unifying_equality;

        struct TestSlogger {}

        impl Slogger for TestSlogger {
            fn slog(&mut self, _stack: &mut NockStack, _pri: u64, _noun: Noun) {
                eprintln!("Jet slogged.");
            }

            fn flog(&mut self, _stack: &mut NockStack, _cord: Noun) {
                eprintln!("Jet flogged.");
            }
        }

        pub fn init_context() -> Context {
            let mut stack = NockStack::new(8 << 10 << 10, 0);
            let cold = Cold::new(&mut stack);
            let warm = Warm::new(&mut stack);
            let hot = Hot::init(&mut stack, URBIT_HOT_STATE);
            let cache = Hamt::<Noun>::new(&mut stack);
            let slogger = std::boxed::Box::pin(TestSlogger {});
            let cancel = Arc::new(AtomicIsize::new(NockCancelToken::RUNNING_IDLE));
            let test_jets = Hamt::<()>::new(&mut stack);

            Context {
                stack,
                slogger,
                cold,
                warm,
                hot,
                cache,
                scry_stack: D(0),
                trace_info: None,
                running_status: cancel,
                test_jets,
            }
        }

        #[allow(non_snake_case)]
        pub fn A(stack: &mut NockStack, ubig: &UBig) -> Noun {
            Atom::from_ubig(stack, ubig).as_noun()
        }

        pub fn assert_noun_eq(stack: &mut NockStack, mut a: Noun, mut b: Noun) {
            let eq = unsafe { unifying_equality(stack, &mut a, &mut b) };
            assert!(eq, "got: {a:?}, need: {b:?}");
        }

        pub fn assert_jet(context: &mut Context, jet: Jet, sam: Noun, res: Noun) {
            assert_jet_door(context, jet, sam, D(0), res)
        }

        pub fn assert_jet_door(context: &mut Context, jet: Jet, sam: Noun, pay: Noun, res: Noun) {
            let sam = T(&mut context.stack, &[D(0), sam, pay]);
            // FIXME: This assert_no_alloc was failing the tests, but we allocate in jets
            // all of the time, why was assert_no_alloc wrapped around the jet?
            // let jet_res = assert_no_alloc(|| jet(context, sam).unwrap_or_else(|| panic!("Panicked at {}:{} (git sha: {:?})", file!(), line!(), option_env!("GIT_SHA"))));
            let jet_res = jet(context, sam).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            assert_noun_eq(&mut context.stack, jet_res, res);
        }

        pub fn assert_jet_ubig(context: &mut Context, jet: Jet, sam: Noun, res: UBig) {
            let res = A(&mut context.stack, &res);
            assert_jet(context, jet, sam, res);
        }

        pub fn assert_nary_jet_ubig(context: &mut Context, jet: Jet, sam: &[Noun], res: UBig) {
            let sam = T(&mut context.stack, sam);
            assert_jet_ubig(context, jet, sam, res);
        }

        pub fn assert_jet_err(context: &mut Context, jet: Jet, sam: Noun, err: JetErr) {
            let sam = T(&mut context.stack, &[D(0), sam, D(0)]);
            let jet_res = jet(context, sam);
            assert!(
                jet_res.is_err(),
                "with sample: {:?}, expected err: {:?}, got: {:?}",
                sam,
                err,
                &jet_res
            );
            let jet_err = jet_res.expect_err("expected error in assert_jet_err");
            match (jet_err, err) {
                (JetErr::Punt, JetErr::Punt) => {}
                (JetErr::Fail(actual_err), JetErr::Fail(expected_err)) => {
                    match (actual_err, expected_err) {
                        (Error::ScryBlocked(mut actual), Error::ScryBlocked(mut expected))
                        | (Error::ScryCrashed(mut actual), Error::ScryCrashed(mut expected))
                        | (
                            Error::Deterministic(_, mut actual),
                            Error::Deterministic(_, mut expected),
                        )
                        | (
                            Error::NonDeterministic(_, mut actual),
                            Error::NonDeterministic(_, mut expected),
                        ) => unsafe {
                            assert!(unifying_equality(
                                &mut context.stack, &mut actual, &mut expected
                            ));
                        },
                        _ => {
                            panic!(
                                "with sample: {sam:?}, expected err: {expected_err:?}, got: {actual_err:?}"
                            );
                        }
                    }
                }
                _ => {
                    panic!("with sample: {sam:?}, expected err: {err:?}, got: {jet_err:?}");
                }
            }
        }

        pub fn assert_jet_size(context: &mut Context, jet: Jet, sam: Noun, siz: usize) {
            let sam = T(&mut context.stack, &[D(0), sam, D(0)]);
            let res = jet(context, sam).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            assert!(res.is_atom(), "jet result not atom");
            let res_siz = res
                .atom()
                .unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                })
                .size();
            assert!(siz == res_siz, "got: {res_siz}, need: {siz}");
        }

        pub fn assert_common_jet(
            context: &mut Context,
            jet: Jet,
            sam: &[fn(&mut NockStack) -> Noun],
            res: UBig,
        ) {
            let sam: Vec<Noun> = sam.iter().map(|f| f(&mut context.stack)).collect();
            assert_nary_jet_ubig(context, jet, &sam, res);
        }

        pub fn assert_common_jet_noun(
            context: &mut Context,
            jet: Jet,
            sam: &[fn(&mut NockStack) -> Noun],
            res: Noun,
        ) {
            let sam: Vec<Noun> = sam.iter().map(|f| f(&mut context.stack)).collect();
            let sam = T(&mut context.stack, &sam);
            assert_jet(context, jet, sam, res);
        }

        pub fn assert_common_jet_err(
            context: &mut Context,
            jet: Jet,
            sam: &[fn(&mut NockStack) -> Noun],
            err: JetErr,
        ) {
            let sam: Vec<Noun> = sam.iter().map(|f| f(&mut context.stack)).collect();
            let sam = T(&mut context.stack, &sam);
            assert_jet_err(context, jet, sam, err);
        }

        pub fn assert_common_jet_size(
            context: &mut Context,
            jet: Jet,
            sam: &[fn(&mut NockStack) -> Noun],
            siz: usize,
        ) {
            let sam: Vec<Noun> = sam.iter().map(|f| f(&mut context.stack)).collect();
            let sam = T(&mut context.stack, &sam);
            assert_jet_size(context, jet, sam, siz)
        }
    }
}
