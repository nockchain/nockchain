use nockchain_types::tx_engine::common::Hash as NockPkh;

/// Replicate Hoon's active-proposer logic: sort nodes by nock-pkh (b58), rotate by height.
///
/// This matches the Hoon logic in `++active-proposer` from types.hoon:593-611.
///
/// # Arguments
/// * `height` - Current block height
/// * `node_pkhs` - List of nock public key hashes (in config order, not sorted)
///
/// # Returns
/// Index of the proposer node (index into the SORTED node list)
pub fn hoon_proposer(height: u64, node_pkhs: &[NockPkh]) -> usize {
    // Sort nodes by base58-encoded PKH (lexicographic string comparison)
    let mut sorted_indices: Vec<usize> = (0..node_pkhs.len()).collect();
    sorted_indices.sort_by_key(|&i| node_pkhs[i].to_base58());

    // Rotate by height mod num_nodes
    let rotation_offset = (height as usize) % node_pkhs.len();
    sorted_indices[rotation_offset]
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
        let proposer_0 = hoon_proposer(0, &pkhs);

        // At height 1, should be second in sorted order
        let proposer_1 = hoon_proposer(1, &pkhs);

        // Should rotate through all nodes
        assert_ne!(proposer_0, proposer_1);

        // At height 5 (one full rotation), should be same as height 0
        let proposer_5 = hoon_proposer(5, &pkhs);
        assert_eq!(proposer_0, proposer_5);
    }

    #[test]
    fn test_hoon_proposer_sorts_by_b58() {
        let pkhs = sample_pkhs();

        // Get sorted indices
        let mut sorted_indices: Vec<usize> = (0..pkhs.len()).collect();
        sorted_indices.sort_by_key(|&i| pkhs[i].to_base58());

        // Proposer at height 0 should be first sorted index
        let proposer = hoon_proposer(0, &pkhs);
        assert_eq!(proposer, sorted_indices[0]);

        // Proposer at height 1 should be second sorted index
        let proposer = hoon_proposer(1, &pkhs);
        assert_eq!(proposer, sorted_indices[1]);
    }
}
