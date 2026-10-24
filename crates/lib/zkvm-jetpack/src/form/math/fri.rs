use std::collections::HashSet;

use nockchain_math::belt::{based_check, Belt, FieldError};
use nockchain_math::felt::{fdiv_, fmul_, fpow_, Felt};
use nockchain_math::fpoly::{fp_ntt, fpeval, fpscal};
use nockchain_math::poly::{FPolyVec, Poly, PolyVec};
use nockvm::jets::JetErr;

use crate::form::math::stark::StarkCalc;
use crate::form::merk::{hash_fpoly_leaf, MerkData};
use crate::form::proof::{ProofData, ProofMap, ProofStream, ProofStreamError};
use crate::form::tog::felt;

#[derive(Debug)]
pub enum FriError {
    Field(FieldError),
    Jet(JetErr),
    InvalidSample,
    Consistency,
    InvalidProof(&'static str),
    ProofStream(ProofStreamError),
}

impl From<FieldError> for FriError {
    fn from(value: FieldError) -> Self {
        FriError::Field(value)
    }
}

impl From<JetErr> for FriError {
    fn from(value: JetErr) -> Self {
        FriError::Jet(value)
    }
}

impl From<ProofStreamError> for FriError {
    fn from(value: ProofStreamError) -> Self {
        FriError::ProofStream(value)
    }
}

pub struct FriVerifyOutput {
    pub indices: Vec<u64>,
    pub merks: Vec<MerkData>,
    pub deep_cosets: ProofMap<u64, FPolyVec>,
}

pub fn fri_verify(
    calc: &StarkCalc,
    stream: &mut ProofStream<'_>,
    root: [u64; 5],
    enforce_v3_shapes: bool,
) -> Result<FriVerifyOutput, FriError> {
    let folding_deg = calc.fri.folding_deg;
    let num_rounds = calc.fri.num_rounds();
    let init_domain_len = calc.fri.init_domain_len;

    let mut roots = Vec::with_capacity(num_rounds);
    let mut alphas = Vec::with_capacity(num_rounds);
    roots.push(root);

    let mut rng = stream.transcript_rng()?;
    alphas.push(felt(&mut rng));

    for _ in 1..num_rounds {
        let next_root = match stream.pull()? {
            ProofData::MRoot { p } => *p,
            _ => return Err(FriError::InvalidProof("expected FRI merkle root")),
        };
        roots.push(next_root);
        let mut rng = stream.transcript_rng()?;
        alphas.push(felt(&mut rng));
    }

    let last_codeword = match stream.pull()? {
        ProofData::Codeword(codeword) => codeword.clone(),
        _ => return Err(FriError::InvalidProof("expected FRI last codeword")),
    };

    let expected_len = calc.fri.last_codeword_len();
    if last_codeword.0.len() != expected_len {
        return Err(FriError::InvalidProof("last codeword length mismatch"));
    }
    ensure_fpoly_based(&last_codeword, "last codeword contains non-based elements")?;

    let poly = fp_ifft(&last_codeword.0)?;
    let degree = PolyVec(poly).degree() as usize;
    let degree_bound =
        (init_domain_len / calc.fri.expand_factor) / folding_deg.pow(num_rounds as u32);
    if degree >= degree_bound {
        return Err(FriError::InvalidProof("last codeword not low degree"));
    }

    let mut rng = stream.transcript_rng()?;
    let indices = sample_indices(
        &mut rng, calc.fri.num_spot_checks, init_domain_len, expected_len,
    )?;
    let top_level_indices = indices.clone();

    let mut deep_cosets = ProofMap::new();
    let mut merks = Vec::new();
    let coset_span = init_domain_len / folding_deg;
    let depth = depth_for_len(coset_span as u64);

    for &idx in &top_level_indices {
        let coset_idx = idx % coset_span;
        let axis = index_to_axis(depth, coset_idx as u64);
        let opening = match stream.pull()? {
            ProofData::MPath(path) => path.clone(),
            _ => return Err(FriError::InvalidProof("expected FRI m-path")),
        };
        if enforce_v3_shapes && opening.leaf.0.len() != folding_deg {
            return Err(FriError::InvalidProof("FRI opening length mismatch"));
        }
        ensure_fpoly_based(&opening.leaf, "FRI opening contains non-based elements")?;
        deep_cosets.insert(coset_idx as u64, opening.leaf.clone());
        merks.push(MerkData {
            leaf: hash_fpoly_leaf(&opening.leaf),
            axis,
            root,
            path: opening.path.clone(),
        });
    }

    let mut prev_indices = top_level_indices;
    let mut prev_cosets = deep_cosets.clone();
    let mut prev_len = init_domain_len;
    let mut omega = Felt::lift(calc.fri.omega);
    let mut round_offset = Felt::lift(calc.fri.generator);

    for round in 0..num_rounds {
        let new_len = prev_len / folding_deg;
        let mut next_cosets = ProofMap::new();
        let mut next_indices = Vec::with_capacity(prev_indices.len());

        if round + 1 < num_rounds {
            let root = roots[round + 1];
            let depth = depth_for_len((new_len / folding_deg) as u64);
            for &prev_idx in &prev_indices {
                let new_idx = prev_idx % (prev_len / folding_deg);
                let coset_idx = new_idx % (new_len / folding_deg);
                let axis = index_to_axis(depth, coset_idx as u64);
                let opening = match stream.pull()? {
                    ProofData::MPath(path) => path.clone(),
                    _ => return Err(FriError::InvalidProof("expected FRI m-path")),
                };
                if enforce_v3_shapes && opening.leaf.0.len() != folding_deg {
                    return Err(FriError::InvalidProof("FRI opening length mismatch"));
                }
                ensure_fpoly_based(&opening.leaf, "FRI opening contains non-based elements")?;
                next_indices.push(new_idx);
                next_cosets.insert(coset_idx as u64, opening.leaf.clone());
                merks.push(MerkData {
                    leaf: hash_fpoly_leaf(&opening.leaf),
                    axis,
                    root,
                    path: opening.path.clone(),
                });
            }
        }

        let alpha = alphas[round];
        for &prev_idx in &prev_indices {
            let prev_coset_idx = prev_idx % (prev_len / folding_deg);
            let coset = prev_cosets
                .get(&(prev_coset_idx as u64))
                .ok_or(FriError::InvalidProof("missing FRI coset"))?;
            let coeffs = fp_ifft(&coset.0)?;
            let omega_pow = fpow_(&omega, prev_coset_idx as u64);
            let denom = fmul_(&round_offset, &omega_pow);
            let eval_point = fdiv_(&alpha, &denom);
            let folded_val = fpeval(coeffs.as_slice(), eval_point);

            let new_codeword_val = if round + 1 == num_rounds {
                last_codeword.0[prev_coset_idx]
            } else {
                let new_coset_idx = prev_coset_idx % (new_len / folding_deg);
                let entry = prev_coset_idx / (new_len / folding_deg);
                let coset = next_cosets
                    .get(&(new_coset_idx as u64))
                    .ok_or(FriError::InvalidProof("missing FRI coset"))?;
                coset.0[entry]
            };

            if folded_val != new_codeword_val {
                return Err(FriError::Consistency);
            }
        }

        if round + 1 < num_rounds {
            prev_indices = next_indices;
            prev_cosets = next_cosets;
        }
        prev_len = new_len;
        omega = fpow_(&omega, folding_deg as u64);
        round_offset = fpow_(&round_offset, folding_deg as u64);
    }

    Ok(FriVerifyOutput {
        indices: indices.into_iter().map(|idx| idx as u64).collect(),
        merks,
        deep_cosets,
    })
}

fn fp_ifft(fp: &[Felt]) -> Result<Vec<Felt>, FieldError> {
    let order = fp.len() as u64;
    let ordered_root = Belt(order).ordered_root()?;
    let root = Felt::constant(ordered_root.inv().0);
    let scale_factor = Felt::from([Belt(order).inv(), Belt(0), Belt(0)]);
    let ntt_result = fp_ntt(fp, &root);
    let mut scaled = vec![Felt::zero(); order as usize];
    fpscal(&scale_factor, &ntt_result, &mut scaled);
    Ok(scaled)
}

fn depth_for_len(len: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    len.ilog2() as u64 + 1
}

fn index_to_axis(depth: u64, index: u64) -> u64 {
    (1u64 << depth.saturating_sub(1)) + index
}

fn ensure_fpoly_based(values: &FPolyVec, label: &'static str) -> Result<(), FriError> {
    if values
        .0
        .iter()
        .all(|felt| felt.0.iter().all(|belt| based_check(belt.0)))
    {
        Ok(())
    } else {
        Err(FriError::InvalidProof(label))
    }
}

fn random_index(rng: &mut crate::form::tog::Tog, size: usize) -> Result<usize, FriError> {
    if size == 0 {
        return Err(FriError::InvalidSample);
    }
    let value = crate::form::tog::belts(rng, 1)[0].0 as usize;
    Ok(value % size)
}

fn sample_indices(
    rng: &mut crate::form::tog::Tog,
    n: usize,
    size: usize,
    reduced_size: usize,
) -> Result<Vec<usize>, FriError> {
    // Indices are unique by their reduced position (`idx % reduced_size`).
    // There are only `reduced_size` reduced positions, so more samples would never finish.
    if reduced_size == 0 || size < reduced_size || n > reduced_size {
        return Err(FriError::InvalidSample);
    }
    let mut indices = Vec::with_capacity(n);
    let mut seen = HashSet::with_capacity(n);
    while indices.len() < n {
        let idx = random_index(rng, size)?;
        let reduced = idx % reduced_size;
        if seen.insert(reduced) {
            indices.push(idx);
        }
    }
    Ok(indices)
}
