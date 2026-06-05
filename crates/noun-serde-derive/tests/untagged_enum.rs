use nockvm::mem::NockStack;
use nockvm::noun::{D, T};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounEncode};

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
struct LockMerkleProofStub {
    spend_condition: u64,
    axis: u64,
    proof: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
struct LockMerkleProofFull {
    spend_condition: u64,
    axis: u64,
    proof: u64,
    version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
#[noun(untagged)]
enum LockMerkleProof {
    Full(LockMerkleProofFull),
    Stub(LockMerkleProofStub),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
#[noun(untagged)]
enum UntaggedWithUnit {
    Empty,
    Value(u64),
    Pair(u64, u64),
}

#[test]
fn untagged_enum_roundtrip_full() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let proof = LockMerkleProof::Full(LockMerkleProofFull {
        spend_condition: 1,
        axis: 2,
        proof: 3,
        version: tas!(b"full"),
    });
    let noun = proof.to_noun(&mut stack);
    let decoded = LockMerkleProof::from_noun(&noun).expect("decode full");
    assert_eq!(decoded, proof);
}

#[test]
fn untagged_enum_decodes_stub() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let proof = LockMerkleProofStub {
        spend_condition: 7,
        axis: 1,
        proof: 9,
    };
    let noun = proof.to_noun(&mut stack);
    let decoded = LockMerkleProof::from_noun(&noun).expect("decode stub");
    assert_eq!(decoded, LockMerkleProof::Stub(proof));
}

#[test]
fn untagged_enum_stub_roundtrip_via_wrapper() {
    // Encode Stub via wrapper, decode back - should stay Stub
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let proof = LockMerkleProof::Stub(LockMerkleProofStub {
        spend_condition: 100,
        axis: 200,
        proof: 300,
    });
    let noun = proof.to_noun(&mut stack);
    let decoded = LockMerkleProof::from_noun(&noun).expect("decode stub via wrapper");
    assert_eq!(decoded, proof);
}

#[test]
fn untagged_enum_full_has_correct_version() {
    // Verify that decoded Full has version = tas!(b"full")
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let proof = LockMerkleProof::Full(LockMerkleProofFull {
        spend_condition: 1,
        axis: 2,
        proof: 3,
        version: tas!(b"full"),
    });
    let noun = proof.to_noun(&mut stack);
    let decoded = LockMerkleProof::from_noun(&noun).expect("decode full");
    match decoded {
        LockMerkleProof::Full(full) => {
            assert_eq!(full.version, tas!(b"full"));
        }
        LockMerkleProof::Stub(_) => panic!("expected Full, got Stub"),
    }
}

#[test]
fn untagged_enum_discriminates_by_structure() {
    // 3-tuple decodes as Stub, 4-tuple decodes as Full
    let mut stack = NockStack::new(8 << 10 << 10, 0);

    // Build a 3-tuple manually: [spend_condition axis proof]
    let three_tuple = T(&mut stack, &[D(10), D(20), D(30)]);
    let decoded_3 = LockMerkleProof::from_noun(&three_tuple).expect("decode 3-tuple");
    assert!(matches!(decoded_3, LockMerkleProof::Stub(_)));

    // Build a 4-tuple manually: [spend_condition axis proof version]
    let four_tuple = T(&mut stack, &[D(10), D(20), D(30), D(tas!(b"full"))]);
    let decoded_4 = LockMerkleProof::from_noun(&four_tuple).expect("decode 4-tuple");
    assert!(matches!(decoded_4, LockMerkleProof::Full(_)));
}

#[test]
fn untagged_enum_four_tuple_with_wrong_version_still_decodes_as_full() {
    // A 4-tuple with version != %full still decodes as Full (structure-based discrimination)
    // The version field is just data - discrimination is by arity
    let mut stack = NockStack::new(8 << 10 << 10, 0);

    let four_tuple_wrong_version = T(&mut stack, &[D(10), D(20), D(30), D(999)]);
    let decoded = LockMerkleProof::from_noun(&four_tuple_wrong_version)
        .expect("decode 4-tuple with wrong version");
    match decoded {
        LockMerkleProof::Full(full) => {
            // It decoded as Full, but version is wrong
            assert_eq!(full.version, 999);
            assert_ne!(full.version, tas!(b"full"));
        }
        LockMerkleProof::Stub(_) => panic!("expected Full (by structure), got Stub"),
    }
}

#[test]
fn untagged_enum_full_and_stub_decode_correctly_with_same_prefix() {
    // Full and Stub with same first 3 fields should still decode correctly
    let mut stack = NockStack::new(8 << 10 << 10, 0);

    let stub = LockMerkleProofStub {
        spend_condition: 1,
        axis: 2,
        proof: 3,
    };
    let full = LockMerkleProofFull {
        spend_condition: 1,
        axis: 2,
        proof: 3,
        version: tas!(b"full"),
    };

    // Encode and decode both
    let stub_noun = stub.to_noun(&mut stack);
    let full_noun = full.to_noun(&mut stack);

    let decoded_stub = LockMerkleProof::from_noun(&stub_noun).expect("decode stub");
    let decoded_full = LockMerkleProof::from_noun(&full_noun).expect("decode full");

    // Stub decodes as Stub, Full decodes as Full
    assert!(matches!(decoded_stub, LockMerkleProof::Stub(_)));
    assert!(matches!(decoded_full, LockMerkleProof::Full(_)));

    // Verify the data is preserved
    match decoded_stub {
        LockMerkleProof::Stub(s) => {
            assert_eq!(s.spend_condition, 1);
            assert_eq!(s.axis, 2);
            assert_eq!(s.proof, 3);
        }
        _ => panic!("expected Stub"),
    }
    match decoded_full {
        LockMerkleProof::Full(f) => {
            assert_eq!(f.spend_condition, 1);
            assert_eq!(f.axis, 2);
            assert_eq!(f.proof, 3);
            assert_eq!(f.version, tas!(b"full"));
        }
        _ => panic!("expected Full"),
    }
}

#[test]
fn untagged_enum_unit_variant_requires_zero_atom() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);

    let atom_one = D(1);
    let decoded_atom = UntaggedWithUnit::from_noun(&atom_one).expect("decode atom");
    assert_eq!(decoded_atom, UntaggedWithUnit::Value(1));

    let cell = T(&mut stack, &[D(2), D(3)]);
    let decoded_cell = UntaggedWithUnit::from_noun(&cell).expect("decode cell");
    assert_eq!(decoded_cell, UntaggedWithUnit::Pair(2, 3));

    let zero = D(0);
    let decoded_zero = UntaggedWithUnit::from_noun(&zero).expect("decode zero atom");
    assert_eq!(decoded_zero, UntaggedWithUnit::Empty);
}
