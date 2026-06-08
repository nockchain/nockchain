use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockapp::NounAllocator;
use nockchain_types::tx_engine::v1::{self, SigHashable};
use nockvm::noun::NounSpace;
use noun_serde::NounDecode;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Spend1SigHashOracle(Vec<(v1::Name, v1::Hash)>);

impl NounDecode for Spend1SigHashOracle {
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> Result<Self, noun_serde::NounDecodeError> {
        Vec::<(v1::Name, v1::Hash)>::from_noun(noun, space).map(Self)
    }
}

fn load_raw_tx_fixture() -> TestResult<v1::RawTx> {
    const RAW_TX_JAM: &[u8] = include_bytes!("../jams/v1/raw-tx.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(RAW_TX_JAM))?;
    let space = slab.noun_space();
    Ok(v1::RawTx::from_noun(&noun, &space)?)
}

fn load_spend1_sig_hash_oracle() -> TestResult<Spend1SigHashOracle> {
    const ORACLE_JAM: &[u8] = include_bytes!("../jams/v1/raw-tx-spend1-sig-hash-oracle.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(ORACLE_JAM))?;
    let space = slab.noun_space();
    Ok(Spend1SigHashOracle::from_noun(&noun, &space)?)
}

fn first_fixture_witness_signature() -> TestResult<(v1::Spend1, v1::PkhSignatureEntry)> {
    let raw_tx = load_raw_tx_fixture()?;
    for (_, spend) in raw_tx.spends.0 {
        let v1::Spend::Witness(spend1) = spend else {
            continue;
        };
        if let Some(entry) = spend1.witness.pkh_signature.0.first().cloned() {
            return Ok((spend1, entry));
        }
    }
    Err("raw tx fixture missing witness signature".into())
}

#[test]
fn test_spend1_signature_hash_matches_spec_builder_v1() -> TestResult<()> {
    let raw_tx = load_raw_tx_fixture()?;
    let oracle = load_spend1_sig_hash_oracle()?;
    let mut checked = 0usize;

    for (name, spend) in raw_tx.spends.0 {
        let v1::Spend::Witness(spend1) = spend else {
            continue;
        };
        let (_, expected_hash) = oracle
            .0
            .iter()
            .find(|(expected_name, _)| *expected_name == name)
            .ok_or_else(|| format!("oracle missing spend name {:?}", name))?;
        assert_eq!(
            spend1.sig_hash_digest()?,
            *expected_hash,
            "spend-1 sig-hash drifted from the Hoon oracle fixture",
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "expected raw tx fixture to contain a %1 witness spend"
    );
    assert_eq!(
        checked,
        oracle.0.len(),
        "oracle should contain exactly one entry for each %1 witness spend",
    );
    Ok(())
}

#[test]
fn raw_tx_fixture_witness_signatures_verify_v1() -> TestResult<()> {
    let raw_tx = load_raw_tx_fixture()?;
    let mut verified = 0usize;

    for (_, spend) in raw_tx.spends.0 {
        let v1::Spend::Witness(spend1) = spend else {
            continue;
        };
        for entry in &spend1.witness.pkh_signature.0 {
            spend1.verify_pkh_signature(entry)?;
            verified += 1;
        }
    }

    assert!(
        verified > 0,
        "expected raw tx fixture to contain witness signatures"
    );
    Ok(())
}

#[test]
fn spend1_signature_verifier_rejects_pubkey_hash_mismatch_v1() -> TestResult<()> {
    let (spend1, mut entry) = first_fixture_witness_signature()?;
    entry.pkh = v1::Hash([1, 2, 3, 4, 5].map(nockchain_math::belt::Belt));

    let err = spend1
        .verify_pkh_signature(&entry)
        .expect_err("mismatched signer hash should fail");
    assert!(matches!(
        err,
        v1::Spend1SignatureVerificationError::PubkeyHashMismatch { .. }
    ));
    Ok(())
}

#[test]
fn spend1_signature_verifier_rejects_non_canonical_limb_v1() -> TestResult<()> {
    let (spend1, mut entry) = first_fixture_witness_signature()?;
    // A limb of exactly 2^32 is a valid Goldilocks element but not a canonical
    // 32-bit digit, so it must be rejected before any curve arithmetic.
    entry.signature.chal[0] = nockchain_math::belt::Belt(1u64 << 32);

    let err = spend1
        .verify_pkh_signature(&entry)
        .expect_err("non-canonical limb should fail");
    assert!(matches!(
        err,
        v1::Spend1SignatureVerificationError::NonCanonicalSignature
    ));
    Ok(())
}

#[test]
fn spend1_signature_verifier_rejects_mutated_signature_v1() -> TestResult<()> {
    let (spend1, mut entry) = first_fixture_witness_signature()?;
    entry.signature.chal[0] =
        nockchain_math::belt::Belt(entry.signature.chal[0].0.saturating_add(1));

    let err = spend1
        .verify_pkh_signature(&entry)
        .expect_err("mutated signature should fail");
    assert!(matches!(
        err,
        v1::Spend1SignatureVerificationError::InvalidSignature
            | v1::Spend1SignatureVerificationError::SignatureArithmetic
    ));
    Ok(())
}
