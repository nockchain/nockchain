//! Golden program fingerprints for the supported consensus rule versions.
//! Hash width followed by each Goldilocks value, all encoded as little-endian u64.
use ai_pow_zk::canonical::{
    canonical_program_for_strip_schedule_with_rules, BlockPublic, StripIndexSchedule,
};
use ai_pow_zk::params::ZkParams;
use ai_pow_zk::proof_rules::ProofRules;
use p3_field::PrimeField64;

#[test]
fn preserves_both_reference_programs_including_partial_chunks() {
    for (m, k, n, legacy, hardened) in [
        (
            64, 1024, 64, "74575ddd842179d97fb3232d72e6262c61f2516d550477b450e22b47543a5818",
            "a0552c04bf6c0a044e46ed417833c62e4c7da44018d0b048830bab8d5478db98",
        ),
        (
            8, 1088, 8, "6bc06b98fe7c857703556858ea043f0e642c56ce60cede371bf000a8ae2e4403",
            "d36f52d3f3fa1b612a8f2975123b6bc677a599d7c735a6cd7978a365fc6827df",
        ),
    ] {
        let params = ZkParams {
            m,
            k,
            n,
            noise_rank: 64,
            tile: 8,
            difficulty_bits: 0,
        };
        let public = BlockPublic {
            tile_i: 0,
            tile_j: 0,
            kappa: [1; 32],
            s_a: [2; 32],
            s_b: [3; 32],
        };
        let schedule = StripIndexSchedule::from_tile(&params, 0, 0).unwrap();
        for (rules, expected) in [(ProofRules::Legacy, legacy), (ProofRules::Hardened, hardened)] {
            let program = canonical_program_for_strip_schedule_with_rules(
                &params,
                &schedule,
                &public,
                1 << 15,
                rules,
            )
            .unwrap();
            let mut hash = blake3::Hasher::new();
            hash.update(&(program.width as u64).to_le_bytes());
            for value in &program.values {
                hash.update(&value.as_canonical_u64().to_le_bytes());
            }
            assert_eq!(
                hash.finalize().to_hex().as_str(),
                expected,
                "{m}x{k}x{n}, {rules:?}"
            );
        }
    }
}
