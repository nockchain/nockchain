/** Call site of a kick (Nock 9), used to cache call targets. */
use bitvec::order::Lsb0;
use bitvec::slice::BitSlice;

use crate::interpreter::{interpret, Context, Mote};
use crate::jets::util::slot;
use crate::jets::{Jet, JetErr};
use crate::noun::{Noun, D, T};
use crate::unifying_equality::unifying_equality;

/// Return Err if the computation crashed or should punt to Nock
pub(crate) type Result = std::result::Result<Noun, JetErr>;

pub struct Site {
    pub(crate) battery: Noun,            // battery
    pub(crate) context: Noun,            // context
    pub(crate) jet: Option<(Jet, bool)>, // jet driver
    pub(crate) path: Noun,               // label
}

impl Site {
    /// Prepare a locally cached gate to call repeatedly.
    pub fn new(ctx: &mut Context, core: &mut Noun) -> Site {
        let mut battery = slot(*core, 2).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let context = slot(*core, 7).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });

        let mut warm_result = ctx
            .warm
            .find_jet(&mut ctx.stack, core, &mut battery)
            .filter(|(_jet, mut path, _test)| {
                // check that 7 is a prefix of the parent battery axis,
                // to ensure that the sample (axis 6) is not part of the jet match.
                //
                // XX TODO this check is pessimized since there could be multiple ways to match the
                // jet and we only actually match one of them, but we check all of them and run
                // unjetted if any have an axis outside 7.
                let axis_7_bits: &BitSlice<u64, Lsb0> = BitSlice::from_element(&7u64);
                let batteries_list = ctx.cold.find(&mut ctx.stack, &mut path);
                let mut ret = true;
                for mut batteries in batteries_list {
                    if let Some((_battery, parent_axis)) = batteries.next() {
                        let parent_axis_prefix_bits = &parent_axis.as_bitslice()[0..3];
                        if parent_axis_prefix_bits == axis_7_bits {
                            continue;
                        } else {
                            ret = false;
                            break;
                        }
                    } else {
                        ret = false;
                        break;
                    }
                }
                ret
            });
        let warm_result_opt = warm_result.next();
        Site {
            battery,
            context,
            jet: warm_result_opt.map(|(jet, _, test)| (jet, test)),
            path: warm_result_opt.map(|(_, path, _)| path).unwrap_or(D(0)),
        }
    }
}

/// Slam a cached call site.
pub fn site_slam(ctx: &mut Context, site: &Site, sample: Noun) -> Result {
    let subject = T(&mut ctx.stack, &[site.battery, sample, site.context]);
    // TODO run test if necessary
    if let Some((jet, test)) = site.jet {
        let jet_res = jet(ctx, subject);
        match jet_res {
            Ok(mut jet_res) => {
                if test {
                    let mut test_res = interpret(ctx, subject, site.battery)?;
                    if unsafe { !unifying_equality(&mut ctx.stack, &mut test_res, &mut jet_res) } {
                        return Err(JetErr::Fail(crate::interpreter::Error::NonDeterministic(
                            Mote::Jest,
                            D(0),
                        )));
                    }
                }
                Ok(jet_res)
            }
            Err(err) => Err(err),
        }
    } else {
        Ok(interpret(ctx, subject, site.battery)?)
    }
}
