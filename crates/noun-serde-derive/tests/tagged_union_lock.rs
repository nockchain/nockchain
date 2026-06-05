use nockvm::mem::NockStack;
use nockvm::noun::{D, T};
use noun_serde::{NounDecode, NounEncode};

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
enum DerivedLock {
    #[noun(untagged)]
    SpendCondition(u64),
    #[noun(tag = 2)]
    V2((u64, u64)),
    #[noun(tag = 4)]
    V4(((u64, u64), (u64, u64))),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
enum MixedTags {
    #[noun(tag = 0)]
    Zero(u64),
    #[noun(tag = "ok")]
    Ok,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
enum MultiFieldNumericTag {
    #[noun(tag = 0)]
    Payload(String, [u64; 3]),
}

#[test]
fn lock_leaf_roundtrip_uses_untagged_variant() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let lock = DerivedLock::SpendCondition(42);
    let noun = lock.to_noun(&mut stack);
    let atom = noun.as_atom().expect("leaf lock should encode as atom");
    assert_eq!(atom.as_u64().expect("leaf lock atom should fit"), 42);

    let decoded = DerivedLock::from_noun(&noun).expect("decode lock leaf");
    assert_eq!(decoded, lock);
}

#[test]
fn lock_tree_roundtrip_uses_integer_atom_tag_variants() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let lock = DerivedLock::V4(((11, 12), (13, 14)));
    let noun = lock.to_noun(&mut stack);

    let cell = noun.as_cell().expect("v4 lock encodes as tagged cell");
    let tag_atom = cell.head().as_atom().expect("tag must be atom");
    assert_eq!(tag_atom.as_u64().expect("tag should fit"), 4);

    let decoded = DerivedLock::from_noun(&noun).expect("decode lock v4");
    assert_eq!(decoded, lock);
}

#[test]
fn lock_tree_decodes_from_manual_tagged_noun() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let payload = T(&mut stack, &[D(7), D(8)]);
    let noun = T(&mut stack, &[D(2), payload]);

    let decoded = DerivedLock::from_noun(&noun).expect("decode lock v2 from noun");
    assert_eq!(decoded, DerivedLock::V2((7, 8)));
}

#[test]
fn mixed_string_and_integer_tags_roundtrip() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);

    let zero = MixedTags::Zero(9);
    let zero_noun = zero.to_noun(&mut stack);
    assert_eq!(MixedTags::from_noun(&zero_noun).expect("decode zero"), zero);

    let ok = MixedTags::Ok;
    let ok_noun = ok.to_noun(&mut stack);
    assert_eq!(MixedTags::from_noun(&ok_noun).expect("decode ok"), ok);
}

#[test]
fn multi_field_integer_tag_roundtrip_uses_numeric_atom() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);
    let value = MultiFieldNumericTag::Payload("base".to_string(), [11, 22, 33]);
    let noun = value.to_noun(&mut stack);

    let cell = noun
        .as_cell()
        .expect("payload should encode as tagged cell");
    let tag = cell
        .head()
        .as_atom()
        .expect("payload tag should be atom")
        .as_u64()
        .expect("payload tag should fit");
    assert_eq!(tag, 0);

    let decoded = MultiFieldNumericTag::from_noun(&noun).expect("decode multi-field payload");
    assert_eq!(decoded, value);
}
