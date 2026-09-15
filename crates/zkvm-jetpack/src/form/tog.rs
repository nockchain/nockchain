use nockchain_math::belt::Belt;
use nockchain_math::mary::MarySlice;
use nockchain_math::tip5::hash::{
    absorb as tip5_absorb, hash_belts_slice, hash_ten_cell as tip5_hash_ten_cell,
    squeeze as tip5_squeeze,
};
use nockchain_math::tip5::{DIGEST_LENGTH, RATE, STATE_SIZE};
use nockvm::jets::JetErr;

use crate::based;
use crate::form::felt::Felt;
use crate::form::proof::{Proof, ProofData};

pub struct Tog {
    pub sponge: [u64; STATE_SIZE],
}

fn hash_ten_cell(input: [u64; RATE]) -> [u64; DIGEST_LENGTH] {
    tip5_hash_ten_cell(input)
}

fn hash_belts_list(input: &[u64]) -> [u64; DIGEST_LENGTH] {
    hash_belts_slice(input)
}

fn hash_u64(value: u64) -> [u64; DIGEST_LENGTH] {
    hash_belts_list(&[1, value])
}

fn belts_to_u64s(values: &[Belt]) -> Vec<u64> {
    values.iter().map(|value| value.0).collect()
}

fn felt_to_u64s(values: &[Felt]) -> Vec<u64> {
    values
        .iter()
        .flat_map(|value| value.0.map(|belt| belt.0))
        .collect()
}

fn hash_mary(mary: MarySlice<'_>) -> [u64; DIGEST_LENGTH] {
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

pub fn absorb(sponge: &mut [u64; STATE_SIZE], input: &[u64]) {
    tip5_absorb(sponge, input);
}

fn squeeze(sponge: &mut [u64; STATE_SIZE]) -> [u64; RATE] {
    tip5_squeeze(sponge)
}

pub fn verifier_fiat_shamir(proof: &Proof) -> Result<Tog, JetErr> {
    let (objs, hashes) = consumed_verifier_transcript(proof);
    absorb_proof_objects(objs, hashes)
}

/// Returns the consumed verifier transcript objects in an easy-to-audit form.
///
/// In verifier mode `read_index` is authoritative (advanced by `ProofStream::pull`).
/// `hashes` is only a cache of precomputed object hashes and may lag behind.
#[inline]
fn consumed_verifier_transcript(proof: &Proof) -> (&[ProofData], &[[u64; 5]]) {
    let consumed = (proof.read_index as usize).min(proof.objects.len());
    let cached_hashes = proof.hashes.len().min(consumed);
    (&proof.objects[..consumed], &proof.hashes[..cached_hashes])
}

pub fn absorb_proof_objects(objs: &[ProofData], hashes: &[[u64; 5]]) -> Result<Tog, JetErr> {
    // Defensive clamp: callers can pass stale/malformed caches.
    // Treat `hashes` as an optional leading cache over `objs`.
    let cached_hashes = hashes.len().min(objs.len());
    let objs = &objs[cached_hashes..];
    let hashes = &hashes[..cached_hashes];

    let new_hashes = objs.iter().map(hash_proof_data).collect::<Vec<_>>();

    let mut sponge = [0; STATE_SIZE];
    for hash in hashes {
        absorb(&mut sponge, &hash[..]);
    }
    for hash in new_hashes {
        absorb(&mut sponge, &hash[..]);
    }
    Ok(Tog { sponge })
}

fn term_to_belt<const N: usize>(bytes: &[u8; N]) -> u64 {
    if bytes.len() > size_of::<u64>() {
        panic!(
            "\"{:?}\" does not fit in a u64: must be 8 or fewer characters, not {}",
            bytes,
            bytes.len()
        );
    }
    let mut val: u64 = 0;
    for byte in bytes.iter().rev() {
        val = (val << u8::BITS) | u64::from(*byte);
    }
    based!(val);
    val
}

fn hash_term<const N: usize>(term: &[u8; N]) -> [u64; 5] {
    let belt = term_to_belt(term);
    hash_belt(belt)
}

fn hash_belt(belt: u64) -> [u64; 5] {
    hash_belts_list(&[1, belt])
}

fn hash_hoon_list(list: &[u64]) -> [u64; 5] {
    // The leaf word of a hoon list is just the list itself in a vec (including the final 0).
    // The dyck word for an n-length list is just [0 1] repeated n times.
    // The size is n+1 (because the leaf vector includes the final 0).
    let mut dat = vec![0; 3 * list.len() + 2];
    dat[0] = list.len() as u64 + 1; // size
    dat[1..list.len() + 1].copy_from_slice(list); // leaf
    dat[list.len() + 1] = 0; // final 0 in leaf
    dat[list.len() + 2..].copy_from_slice(&[0, 1].repeat(list.len())); // dyck
    hash_belts_list(&dat)
}

fn hash_noun_digests(list: &[[u64; 5]]) -> [u64; 5] {
    // leaf is just all the hashes flattened together, plus the final 0 because it's a hoon list
    // size of n hashes is 5*n + 1 (for the final 0)
    // dyck of one hash is [0 0 1 0 1 0 1 0 1 1].
    // dyck of n hashes is that same dyck repeated n times.
    let mut dat = vec![0; 1 + (5 * list.len() + 1) + (10 * list.len())];
    // size
    dat[0] = (5 * list.len() as u64) + 1;
    // leaf
    dat[1..(5 * list.len()) + 1]
        .copy_from_slice(&list.iter().flatten().copied().collect::<Vec<_>>());
    // final 0 in leaf
    dat[(5 * list.len()) + 1] = 0;
    // dyck
    dat[(5 * list.len()) + 2..].copy_from_slice(&[0, 0, 1, 0, 1, 0, 1, 0, 1, 1].repeat(list.len()));
    hash_belts_list(&dat)
}

pub fn hash_proof_data(data: &ProofData) -> [u64; 5] {
    match data {
        ProofData::MRoot { p } => {
            let term_hash = hash_term(b"m-root");
            // eprintln!("hash(m-root): {:?}", term_hash);
            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(p);
            hash_ten_cell(ten_cell)
        }
        ProofData::Puzzle {
            com,
            nonce,
            len,
            leaf,
            dyck,
        } => {
            let term_hash = hash_term(b"puzzle");
            let len_hash = hash_belt(*len);

            // hash p using precomputed leaf and dyck
            let size = leaf.len();
            let mut dat = vec![0; leaf.len() + dyck.len() + 1];
            dat[0] = size as u64;
            dat[1..leaf.len() + 1].copy_from_slice(leaf);
            dat[(leaf.len() + 1)..].copy_from_slice(dyck);
            let p_hash = hash_belts_list(&dat);

            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&len_hash);
            ten_cell[5..].copy_from_slice(&p_hash);
            let hash = hash_ten_cell(ten_cell);

            ten_cell[..5].copy_from_slice(nonce);
            ten_cell[5..].copy_from_slice(&hash);
            let hash = hash_ten_cell(ten_cell);

            ten_cell[..5].copy_from_slice(com);
            ten_cell[5..].copy_from_slice(&hash);
            let hash = hash_ten_cell(ten_cell);

            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::CompM { p, num } => {
            let term_hash = hash_term(b"comp-m");
            let num_hash = hash_belt(*num);

            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(p);
            ten_cell[5..].copy_from_slice(&num_hash);
            let hash = hash_ten_cell(ten_cell);

            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::Heights(heights) => {
            let term_hash = hash_term(b"heights");
            let list_hash = hash_hoon_list(heights);
            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&list_hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::Codeword(fpoly) => {
            let term_hash = hash_term(b"codeword");
            let belts = felt_to_u64s(fpoly.as_slice());
            let mary = MarySlice {
                step: 3,
                len: fpoly.0.len() as u32,
                dat: &belts,
            };
            let mary_hash = hash_mary(mary);
            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&mary_hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::Evals(fpoly) => {
            let term_hash = hash_term(b"evals");
            let belts = felt_to_u64s(fpoly.as_slice());
            let mary = MarySlice {
                step: 3,
                len: fpoly.0.len() as u32,
                dat: &belts,
            };
            let mary_hash = hash_mary(mary);
            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&mary_hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::Terms(bpoly) => {
            let term_hash = hash_term(b"terms");
            let belts = belts_to_u64s(bpoly.as_slice());
            let mary = MarySlice {
                step: 1,
                len: bpoly.0.len() as u32,
                dat: &belts,
            };
            let mary_hash = hash_mary(mary);
            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&mary_hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::Poly(bpoly) => {
            let term_hash = hash_term(b"poly");
            let belts = belts_to_u64s(bpoly.as_slice());
            let mary = MarySlice {
                step: 1,
                len: bpoly.0.len() as u32,
                dat: &belts,
            };
            let mary_hash = hash_mary(mary);
            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&mary_hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::MPathBf(proof_path_bf) => {
            let term_hash = hash_term(b"m-pathbf");

            let belts = belts_to_u64s(proof_path_bf.leaf.as_slice());
            let mary = MarySlice {
                step: 1,
                len: proof_path_bf.leaf.0.len() as u32,
                dat: &belts,
            };
            let leaf_hash = hash_mary(mary);
            let path_hash = hash_noun_digests(&proof_path_bf.path);

            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&leaf_hash);
            ten_cell[5..].copy_from_slice(&path_hash);
            let hash = hash_ten_cell(ten_cell);
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&hash);
            hash_ten_cell(ten_cell)
        }
        ProofData::MPath(proof_path) => {
            let term_hash = hash_term(b"m-mpath");

            let belts = felt_to_u64s(proof_path.leaf.as_slice());
            let mary = MarySlice {
                step: 3,
                len: proof_path.leaf.0.len() as u32,
                dat: &belts,
            };
            let leaf_hash = hash_mary(mary);
            let path_hash = hash_noun_digests(&proof_path.path);

            let mut ten_cell: [u64; 10] = [0; 10];
            ten_cell[..5].copy_from_slice(&leaf_hash);
            ten_cell[5..].copy_from_slice(&path_hash);
            let hash = hash_ten_cell(ten_cell);
            ten_cell[..5].copy_from_slice(&term_hash);
            ten_cell[5..].copy_from_slice(&hash);
            hash_ten_cell(ten_cell)
        }
    }
}

pub fn belts(tog: &mut Tog, n: u32) -> Vec<Belt> {
    let q = n / (RATE as u32);
    let r = n % (RATE as u32);
    let mut output: Vec<Belt> = Vec::with_capacity(n as usize);
    for _ in 0..q {
        let belts = squeeze(&mut tog.sponge).map(Belt);
        output.extend_from_slice(&belts[..]);
    }
    let belts = squeeze(&mut tog.sponge).map(Belt);
    output.extend_from_slice(&belts[..r as usize]);
    output
}

pub fn felts(tog: &mut Tog, n: u32) -> Vec<Felt> {
    let belts = belts(tog, 3 * n);
    belts
        .as_slice()
        .chunks(3)
        .map(|chunk| Felt([chunk[0], chunk[1], chunk[2]]))
        .collect()
}

pub fn felt(tog: &mut Tog) -> Felt {
    felts(tog, 1)[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::proof::ProofVersion;

    fn sample_proof() -> Proof {
        Proof {
            version: ProofVersion::V2,
            objects: vec![
                ProofData::MRoot { p: [1, 2, 3, 4, 5] },
                ProofData::MRoot {
                    p: [6, 7, 8, 9, 10],
                },
            ],
            hashes: vec![],
            read_index: 0,
        }
    }

    #[test]
    fn consumed_verifier_transcript_uses_read_index_and_clamps() {
        let mut proof = sample_proof();
        proof.read_index = 99;
        proof.hashes = vec![
            hash_proof_data(&proof.objects[0]),
            hash_proof_data(&proof.objects[1]),
            [11, 12, 13, 14, 15],
        ];

        let (objs, hashes) = consumed_verifier_transcript(&proof);
        assert_eq!(objs.len(), proof.objects.len());
        assert_eq!(hashes.len(), proof.objects.len());
    }

    #[test]
    fn verifier_fiat_shamir_uses_consumed_objects_even_with_partial_hash_cache() {
        let mut proof = sample_proof();
        proof.read_index = 2;
        proof.hashes = vec![hash_proof_data(&proof.objects[0])];

        let expected = absorb_proof_objects(&proof.objects[..2], &proof.hashes[..1])
            .expect("expected transcript construction to succeed");
        let got = verifier_fiat_shamir(&proof).expect("verifier transcript construction failed");
        assert_eq!(got.sponge, expected.sponge);
    }

    #[test]
    fn absorb_proof_objects_clamps_excess_hash_cache() {
        let proof = sample_proof();
        let hashes = vec![hash_proof_data(&proof.objects[0]), [11, 12, 13, 14, 15]];

        let expected = absorb_proof_objects(&proof.objects[..1], &hashes[..1])
            .expect("expected transcript construction to succeed");
        let got = absorb_proof_objects(&proof.objects[..1], &hashes)
            .expect("clamped transcript construction failed");
        assert_eq!(got.sponge, expected.sponge);
    }

    #[test]
    fn v5_fiat_shamir_binds_candidate_commitment_and_nonce() {
        let puzzle = ProofData::Puzzle {
            com: [1, 2, 3, 4, 5],
            nonce: [6, 7, 8, 9, 10],
            len: 1,
            leaf: vec![11],
            dyck: vec![1],
        };
        let proof = Proof {
            version: ProofVersion::V5,
            objects: vec![puzzle],
            hashes: vec![],
            read_index: 1,
        };
        let before = verifier_fiat_shamir(&proof)
            .expect("v5 transcript should hash")
            .sponge;

        let mut changed_nonce = proof.clone();
        match &mut changed_nonce.objects[0] {
            ProofData::Puzzle { nonce, .. } => nonce[0] += 1,
            _ => unreachable!(),
        }
        let after_nonce = verifier_fiat_shamir(&changed_nonce)
            .expect("v5 transcript should hash after nonce change")
            .sponge;

        let mut changed_commitment = proof;
        match &mut changed_commitment.objects[0] {
            ProofData::Puzzle { com, .. } => com[0] = if com[0] == 0 { 1 } else { 0 },
            _ => unreachable!(),
        }
        let after_commitment = verifier_fiat_shamir(&changed_commitment)
            .expect("v5 transcript should hash after candidate commitment change")
            .sponge;

        assert_ne!(before, after_nonce);
        assert_ne!(before, after_commitment);
    }
}
