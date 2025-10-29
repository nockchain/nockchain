use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, IndirectAtom, Noun};

use crate::based;
use crate::form::belt::*;
use crate::form::felt::Felt;
use crate::form::fpoly::fp_ntt;
use crate::form::handle::{finalize_poly, new_handle_mut_slice};
use crate::form::noun_ext::NounMathExt;
use crate::form::poly::*;
use crate::utils::hoon_list_to_vecbelt;

const DEG: u64 = 3; // field extension degree

// frep: inverse of frip; list of belts are rep'd to a felt
fn frep(x: Vec<Belt>) -> Result<Felt, JetErr> {
    assert_eq!(x.len() as u64, DEG);
    x.iter().for_each(|b| based!(b.0));
    Ok(felt_from_u64s(x[0].0, x[1].0, x[2].0))
}

// build felt from 3 given u64s
pub fn felt_from_u64s(x0: u64, x1: u64, x2: u64) -> Felt {
    let data: [u64; 3] = [x0, x1, x2];
    Felt::from(data)
}

// create a noun of a felt
pub fn felt_as_noun(context: &mut Context, felt: Felt) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;

    // Build the result using stack-aware operations
    // Use from_unsigned_stack to avoid allocation through global allocator
    let part0 = UBig::from_unsigned_stack(stack, felt[0].0);

    let val1 = UBig::from_unsigned_stack(stack, felt[1].0);
    let part1 = UBig::shl_stack(stack, val1, 64);

    let val2 = UBig::from_unsigned_stack(stack, felt[2].0);
    let part2 = UBig::shl_stack(stack, val2, 128);

    let val3 = UBig::from_unsigned_stack(stack, 1u64);
    let part3 = UBig::shl_stack(stack, val3, 192);

    let res1 = UBig::bitor_stack(stack, part0, part1);
    let res2 = UBig::bitor_stack(stack, res1, part2);
    let res_big = UBig::bitor_stack(stack, res2, part3);

    Ok(Atom::from_ubig(stack, &res_big).as_noun())
}

// frep_jet
pub fn frep_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sample = slot(subject, 6)?;
    let x = hoon_list_to_vecbelt(sample)?;
    let felt = frep(x)?;
    felt_as_noun(context, felt)
}

pub fn fp_ntt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sample = slot(subject, 6)?;
    let [fp_noun, root_noun] = sample.uncell()?;

    let (Ok(fp), Ok(root)) = (FPolySlice::try_from(fp_noun), root_noun.as_felt()) else {
        return Err(BAIL_FAIL);
    };

    let returned_fpoly = fp_ntt(fp.0, root);
    let (res_atom, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_fpoly.len() as usize));
    res_poly.copy_from_slice(&returned_fpoly[..]);

    let res_cell: Noun = finalize_poly(&mut context.stack, Some(res_poly.len()), res_atom);
    Ok(res_cell)
}

#[cfg(test)]
mod tests {
    use nockvm::jets::util::test::*;
    use nockvm::noun::{D, T};

    use super::*;

    #[test]
    fn test_frep_jet() {
        let c = &mut init_context();

        // > (frep.two ~[1 2 3])
        // 0x1.0000.0000.0000.0003.0000.0000.0000.0002.0000.0000.0000.0001
        let sam = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let res = felt_as_noun(c, felt_from_u64s(1, 2, 3)).expect("felt_as_noun");
        assert_jet(c, frep_jet, sam, res);

        // > (frep.two ~[154.432.865.123.134.542 252.542.541.761.653.234 354.345.546.134.763.356])
        // 0x1.04ea.e365.951a.b75c.0381.361a.8c60.a9f2.0224.a7df.634f.6c4e
        let sam = T(
            &mut c.stack,
            &[D(154432865123134542), D(252542541761653234), D(354345546134763356), D(0)],
        );
        let res = felt_as_noun(
            c,
            felt_from_u64s(0x0224a7df634f6c4e, 0x0381361a8c60a9f2, 0x04eae365951ab75c),
        )
        .expect("felt_as_noun");
        assert_jet(c, frep_jet, sam, res);
    }
}
