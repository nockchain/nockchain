use ai_pow::commit::matrix_commitment;
use ai_pow::fiat_shamir::{noise_seed_a, noise_seed_b};
use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    compute_pearl_pattern_ticket, evaluate_pearl_merge_ticket_attempt,
    mine_pearl_merge_ticket_attempt, pearl_adjust_target_for_config,
    pearl_bitcoin_double_sha256_raw, pearl_kappa, pearl_nbits_to_target_le,
    pearl_nockchain_aux_commitment, validate_pearl_merge_config_for_recursive_prover,
    verify_pearl_aux_inclusion, verify_pearl_compatible_public_data, verify_pearl_compatible_work,
    verify_pearl_merge_mining_public_data, verify_pearl_merge_mining_public_data_with_aux_bytes,
    verify_pearl_merge_public_statement_bytes,
    verify_pearl_merge_public_statement_bytes_with_aux_inclusion, verify_pearl_pattern_ticket,
    PearlAttempt, PearlAuxInclusionProof, PearlCompatError, PearlIncompleteBlockHeader,
    PearlMergePublicStatement, PearlMiningConfig, PearlNockchainAux, PearlPeriodicPattern,
    PearlPublicProofParams, PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES,
    PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH, PEARL_INCOMPLETE_BLOCK_HEADER_SIZE,
    PEARL_MERGE_PUBLIC_STATEMENT_MAGIC, PEARL_MINING_CONFIG_RESERVED_SIZE,
    PEARL_MINING_CONFIG_SIZE, PEARL_MMA_INT7XINT7_TO_INT32, PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX,
    PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG, PEARL_NOCKCHAIN_AUX_DOMAIN, PEARL_NOCKCHAIN_AUX_EXTRA_MAX,
    PEARL_NOCKCHAIN_AUX_MAGIC, PEARL_PUBLIC_PROOF_PARAMS_SIZE,
};
use ai_pow::prover::{params_tag, BlockContext};
use ai_pow::synth::synth_matrices;
use ai_pow::tile_hash::hash_le_target;

fn simple_pattern(length: u32) -> PearlPeriodicPattern {
    PearlPeriodicPattern {
        shape: [(1, length), (length, 1), (length, 1)],
    }
}

fn header() -> PearlIncompleteBlockHeader {
    PearlIncompleteBlockHeader {
        version: 0x0102_0304,
        prev_block: [0x11; 32],
        merkle_root: [0x22; 32],
        timestamp: 0x6677_8899,
        nbits: 0x1d00_ffff,
    }
}

fn varint(n: usize) -> Vec<u8> {
    match n {
        0..=0xfc => vec![n as u8],
        0xfd..=0xffff => {
            let mut out = vec![0xfd];
            out.extend_from_slice(&(n as u16).to_le_bytes());
            out
        }
        0x1_0000..=0xffff_ffff => {
            let mut out = vec![0xfe];
            out.extend_from_slice(&(n as u32).to_le_bytes());
            out
        }
        _ => {
            let mut out = vec![0xff];
            out.extend_from_slice(&(n as u64).to_le_bytes());
            out
        }
    }
}

fn coinbase_tx_with_script(script_sig: &[u8], witness: Option<&[u8]>) -> Vec<u8> {
    coinbase_tx_with_scripts(script_sig, &[0x51], witness)
}

fn coinbase_tx_with_scripts(
    script_sig: &[u8],
    output_script: &[u8],
    witness: Option<&[u8]>,
) -> Vec<u8> {
    let mut tx = Vec::new();
    tx.extend_from_slice(&1u32.to_le_bytes());
    if witness.is_some() {
        tx.extend_from_slice(&[0x00, 0x01]);
    }
    tx.extend_from_slice(&varint(1));
    tx.extend_from_slice(&[0u8; 32]);
    tx.extend_from_slice(&u32::MAX.to_le_bytes());
    tx.extend_from_slice(&varint(script_sig.len()));
    tx.extend_from_slice(script_sig);
    tx.extend_from_slice(&u32::MAX.to_le_bytes());
    tx.extend_from_slice(&varint(1));
    tx.extend_from_slice(&0u64.to_le_bytes());
    tx.extend_from_slice(&varint(output_script.len()));
    tx.extend_from_slice(output_script);
    if let Some(witness_item) = witness {
        tx.extend_from_slice(&varint(1));
        tx.extend_from_slice(&varint(witness_item.len()));
        tx.extend_from_slice(witness_item);
    }
    tx.extend_from_slice(&0u32.to_le_bytes());
    tx
}

fn coinbase_aux_script(aux_commitment: &[u8; 32]) -> Vec<u8> {
    let mut script = Vec::from([0x01, 0x00]);
    script.extend_from_slice(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG);
    script.extend_from_slice(aux_commitment);
    script
}

fn display_root_from_raw(mut root_raw: [u8; 32]) -> [u8; 32] {
    root_raw.reverse();
    root_raw
}

fn mining_config() -> PearlMiningConfig {
    PearlMiningConfig {
        common_dim: 64,
        rank: 4,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: simple_pattern(8),
        cols_pattern: simple_pattern(8),
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    }
}

fn pearl_default_cols_pattern() -> Vec<u32> {
    (0..32).flat_map(|i| [8 * i, 8 * i + 1]).collect()
}

fn u256_le_div_ceil_u32(value: &[u8; 32], divisor: u32) -> [u8; 32] {
    assert!(divisor > 0);
    let mut out = [0u8; 32];
    let mut remainder = 0u32;
    for i in (0..32).rev() {
        let acc = (u64::from(remainder) << 8) | u64::from(value[i]);
        out[i] = (acc / u64::from(divisor)) as u8;
        remainder = (acc % u64::from(divisor)) as u32;
    }
    if remainder != 0 {
        for byte in &mut out {
            let (next, carry) = byte.overflowing_add(1);
            *byte = next;
            if !carry {
                break;
            }
        }
    }
    out
}

fn pearl_compat_test_pattern() -> PearlPeriodicPattern {
    PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73]).unwrap()
}

fn pearl_square_params() -> MatmulParams {
    MatmulParams {
        m: 128,
        k: 1024,
        n: 128,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    }
}

fn pearl_square_config() -> PearlMiningConfig {
    PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: simple_pattern(8),
        cols_pattern: simple_pattern(8),
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    }
}

#[test]
fn pearl_aux_inclusion_verifies_tagged_coinbase_commitment_against_merkle_root() {
    let aux_commitment =
        pearl_nockchain_aux_commitment(b"nockchain-mainnet", &[0x42; 32], 123_456, b"merge-window")
            .unwrap();
    let coinbase = coinbase_tx_with_script(&coinbase_aux_script(&aux_commitment), None);
    let mut header = header();
    header.merkle_root = display_root_from_raw(pearl_bitcoin_double_sha256_raw(&coinbase));

    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    verify_pearl_aux_inclusion(&header, &aux_commitment, &proof)
        .expect("tagged aux commitment is included in Pearl merkle root");
}

#[test]
fn pearl_aux_inclusion_rejects_witness_only_commitment() {
    let aux_commitment =
        pearl_nockchain_aux_commitment(b"nockchain-mainnet", &[0x42; 32], 123_456, b"merge-window")
            .unwrap();
    let witness_only = coinbase_aux_script(&aux_commitment);
    let coinbase = coinbase_tx_with_script(&[0x01, 0x00], Some(&witness_only));
    let committed_tx = coinbase_tx_with_script(&[0x01, 0x00], None);
    let mut header = header();
    header.merkle_root = display_root_from_raw(pearl_bitcoin_double_sha256_raw(&committed_tx));

    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    assert_eq!(
        verify_pearl_aux_inclusion(&header, &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxCommitmentTagMissing)
    );
}

#[test]
fn pearl_aux_inclusion_rejects_output_only_commitment() {
    let aux_commitment =
        pearl_nockchain_aux_commitment(b"nockchain-mainnet", &[0x42; 32], 123_456, b"merge-window")
            .unwrap();
    let output_only = coinbase_aux_script(&aux_commitment);
    let coinbase = coinbase_tx_with_scripts(&[0x01, 0x00], &output_only, None);
    let mut header = header();
    header.merkle_root = display_root_from_raw(pearl_bitcoin_double_sha256_raw(&coinbase));

    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    assert_eq!(
        verify_pearl_aux_inclusion(&header, &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxCommitmentTagMissing)
    );
}

#[test]
fn pearl_aux_inclusion_rejects_non_empty_branch_and_non_coinbase_leaf() {
    let aux_commitment =
        pearl_nockchain_aux_commitment(b"nockchain-mainnet", &[0x42; 32], 123_456, b"merge-window")
            .unwrap();
    let coinbase = coinbase_tx_with_script(&coinbase_aux_script(&aux_commitment), None);
    let mut header = header();
    header.merkle_root = display_root_from_raw(pearl_bitcoin_double_sha256_raw(&coinbase));

    let bad_branch = [0x55u8; 32];
    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase.clone(),
        merkle_branch: vec![bad_branch],
    };
    assert_eq!(
        verify_pearl_aux_inclusion(&header, &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxMerkleBranchTooDeep(1))
    );

    let mut non_coinbase = coinbase;
    non_coinbase[5] = 0x01;
    let proof = PearlAuxInclusionProof {
        coinbase_tx: non_coinbase,
        merkle_branch: vec![],
    };
    assert_eq!(
        verify_pearl_aux_inclusion(&header, &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxNotCoinbase)
    );
}

#[test]
fn pearl_aux_inclusion_rejects_multi_input_coinbase_like_tx() {
    let aux_commitment =
        pearl_nockchain_aux_commitment(b"nockchain-mainnet", &[0x42; 32], 123_456, b"merge-window")
            .unwrap();
    let script = coinbase_aux_script(&aux_commitment);
    let mut tx = Vec::new();
    tx.extend_from_slice(&1u32.to_le_bytes());
    tx.push(2);
    tx.extend_from_slice(&[0u8; 32]);
    tx.extend_from_slice(&u32::MAX.to_le_bytes());
    tx.push(script.len() as u8);
    tx.extend_from_slice(&script);
    tx.extend_from_slice(&u32::MAX.to_le_bytes());
    tx.extend_from_slice(&[0x44u8; 32]);
    tx.extend_from_slice(&0u32.to_le_bytes());
    tx.push(0);
    tx.extend_from_slice(&u32::MAX.to_le_bytes());
    tx.push(1);
    tx.extend_from_slice(&0u64.to_le_bytes());
    tx.push(1);
    tx.push(0x51);
    tx.extend_from_slice(&0u32.to_le_bytes());

    let proof = PearlAuxInclusionProof {
        coinbase_tx: tx,
        merkle_branch: vec![],
    };

    assert_eq!(
        verify_pearl_aux_inclusion(&header(), &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxMalformedCoinbaseTx)
    );
}

#[test]
fn pearl_aux_inclusion_rejects_oversized_branch_before_hashing() {
    let aux_commitment = [0x42; 32];
    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase_tx_with_script(&coinbase_aux_script(&aux_commitment), None),
        merkle_branch: vec![[0u8; 32]; PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH + 1],
    };

    assert_eq!(
        verify_pearl_aux_inclusion(&header(), &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxMerkleBranchTooDeep(
            PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH + 1
        ))
    );
}

#[test]
fn pearl_aux_inclusion_rejects_empty_or_oversized_coinbase_before_parsing() {
    let aux_commitment = [0x42; 32];
    let empty = PearlAuxInclusionProof {
        coinbase_tx: Vec::new(),
        merkle_branch: vec![],
    };
    assert_eq!(
        verify_pearl_aux_inclusion(&header(), &aux_commitment, &empty),
        Err(PearlCompatError::PearlAuxCoinbaseTxEmpty)
    );

    let oversized = PearlAuxInclusionProof {
        coinbase_tx: vec![0u8; PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES + 1],
        merkle_branch: vec![],
    };
    assert_eq!(
        verify_pearl_aux_inclusion(&header(), &aux_commitment, &oversized),
        Err(PearlCompatError::PearlAuxCoinbaseTxTooLarge(
            PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES + 1
        ))
    );
}

#[test]
fn pearl_aux_inclusion_rejects_malformed_segwit_flag() {
    let aux_commitment = [0x42; 32];
    let mut coinbase =
        coinbase_tx_with_script(&coinbase_aux_script(&aux_commitment), Some(&[0u8; 32]));
    coinbase[5] = 0x02;
    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    assert_eq!(
        verify_pearl_aux_inclusion(&header(), &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxMalformedCoinbaseTx)
    );
}

#[test]
fn pearl_aux_inclusion_rejects_noncanonical_varint_lengths() {
    let aux_commitment = [0x42; 32];
    let script = coinbase_aux_script(&aux_commitment);
    let mut coinbase = Vec::new();
    coinbase.extend_from_slice(&1u32.to_le_bytes());
    coinbase.push(1);
    coinbase.extend_from_slice(&[0u8; 32]);
    coinbase.extend_from_slice(&u32::MAX.to_le_bytes());
    coinbase.push(0xfd);
    coinbase.extend_from_slice(&(script.len() as u16).to_le_bytes());
    coinbase.extend_from_slice(&script);
    coinbase.extend_from_slice(&u32::MAX.to_le_bytes());
    coinbase.push(1);
    coinbase.extend_from_slice(&0u64.to_le_bytes());
    coinbase.push(1);
    coinbase.push(0x51);
    coinbase.extend_from_slice(&0u32.to_le_bytes());

    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    assert_eq!(
        verify_pearl_aux_inclusion(&header(), &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxMalformedCoinbaseTx)
    );
}

#[test]
fn pearl_aux_inclusion_rejects_huge_varint_lengths_without_truncation() {
    let aux_commitment = [0x42; 32];
    let mut coinbase = Vec::new();
    coinbase.extend_from_slice(&1u32.to_le_bytes());
    coinbase.push(1);
    coinbase.extend_from_slice(&[0u8; 32]);
    coinbase.extend_from_slice(&u32::MAX.to_le_bytes());
    coinbase.push(0xff);
    coinbase.extend_from_slice(&u64::MAX.to_le_bytes());

    let proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    assert_eq!(
        verify_pearl_aux_inclusion(&header(), &aux_commitment, &proof),
        Err(PearlCompatError::PearlAuxMalformedCoinbaseTx)
    );
}

#[test]
fn pearl_header_and_mining_config_serialization_are_exact() {
    let header = header();
    let encoded_header = header.to_bytes();
    assert_eq!(encoded_header.len(), PEARL_INCOMPLETE_BLOCK_HEADER_SIZE);
    assert_eq!(&encoded_header[0..4], &0x0102_0304u32.to_le_bytes());
    assert_eq!(&encoded_header[4..36], &[0x11; 32]);
    assert_eq!(&encoded_header[36..68], &[0x22; 32]);
    assert_eq!(&encoded_header[68..72], &0x6677_8899u32.to_le_bytes());
    assert_eq!(&encoded_header[72..76], &0x1d00_ffffu32.to_le_bytes());
    assert_eq!(
        PearlIncompleteBlockHeader::from_bytes(&encoded_header).unwrap(),
        header
    );

    let config = mining_config();
    let encoded_config = config.to_bytes().unwrap();
    assert_eq!(encoded_config.len(), PEARL_MINING_CONFIG_SIZE);
    assert_eq!(&encoded_config[0..4], &64u32.to_le_bytes());
    assert_eq!(&encoded_config[4..6], &4u16.to_le_bytes());
    assert_eq!(
        &encoded_config[6..8],
        &PEARL_MMA_INT7XINT7_TO_INT32.to_le_bytes()
    );
    assert_eq!(&encoded_config[8..14], &[0, 7, 0, 0, 0, 0]);
    assert_eq!(&encoded_config[14..20], &[0, 7, 0, 0, 0, 0]);
    assert_eq!(&encoded_config[20..52], &[0u8; 32]);
    assert_eq!(
        PearlMiningConfig::from_bytes(&encoded_config).unwrap(),
        config
    );
}

#[test]
fn pearl_periodic_pattern_matches_reference_semantics() {
    let rows = PearlPeriodicPattern::from_list(&[0, 8]).unwrap();
    assert_eq!(rows.shape, [(8, 2), (16, 1), (16, 1)]);
    assert_eq!(rows.to_bytes().unwrap(), [7, 1, 0, 0, 0, 0]);
    assert_eq!(
        PearlPeriodicPattern::from_bytes(&[7, 1, 0, 0, 0, 0]).unwrap(),
        rows
    );
    assert_eq!(rows.to_list().unwrap(), vec![0, 8]);
    assert_eq!(rows.indices_with_offset_bounded(5, 8).unwrap(), vec![5, 13]);
    assert_eq!(rows.size().unwrap(), 2);
    assert_eq!(rows.period().unwrap(), 16);
    assert_eq!(rows.max().unwrap(), 8);
    assert!(rows.offset_is_valid(0));
    assert!(rows.offset_is_valid(7));
    assert!(!rows.offset_is_valid(8));

    let cols_list = pearl_default_cols_pattern();
    let cols = PearlPeriodicPattern::from_list(&cols_list).unwrap();
    assert_eq!(cols.shape, [(1, 2), (8, 32), (256, 1)]);
    assert_eq!(cols.to_bytes().unwrap(), [0, 1, 3, 31, 0, 0]);
    assert_eq!(cols.to_list().unwrap(), cols_list);
    assert_eq!(cols.size().unwrap(), 64);
    assert_eq!(cols.period().unwrap(), 256);
    assert_eq!(cols.max().unwrap(), 249);
    assert!(cols.offset_is_valid(0));
    assert!(cols.offset_is_valid(2));
    assert!(!cols.offset_is_valid(1));
    assert!(!cols.offset_is_valid(8));
}

#[test]
fn pearl_periodic_pattern_rejects_noncanonical_or_unbounded_shapes() {
    assert_eq!(
        PearlPeriodicPattern::from_list(&[]),
        Err(PearlCompatError::PatternEmpty)
    );
    assert_eq!(
        PearlPeriodicPattern::from_list(&[1, 2]),
        Err(PearlCompatError::PatternMustStartAtZero)
    );
    assert_eq!(
        PearlPeriodicPattern::from_list(&[0, 2, 2]),
        Err(PearlCompatError::PatternNotStrictlyIncreasing)
    );
    assert_eq!(
        PearlPeriodicPattern::from_list(&[0, 1, 3]),
        Err(PearlCompatError::PatternNotRepresentable)
    );
    assert_eq!(
        PearlPeriodicPattern::from_bytes(&[0, 1, 0, 1, 0, 0]),
        Err(PearlCompatError::BrokenSingleStride)
    );
    assert_eq!(
        PearlPeriodicPattern {
            shape: [(0, 1), (1, 1), (1, 1)]
        }
        .to_bytes(),
        Err(PearlCompatError::BadPatternStride)
    );
    assert_eq!(
        PearlPeriodicPattern {
            shape: [(1, 257), (257, 1), (257, 1)]
        }
        .to_bytes(),
        Err(PearlCompatError::PatternByteOverflow)
    );
    assert_eq!(
        PearlPeriodicPattern {
            shape: [(1, 2), (2, 2), (4, 2)]
        }
        .to_list_bounded(7),
        Err(PearlCompatError::PatternListTooLarge)
    );
}

#[test]
fn pearl_recursive_prover_config_preflight_accepts_bounded_patterns() {
    let params = MatmulParams {
        m: 8,
        k: 1024,
        n: 8,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    params.validate_prod_envelope().unwrap();

    let supported = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: simple_pattern(8),
        cols_pattern: simple_pattern(8),
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    validate_pearl_merge_config_for_recursive_prover(&supported, &params, 16)
        .expect("contiguous square tile pattern is currently supported");

    let mut multi_tile = params;
    multi_tile.m = 16;
    multi_tile.n = 16;
    validate_pearl_merge_config_for_recursive_prover(&supported, &multi_tile, 16).expect(
        "Pearl-compatible recursive preflight supports square-contiguous multi-tile tickets",
    );

    let mut wrong_difficulty = params;
    wrong_difficulty.difficulty_bits = 1;
    assert_eq!(
        validate_pearl_merge_config_for_recursive_prover(&supported, &wrong_difficulty, 16),
        Err(PearlCompatError::UnsupportedRecursivePearlParams(
            "difficulty_bits must be 0; Nockchain target is verifier-supplied"
        ))
    );

    let mut wrong_spot_checks = params;
    wrong_spot_checks.spot_checks = 2;
    assert_eq!(
        validate_pearl_merge_config_for_recursive_prover(&supported, &wrong_spot_checks, 16),
        Err(PearlCompatError::UnsupportedRecursivePearlParams(
            "spot_checks must be 1; Pearl-compatible mode proves one explicit ticket"
        ))
    );

    let mut wrong_rank = supported;
    wrong_rank.rank = 32;
    assert_eq!(
        validate_pearl_merge_config_for_recursive_prover(&wrong_rank, &params, 16),
        Err(PearlCompatError::RankMismatch)
    );

    let pearl_invalid_params = MatmulParams {
        k: 512,
        noise_rank: 32,
        ..params
    };
    pearl_invalid_params.validate_prod_envelope().unwrap();
    let pearl_invalid_config = PearlMiningConfig {
        common_dim: 512,
        rank: 32,
        ..supported
    };
    assert_eq!(
        validate_pearl_merge_config_for_recursive_prover(
            &pearl_invalid_config, &pearl_invalid_params, 16
        ),
        Err(PearlCompatError::PublicParamEnvelope)
    );

    let mut noncontiguous_params = params;
    noncontiguous_params.m = 128;
    noncontiguous_params.n = 128;
    let mut noncontiguous = supported;
    noncontiguous.rows_pattern =
        PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73]).unwrap();
    validate_pearl_merge_config_for_recursive_prover(&noncontiguous, &noncontiguous_params, 16)
        .expect(
            "bounded non-contiguous Pearl patterns are supported by the explicit schedule bridge",
        );

    let rectangular_params = MatmulParams {
        m: 128,
        n: 125,
        tile: 6,
        ..params
    };
    assert!(
        rectangular_params.validate().is_err(),
        "legacy native tile grid rejects this Pearl-valid rectangular schedule"
    );
    let rectangular = PearlMiningConfig {
        rows_pattern: simple_pattern(6),
        cols_pattern: simple_pattern(8),
        ..supported
    };
    validate_pearl_merge_config_for_recursive_prover(&rectangular, &rectangular_params, 16)
        .expect("Pearl h*w, not legacy square tile^2, determines ticket entropy");

    assert_eq!(
        validate_pearl_merge_config_for_recursive_prover(&supported, &params, 7),
        Err(PearlCompatError::PatternListTooLarge)
    );
}

#[test]
fn pearl_public_proof_params_round_trip_and_sanity_match_reference_shape() {
    let pattern = pearl_compat_test_pattern();
    let config = PearlMiningConfig {
        common_dim: 1216,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: pattern,
        cols_pattern: pattern,
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    let params = PearlPublicProofParams {
        block_header: header(),
        mining_config: config,
        hash_a: [0xA1; 32],
        hash_b: [0xB2; 32],
        hash_jackpot: [0xC3; 32],
        m: 6144,
        n: 4096,
        t_rows: 0,
        t_cols: 0,
    };

    params.sanity_check().unwrap();
    let public_data = params.to_public_data().unwrap();
    assert_eq!(public_data.len(), PEARL_PUBLIC_PROOF_PARAMS_SIZE);
    let restored = PearlPublicProofParams::from_public_data(header(), &public_data).unwrap();
    assert_eq!(restored, params);
    assert_eq!(
        restored.a_rows_indices_bounded(16).unwrap(),
        vec![0, 1, 8, 9, 64, 65, 72, 73]
    );
    assert_eq!(
        restored.b_cols_indices_bounded(16).unwrap(),
        vec![0, 1, 8, 9, 64, 65, 72, 73]
    );

    let row_partitions = restored.row_thread_partitions_bounded(8, 1024).unwrap();
    assert_eq!(row_partitions.len(), 6144 / 8);
    assert_eq!(row_partitions[0], vec![0, 1, 8, 9, 64, 65, 72, 73]);
}

#[test]
fn pearl_public_proof_params_reject_bad_offsets_and_envelope_violations() {
    let pattern = pearl_compat_test_pattern();
    let config = PearlMiningConfig {
        common_dim: 1216,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: pattern,
        cols_pattern: pattern,
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    let mut params = PearlPublicProofParams {
        block_header: header(),
        mining_config: config,
        hash_a: [0xA1; 32],
        hash_b: [0xB2; 32],
        hash_jackpot: [0xC3; 32],
        m: 6144,
        n: 4096,
        t_rows: 0,
        t_cols: 0,
    };

    let mut public_data = params.to_public_data().unwrap();
    public_data[156..160].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        PearlPublicProofParams::from_public_data(header(), &public_data),
        Err(PearlCompatError::InvalidPatternOffset)
    );

    params.m = 72;
    assert_eq!(
        params.sanity_check(),
        Err(PearlCompatError::PatternOutOfMatrix)
    );

    params.m = 6144;
    params.mining_config.rank = 4;
    assert_eq!(
        params.sanity_check(),
        Err(PearlCompatError::PublicParamEnvelope)
    );
}

#[test]
fn pearl_nbits_and_adjusted_target_match_reference_edges() {
    let small = pearl_nbits_to_target_le(0x0312_3456);
    assert_eq!(&small[..4], &[0x56, 0x34, 0x12, 0x00]);
    assert!(small[4..].iter().all(|&b| b == 0));

    let genesis = pearl_nbits_to_target_le(0x1d00_ffff);
    assert_eq!(genesis[26], 0xff);
    assert_eq!(genesis[27], 0xff);
    assert!(genesis[..26].iter().all(|&b| b == 0));
    assert!(genesis[28..].iter().all(|&b| b == 0));

    assert_eq!(pearl_nbits_to_target_le(0x1d00_0000), [0u8; 32]);
    assert_eq!(pearl_nbits_to_target_le(0x1d80_0000), [0u8; 32]);

    let exp_33 = pearl_nbits_to_target_le(0x2112_3456);
    assert_eq!(exp_33[30], 0x56);
    assert_eq!(exp_33[31], 0x34);
    assert!(exp_33[..30].iter().all(|&b| b == 0));

    let exp_34 = pearl_nbits_to_target_le(0x2212_3456);
    assert_eq!(exp_34[31], 0x56);
    assert!(exp_34[..31].iter().all(|&b| b == 0));

    assert_eq!(
        pearl_nbits_to_target_le(0x2312_3456),
        [0u8; 32],
        "Pearl U256 left-shift by 256 bits truncates to zero"
    );

    let adjusted = pearl_adjust_target_for_config(0x0312_3456, &mining_config()).unwrap();
    let expected = 0x123456u128 * 4096u128;
    assert_eq!(&adjusted[..16], &expected.to_le_bytes());
    assert!(adjusted[16..].iter().all(|&b| b == 0));

    let saturated = pearl_adjust_target_for_config(0x207f_ffff, &mining_config()).unwrap();
    assert_eq!(saturated, [0xffu8; 32]);
}

#[test]
fn pearl_and_nockchain_target_checks_are_independent() {
    let mut public = PearlPublicProofParams {
        block_header: PearlIncompleteBlockHeader {
            nbits: 0x0100_0001,
            ..header()
        },
        mining_config: mining_config(),
        hash_a: [0xA1; 32],
        hash_b: [0xB2; 32],
        hash_jackpot: [1u8; 32],
        m: 64,
        n: 64,
        t_rows: 0,
        t_cols: 0,
    };

    assert_eq!(
        public.check_pearl_jackpot_difficulty(),
        Err(PearlCompatError::PearlTargetNotMet)
    );
    public.hash_jackpot = [0u8; 32];
    assert_eq!(public.check_pearl_jackpot_difficulty(), Ok(()));

    public.hash_jackpot = {
        let mut bytes = [0u8; 32];
        bytes[0] = 2;
        bytes
    };
    let nock_target = {
        let mut bytes = [0u8; 32];
        bytes[0] = 1;
        bytes
    };
    let nock_adjusted = public.nockchain_adjusted_target(&nock_target).unwrap();
    assert_eq!(&nock_adjusted[..16], &4096u128.to_le_bytes());
    assert!(
        public.check_nockchain_jackpot_target(&nock_target).is_ok(),
        "Nockchain target checks use Pearl's h*w*dot_product_length work factor"
    );
    let nock_target = [0u8; 32];
    assert_eq!(
        public.check_nockchain_jackpot_target(&nock_target),
        Err(PearlCompatError::NockchainTargetNotMet)
    );
}

#[test]
fn pearl_pattern_ticket_matches_square_tile_when_patterns_are_contiguous() {
    let params = pearl_square_params();
    params.validate().unwrap();
    let config = pearl_square_config();
    let (a, b) = synth_matrices(b"pearl-pattern-square-ticket", &params);
    let attempt = PearlAttempt::build_with_config(&header(), &config, &a, &b, &params).unwrap();
    let first = &attempt.tile_digests[0];
    assert_eq!((first.tile_i, first.tile_j), (0, 0));

    let public = PearlPublicProofParams {
        block_header: header(),
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: first.jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };

    let ticket = verify_pearl_pattern_ticket(&public, &a, &b, &attempt.commitments, 16).unwrap();
    assert_eq!(ticket.a_rows, (0..8).collect::<Vec<u32>>());
    assert_eq!(ticket.b_cols, (0..8).collect::<Vec<u32>>());
    assert_eq!(ticket.tile_state, first.tile_state);
    assert_eq!(ticket.jackpot_hash, first.jackpot_hash);
}

#[test]
fn pearl_pattern_ticket_rejects_commitment_and_jackpot_tampering() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let (a, b) = synth_matrices(b"pearl-pattern-ticket-tamper", &params);
    let attempt = PearlAttempt::build_with_config(&header(), &config, &a, &b, &params).unwrap();
    let mut public = PearlPublicProofParams {
        block_header: header(),
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };

    public.hash_a[0] ^= 0x01;
    assert_eq!(
        compute_pearl_pattern_ticket(&public, &a, &b, &attempt.commitments, 16),
        Err(PearlCompatError::PublicCommitmentMismatch)
    );

    public.hash_a = attempt.commitments.h_a;
    public.hash_jackpot[0] ^= 0x01;
    assert_eq!(
        verify_pearl_pattern_ticket(&public, &a, &b, &attempt.commitments, 16),
        Err(PearlCompatError::JackpotHashMismatch)
    );
}

#[test]
fn pearl_pattern_ticket_computes_noncontiguous_pattern_indices() {
    let params = pearl_square_params();
    let (a, b) = synth_matrices(b"pearl-pattern-noncontiguous-ticket", &params);
    let config = PearlMiningConfig {
        common_dim: params.k,
        rank: params.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: pearl_compat_test_pattern(),
        cols_pattern: pearl_compat_test_pattern(),
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    let commitments = ai_pow::pearl_compat::derive_pearl_work_commitments(
        &header().to_bytes(),
        &config.to_bytes().unwrap(),
        &a,
        &b,
    );
    let mut public = PearlPublicProofParams {
        block_header: header(),
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: [0u8; 32],
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };

    let ticket = compute_pearl_pattern_ticket(&public, &a, &b, &commitments, 16).unwrap();
    assert_eq!(ticket.a_rows, vec![0, 1, 8, 9, 64, 65, 72, 73]);
    assert_eq!(ticket.b_cols, vec![0, 1, 8, 9, 64, 65, 72, 73]);
    public.hash_jackpot = ticket.jackpot_hash;
    assert_eq!(
        verify_pearl_pattern_ticket(&public, &a, &b, &commitments, 16).unwrap(),
        ticket
    );
}

#[test]
fn pearl_attempt_transcript_matches_reference_formulas() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"pearl-merge-transcript", &params);
    let header = header();
    let config = mining_config();
    let sigma = header.to_bytes();
    let mu = config.to_bytes().unwrap();
    let attempt = PearlAttempt::build_with_config(&header, &config, &a, &b, &params).unwrap();

    let mut raw = Vec::new();
    raw.extend_from_slice(&sigma);
    raw.extend_from_slice(&mu);
    let expected_kappa = *blake3::hash(&raw).as_bytes();
    assert_eq!(attempt.commitments.kappa, expected_kappa);
    assert_eq!(attempt.commitments.kappa, pearl_kappa(&sigma, &mu));

    let a_bytes: Vec<u8> = a.iter().map(|&v| v as u8).collect();
    let b_bytes: Vec<u8> = b.iter().map(|&v| v as u8).collect();
    let expected_h_a = matrix_commitment(&a_bytes, &expected_kappa);
    let expected_h_b = matrix_commitment(&b_bytes, &expected_kappa);
    assert_eq!(attempt.commitments.h_a, expected_h_a);
    assert_eq!(attempt.commitments.h_b, expected_h_b);

    let expected_s_b = noise_seed_b(&expected_kappa, &expected_h_b);
    let expected_s_a = noise_seed_a(&expected_s_b, &expected_h_a);
    assert_eq!(attempt.commitments.s_a, expected_s_a);
    assert_eq!(attempt.commitments.s_b, expected_s_b);

    assert_eq!(attempt.tile_digests.len(), params.num_tiles() as usize);
    let first = &attempt.tile_digests[0];
    assert_eq!(first.tile_i, 0);
    assert_eq!(first.tile_j, 0);
    assert_eq!(
        first.jackpot_hash,
        first.tile_state.keyed_hash(&attempt.commitments.s_a),
        "Pearl-compatible jackpot hashing must key directly with s_A"
    );
}

#[test]
fn pearl_attempt_is_not_the_native_explicit_nonce_path() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"pearl-merge-native-separation", &params);
    let header = header();
    let config = mining_config();
    let sigma = header.to_bytes();
    let pearl = PearlAttempt::build_with_config(&header, &config, &a, &b, &params).unwrap();

    let native_nonce = b"native-explicit-nonce-bytes";
    let native = BlockContext::build(&sigma, native_nonce, &a, &b, &params).unwrap();
    assert_ne!(
        pearl.commitments.kappa,
        *native.kappa(),
        "native mode length-prefixes block/nonce and hashes params_tag; Pearl mode is sigma || mu"
    );
    assert_ne!(
        pearl.commitments.s_a,
        native.pow_key(),
        "Pearl-compatible mode must not use pow_key_for_nonce(s_A, nonce)"
    );

    let native_selected = ai_pow::fiat_shamir::attempt_tile_index(
        native.attempt_state(),
        &params_tag(&params),
        native.s_a(),
        params.num_tiles(),
    ) as usize;
    assert!(native_selected < pearl.tile_digests.len());
    assert_eq!(
        pearl.tile_digests.len(),
        params.num_tiles() as usize,
        "Pearl-compatible mining exposes every legal tile as a ticket"
    );
}

#[test]
fn changing_sigma_changes_pearl_work_before_jackpot_hashing() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"pearl-merge-sigma-binding", &params);
    let sigma_a = header().to_bytes();
    let mut sigma_b = sigma_a;
    sigma_b[10] ^= 0x80;
    let mu = mining_config().to_bytes().unwrap();
    let attempt_a = PearlAttempt::build_from_serialized(&sigma_a, &mu, &a, &b, &params).unwrap();
    let attempt_b = PearlAttempt::build_from_serialized(&sigma_b, &mu, &a, &b, &params).unwrap();

    assert_ne!(attempt_a.commitments.kappa, attempt_b.commitments.kappa);
    assert_ne!(attempt_a.commitments.h_a, attempt_b.commitments.h_a);
    assert_ne!(attempt_a.commitments.h_b, attempt_b.commitments.h_b);
    assert_ne!(attempt_a.commitments.s_a, attempt_b.commitments.s_a);
    assert_ne!(
        attempt_a.tile_digests[0].jackpot_hash,
        attempt_b.tile_digests[0].jackpot_hash
    );

    assert_ne!(sigma_a, sigma_b);
}

#[test]
fn changing_ticket_offset_reuses_work_instance_but_changes_opened_ticket() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-ticket-offset-binding", &params);
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };

    let first = evaluate_pearl_merge_ticket_attempt(
        &easy_header,
        &config,
        &params,
        0,
        0,
        &a,
        &b,
        &[0xff; 32],
        16,
        aux.clone(),
    )
    .expect("evaluate first Pearl ticket");
    let second = evaluate_pearl_merge_ticket_attempt(
        &easy_header, &config, &params, 8, 0, &a, &b, &[0xff; 32], 16, aux,
    )
    .expect("evaluate second Pearl ticket");

    assert_eq!(first.commitments, second.commitments);
    assert_eq!(first.public_params.hash_a, second.public_params.hash_a);
    assert_eq!(first.public_params.hash_b, second.public_params.hash_b);
    assert_ne!(first.ticket.a_rows, second.ticket.a_rows);
    assert_eq!(first.ticket.b_cols, second.ticket.b_cols);
    assert_ne!(first.ticket.tile_state, second.ticket.tile_state);
    assert_ne!(
        first.public_params.hash_jackpot,
        second.public_params.hash_jackpot
    );
}

#[test]
fn pearl_attempt_rejects_config_that_does_not_match_params() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"pearl-merge-config-binding", &params);
    let header = header();
    let mut config = mining_config();

    config.common_dim = params.k + 1;
    assert_eq!(
        PearlAttempt::build_with_config(&header, &config, &a, &b, &params),
        Err(PearlCompatError::CommonDimMismatch)
    );

    config = mining_config();
    config.rank = (params.noise_rank + 1) as u16;
    assert_eq!(
        PearlAttempt::build_with_config(&header, &config, &a, &b, &params),
        Err(PearlCompatError::RankMismatch)
    );
}

#[test]
fn pearl_compatible_work_precheck_accepts_shared_attempt_for_both_targets() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-compatible-work-precheck", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let nockchain_target = [0xffu8; 32];

    let precheck = verify_pearl_compatible_work(&public, &a, &b, &nockchain_target, 16).unwrap();
    assert_eq!(precheck.commitments, attempt.commitments);
    assert_eq!(precheck.ticket.jackpot_hash, public.hash_jackpot);
    assert_eq!(
        precheck.ticket.tile_state,
        attempt.tile_digests[0].tile_state
    );
    assert_eq!(precheck.pearl_target, [0xffu8; 32]);
    assert_eq!(precheck.nockchain_target, nockchain_target);
}

#[test]
fn pearl_compatible_work_precheck_rejects_tampered_statement_fields() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-compatible-work-tamper", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let nockchain_target = [0xffu8; 32];

    let mut bad_header = public.clone();
    bad_header.block_header.timestamp ^= 1;
    assert_eq!(
        verify_pearl_compatible_work(&bad_header, &a, &b, &nockchain_target, 16),
        Err(PearlCompatError::PublicCommitmentMismatch)
    );

    let mut bad_config = public.clone();
    bad_config.mining_config.rows_pattern = pearl_compat_test_pattern();
    assert_eq!(
        verify_pearl_compatible_work(&bad_config, &a, &b, &nockchain_target, 16),
        Err(PearlCompatError::PublicCommitmentMismatch)
    );

    let mut bad_jackpot = public.clone();
    bad_jackpot.hash_jackpot[0] ^= 1;
    assert_eq!(
        verify_pearl_compatible_work(&bad_jackpot, &a, &b, &nockchain_target, 16),
        Err(PearlCompatError::JackpotHashMismatch)
    );
}

#[test]
fn pearl_compatible_work_precheck_rejects_target_and_input_failures() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-compatible-work-targets", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let mut public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    assert_ne!(public.hash_jackpot, [0u8; 32]);

    let priced_nockchain_target = u256_le_div_ceil_u32(&public.hash_jackpot, 65_536);
    assert!(
        !hash_le_target(&public.hash_jackpot, &priced_nockchain_target),
        "fixture must be above the raw chain target"
    );
    let priced_precheck =
        verify_pearl_compatible_work(&public, &a, &b, &priced_nockchain_target, 16)
            .expect("Nockchain-side precheck must apply Pearl's ticket work factor");
    assert_eq!(priced_precheck.nockchain_target, priced_nockchain_target);
    assert!(hash_le_target(
        &public.hash_jackpot, &priced_precheck.nockchain_adjusted_target
    ));

    let nockchain_target = [0u8; 32];
    assert_eq!(
        verify_pearl_compatible_work(&public, &a, &b, &nockchain_target, 16),
        Err(PearlCompatError::NockchainTargetNotMet)
    );

    let hard_header = PearlIncompleteBlockHeader {
        nbits: 0x0100_0001,
        ..header()
    };
    let hard_attempt =
        PearlAttempt::build_with_config(&hard_header, &config, &a, &b, &params).unwrap();
    public = PearlPublicProofParams {
        block_header: hard_header,
        mining_config: config,
        hash_a: hard_attempt.commitments.h_a,
        hash_b: hard_attempt.commitments.h_b,
        hash_jackpot: hard_attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let nockchain_target = [0xffu8; 32];
    assert!(public.check_pearl_jackpot_difficulty().is_err());
    verify_pearl_compatible_work(&public, &a, &b, &nockchain_target, 16)
        .expect("Nockchain-side precheck should not require the Pearl target");

    public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    assert_eq!(
        verify_pearl_compatible_work(&public, &a[..a.len() - 1], &b, &nockchain_target, 16),
        Err(PearlCompatError::InputAShape {
            expected: params.m as usize * params.k as usize,
            actual: params.m as usize * params.k as usize - 1,
        })
    );
}

#[test]
fn pearl_compatible_public_data_precheck_accepts_exact_wire_bytes() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-compatible-public-data", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let header_bytes = easy_header.to_bytes();
    let public_data = public.to_public_data().unwrap();
    let nockchain_target = [0xffu8; 32];

    let precheck = verify_pearl_compatible_public_data(
        &header_bytes, &public_data, &a, &b, &nockchain_target, 16,
    )
    .unwrap();
    assert_eq!(precheck.commitments, attempt.commitments);
    assert_eq!(precheck.ticket.jackpot_hash, public.hash_jackpot);
}

#[test]
fn pearl_compatible_public_data_precheck_rejects_bad_wire_boundaries() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-compatible-public-data-boundaries", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let header_bytes = easy_header.to_bytes();
    let public_data = public.to_public_data().unwrap();
    let nockchain_target = [0xffu8; 32];

    assert_eq!(
        verify_pearl_compatible_public_data(
            &header_bytes[..PEARL_INCOMPLETE_BLOCK_HEADER_SIZE - 1],
            &public_data,
            &a,
            &b,
            &nockchain_target,
            16,
        ),
        Err(PearlCompatError::BadHeaderLen(
            PEARL_INCOMPLETE_BLOCK_HEADER_SIZE - 1
        ))
    );

    let mut header_with_trailing = header_bytes.to_vec();
    header_with_trailing.push(0);
    assert_eq!(
        verify_pearl_compatible_public_data(
            &header_with_trailing, &public_data, &a, &b, &nockchain_target, 16,
        ),
        Err(PearlCompatError::BadHeaderLen(
            PEARL_INCOMPLETE_BLOCK_HEADER_SIZE + 1
        ))
    );

    assert_eq!(
        verify_pearl_compatible_public_data(
            &header_bytes,
            &public_data[..PEARL_PUBLIC_PROOF_PARAMS_SIZE - 1],
            &a,
            &b,
            &nockchain_target,
            16,
        ),
        Err(PearlCompatError::BadPublicParamsLen(
            PEARL_PUBLIC_PROOF_PARAMS_SIZE - 1
        ))
    );

    let mut public_data_with_trailing = public_data.to_vec();
    public_data_with_trailing.push(0);
    assert_eq!(
        verify_pearl_compatible_public_data(
            &header_bytes, &public_data_with_trailing, &a, &b, &nockchain_target, 16,
        ),
        Err(PearlCompatError::BadPublicParamsLen(
            PEARL_PUBLIC_PROOF_PARAMS_SIZE + 1
        ))
    );
}

#[test]
fn pearl_compatible_public_data_precheck_rejects_decode_time_tampering() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-compatible-public-data-tamper", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let header_bytes = easy_header.to_bytes();
    let mut public_data = public.to_public_data().unwrap();
    let nockchain_target = [0xffu8; 32];

    public_data[156..160].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        verify_pearl_compatible_public_data(
            &header_bytes, &public_data, &a, &b, &nockchain_target, 16,
        ),
        Err(PearlCompatError::InvalidPatternOffset)
    );

    public_data = public.to_public_data().unwrap();
    public_data[52] ^= 1;
    assert_eq!(
        verify_pearl_compatible_public_data(
            &header_bytes, &public_data, &a, &b, &nockchain_target, 16,
        ),
        Err(PearlCompatError::PublicCommitmentMismatch)
    );
}

#[test]
fn pearl_nockchain_aux_commitment_matches_explicit_bounded_encoding() {
    let chain_id = b"nockchain-mainnet";
    let nock_block_commitment = [0x42; 32];
    let epoch = 123_456u64;
    let extra = b"ai-pow-target-window";

    let commitment =
        pearl_nockchain_aux_commitment(chain_id, &nock_block_commitment, epoch, extra).unwrap();

    let mut hasher = blake3::Hasher::new();
    hasher.update(PEARL_NOCKCHAIN_AUX_DOMAIN);
    hasher.update(&(chain_id.len() as u32).to_le_bytes());
    hasher.update(chain_id);
    hasher.update(&nock_block_commitment);
    hasher.update(&epoch.to_le_bytes());
    hasher.update(&(extra.len() as u32).to_le_bytes());
    hasher.update(extra);
    assert_eq!(commitment, *hasher.finalize().as_bytes());
}

#[test]
fn pearl_nockchain_aux_commitment_binds_every_replay_protection_field() {
    let chain_id = b"nockchain-mainnet";
    let nock_block_commitment = [0x42; 32];
    let epoch = 123_456u64;
    let extra = b"ai-pow-target-window";
    let baseline =
        pearl_nockchain_aux_commitment(chain_id, &nock_block_commitment, epoch, extra).unwrap();

    assert_ne!(
        baseline,
        pearl_nockchain_aux_commitment(b"nockchain-testnet", &nock_block_commitment, epoch, extra)
            .unwrap()
    );

    let mut other_block = nock_block_commitment;
    other_block[7] ^= 1;
    assert_ne!(
        baseline,
        pearl_nockchain_aux_commitment(chain_id, &other_block, epoch, extra).unwrap()
    );

    assert_ne!(
        baseline,
        pearl_nockchain_aux_commitment(chain_id, &nock_block_commitment, epoch + 1, extra).unwrap()
    );
    assert_ne!(
        baseline,
        pearl_nockchain_aux_commitment(chain_id, &nock_block_commitment, epoch, b"").unwrap()
    );
}

#[test]
fn pearl_nockchain_aux_commitment_length_prefixes_variable_fields() {
    let block = [0xAA; 32];
    let epoch = 9u64;
    let left = pearl_nockchain_aux_commitment(b"ab", &block, epoch, b"c").unwrap();
    let right = pearl_nockchain_aux_commitment(b"a", &block, epoch, b"bc").unwrap();
    assert_ne!(left, right);
}

#[test]
fn pearl_nockchain_aux_commitment_rejects_unbounded_or_missing_fields() {
    let block = [0x11; 32];
    assert_eq!(
        pearl_nockchain_aux_commitment(b"", &block, 0, b""),
        Err(PearlCompatError::NockchainAuxChainIdEmpty)
    );

    let long_chain_id = vec![0x22; PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX + 1];
    assert_eq!(
        pearl_nockchain_aux_commitment(&long_chain_id, &block, 0, b""),
        Err(PearlCompatError::NockchainAuxChainIdTooLarge(
            PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX + 1
        ))
    );

    let long_extra = vec![0x33; PEARL_NOCKCHAIN_AUX_EXTRA_MAX + 1];
    assert_eq!(
        pearl_nockchain_aux_commitment(b"nockchain-mainnet", &block, 0, &long_extra),
        Err(PearlCompatError::NockchainAuxExtraTooLarge(
            PEARL_NOCKCHAIN_AUX_EXTRA_MAX + 1
        ))
    );
}

#[test]
fn pearl_nockchain_aux_bytes_round_trip_exact_shape() {
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };

    let bytes = aux.to_bytes().unwrap();
    assert_eq!(&bytes[0..4], &PEARL_NOCKCHAIN_AUX_MAGIC);
    assert_eq!(bytes[4] as usize, aux.nockchain_chain_id.len());
    assert_eq!(
        &bytes[5..5 + aux.nockchain_chain_id.len()],
        aux.nockchain_chain_id.as_slice()
    );
    let block_offset = 5 + aux.nockchain_chain_id.len();
    assert_eq!(
        &bytes[block_offset..block_offset + 32],
        &aux.nock_block_commitment
    );
    let epoch_offset = block_offset + 32;
    assert_eq!(
        &bytes[epoch_offset..epoch_offset + 8],
        &aux.nockchain_target_epoch_or_height.to_le_bytes()
    );
    let extra_len_offset = epoch_offset + 8;
    assert_eq!(
        &bytes[extra_len_offset..extra_len_offset + 2],
        &(aux.extra_domain_data.len() as u16).to_le_bytes()
    );
    assert_eq!(
        &bytes[extra_len_offset + 2..],
        aux.extra_domain_data.as_slice()
    );
    assert_eq!(PearlNockchainAux::from_bytes(&bytes).unwrap(), aux);
}

#[test]
fn pearl_nockchain_aux_bytes_reject_malformed_encodings() {
    assert_eq!(
        PearlNockchainAux::from_bytes(&[0u8; 8]),
        Err(PearlCompatError::BadNockchainAuxLen(8))
    );

    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let bytes = aux.to_bytes().unwrap();

    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    assert_eq!(
        PearlNockchainAux::from_bytes(&bad_magic),
        Err(PearlCompatError::BadNockchainAuxMagic(*b"XPA1"))
    );

    let mut empty_chain = bytes.clone();
    empty_chain[4] = 0;
    assert_eq!(
        PearlNockchainAux::from_bytes(&empty_chain),
        Err(PearlCompatError::NockchainAuxChainIdEmpty)
    );

    let mut long_chain = Vec::new();
    long_chain.extend_from_slice(&PEARL_NOCKCHAIN_AUX_MAGIC);
    long_chain.push((PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX + 1) as u8);
    long_chain.extend(std::iter::repeat_n(
        b'n',
        PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX + 1,
    ));
    long_chain.extend_from_slice(&[0x42; 32]);
    long_chain.extend_from_slice(&123_456u64.to_le_bytes());
    long_chain.extend_from_slice(&0u16.to_le_bytes());
    assert_eq!(
        PearlNockchainAux::from_bytes(&long_chain),
        Err(PearlCompatError::NockchainAuxChainIdTooLarge(
            PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX + 1
        ))
    );

    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        PearlNockchainAux::from_bytes(&trailing),
        Err(PearlCompatError::NockchainAuxTrailingData {
            expected: bytes.len(),
            actual: bytes.len() + 1,
        })
    );

    let mut long_extra = Vec::new();
    long_extra.extend_from_slice(&PEARL_NOCKCHAIN_AUX_MAGIC);
    long_extra.push(1);
    long_extra.push(b'n');
    long_extra.extend_from_slice(&[0x42; 32]);
    long_extra.extend_from_slice(&123_456u64.to_le_bytes());
    long_extra.extend_from_slice(&((PEARL_NOCKCHAIN_AUX_EXTRA_MAX + 1) as u16).to_le_bytes());
    long_extra.extend(std::iter::repeat_n(0x33, PEARL_NOCKCHAIN_AUX_EXTRA_MAX + 1));
    assert_eq!(
        PearlNockchainAux::from_bytes(&long_extra),
        Err(PearlCompatError::NockchainAuxExtraTooLarge(
            PEARL_NOCKCHAIN_AUX_EXTRA_MAX + 1
        ))
    );
}

#[test]
fn pearl_merge_mining_precheck_binds_work_to_expected_aux_commitment() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-mining-precheck", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let header_bytes = easy_header.to_bytes();
    let public_data = public.to_public_data().unwrap();
    let nockchain_target = [0xffu8; 32];
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let expected_aux_commitment = aux.commitment().unwrap();

    let precheck = verify_pearl_merge_mining_public_data(
        &aux.nock_block_commitment,
        &header_bytes,
        &public_data,
        &a,
        &b,
        &nockchain_target,
        16,
        aux.clone(),
        &expected_aux_commitment,
    )
    .unwrap();
    assert_eq!(precheck.work.commitments, attempt.commitments);
    assert_eq!(precheck.work.ticket.jackpot_hash, public.hash_jackpot);
    assert_eq!(precheck.aux, aux);
    assert_eq!(precheck.aux_commitment, expected_aux_commitment);
}

#[test]
fn pearl_merge_mining_precheck_accepts_canonical_aux_bytes() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-mining-aux-bytes", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let header_bytes = easy_header.to_bytes();
    let public_data = public.to_public_data().unwrap();
    let nockchain_target = [0xffu8; 32];
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let aux_bytes = aux.to_bytes().unwrap();
    let expected_aux_commitment = aux.commitment().unwrap();

    let precheck = verify_pearl_merge_mining_public_data_with_aux_bytes(
        &aux.nock_block_commitment, &header_bytes, &public_data, &a, &b, &nockchain_target, 16,
        &aux_bytes, &expected_aux_commitment,
    )
    .unwrap();
    assert_eq!(precheck.work.commitments, attempt.commitments);
    assert_eq!(precheck.aux, aux);
    assert_eq!(precheck.aux_commitment, expected_aux_commitment);
}

#[test]
fn pearl_merge_mining_precheck_rejects_malformed_aux_bytes() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-mining-bad-aux-bytes", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let header_bytes = easy_header.to_bytes();
    let public_data = public.to_public_data().unwrap();
    let nockchain_target = [0xffu8; 32];
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let expected_aux_commitment = aux.commitment().unwrap();

    assert_eq!(
        verify_pearl_merge_mining_public_data_with_aux_bytes(
            &aux.nock_block_commitment, &header_bytes, &public_data, &a, &b, &nockchain_target, 16,
            &[0u8; 8], &expected_aux_commitment,
        ),
        Err(PearlCompatError::BadNockchainAuxLen(8))
    );

    let mut trailing = aux.to_bytes().unwrap();
    let expected_len = trailing.len();
    trailing.push(0);
    assert_eq!(
        verify_pearl_merge_mining_public_data_with_aux_bytes(
            &aux.nock_block_commitment, &header_bytes, &public_data, &a, &b, &nockchain_target, 16,
            &trailing, &expected_aux_commitment,
        ),
        Err(PearlCompatError::NockchainAuxTrailingData {
            expected: expected_len,
            actual: expected_len + 1,
        })
    );
}

#[test]
fn pearl_merge_public_statement_bytes_round_trip_and_verify() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-public-statement-bytes", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let statement = PearlMergePublicStatement {
        block_header: easy_header.to_bytes(),
        public_data: public.to_public_data().unwrap(),
        expected_aux_commitment: aux.commitment().unwrap(),
        aux_bytes: aux.to_bytes().unwrap(),
    };
    let bytes = statement.to_bytes().unwrap();
    assert_eq!(&bytes[0..4], &PEARL_MERGE_PUBLIC_STATEMENT_MAGIC);
    assert_eq!(
        PearlMergePublicStatement::from_bytes(&bytes).unwrap(),
        statement
    );

    let precheck = verify_pearl_merge_public_statement_bytes(
        &aux.nock_block_commitment, &bytes, &a, &b, &[0xffu8; 32], 16,
    )
    .unwrap();
    assert_eq!(precheck.work.commitments, attempt.commitments);
    assert_eq!(precheck.aux, aux);

    let expected_aux_offset =
        4 + PEARL_INCOMPLETE_BLOCK_HEADER_SIZE + PEARL_PUBLIC_PROOF_PARAMS_SIZE;
    let mut bad_expected_aux = bytes;
    bad_expected_aux[expected_aux_offset] ^= 1;
    assert_eq!(
        verify_pearl_merge_public_statement_bytes(
            &aux.nock_block_commitment, &bad_expected_aux, &a, &b, &[0xffu8; 32], 16,
        ),
        Err(PearlCompatError::NockchainAuxCommitmentMismatch)
    );
}

#[test]
fn pearl_merge_public_statement_with_aux_inclusion_closes_header_binding() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let aux_commitment = aux.commitment().unwrap();
    let coinbase = coinbase_tx_with_script(&coinbase_aux_script(&aux_commitment), None);
    let mut easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    easy_header.merkle_root = display_root_from_raw(pearl_bitcoin_double_sha256_raw(&coinbase));
    let inclusion_proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    let (a, b) = synth_matrices(b"pearl-merge-public-statement-aux-inclusion", &params);
    let attempt = evaluate_pearl_merge_ticket_attempt(
        &easy_header,
        &config,
        &params,
        0,
        0,
        &a,
        &b,
        &[0xffu8; 32],
        16,
        aux.clone(),
    )
    .unwrap();
    let statement_bytes = attempt.statement.to_bytes().unwrap();

    let precheck = verify_pearl_merge_public_statement_bytes_with_aux_inclusion(
        &aux.nock_block_commitment, &statement_bytes, &a, &b, &[0xffu8; 32], 16, &inclusion_proof,
    )
    .unwrap();
    assert_eq!(precheck.aux_commitment, aux_commitment);
    assert_eq!(precheck.work.ticket, attempt.ticket);

    let bad_proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase_tx_with_script(&[0x01, 0x00], None),
        merkle_branch: vec![],
    };
    assert_eq!(
        verify_pearl_merge_public_statement_bytes_with_aux_inclusion(
            &aux.nock_block_commitment, &statement_bytes, &a, &b, &[0xffu8; 32], 16, &bad_proof,
        ),
        Err(PearlCompatError::PearlAuxCommitmentTagMissing)
    );
}

#[test]
fn pearl_merge_public_statement_with_aux_inclusion_uses_nockchain_target_only() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x24; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let aux_commitment = aux.commitment().unwrap();
    let coinbase = coinbase_tx_with_script(&coinbase_aux_script(&aux_commitment), None);
    let coinbase_txid_raw = pearl_bitcoin_double_sha256_raw(&coinbase);
    let mut hard_pearl_header = PearlIncompleteBlockHeader {
        nbits: 0x0100_0001,
        ..header()
    };
    hard_pearl_header.merkle_root = display_root_from_raw(coinbase_txid_raw);
    let inclusion_proof = PearlAuxInclusionProof {
        coinbase_tx: coinbase,
        merkle_branch: vec![],
    };

    let (a, b) = synth_matrices(
        b"pearl-merge-public-statement-nockchain-target-only", &params,
    );
    let attempt = evaluate_pearl_merge_ticket_attempt(
        &hard_pearl_header,
        &config,
        &params,
        0,
        0,
        &a,
        &b,
        &[0xffu8; 32],
        16,
        aux.clone(),
    )
    .unwrap();
    assert!(
        attempt
            .public_params
            .check_pearl_jackpot_difficulty()
            .is_err(),
        "fixture must miss Pearl nbits target"
    );

    verify_pearl_merge_public_statement_bytes_with_aux_inclusion(
        &aux.nock_block_commitment,
        &attempt.statement.to_bytes().unwrap(),
        &a,
        &b,
        &[0xffu8; 32],
        16,
        &inclusion_proof,
    )
    .expect("Nockchain-side merge statement must not require Pearl target satisfaction");

    assert_eq!(
        verify_pearl_merge_public_statement_bytes_with_aux_inclusion(
            &aux.nock_block_commitment,
            &attempt.statement.to_bytes().unwrap(),
            &a,
            &b,
            &[0u8; 32],
            16,
            &inclusion_proof,
        ),
        Err(PearlCompatError::NockchainTargetNotMet)
    );
}

#[test]
fn pearl_merge_ticket_attempt_builds_statement_for_one_explicit_ticket() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-ticket-attempt", &params);
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };

    let attempt = mine_pearl_merge_ticket_attempt(
        &easy_header,
        &config,
        &params,
        0,
        0,
        &a,
        &b,
        &[0xffu8; 32],
        16,
        aux.clone(),
    )
    .unwrap()
    .expect("easy Pearl and Nockchain targets should accept the explicit ticket");

    assert_eq!(attempt.public_params.t_rows, 0);
    assert_eq!(attempt.public_params.t_cols, 0);
    assert_eq!(attempt.public_params.hash_a, attempt.commitments.h_a);
    assert_eq!(attempt.public_params.hash_b, attempt.commitments.h_b);
    assert_eq!(
        attempt.public_params.hash_jackpot,
        attempt.ticket.jackpot_hash
    );
    assert_eq!(attempt.aux, aux);
    assert_eq!(attempt.aux_commitment, aux.commitment().unwrap());

    let statement_bytes = attempt.statement.to_bytes().unwrap();
    let precheck = verify_pearl_merge_public_statement_bytes(
        &aux.nock_block_commitment, &statement_bytes, &a, &b, &[0xffu8; 32], 16,
    )
    .unwrap();
    assert_eq!(precheck.work.commitments, attempt.commitments);
    assert_eq!(precheck.work.ticket, attempt.ticket);
    assert_eq!(precheck.aux, aux);
}

#[test]
fn pearl_merge_ticket_attempt_supports_rectangular_non_native_tile_grid() {
    let params = MatmulParams {
        m: 128,
        k: 1024,
        n: 125,
        noise_rank: 64,
        tile: 6,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    assert!(
        params.validate().is_err(),
        "native square tile grid rejects this Pearl-valid schedule"
    );
    let config = PearlMiningConfig {
        common_dim: params.k,
        rank: params.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: simple_pattern(6),
        cols_pattern: simple_pattern(8),
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-ticket-rectangular-native-grid", &params);
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };

    let attempt = evaluate_pearl_merge_ticket_attempt(
        &easy_header,
        &config,
        &params,
        0,
        0,
        &a,
        &b,
        &[0xffu8; 32],
        16,
        aux.clone(),
    )
    .expect("Pearl-valid rectangular explicit ticket");
    assert_eq!(attempt.ticket.a_rows, vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(attempt.ticket.b_cols, vec![0, 1, 2, 3, 4, 5, 6, 7]);

    let statement_bytes = attempt.statement.to_bytes().unwrap();
    let precheck = verify_pearl_merge_public_statement_bytes(
        &aux.nock_block_commitment, &statement_bytes, &a, &b, &[0xffu8; 32], 16,
    )
    .unwrap();
    assert_eq!(precheck.work.ticket, attempt.ticket);
}

#[test]
fn pearl_merge_ticket_attempt_returns_none_before_zkp_when_target_fails() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-ticket-target-fail", &params);
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };

    let miss = mine_pearl_merge_ticket_attempt(
        &easy_header, &config, &params, 0, 0, &a, &b, &[0u8; 32], 16, aux,
    )
    .unwrap();
    assert_eq!(miss, None);
}

#[test]
fn pearl_merge_ticket_attempt_rejects_invalid_offsets_and_supports_pattern_offsets() {
    let params = pearl_square_params();
    let config = PearlMiningConfig {
        common_dim: params.k,
        rank: params.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: pearl_compat_test_pattern(),
        cols_pattern: pearl_compat_test_pattern(),
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-ticket-pattern-offsets", &params);
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };

    let attempt = evaluate_pearl_merge_ticket_attempt(
        &easy_header,
        &config,
        &params,
        2,
        4,
        &a,
        &b,
        &[0xffu8; 32],
        16,
        aux.clone(),
    )
    .unwrap();
    assert_eq!(attempt.ticket.a_rows, vec![2, 3, 10, 11, 66, 67, 74, 75]);
    assert_eq!(attempt.ticket.b_cols, vec![4, 5, 12, 13, 68, 69, 76, 77]);

    assert_eq!(
        evaluate_pearl_merge_ticket_attempt(
            &easy_header, &config, &params, 8, 0, &a, &b, &[0xffu8; 32], 16, aux,
        ),
        Err(PearlCompatError::InvalidPatternOffset)
    );
}

#[test]
fn pearl_merge_public_statement_bytes_reject_malformed_envelopes() {
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let statement = PearlMergePublicStatement {
        block_header: header().to_bytes(),
        public_data: PearlPublicProofParams {
            block_header: header(),
            mining_config: pearl_square_config(),
            hash_a: [0xA1; 32],
            hash_b: [0xB2; 32],
            hash_jackpot: [0u8; 32],
            m: pearl_square_params().m,
            n: pearl_square_params().n,
            t_rows: 0,
            t_cols: 0,
        }
        .to_public_data()
        .unwrap(),
        expected_aux_commitment: aux.commitment().unwrap(),
        aux_bytes: aux.to_bytes().unwrap(),
    };
    let bytes = statement.to_bytes().unwrap();

    assert_eq!(
        PearlMergePublicStatement::from_bytes(&[0u8; 16]),
        Err(PearlCompatError::BadMergePublicStatementLen(16))
    );

    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    assert_eq!(
        PearlMergePublicStatement::from_bytes(&bad_magic),
        Err(PearlCompatError::BadMergePublicStatementMagic(*b"XMP1"))
    );

    let aux_len_offset =
        4 + PEARL_INCOMPLETE_BLOCK_HEADER_SIZE + PEARL_PUBLIC_PROOF_PARAMS_SIZE + 32;
    let mut bad_len = bytes.clone();
    let declared = statement.aux_bytes.len() - 1;
    bad_len[aux_len_offset..aux_len_offset + 2].copy_from_slice(&(declared as u16).to_le_bytes());
    assert_eq!(
        PearlMergePublicStatement::from_bytes(&bad_len),
        Err(PearlCompatError::MergePublicStatementTrailingData {
            expected: bytes.len() - 1,
            actual: bytes.len(),
        })
    );

    let mut bad_aux = bytes;
    bad_aux[aux_len_offset + 2] = b'X';
    assert_eq!(
        PearlMergePublicStatement::from_bytes(&bad_aux),
        Err(PearlCompatError::BadNockchainAuxMagic(*b"XPA1"))
    );

    let mut malformed_aux_statement = statement;
    malformed_aux_statement.aux_bytes[0] = b'X';
    assert_eq!(
        malformed_aux_statement.to_bytes(),
        Err(PearlCompatError::BadNockchainAuxMagic(*b"XPA1"))
    );
}

#[test]
fn pearl_merge_mining_precheck_rejects_aux_and_work_tampering() {
    let params = pearl_square_params();
    let config = pearl_square_config();
    let easy_header = PearlIncompleteBlockHeader {
        nbits: 0x207f_ffff,
        ..header()
    };
    let (a, b) = synth_matrices(b"pearl-merge-mining-precheck-tamper", &params);
    let attempt = PearlAttempt::build_with_config(&easy_header, &config, &a, &b, &params).unwrap();
    let mut public = PearlPublicProofParams {
        block_header: easy_header,
        mining_config: config,
        hash_a: attempt.commitments.h_a,
        hash_b: attempt.commitments.h_b,
        hash_jackpot: attempt.tile_digests[0].jackpot_hash,
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let header_bytes = easy_header.to_bytes();
    let nockchain_target = [0xffu8; 32];
    let aux = PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window".to_vec(),
    };
    let expected_aux_commitment = aux.commitment().unwrap();

    let mut bad_aux = aux.clone();
    bad_aux.nock_block_commitment[0] ^= 1;
    assert_eq!(
        verify_pearl_merge_mining_public_data(
            &aux.nock_block_commitment,
            &header_bytes,
            &public.to_public_data().unwrap(),
            &a,
            &b,
            &nockchain_target,
            16,
            bad_aux,
            &expected_aux_commitment,
        ),
        Err(PearlCompatError::NockchainAuxBlockCommitmentMismatch)
    );

    let mut bad_candidate_nock_block_commitment = aux.nock_block_commitment;
    bad_candidate_nock_block_commitment[0] ^= 1;
    assert_eq!(
        verify_pearl_merge_mining_public_data(
            &bad_candidate_nock_block_commitment,
            &header_bytes,
            &public.to_public_data().unwrap(),
            &a,
            &b,
            &nockchain_target,
            16,
            aux.clone(),
            &expected_aux_commitment,
        ),
        Err(PearlCompatError::NockchainAuxBlockCommitmentMismatch)
    );

    let mut bad_expected_aux_commitment = expected_aux_commitment;
    bad_expected_aux_commitment[31] ^= 1;
    assert_eq!(
        verify_pearl_merge_mining_public_data(
            &aux.nock_block_commitment,
            &header_bytes,
            &public.to_public_data().unwrap(),
            &a,
            &b,
            &nockchain_target,
            16,
            aux.clone(),
            &bad_expected_aux_commitment,
        ),
        Err(PearlCompatError::NockchainAuxCommitmentMismatch)
    );

    public.hash_jackpot[0] ^= 1;
    let candidate_nock_block_commitment = aux.nock_block_commitment;
    assert_eq!(
        verify_pearl_merge_mining_public_data(
            &candidate_nock_block_commitment,
            &header_bytes,
            &public.to_public_data().unwrap(),
            &a,
            &b,
            &nockchain_target,
            16,
            aux,
            &expected_aux_commitment,
        ),
        Err(PearlCompatError::JackpotHashMismatch)
    );
}
