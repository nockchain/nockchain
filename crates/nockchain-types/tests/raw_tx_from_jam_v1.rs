use std::collections::BTreeMap;

use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockchain_math::belt::Belt;
use nockchain_types::common::{BlockHeight, Version};
use nockchain_types::tx_engine::v1;
use nockvm::noun::NounAllocator;
use noun_serde::{NounDecode, NounEncode};

// These constants are pinned to the checked-in raw tx fixture at:
// open/crates/nockchain-types/jams/v1/raw-tx.jam
const EXPECTED_SEED_COUNT: u64 = 7;
const EXPECTED_WITNESS_COUNT: u64 = 133;
const EXPECTED_TX_WORD_COUNT: u64 = 140;
const EXPECTED_MINIMUM_FEE: u64 = 659_456;
const EXPECTED_TOTAL_PAID_FEE: u64 = 1_024;

fn noun_leaf_count(noun: nockapp::Noun, space: &nockvm::noun::NounSpace) -> u64 {
    if noun.is_atom() {
        return 1;
    }
    let cell = noun
        .in_space(space)
        .as_cell()
        .expect("noun should decode as cell");
    noun_leaf_count(cell.head().noun(), space)
        .saturating_add(noun_leaf_count(cell.tail().noun(), space))
}

fn word_count_from_noun_encode<T: NounEncode>(value: &T) -> u64 {
    let mut slab: NounSlab = NounSlab::new();
    let noun = value.to_noun(&mut slab);
    let space = slab.noun_space();
    noun_leaf_count(noun, &space)
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
                merged.insert(note_data_entry.key.clone(), note_data_entry.raw_blob());
            }
        }
    }

    merged_by_lock_root
        .into_values()
        .map(|merged| {
            let entries = merged
                .into_iter()
                .map(|(key, blob)| {
                    v1::note::NoteDataEntry::from_raw_blob(key, blob)
                        .expect("fixture raw note-data should decode")
                })
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
    let space = slab.noun_space();

    let raw_tx = v1::RawTx::from_noun(&noun, &space)?;

    // basic structural checks
    assert_eq!(raw_tx.version, Version::V1);

    // noun roundtrip
    let mut encode_slab: NounSlab = NounSlab::new();
    let encoded = v1::RawTx::to_noun(&raw_tx, &mut encode_slab);
    let encode_space = encode_slab.noun_space();
    let round_trip = v1::RawTx::from_noun(&encoded, &encode_space)?;
    assert_eq!(round_trip, raw_tx);

    Ok(())
}

#[test]
fn raw_tx_id_from_jam_v1_matches_recomputed_id() -> Result<(), Box<dyn std::error::Error>> {
    const RAW_TX_JAM: &[u8] = include_bytes!("../jams/v1/raw-tx.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(RAW_TX_JAM))?;
    let space = slab.noun_space();
    let raw_tx = v1::RawTx::from_noun(&noun, &space)?;

    assert_eq!(raw_tx.compute_id()?, raw_tx.id);
    assert_eq!(raw_tx.compute_id_base58()?, raw_tx.id.to_base58());

    Ok(())
}

#[test]
fn decode_raw_tx_word_count_oracle_v1() -> Result<(), Box<dyn std::error::Error>> {
    const RAW_TX_JAM: &[u8] = include_bytes!("../jams/v1/raw-tx.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(RAW_TX_JAM))?;
    let space = slab.noun_space();
    let raw_tx = v1::RawTx::from_noun(&noun, &space)?;

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
fn decode_notes_from_jam_v1() -> Result<(), Box<dyn std::error::Error>> {
    const NOTE_JAM: &[u8] = include_bytes!("../jams/v1/note.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(NOTE_JAM))?;
    let space = slab.noun_space();

    let notes = <Vec<v1::Note>>::from_noun(&noun, &space)?;
    assert_eq!(notes.len(), 2, "V1 note fixture should include two notes");

    let complex_note = notes
        .iter()
        .find(|note| match note {
            v1::Note::V1(n) => n.origin_page == BlockHeight(Belt(24)),
            _ => false,
        })
        .expect("V1 note fixture should include the complex note at origin page 24");

    match complex_note {
        v1::Note::V1(ref n) => {
            assert_eq!(n.origin_page, BlockHeight(Belt(24)));
            assert!(
                n.note_data.iter().any(|entry| entry.key == "memo"),
                "complex V1 note should include memo metadata"
            );
        }
        _ => panic!("note not V1: {:?}", complex_note),
    }

    let mut encode_slab: NounSlab = NounSlab::new();
    let encoded = notes.to_noun(&mut encode_slab);
    let encode_space = encode_slab.noun_space();
    let round_trip = <Vec<v1::Note>>::from_noun(&encoded, &encode_space)?;
    assert_eq!(round_trip, notes);

    Ok(())
}
