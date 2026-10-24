use crate::form::belt::{Belt, FieldError};
use crate::form::bpoly::{bp_fft, bpoly_zero_extend};
use crate::form::mary::{snag_as_bpoly, MarySlice};

pub fn precompute_ntts(
    polys: MarySlice,
    height: usize,
    max_ntt_len: usize,
    res: &mut [Belt],
) -> Result<(), FieldError> {
    let new_len = height * max_ntt_len;

    for i in 0..polys.len as usize {
        let bp = snag_as_bpoly(polys, i);
        let mut extended = vec![Belt::zero(); new_len];
        bpoly_zero_extend(bp, &mut extended);
        let fft = bp_fft(&extended)?;
        res[i * new_len..(i + 1) * new_len].copy_from_slice(&fft);
    }
    Ok(())
}
