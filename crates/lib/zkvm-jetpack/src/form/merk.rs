use nockchain_math::belt::Belt;
use nockchain_math::mary::{Mary, MarySlice, MarySliceMut};
use nockchain_math::poly::{BPolyVec, FPolyVec};
use nockchain_math::tip5::hash::{hash_belts_slice, hash_ten_cell as tip5_hash_ten_cell};
use nockchain_math::tip5::{DIGEST_LENGTH, RATE};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MerkHeapError {
    Empty,
    InvalidStep,
    InvalidOutput,
    NonPowerOfTwoLen,
}

#[derive(Clone, Debug)]
pub struct MerkData {
    pub leaf: [u64; DIGEST_LENGTH],
    pub axis: u64,
    pub root: [u64; DIGEST_LENGTH],
    pub path: Vec<[u64; DIGEST_LENGTH]>,
}

fn belts_to_u64s(values: &[Belt]) -> Vec<u64> {
    values.iter().map(|value| value.0).collect()
}

fn felt_to_u64s(values: &[nockchain_math::felt::Felt]) -> Vec<u64> {
    values
        .iter()
        .flat_map(|value| value.0.map(|belt| belt.0))
        .collect()
}

fn hash_u64(value: u64) -> [u64; DIGEST_LENGTH] {
    hash_belts_slice(&[1, value])
}

fn hash_ten_cell(input: [u64; RATE]) -> [u64; DIGEST_LENGTH] {
    tip5_hash_ten_cell(input)
}

fn hash_belts_list(input: &[u64]) -> [u64; DIGEST_LENGTH] {
    hash_belts_slice(input)
}

pub fn hash_mary(mary: MarySlice<'_>) -> [u64; DIGEST_LENGTH] {
    let step_hash = hash_u64(mary.step as u64);
    let len_hash = hash_u64(mary.len as u64);
    let dat_hash = hash_belts_list(mary.dat);

    let mut len_dat_ten_cell = [0u64; RATE];
    len_dat_ten_cell[..DIGEST_LENGTH].copy_from_slice(&len_hash);
    len_dat_ten_cell[DIGEST_LENGTH..].copy_from_slice(&dat_hash);
    let len_dat_hash = hash_ten_cell(len_dat_ten_cell);

    let mut step_len_ten_cell = [0u64; RATE];
    step_len_ten_cell[..DIGEST_LENGTH].copy_from_slice(&step_hash);
    step_len_ten_cell[DIGEST_LENGTH..].copy_from_slice(&len_dat_hash);
    hash_ten_cell(step_len_ten_cell)
}

pub fn merk_heap_size(len: u32) -> Result<u32, MerkHeapError> {
    if len == 0 {
        return Err(MerkHeapError::Empty);
    }
    if !len.is_power_of_two() {
        return Err(MerkHeapError::NonPowerOfTwoLen);
    }
    len.checked_mul(2)
        .and_then(|value| value.checked_sub(1))
        .ok_or(MerkHeapError::InvalidOutput)
}

fn validate_heap_output(res: &MarySliceMut<'_>, total_size: u32) -> Result<(), MerkHeapError> {
    if res.step != DIGEST_LENGTH as u32
        || res.len != total_size
        || res.dat.len() < total_size as usize * DIGEST_LENGTH
    {
        return Err(MerkHeapError::InvalidOutput);
    }
    Ok(())
}

fn build_digest_heap<F>(
    mary: &MarySlice<'_>,
    res: &mut MarySliceMut<'_>,
    mut leaf_hash: F,
) -> Result<(), MerkHeapError>
where
    F: FnMut(&[u64]) -> [u64; DIGEST_LENGTH],
{
    let total_size = merk_heap_size(mary.len)?;
    validate_heap_output(res, total_size)?;

    let leaves = &mut res.dat[((total_size - mary.len) * DIGEST_LENGTH as u32) as usize..];
    mary.dat
        .chunks(mary.step as usize)
        .zip(leaves.chunks_mut(DIGEST_LENGTH))
        .for_each(|(leaf, out)| out.copy_from_slice(&leaf_hash(leaf)));

    let mut size = mary.len;
    let mut split = total_size - size;
    while size != 0 {
        let (mut left, mut right) = res.dat.split_at_mut(split as usize * DIGEST_LENGTH);
        right = &mut right[..size as usize * DIGEST_LENGTH];

        size >>= 1;
        left = &mut left[(split - size) as usize * DIGEST_LENGTH..];

        right
            .chunks(RATE)
            .zip(left.chunks_mut(DIGEST_LENGTH))
            .for_each(|(pair, out)| {
                let pair: [u64; RATE] = pair
                    .try_into()
                    .expect("power-of-two digest layers produce digest pairs");
                out.copy_from_slice(&hash_ten_cell(pair));
            });
        split -= size;
    }

    Ok(())
}

pub fn build_merk_heap(
    mary: &MarySlice<'_>,
    res: &mut MarySliceMut<'_>,
) -> Result<(), MerkHeapError> {
    if mary.step == 0 || !mary.step.is_multiple_of(3) {
        return Err(MerkHeapError::InvalidStep);
    }

    build_digest_heap(mary, res, |leaf| {
        let leaf_mary = Mary {
            step: 3,
            len: mary.step / 3,
            dat: leaf.to_vec(),
        };
        hash_mary(leaf_mary.as_slice())
    })
}

pub fn bp_build_merk_heap(
    mary: &MarySlice<'_>,
    res: &mut MarySliceMut<'_>,
) -> Result<(), MerkHeapError> {
    if mary.step == 0 {
        return Err(MerkHeapError::InvalidStep);
    }

    build_digest_heap(mary, res, |leaf| {
        let leaf_mary = Mary {
            step: 1,
            len: mary.step,
            dat: leaf.to_vec(),
        };
        hash_mary(leaf_mary.as_slice())
    })
}

pub fn hash_bpoly_leaf(bpoly: &BPolyVec) -> [u64; DIGEST_LENGTH] {
    let belts = belts_to_u64s(bpoly.as_slice());
    let mary = MarySlice {
        step: 1,
        len: bpoly.0.len() as u32,
        dat: &belts,
    };
    hash_mary(mary)
}

pub fn hash_fpoly_leaf(fpoly: &FPolyVec) -> [u64; DIGEST_LENGTH] {
    let belts = felt_to_u64s(fpoly.as_slice());
    let mary = MarySlice {
        step: 3,
        len: fpoly.0.len() as u32,
        dat: &belts,
    };
    hash_mary(mary)
}

pub fn verify_merk_proof(
    leaf: [u64; DIGEST_LENGTH],
    axis: u64,
    root: [u64; DIGEST_LENGTH],
    path: &[[u64; DIGEST_LENGTH]],
) -> bool {
    if axis == 0 {
        return false;
    }

    let mut current = leaf;
    let mut axis = axis;
    let mut iter = path.iter();
    loop {
        if axis == 1 {
            return iter.next().is_none() && current == root;
        }

        let sibling = match iter.next() {
            Some(sibling) => *sibling,
            None => return false,
        };
        let mut input = [0u64; RATE];
        if axis.is_multiple_of(2) {
            input[..DIGEST_LENGTH].copy_from_slice(&current);
            input[DIGEST_LENGTH..].copy_from_slice(&sibling);
            axis /= 2;
        } else {
            input[..DIGEST_LENGTH].copy_from_slice(&sibling);
            input[DIGEST_LENGTH..].copy_from_slice(&current);
            axis = (axis - 1) / 2;
        }
        current = hash_ten_cell(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_merk_heap_matches_manual_two_leaf_root() {
        let dat = (0..12).collect::<Vec<_>>();
        let mary = MarySlice {
            step: 6,
            len: 2,
            dat: &dat,
        };
        let total = merk_heap_size(mary.len).expect("valid heap size");
        let mut res_dat = vec![0; total as usize * DIGEST_LENGTH];
        let mut res = MarySliceMut {
            step: DIGEST_LENGTH as u32,
            len: total,
            dat: &mut res_dat,
        };

        build_merk_heap(&mary, &mut res).expect("heap builds");

        let left = hash_mary(
            Mary {
                step: 3,
                len: 2,
                dat: dat[0..6].to_vec(),
            }
            .as_slice(),
        );
        let right = hash_mary(
            Mary {
                step: 3,
                len: 2,
                dat: dat[6..12].to_vec(),
            }
            .as_slice(),
        );
        let mut pair = [0; RATE];
        pair[..DIGEST_LENGTH].copy_from_slice(&left);
        pair[DIGEST_LENGTH..].copy_from_slice(&right);

        assert_eq!(res.dat[0..DIGEST_LENGTH], hash_ten_cell(pair));
        assert_eq!(res.dat[DIGEST_LENGTH..DIGEST_LENGTH * 2], left);
        assert_eq!(res.dat[DIGEST_LENGTH * 2..DIGEST_LENGTH * 3], right);
    }

    #[test]
    fn bp_build_merk_heap_rejects_non_power_of_two_len() {
        let dat = (0..6).collect::<Vec<_>>();
        let mary = MarySlice {
            step: 2,
            len: 3,
            dat: &dat,
        };
        let mut res_dat = vec![0; 7 * DIGEST_LENGTH];
        let mut res = MarySliceMut {
            step: DIGEST_LENGTH as u32,
            len: 7,
            dat: &mut res_dat,
        };

        assert_eq!(
            bp_build_merk_heap(&mary, &mut res),
            Err(MerkHeapError::NonPowerOfTwoLen)
        );
    }
}
