use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::v0;
use noun_serde::{NounDecode, NounEncode};

#[test]
fn decode_raw_tx_from_jam_v0() -> Result<(), Box<dyn std::error::Error>> {
    const RAW_TX_JAM: &[u8] = include_bytes!("../jams/v0/raw-tx.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(RAW_TX_JAM))?;

    let raw_tx = v0::RawTx::from_noun(&noun)?;

    // basic structural checks
    assert_eq!(raw_tx.inputs.0.len(), 10, "expected ten named inputs");
    assert_eq!(raw_tx.total_fees, v0::Nicks(0));
    assert_eq!(
        raw_tx.timelock_range.min,
        Some(v0::BlockHeight(Belt(10))),
        "timelock minimum page should match peek sample",
    );
    assert!(
        raw_tx.timelock_range.max.is_none(),
        "timelock maximum is unset for the sample",
    );

    let (_, first_input) = raw_tx
        .inputs
        .0
        .first()
        .expect("raw tx should contain at least one input");
    assert_eq!(
        first_input.spend.seeds.seeds.len(),
        1,
        "each sample spend should have a single seed",
    );

    // noun roundtrip
    let mut encode_slab: NounSlab = NounSlab::new();
    let encoded = v0::RawTx::to_noun(&raw_tx, &mut encode_slab);
    let round_trip = v0::RawTx::from_noun(&encoded)?;
    assert_eq!(round_trip, raw_tx);

    Ok(())
}

#[test]
fn decode_note_from_jam_v0() -> Result<(), Box<dyn std::error::Error>> {
    const NOTE_JAM: &[u8] = include_bytes!("../jams/v0/note.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(NOTE_JAM))?;

    let note = v0::NoteV0::from_noun(&noun)?;

    // basic structural checks

    // noun roundtrip
    let mut encode_slab: NounSlab = NounSlab::new();
    let encoded = v0::NoteV0::to_noun(&note, &mut encode_slab);
    let round_trip = v0::NoteV0::from_noun(&encoded)?;
    assert_eq!(round_trip, note);

    Ok(())
}
