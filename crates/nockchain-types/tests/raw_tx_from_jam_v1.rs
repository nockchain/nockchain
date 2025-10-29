use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockchain_math::belt::Belt;
use nockchain_types::common::{BlockHeight, Version};
use nockchain_types::tx_engine::v1;
use noun_serde::{NounDecode, NounEncode};

#[test]
fn decode_raw_tx_from_jam_v1() -> Result<(), Box<dyn std::error::Error>> {
    const RAW_TX_JAM: &[u8] = include_bytes!("../jams/v1/raw-tx.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(RAW_TX_JAM))?;

    let raw_tx = v1::RawTx::from_noun(&noun)?;

    // basic structural checks
    assert_eq!(raw_tx.version, Version::V1);

    // noun roundtrip
    let mut encode_slab: NounSlab = NounSlab::new();
    let encoded = v1::RawTx::to_noun(&raw_tx, &mut encode_slab);
    let round_trip = v1::RawTx::from_noun(&encoded)?;
    assert_eq!(round_trip, raw_tx);

    Ok(())
}

#[test]
fn decode_note_from_jam_v1() -> Result<(), Box<dyn std::error::Error>> {
    const NOTE_JAM: &[u8] = include_bytes!("../jams/v1/note.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(NOTE_JAM))?;

    eprintln!("decoding note");
    let ver = noun.as_cell().expect("not a cell").head();
    eprintln!("version: {:?}", ver);
    let note = v1::Note::from_noun(&noun)?;
    eprintln!("decoded note");

    // basic structural checks
    match note {
        v1::Note::V1(ref n) => {
            assert_eq!(n.origin_page, BlockHeight(Belt(24)));
        }
        _ => panic!("note not V1: {:?}", note),
    }

    // noun roundtrip
    let mut encode_slab: NounSlab = NounSlab::new();
    let encoded = v1::Note::to_noun(&note, &mut encode_slab);
    let round_trip = v1::Note::from_noun(&encoded)?;
    assert_eq!(round_trip, note);

    Ok(())
}

#[test]
fn decode_name_from_jam_v1() -> Result<(), Box<dyn std::error::Error>> {
    const NOTE_JAM: &[u8] = include_bytes!("../jams/v1/note.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(NOTE_JAM))?;

    eprintln!("decoding note");
    let ver = noun.as_cell().expect("not a cell").head();
    eprintln!("version: {:?}", ver);
    let note = v1::Note::from_noun(&noun)?;
    eprintln!("decoded note");

    // basic structural checks
    match note {
        v1::Note::V1(ref n) => {
            assert_eq!(n.origin_page, BlockHeight(Belt(24)));
        }
        _ => panic!("note not V1: {:?}", note),
    }

    // noun roundtrip
    let mut encode_slab: NounSlab = NounSlab::new();
    let encoded = v1::Note::to_noun(&note, &mut encode_slab);
    let round_trip = v1::Note::from_noun(&encoded)?;
    assert_eq!(round_trip, note);

    Ok(())
}
