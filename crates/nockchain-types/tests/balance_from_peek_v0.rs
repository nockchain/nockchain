use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::v0;
use noun_serde::{NounDecode, NounEncode};

#[test]
fn decode_balance_from_peeks_and_snapshots_v1() -> Result<(), Box<dyn std::error::Error>> {
    const EARLY_BALANCE_JAM: &[u8] = include_bytes!("../jams/v0/early-balance.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(EARLY_BALANCE_JAM))?;

    let double_option: Option<Option<v0::BalanceUpdate>> =
        Option::<Option<v0::BalanceUpdate>>::from_noun(&noun)?;
    assert!(
        matches!(double_option, Some(Some(_))),
        "jam should decode as Option<Option<BalanceUpdate>>"
    );

    let balance_update = double_option.clone().unwrap().unwrap();

    let notes = &balance_update.notes;
    assert_eq!(
        notes.0.len(),
        122,
        "expected early balance jam to contain 122 notes"
    );

    let (first_name, first_note) = notes
        .0
        .first()
        .expect("balance should contain at least one note");
    assert_eq!(
        first_name, &first_note.tail.name,
        "note key should match embedded name"
    );
    assert_eq!(first_note.head.version, v0::Version::V0);
    assert_eq!(first_note.head.origin_page, v0::BlockHeight(Belt(69)));

    let intent = first_note
        .head
        .timelock
        .0
        .clone()
        .expect("expected first note to contain a timelock intent");
    assert!(intent.absolute.min.is_none());
    assert!(intent.absolute.max.is_none());
    assert_eq!(intent.relative.min, Some(v0::BlockHeightDelta(Belt(4383))));
    assert!(intent.relative.max.is_none());

    assert_eq!(first_note.tail.lock.keys_required, 1);
    assert_eq!(first_note.tail.lock.pubkeys.len(), 1);
    assert!(first_note.tail.source.is_coinbase);
    assert_eq!(first_note.tail.assets, v0::Nicks(2576980378));

    let mut balance_slab: NounSlab = NounSlab::new();
    let notes_noun = v0::Balance::to_noun(notes, &mut balance_slab);
    let notes_roundtrip = v0::Balance::from_noun(&notes_noun)?;
    assert_eq!(notes, &notes_roundtrip);

    let encoded_option = Some(Some(balance_update.clone()));
    let mut option_slab: NounSlab = NounSlab::new();
    let option_noun =
        Option::<Option<v0::BalanceUpdate>>::to_noun(&encoded_option, &mut option_slab);
    let option_roundtrip = Option::<Option<v0::BalanceUpdate>>::from_noun(&option_noun)?;
    assert_eq!(option_roundtrip, encoded_option);

    Ok(())
}
