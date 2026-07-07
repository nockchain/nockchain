//! Pearl MoE (GROUPED_GEMM) routing-data canonicalization — off-circuit.
//!
//! Byte-exact port of Pearl's `build_routing_data`
//! (`miner/pearl-gemm/csrc/moe/build_routing_data.cu`), which is a **stable
//! counting sort by expert id** of the `m·top_k` flattened routing slots:
//!
//! ```text
//! numel   = num_tokens · top_k
//! slot s  = (token s / top_k, k-position s % top_k), expert = topk_ids[s]
//! slot_indices    = [0, numel) stably sorted by expert id (ties keep ascending slot order)
//! routing_data[i] = slot_indices[i] / top_k          (token index of the i-th routed slot)
//! routing_offsets[e] = #{ slots with expert_id ≤ e } (exclusive-end cumulative counts;
//!                      empty experts repeat the previous offset; last entry == numel)
//! ```
//!
//! The committed routing array is `routing_data` (the token indices), serialized
//! as little-endian `u32`. `routing_offsets` is likewise little-endian `u32`.
//! Both feed the MoE commitment splice (see `fiat_shamir::moe_hash_activations`).
//!
//! This is a consensus-load-bearing canonicalization: a different tie-break would
//! change `routing_root` and every downstream hash. The stable sort here matches
//! CUB's stable radix sort exactly (within-expert order = ascending original slot
//! index).

use thiserror::Error;

/// Pearl `MAX_NUM_EXPERTS` (`PublicProofParams::MAX_NUM_EXPERTS`). The CUDA fast
/// path sorts 8-bit keys (≤256 experts); the protocol envelope allows up to 1024
/// and the stable sort here is correct for the full range.
pub const MAX_NUM_EXPERTS: usize = 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RoutingError {
    #[error("top_k must be > 0")]
    ZeroTopK,
    #[error("num_experts must be in 1..={MAX_NUM_EXPERTS}, got {0}")]
    NumExpertsOutOfRange(usize),
    #[error("topk_ids length {actual} must equal num_tokens*top_k = {expected}")]
    BadTopkLen { expected: usize, actual: usize },
    #[error("num_tokens*top_k overflows u32 (m={m}, top_k={top_k})")]
    RoutingCountOverflow { m: usize, top_k: usize },
    #[error("expert id {expert} at slot {slot} is out of range (num_experts={num_experts})")]
    ExpertIdOutOfRange {
        slot: usize,
        expert: u32,
        num_experts: usize,
    },
}

/// Canonical routing for one MoE job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingData {
    pub num_tokens: usize,
    pub top_k: usize,
    pub num_experts: usize,
    /// Flat slot indices `[0, m·top_k)` stably sorted by expert id.
    pub slot_indices: Vec<u32>,
    /// Token index of each routed slot: `routing_data[i] = slot_indices[i] / top_k`.
    /// This is the array committed by `routing_root`.
    pub routing_data: Vec<u32>,
    /// Exclusive-end cumulative token counts per expert. `len == num_experts`,
    /// `routing_offsets[e] = #{ slots with expert ≤ e }`, last entry `== m·top_k`.
    pub routing_offsets: Vec<u32>,
}

/// Build canonical routing data from per-token expert assignments.
///
/// `topk_ids` is the row-major flattening of the `(num_tokens, top_k)` expert
/// assignment matrix: `topk_ids[t*top_k + j]` is the `j`-th expert token `t` is
/// routed to. Expert ids must be `< num_experts`.
pub fn build_routing_data(
    topk_ids: &[u32],
    num_tokens: usize,
    top_k: usize,
    num_experts: usize,
) -> Result<RoutingData, RoutingError> {
    if top_k == 0 {
        return Err(RoutingError::ZeroTopK);
    }
    if num_experts == 0 || num_experts > MAX_NUM_EXPERTS {
        return Err(RoutingError::NumExpertsOutOfRange(num_experts));
    }
    let numel = num_tokens
        .checked_mul(top_k)
        .ok_or(RoutingError::RoutingCountOverflow {
            m: num_tokens,
            top_k,
        })?;
    if topk_ids.len() != numel {
        return Err(RoutingError::BadTopkLen {
            expected: numel,
            actual: topk_ids.len(),
        });
    }
    // Routing offsets and indices are u32-addressed (Pearl `m*top_k < 2^32`).
    if u32::try_from(numel).is_err() {
        return Err(RoutingError::RoutingCountOverflow {
            m: num_tokens,
            top_k,
        });
    }

    // Per-expert counts (counting-sort histogram) with range validation.
    let mut counts = vec![0u32; num_experts];
    for (slot, &expert) in topk_ids.iter().enumerate() {
        let e = expert as usize;
        if e >= num_experts {
            return Err(RoutingError::ExpertIdOutOfRange {
                slot,
                expert,
                num_experts,
            });
        }
        counts[e] += 1;
    }

    // Exclusive-start offsets per expert (prefix sum of counts). We fill each
    // expert's contiguous run in ascending slot order — this reproduces the
    // stable radix sort's within-expert ordering without a comparison sort.
    let mut next = vec![0u32; num_experts];
    let mut acc = 0u32;
    for e in 0..num_experts {
        next[e] = acc;
        acc += counts[e];
    }
    debug_assert_eq!(acc as usize, numel);

    let mut slot_indices = vec![0u32; numel];
    for (slot, &expert) in topk_ids.iter().enumerate() {
        let e = expert as usize;
        let pos = next[e] as usize;
        slot_indices[pos] = slot as u32;
        next[e] += 1;
    }

    let top_k_u32 = top_k as u32;
    let routing_data: Vec<u32> = slot_indices.iter().map(|&s| s / top_k_u32).collect();

    // Exclusive-end cumulative counts: routing_offsets[e] = #{ expert ≤ e }.
    let mut routing_offsets = vec![0u32; num_experts];
    let mut cum = 0u32;
    for e in 0..num_experts {
        cum += counts[e];
        routing_offsets[e] = cum;
    }

    Ok(RoutingData {
        num_tokens,
        top_k,
        num_experts,
        slot_indices,
        routing_data,
        routing_offsets,
    })
}

impl RoutingData {
    /// Total routed slots `m·top_k` (== last routing offset).
    pub fn numel(&self) -> usize {
        self.routing_data.len()
    }

    /// Exclusive start offset of `expert_idx` within the flat routing.
    pub fn expert_start(&self, expert_idx: usize) -> u32 {
        if expert_idx == 0 {
            0
        } else {
            self.routing_offsets[expert_idx - 1]
        }
    }

    /// The token indices routed to `expert_idx`, in canonical order.
    pub fn expert_tokens(&self, expert_idx: usize) -> &[u32] {
        let start = self.expert_start(expert_idx) as usize;
        let end = self.routing_offsets[expert_idx] as usize;
        &self.routing_data[start..end]
    }

    /// `routing_data` as the committed little-endian `u32` byte string. This is
    /// the input to `routing_root = matrix_commitment(bytes, job_key)`.
    pub fn routing_data_le_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.routing_data.len() * 4);
        for &v in &self.routing_data {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// `routing_offsets` as the committed little-endian `u32` byte string. This
    /// is the input to `hash_offsets = matrix_commitment(bytes, job_key)`.
    pub fn routing_offsets_le_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.routing_offsets.len() * 4);
        for &v in &self.routing_offsets {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent reference: an explicit stable sort by `(expert, slot)`.
    /// Differential oracle for the counting-sort production path.
    fn reference_routing(
        topk_ids: &[u32],
        num_tokens: usize,
        top_k: usize,
        num_experts: usize,
    ) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        let numel = num_tokens * top_k;
        let mut slots: Vec<u32> = (0..numel as u32).collect();
        // Stable sort by expert id only; ties keep original (ascending slot) order.
        slots.sort_by_key(|&s| topk_ids[s as usize]);
        let routing_data: Vec<u32> = slots.iter().map(|&s| s / top_k as u32).collect();
        let mut offsets = vec![0u32; num_experts];
        for &id in topk_ids {
            for e in (id as usize)..num_experts {
                offsets[e] += 1;
            }
        }
        (slots, routing_data, offsets)
    }

    #[test]
    fn hand_kat_small_example() {
        // m=4, top_k=2, e=3. Flattened topk_ids (row-major):
        //   t0:[0,2] t1:[1,0] t2:[2,1] t3:[0,0]
        // slot expert ids: [0,2,1,0,2,1,0,0]
        let topk = [0u32, 2, 1, 0, 2, 1, 0, 0];
        let r = build_routing_data(&topk, 4, 2, 3).unwrap();
        // e0 slots 0,3,6,7 ; e1 slots 2,5 ; e2 slots 1,4
        assert_eq!(r.slot_indices, vec![0, 3, 6, 7, 2, 5, 1, 4]);
        // routing_data = slot/2
        assert_eq!(r.routing_data, vec![0, 1, 3, 3, 1, 2, 0, 2]);
        // counts e0=4,e1=2,e2=2 → cumulative
        assert_eq!(r.routing_offsets, vec![4, 6, 8]);
        assert_eq!(*r.routing_offsets.last().unwrap() as usize, r.numel());
        assert_eq!(r.expert_tokens(0), &[0, 1, 3, 3]);
        assert_eq!(r.expert_tokens(1), &[1, 2]);
        assert_eq!(r.expert_tokens(2), &[0, 2]);
    }

    #[test]
    fn matches_independent_reference_over_many_inputs() {
        // Deterministic pseudo-random inputs (index-derived; no RNG).
        for seed in 0u64..64 {
            let m = 1 + (seed as usize % 17);
            let top_k = 1 + (seed as usize % 4);
            let num_experts = 1 + (seed as usize % 7);
            let numel = m * top_k;
            let topk: Vec<u32> = (0..numel)
                .map(|i| {
                    let x = (i as u64)
                        .wrapping_mul(2654435761)
                        .wrapping_add(seed.wrapping_mul(40503));
                    (x % num_experts as u64) as u32
                })
                .collect();
            let got = build_routing_data(&topk, m, top_k, num_experts).unwrap();
            let (rs, rd, ro) = reference_routing(&topk, m, top_k, num_experts);
            assert_eq!(got.slot_indices, rs, "seed {seed} slot_indices");
            assert_eq!(got.routing_data, rd, "seed {seed} routing_data");
            assert_eq!(got.routing_offsets, ro, "seed {seed} routing_offsets");
        }
    }

    #[test]
    fn structural_invariants_hold() {
        let topk = [3u32, 3, 0, 1, 3, 0, 2, 2, 1, 0];
        let (m, top_k, e) = (5, 2, 4);
        let r = build_routing_data(&topk, m, top_k, e).unwrap();
        // offsets monotone non-decreasing, last == numel.
        assert!(r.routing_offsets.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(r.routing_offsets[e - 1] as usize, m * top_k);
        // slot_indices is a permutation of [0, numel).
        let mut sorted = r.slot_indices.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..(m * top_k) as u32).collect::<Vec<_>>());
        // routing_data[i] == slot_indices[i] / top_k.
        for (i, &s) in r.slot_indices.iter().enumerate() {
            assert_eq!(r.routing_data[i], s / top_k as u32);
        }
        // Each token appears exactly top_k times across routing_data.
        let mut token_counts = vec![0usize; m];
        for &tok in &r.routing_data {
            token_counts[tok as usize] += 1;
        }
        assert!(token_counts.iter().all(|&c| c == top_k));
        // Within each expert, slots ascend (stability).
        for e_idx in 0..e {
            let start = r.expert_start(e_idx) as usize;
            let end = r.routing_offsets[e_idx] as usize;
            assert!(r.slot_indices[start..end].windows(2).all(|w| w[0] < w[1]));
        }
    }

    #[test]
    fn empty_experts_repeat_offset() {
        // expert 1 gets nothing.
        let topk = [0u32, 2, 2, 0];
        let r = build_routing_data(&topk, 2, 2, 3).unwrap();
        // counts e0=2,e1=0,e2=2 → offsets 2,2,4
        assert_eq!(r.routing_offsets, vec![2, 2, 4]);
        assert_eq!(r.expert_tokens(1), &[] as &[u32]);
    }

    #[test]
    fn single_expert_and_top_k_one() {
        let topk = [0u32, 0, 0, 0];
        let r = build_routing_data(&topk, 4, 1, 1).unwrap();
        assert_eq!(r.slot_indices, vec![0, 1, 2, 3]);
        assert_eq!(r.routing_data, vec![0, 1, 2, 3]);
        assert_eq!(r.routing_offsets, vec![4]);
    }

    #[test]
    fn non_16_aligned_numel_is_handled() {
        // m*top_k = 1000 (not a multiple of 16), the moe_test.rs edge case.
        let m = 1000;
        let top_k = 1;
        let e = 4;
        let topk: Vec<u32> = (0..m).map(|i| (i % e) as u32).collect();
        let r = build_routing_data(&topk, m, top_k, e).unwrap();
        assert_eq!(r.numel(), 1000);
        assert_eq!(*r.routing_offsets.last().unwrap(), 1000);
        assert_eq!(r.routing_data_le_bytes().len(), 1000 * 4);
        assert_eq!(r.routing_offsets_le_bytes().len(), e * 4);
    }

    #[test]
    fn le_byte_serialization_is_exact() {
        let topk = [1u32, 0, 1, 0];
        let r = build_routing_data(&topk, 2, 2, 2).unwrap();
        // routing_data little-endian bytes.
        let mut expected = Vec::new();
        for &v in &r.routing_data {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(r.routing_data_le_bytes(), expected);
        let mut exp_off = Vec::new();
        for &v in &r.routing_offsets {
            exp_off.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(r.routing_offsets_le_bytes(), exp_off);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert_eq!(
            build_routing_data(&[0, 1], 2, 0, 2),
            Err(RoutingError::ZeroTopK)
        );
        assert_eq!(
            build_routing_data(&[0, 1], 1, 2, 0),
            Err(RoutingError::NumExpertsOutOfRange(0))
        );
        assert_eq!(
            build_routing_data(&[0, 1], 1, 2, MAX_NUM_EXPERTS + 1),
            Err(RoutingError::NumExpertsOutOfRange(MAX_NUM_EXPERTS + 1))
        );
        assert_eq!(
            build_routing_data(&[0, 1, 2], 2, 2, 3),
            Err(RoutingError::BadTopkLen {
                expected: 4,
                actual: 3
            })
        );
        // expert id out of range.
        assert_eq!(
            build_routing_data(&[0, 5], 1, 2, 3),
            Err(RoutingError::ExpertIdOutOfRange {
                slot: 1,
                expert: 5,
                num_experts: 3
            })
        );
    }
}
