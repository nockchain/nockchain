use nockchain_types::tx_engine::common::Hash as NockPkh;

use crate::shared::types::keccak256;
use crate::withdrawal::types::WithdrawalId;

/// Replicate Hoon's active-proposer logic: sort nodes by nock-pkh (b58), rotate by an index
/// which could be height.
///
/// This matches the Hoon logic in `++active-proposer` from types.hoon:593-611.
///
/// # Arguments
/// * `raw_index` - Current block height, or withdrawal_id hash + epoch
/// * `node_pkhs` - List of nock public key hashes (in config order, not sorted)
///
/// # Returns
/// Index of the proposer node (index into the SORTED node list)
pub fn active_proposer(raw_index: u64, node_pkhs: &[NockPkh]) -> usize {
    // Sort nodes by base58-encoded PKH (lexicographic string comparison)
    let mut sorted_indices: Vec<usize> = (0..node_pkhs.len()).collect();
    sorted_indices.sort_by_key(|&i| node_pkhs[i].to_base58());

    // Rotate by height mod num_nodes
    let rotation_offset = (raw_index as usize) % node_pkhs.len();
    sorted_indices[rotation_offset]
}

/// Chooses the deterministic proposer for a withdrawal epoch.
///
/// The globally unique Base event id seeds the rotation so different
/// withdrawals distribute leadership differently, and the epoch advances
/// ownership for timeout-based failover. The kernel `as_of` is intentionally
/// excluded because the sequencer treats the Base event id as withdrawal
/// identity before canonical proposal facts correct any registration-provided
/// kernel context.
pub fn withdrawal_active_proposer(
    withdrawal_id: &WithdrawalId,
    epoch: u64,
    node_pkhs: &[NockPkh],
) -> usize {
    withdrawal_turn_proposer(withdrawal_id, epoch, 0, node_pkhs)
}

/// Chooses the deterministic proposer for one handoff turn of a withdrawal
/// epoch.
///
/// We intentionally keep `epoch` and `handoff_index` as separate inputs even
/// though proposer selection could be expressed in terms of one combined
/// offset. `epoch` identifies the active withdrawal attempt / tx body, while
/// `handoff_index` only rotates responsibility within that attempt.
pub fn withdrawal_turn_proposer(
    withdrawal_id: &WithdrawalId,
    epoch: u64,
    handoff_index: u64,
    node_pkhs: &[NockPkh],
) -> usize {
    let digest = keccak256(&withdrawal_id.base_event_id.0);
    let mut seed_bytes = [0u8; 8];
    seed_bytes.copy_from_slice(&digest[..8]);
    let offset = u64::from_be_bytes(seed_bytes);
    active_proposer(
        offset.wrapping_add(epoch).wrapping_add(handoff_index),
        node_pkhs,
    )
}

#[cfg(test)]
mod tests {

    use super::*;

    fn sample_pkhs() -> Vec<NockPkh> {
        // Create 5 distinct PKHs (Tip5 hashes) with different b58 encodings
        // Fake test PKHs (valid format placeholders, NOT real operator data)
        vec![
            NockPkh::from_base58("2222222222222222222222222222222222222222222222222222").unwrap(),
            NockPkh::from_base58("3333333333333333333333333333333333333333333333333333").unwrap(),
            NockPkh::from_base58("4444444444444444444444444444444444444444444444444444").unwrap(),
            NockPkh::from_base58("5555555555555555555555555555555555555555555555555555").unwrap(),
            NockPkh::from_base58("6666666666666666666666666666666666666666666666666666").unwrap(),
        ]
    }

    #[test]
    fn test_hoon_proposer_rotation() {
        let pkhs = sample_pkhs();

        // At height 0, should be first in sorted order
        let proposer_0 = active_proposer(0, &pkhs);

        // At height 1, should be second in sorted order
        let proposer_1 = active_proposer(1, &pkhs);

        // Should rotate through all nodes
        assert_ne!(proposer_0, proposer_1);

        // At height 5 (one full rotation), should be same as height 0
        let proposer_5 = active_proposer(5, &pkhs);
        assert_eq!(proposer_0, proposer_5);
    }

    #[test]
    fn test_hoon_proposer_sorts_by_b58() {
        let pkhs = sample_pkhs();

        // Get sorted indices
        let mut sorted_indices: Vec<usize> = (0..pkhs.len()).collect();
        sorted_indices.sort_by_key(|&i| pkhs[i].to_base58());

        // Proposer at height 0 should be first sorted index
        let proposer = active_proposer(0, &pkhs);
        assert_eq!(proposer, sorted_indices[0]);

        // Proposer at height 1 should be second sorted index
        let proposer = active_proposer(1, &pkhs);
        assert_eq!(proposer, sorted_indices[1]);
    }

    #[test]
    fn test_withdrawal_proposer_rotates_by_epoch_but_is_seeded_by_withdrawal() {
        let pkhs = sample_pkhs();
        let withdrawal_a = WithdrawalId {
            as_of: NockPkh::from_base58("7777777777777777777777777777777777777777777777777777")
                .unwrap(),
            base_event_id: crate::shared::types::BaseEventId((0..32).collect()),
        };
        let withdrawal_b = WithdrawalId {
            as_of: NockPkh::from_base58("8888888888888888888888888888888888888888888888888888")
                .unwrap(),
            base_event_id: crate::shared::types::BaseEventId((32..64).collect()),
        };

        let a0 = withdrawal_active_proposer(&withdrawal_a, 0, &pkhs);
        let a1 = withdrawal_active_proposer(&withdrawal_a, 1, &pkhs);
        let b0 = withdrawal_active_proposer(&withdrawal_b, 0, &pkhs);

        assert_ne!(a0, a1);
        assert_ne!(a0, b0);
    }

    #[test]
    fn test_withdrawal_turn_proposer_rotates_after_canonical_handoff() {
        let pkhs = sample_pkhs();
        let withdrawal = WithdrawalId {
            as_of: NockPkh::from_base58("7777777777777777777777777777777777777777777777777777")
                .unwrap(),
            base_event_id: crate::shared::types::BaseEventId((0..32).collect()),
        };

        let turn0 = withdrawal_turn_proposer(&withdrawal, 3, 0, &pkhs);
        let turn1 = withdrawal_turn_proposer(&withdrawal, 3, 1, &pkhs);

        assert_ne!(turn0, turn1);
    }

    #[test]
    fn test_withdrawal_proposer_ignores_kernel_as_of() {
        let pkhs = sample_pkhs();
        let withdrawal_a = WithdrawalId {
            as_of: NockPkh::from_base58("7777777777777777777777777777777777777777777777777777")
                .unwrap(),
            base_event_id: crate::shared::types::BaseEventId((0..32).collect()),
        };
        let withdrawal_b = WithdrawalId {
            as_of: NockPkh::from_base58("8888888888888888888888888888888888888888888888888888")
                .unwrap(),
            base_event_id: withdrawal_a.base_event_id.clone(),
        };

        assert_eq!(
            withdrawal_turn_proposer(&withdrawal_a, 3, 2, &pkhs),
            withdrawal_turn_proposer(&withdrawal_b, 3, 2, &pkhs)
        );
    }
}
