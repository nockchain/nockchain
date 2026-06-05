use std::collections::BTreeMap;

use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockchain_math::belt::Belt;
use nockchain_types::common::{BlockHeight, Version};
use nockchain_types::tx_engine::v1;
use noun_serde::{NounDecode, NounEncode};

// These constants are pinned to the checked-in raw tx fixture at:
// open/crates/nockchain-types/jams/v1/raw-tx.jam
const EXPECTED_SEED_COUNT: u64 = 7;
const EXPECTED_WITNESS_COUNT: u64 = 133;
const EXPECTED_TX_WORD_COUNT: u64 = 140;
const EXPECTED_MINIMUM_FEE: u64 = 659_456;
const EXPECTED_TOTAL_PAID_FEE: u64 = 1_024;

fn noun_leaf_count(noun: nockapp::Noun) -> u64 {
    if noun.is_atom() {
        return 1;
    }
    let cell = noun.as_cell().expect("noun should decode as cell");
    noun_leaf_count(cell.head()).saturating_add(noun_leaf_count(cell.tail()))
}

fn word_count_from_noun_encode<T: NounEncode>(value: &T) -> u64 {
    let mut slab: NounSlab = NounSlab::new();
    let noun = value.to_noun(&mut slab);
    noun_leaf_count(noun)
}

fn merged_seed_word_count(raw_tx: &v1::RawTx) -> u64 {
    let mut merged_by_lock_root = BTreeMap::<[u64; 5], BTreeMap<String, Bytes>>::new();

    for (_, spend) in &raw_tx.spends.0 {
        let seeds = match spend {
            v1::Spend::Legacy(spend0) => &spend0.seeds.0,
            v1::Spend::Witness(spend1) => &spend1.seeds.0,
        };

        for seed in seeds {
            let merged = merged_by_lock_root
                .entry(seed.lock_root.to_array())
                .or_default();
            for note_data_entry in seed.note_data.iter() {
                merged.insert(note_data_entry.key.clone(), note_data_entry.blob.clone());
            }
        }
    }

    merged_by_lock_root
        .into_values()
        .map(|merged| {
            let entries = merged
                .into_iter()
                .map(|(key, blob)| v1::note::NoteDataEntry::new(key, blob))
                .collect::<Vec<_>>();
            word_count_from_noun_encode(&v1::note::NoteData::new(entries))
        })
        .sum()
}

fn witness_word_count(raw_tx: &v1::RawTx) -> u64 {
    raw_tx
        .spends
        .0
        .iter()
        .map(|(_, spend)| match spend {
            v1::Spend::Legacy(spend0) => word_count_from_noun_encode(&spend0.signature),
            v1::Spend::Witness(spend1) => word_count_from_noun_encode(&spend1.witness),
        })
        .sum()
}

fn total_paid_fee(raw_tx: &v1::RawTx) -> u64 {
    raw_tx
        .spends
        .0
        .iter()
        .map(|(_, spend)| match spend {
            v1::Spend::Legacy(spend0) => spend0.fee.0 as u64,
            v1::Spend::Witness(spend1) => spend1.fee.0 as u64,
        })
        .sum()
}

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
fn decode_raw_tx_word_count_oracle_v1() -> Result<(), Box<dyn std::error::Error>> {
    const RAW_TX_JAM: &[u8] = include_bytes!("../jams/v1/raw-tx.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(RAW_TX_JAM))?;
    let raw_tx = v1::RawTx::from_noun(&noun)?;

    let seed_count = merged_seed_word_count(&raw_tx);
    let witness_count = witness_word_count(&raw_tx);
    let tx_word_count = seed_count.saturating_add(witness_count);

    // pending-integration defaults in tx-engine:
    // base_fee = 2^14, input_fee_divisor = 4, min_fee_floor = 256
    let base_fee: u64 = 1 << 14;
    let input_fee_divisor: u64 = 4;
    let min_fee_floor: u64 = 256;
    let word_fee = seed_count
        .saturating_mul(base_fee)
        .saturating_add(witness_count.saturating_mul(base_fee) / input_fee_divisor);
    let minimum_fee = word_fee.max(min_fee_floor);

    assert_eq!(seed_count, EXPECTED_SEED_COUNT);
    assert_eq!(witness_count, EXPECTED_WITNESS_COUNT);
    assert_eq!(tx_word_count, EXPECTED_TX_WORD_COUNT);
    assert_eq!(minimum_fee, EXPECTED_MINIMUM_FEE);
    assert_eq!(total_paid_fee(&raw_tx), EXPECTED_TOTAL_PAID_FEE);
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
