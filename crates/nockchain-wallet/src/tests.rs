#![allow(warnings)]
// TODO: all these tests need to also validate the results and not
// just ensure that the wallet can be poked with the expected noun.

use std::collections::BTreeMap;
use std::sync::Once;

use nockapp::kernel::boot::{self, Cli as BootCli};
use nockapp::wire::SystemWire;
use nockapp::{exit_driver, AtomExt, Bytes};
use nockchain_math::belt::Belt;
use nockchain_math::zoon::zmap::ZMap;
use nockchain_types::default_fakenet_blockchain_constants;
use nockchain_types::tx_engine::common::{BlockHeight, BlockHeightDelta, Nicks, Signature};
use nockchain_types::tx_engine::v1::note::{NoteData, NoteDataEntry};
use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh, SpendCondition};
use nockchain_types::tx_engine::{v0, v1};
use nockvm::noun::{Slots, T};
use noun_serde::{NounDecode, NounEncode};
use tempfile::TempDir;
use tokio::sync::mpsc;
use wallet_tx_builder::adapter::normalize_balance_pages;
use wallet_tx_builder::fee::{compute_minimum_fee, FeeInputs};
use wallet_tx_builder::planner::plan_create_tx;
use wallet_tx_builder::types::{
    CandidateVersionPolicy, ChainContext, PlanRequest, PlanningMode, RawNoteDataEntry,
    SelectionMode, SelectionOrder,
};

use super::*;
use crate::create_tx::{
    ensure_manual_planner_parity, ActiveSignerEntryNoun, MigrateV0NotesSummary,
    MigrateV0SignerSummary, PlannerBlockchainConstantsNoun, PlannerNoteDataConstantsNoun,
    SigningKeyLockMatcher,
};
use crate::recipient::{planner_recipient_outputs, RecipientSpec};

static INIT: Once = Once::new();

fn init_tracing() {
    INIT.call_once(|| {
        let cli = boot::default_boot_cli(true);
        boot::init_default_tracing(&cli);
    });
}

fn hash(v: u64) -> Hash {
    Hash::from_limbs(&[v, 0, 0, 0, 0])
}

fn name(first: u64, last: u64) -> Name {
    Name::new(hash(first), hash(last))
}

fn signer_key(pkh: u64) -> Hash {
    hash(pkh)
}

fn note_v1(first: u64, last: u64, origin_page: u64, assets: u64) -> v1::Note {
    v1::Note::V1(v1::NoteV1::new(
        BlockHeight(Belt(origin_page)),
        name(first, last),
        v1::NoteData::new(Vec::new()),
        Nicks(assets as usize),
    ))
}

fn balance_page(height: u64, block_id: u64, notes: Vec<(Name, v1::Note)>) -> v1::BalanceUpdate {
    v1::BalanceUpdate {
        height: BlockHeight(Belt(height)),
        block_id: hash(block_id),
        notes: v1::Balance(notes),
    }
}

fn simple_pkh_lock(pkh: Hash) -> Lock {
    Lock::SpendCondition(SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        1,
        vec![pkh],
    ))]))
}

fn simple_v0_lock(pubkey: SchnorrPubkey) -> v0::Lock {
    v0::Lock {
        keys_required: 1,
        pubkeys: vec![pubkey],
    }
}

fn note_data_from_raw_entries(entries: Vec<RawNoteDataEntry>) -> NoteData {
    NoteData::new(
        entries
            .into_iter()
            .map(|entry| NoteDataEntry::new(entry.key, entry.blob))
            .collect(),
    )
}

fn note_v1_with_lock(name: Name, origin_page: u64, assets: u64, lock: Lock) -> v1::Note {
    let note_data = note_data_from_raw_entries(vec![RawNoteDataEntry::from_lock(lock)]);
    v1::Note::V1(v1::NoteV1::new(
        BlockHeight(Belt(origin_page)),
        name,
        note_data,
        Nicks(assets as usize),
    ))
}

fn note_v0_with_lock(name: Name, origin_page: u64, assets: u64, lock: v0::Lock) -> v1::Note {
    v1::Note::V0(v0::NoteV0 {
        head: v0::NoteHead {
            version: nockchain_types::tx_engine::common::Version::V0,
            origin_page: BlockHeight(Belt(origin_page)),
            timelock: v0::Timelock(None),
        },
        tail: v0::NoteTail {
            name,
            lock,
            source: nockchain_types::tx_engine::common::Source {
                hash: hash(origin_page + assets),
                is_coinbase: false,
            },
            assets: Nicks(assets as usize),
        },
    })
}

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

fn merged_seed_word_count(spends: &v1::Spends) -> u64 {
    let mut merged_by_lock_root = BTreeMap::<[u64; 5], BTreeMap<String, Bytes>>::new();

    for (_, spend) in &spends.0 {
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
                .map(|(key, blob)| NoteDataEntry::new(key, blob))
                .collect::<Vec<_>>();
            word_count_from_noun_encode(&NoteData::new(entries))
        })
        .sum()
}

fn witness_word_count(spends: &v1::Spends) -> u64 {
    spends
        .0
        .iter()
        .map(|(_, spend)| match spend {
            v1::Spend::Legacy(spend0) => word_count_from_noun_encode(&spend0.signature),
            v1::Spend::Witness(spend1) => word_count_from_noun_encode(&spend1.witness),
        })
        .sum()
}

fn total_paid_fee(spends: &v1::Spends) -> u64 {
    spends
        .0
        .iter()
        .map(|(_, spend)| match spend {
            v1::Spend::Legacy(spend0) => spend0.fee.0 as u64,
            v1::Spend::Witness(spend1) => spend1.fee.0 as u64,
        })
        .sum()
}

fn total_seed_gift(spends: &v1::Spends) -> u64 {
    spends
        .0
        .iter()
        .flat_map(|(_, spend)| match spend {
            v1::Spend::Legacy(spend0) => spend0.seeds.0.iter(),
            v1::Spend::Witness(spend1) => spend1.seeds.0.iter(),
        })
        .map(|seed| seed.gift.0 as u64)
        .sum()
}

fn decode_saved_transaction_spends(effects: &[NounSlab]) -> Result<v1::Spends, NockAppError> {
    let tx_bytes = effects
        .iter()
        .find_map(|effect| {
            let noun = unsafe { effect.root() };
            let cell = noun.as_cell().ok()?;
            let tag = cell.head().as_atom().ok()?.into_string().ok()?;
            if tag != "file" {
                return None;
            }
            let op_cell = cell.tail().as_cell().ok()?;
            let op_tag = op_cell.head().as_atom().ok()?.into_string().ok()?;
            if op_tag != "write" {
                return None;
            }
            let write_cell = op_cell.tail().as_cell().ok()?;
            let path = write_cell.head().as_atom().ok()?.into_string().ok()?;
            if !path.ends_with(".tx") {
                return None;
            }
            Some(Bytes::copy_from_slice(
                write_cell.tail().as_atom().ok()?.as_ne_bytes(),
            ))
        })
        .ok_or_else(|| NockAppError::OtherError("missing saved transaction file effect".into()))?;

    decode_transaction_spends_from_bytes(&tx_bytes)
}

fn decode_saved_transaction_spends_from_path(
    transaction_path: &str,
) -> Result<v1::Spends, NockAppError> {
    let tx_bytes = fs::read(transaction_path).map_err(|err| {
        NockAppError::OtherError(format!("failed to read transaction file: {err}"))
    })?;
    decode_transaction_spends_from_bytes(&tx_bytes)
}

fn decode_transaction_spends_from_bytes(tx_bytes: &[u8]) -> Result<v1::Spends, NockAppError> {
    let mut slab: NounSlab = NounSlab::new();
    let transaction_noun = slab.cue_into(Bytes::copy_from_slice(tx_bytes))?;
    let transaction_cell = transaction_noun.as_cell().map_err(|err| {
        NockAppError::OtherError(format!("transaction jam root not a cell: {err}"))
    })?;
    let version = <u64 as NounDecode>::from_noun(&transaction_cell.head()).map_err(|err| {
        NockAppError::OtherError(format!("transaction version did not decode: {err}"))
    })?;
    if version != 1 {
        return Err(NockAppError::OtherError(format!(
            "expected saved transaction version 1, got {version}"
        )));
    }
    let name_and_rest = transaction_cell.tail().as_cell().map_err(|err| {
        NockAppError::OtherError(format!("transaction jam missing name/rest cell: {err}"))
    })?;
    let spends_and_rest = name_and_rest.tail().as_cell().map_err(|err| {
        NockAppError::OtherError(format!("transaction jam missing spends/rest cell: {err}"))
    })?;
    let mut spends = v1::Spends::from_noun(&spends_and_rest.head()).map_err(|err| {
        NockAppError::OtherError(format!("saved transaction spends did not decode: {err}"))
    })?;
    let display_and_witness = spends_and_rest.tail().as_cell().map_err(|err| {
        NockAppError::OtherError(format!(
            "transaction jam missing display/witness-data cell: {err}"
        ))
    })?;
    let witness_data = display_and_witness.tail();
    let witness_cell = witness_data.as_cell().map_err(|err| {
        NockAppError::OtherError(format!("transaction jam witness-data not a cell: {err}"))
    })?;
    let witness_tag = <u64 as NounDecode>::from_noun(&witness_cell.head()).map_err(|err| {
        NockAppError::OtherError(format!("witness-data tag did not decode: {err}"))
    })?;
    match witness_tag {
        0 => {
            let signatures =
                ZMap::<Name, Signature>::from_noun(&witness_cell.tail()).map_err(|err| {
                    NockAppError::OtherError(format!(
                        "legacy witness-data signature map did not decode: {err}"
                    ))
                })?;
            for (name, signature) in signatures.into_entries() {
                let Some((_, v1::Spend::Legacy(spend0))) = spends
                    .0
                    .iter_mut()
                    .find(|(candidate, _)| *candidate == name)
                else {
                    return Err(NockAppError::OtherError(format!(
                        "legacy witness-data referenced unknown spend {} / {}",
                        name.first.to_base58(),
                        name.last.to_base58()
                    )));
                };
                spend0.signature = signature;
            }
        }
        1 => {
            let witnesses =
                ZMap::<Name, v1::Witness>::from_noun(&witness_cell.tail()).map_err(|err| {
                    NockAppError::OtherError(format!("v1 witness-data map did not decode: {err}"))
                })?;
            for (name, witness) in witnesses.into_entries() {
                let Some((_, v1::Spend::Witness(spend1))) = spends
                    .0
                    .iter_mut()
                    .find(|(candidate, _)| *candidate == name)
                else {
                    return Err(NockAppError::OtherError(format!(
                        "witness-data referenced unknown spend {} / {}",
                        name.first.to_base58(),
                        name.last.to_base58()
                    )));
                };
                spend1.witness = witness;
            }
        }
        other => {
            return Err(NockAppError::OtherError(format!(
                "unsupported witness-data tag {other}"
            )));
        }
    }
    Ok(spends)
}

fn effect_tag(effect: &NounSlab) -> Option<String> {
    let noun = unsafe { effect.root() };
    let cell = noun.as_cell().ok()?;
    cell.head().as_atom().ok()?.into_string().ok()
}

fn effect_exit_code(effects: &[NounSlab]) -> Option<u64> {
    effects.iter().find_map(|effect| {
        if effect_tag(effect).as_deref() != Some("exit") {
            return None;
        }
        let noun = unsafe { effect.root() };
        let cell = noun.as_cell().ok()?;
        <u64 as NounDecode>::from_noun(&cell.tail()).ok()
    })
}

fn format_note_names(names: &[Name]) -> String {
    names
        .iter()
        .map(|name| format!("[{} {}]", name.first.to_base58(), name.last.to_base58()))
        .collect::<Vec<_>>()
        .join(",")
}

async fn peek_master_signing_key(wallet: &mut Wallet) -> Result<Hash, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "master-signing-key").as_noun();
    slab.modify(|_| vec![tag, SIG]);

    let result = wallet.app.peek(slab).await?;
    let decoded: Option<Option<Hash>> = unsafe { Option::from_noun(result.root())? };
    decoded.flatten().ok_or_else(|| {
        NockAppError::OtherError("wallet master-signing-key peek returned no payload".to_string())
    })
}

async fn peek_master_signing_pubkey(wallet: &mut Wallet) -> Result<SchnorrPubkey, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "master-signing-pubkey").as_noun();
    slab.modify(|_| vec![tag, SIG]);

    let result = wallet.app.peek(slab).await?;
    let decoded: Option<Option<SchnorrPubkey>> = unsafe { Option::from_noun(result.root())? };
    decoded.flatten().ok_or_else(|| {
        NockAppError::OtherError(
            "wallet master-signing-pubkey peek returned no payload".to_string(),
        )
    })
}

async fn peek_active_signers(
    wallet: &mut Wallet,
) -> Result<Vec<ActiveSignerEntryNoun>, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "active-signers").as_noun();
    slab.modify(|_| vec![tag, SIG]);

    let result = wallet.app.peek(slab).await?;
    let decoded: Option<Option<Vec<ActiveSignerEntryNoun>>> =
        unsafe { Option::from_noun(result.root())? };
    let mut signers = decoded.flatten().unwrap_or_default();
    signers.sort_by_key(|signer| {
        (
            if signer.child_index.is_none() { 0 } else { 1 },
            signer.absolute_index.unwrap_or(0),
            signer.address_b58.clone(),
        )
    });
    signers.dedup_by(|left, right| {
        left.child_index == right.child_index
            && left.hardened == right.hardened
            && left.absolute_index == right.absolute_index
            && left.address_b58 == right.address_b58
    });
    Ok(signers)
}

async fn import_seed_phrase(
    wallet: &mut Wallet,
    seedphrase: &str,
    version: u64,
) -> Result<(), NockAppError> {
    let (noun, _) = Wallet::import_seed_phrase(seedphrase, version)?;
    let wire = WalletWire::Command(Commands::ImportKeys {
        file: None,
        key: None,
        seedphrase: Some(seedphrase.to_string()),
        version: Some(version),
    })
    .to_wire();
    let _ = wallet.app.poke(wire, noun).await?;
    Ok(())
}

async fn derive_child_key(
    wallet: &mut Wallet,
    index: u64,
    hardened: bool,
) -> Result<(), NockAppError> {
    let label = None;
    let (noun, _) = Wallet::derive_child(index, hardened, &label)?;
    let wire = WalletWire::Command(Commands::DeriveChild {
        index,
        hardened,
        label,
    })
    .to_wire();
    let _ = wallet.app.poke(wire, noun).await?;
    Ok(())
}

async fn apply_balance_update(
    wallet: &mut Wallet,
    balance_update: v1::BalanceUpdate,
) -> Result<(), NockAppError> {
    let poke = Wallet::update_balance_grpc_poke_for_tests(balance_update);
    let _ = wallet.app.poke(SystemWire.to_wire(), poke).await?;
    Ok(())
}

async fn boot_test_wallet() -> Result<(Wallet, TempDir), NockAppError> {
    let cli = BootCli::parse_from(["wallet", "--new"]);
    let data_dir = tempfile::tempdir().map_err(NockAppError::IoError)?;
    let prover_hot_state = produce_prover_hot_state();
    let nockapp = boot::setup(
        KERNEL,
        cli.clone(),
        prover_hot_state.as_slice(),
        "wallet",
        Some(data_dir.path().to_path_buf()),
    )
    .await
    .map_err(|e| CrownError::Unknown(e.to_string()))?;
    Ok((Wallet::new(nockapp), data_dir))
}

async fn peek_wallet_blockchain_constants(
    wallet: &mut Wallet,
) -> Result<PlannerBlockchainConstantsNoun, NockAppError> {
    let mut slab = NounSlab::new();
    let state_tag = make_tas(&mut slab, "state").as_noun();
    slab.modify(|_| vec![state_tag, SIG]);

    let result = wallet.app.peek(slab).await?;
    let decoded: Option<Option<Noun>> = unsafe { Option::from_noun(result.root())? };
    let state = decoded
        .flatten()
        .ok_or_else(|| NockAppError::OtherError("missing wallet state payload".to_string()))?;
    PlannerBlockchainConstantsNoun::from_noun(&state.slot(31).map_err(|err| {
        NockAppError::OtherError(format!("wallet state missing blockchain constants: {err}"))
    })?)
    .map_err(|err| NockAppError::OtherError(format!("decode blockchain constants failed: {err}")))
}

async fn peek_balance_state(wallet: &mut Wallet) -> Result<v1::BalanceUpdate, NockAppError> {
    let mut slab = NounSlab::new();
    let balance_tag = make_tas(&mut slab, "balance").as_noun();
    let path = T(&mut slab, &[balance_tag, SIG]);
    slab.set_root(path);

    let result = wallet.app.peek(slab).await?;
    let maybe_balance: Option<Option<v1::BalanceUpdate>> =
        unsafe { <Option<Option<v1::BalanceUpdate>>>::from_noun(result.root())? };
    match maybe_balance {
        Some(Some(balance)) => Ok(balance),
        _ => Err(NockAppError::OtherError(
            "wallet balance peek returned no balance payload".to_string(),
        )),
    }
}

#[test]
fn timelock_cli_accepts_ascending_bound() {
    let range: TimelockRangeCli = "1..5".parse().unwrap();
    let absolute = range.absolute();
    assert_eq!(absolute.min, Some(BlockHeight(Belt(1))));
    assert_eq!(absolute.max, Some(BlockHeight(Belt(5))));
}

#[test]
fn timelock_cli_accepts_open_upper_bound() {
    let range: TimelockRangeCli = "..5".parse().unwrap();
    let absolute = range.absolute();
    assert_eq!(absolute.min, None);
    assert_eq!(absolute.max, Some(BlockHeight(Belt(5))));
}

#[test]
fn timelock_cli_accepts_open_lower_bound() {
    let range: TimelockRangeCli = "7..".parse().unwrap();
    let relative = range.relative();
    assert_eq!(relative.min, Some(BlockHeightDelta(Belt(7))));
    assert_eq!(relative.max, None);
}

#[test]
fn timelock_cli_rejects_descending_bounds() {
    let err = TimelockRangeCli::from_bounds(Some(10), Some(5)).unwrap_err();
    assert!(err.contains("min <= max"));
}

#[test]
fn timelock_cli_allows_fully_open_interval() {
    let range: TimelockRangeCli = "..".parse().unwrap();
    assert!(range.absolute().min.is_none() && range.absolute().max.is_none());
    assert!(range.relative().min.is_none() && range.relative().max.is_none());
    assert!(!range.has_upper_bound());
}

#[test]
fn timelock_intent_from_ranges_handles_none() {
    assert!(Wallet::timelock_intent_from_ranges(None, None).is_none());
    let open_range: TimelockRangeCli = "..".parse().unwrap();

    let explicit_none = Wallet::timelock_intent_from_ranges(
        Some(open_range.absolute()),
        Some(open_range.relative()),
    )
    .expect("expected explicit timelock intent");

    assert_eq!(
        explicit_none,
        v0::TimelockIntent {
            absolute: TimelockRangeAbsolute::none(),
            relative: TimelockRangeRelative::none(),
        }
    );
}

#[test]
fn timelock_intent_from_ranges_accepts_partial_specs() {
    let absolute = TimelockRangeAbsolute::none();
    let intent = Wallet::timelock_intent_from_ranges(Some(absolute.clone()), None)
        .expect("absolute range should produce intent");
    assert_eq!(intent.absolute, absolute);
    assert_eq!(intent.relative, TimelockRangeRelative::none());
}

#[test]
fn parse_note_names_accepts_valid_pairs() {
    let parsed = Wallet::parse_note_names("[foo bar],[baz qux]").expect("valid names");
    assert_eq!(
        parsed,
        vec![("foo".to_string(), "bar".to_string()), ("baz".to_string(), "qux".to_string())]
    );
}

#[test]
fn parse_note_names_rejects_invalid_format() {
    let err = Wallet::parse_note_names("foo bar").expect_err("expected failure");
    assert!(
        err.to_string().contains("Invalid note name"),
        "unexpected error message: {err}"
    );
}

#[test]
fn manual_planner_parity_accepts_matching_names() {
    let requested = vec![name(1, 101), name(2, 102)];
    let planned = vec![name(1, 101), name(2, 102)];
    ensure_manual_planner_parity(&requested, &planned)
        .expect("matching planner output should pass parity check");
}

#[test]
fn manual_planner_parity_accepts_reordered_names() {
    let requested = vec![name(1, 101), name(2, 102)];
    let planned = vec![name(2, 102), name(1, 101)];
    ensure_manual_planner_parity(&requested, &planned)
        .expect("reordered planner output should pass parity check");
}

#[test]
fn manual_planner_parity_rejects_name_mismatch() {
    let requested = vec![name(1, 101), name(2, 102)];
    let planned = vec![name(1, 101)];
    let err = ensure_manual_planner_parity(&requested, &planned)
        .expect_err("name mismatch should fail parity check");
    assert!(
        err.contains("selected names differ"),
        "unexpected parity error: {err}"
    );
}

#[test]
fn union_balance_pages_returns_none_for_empty_input() {
    let merged = Wallet::union_balance_pages(Vec::new()).expect("empty input should succeed");
    assert!(merged.is_none());
}

#[test]
fn union_balance_pages_merges_and_deduplicates_notes() {
    let note_a = note_v1(1, 101, 10, 5);
    let note_b = note_v1(2, 102, 10, 9);
    let page_one = balance_page(
        88,
        777,
        vec![(name(1, 101), note_a.clone()), (name(2, 102), note_b.clone())],
    );
    let page_two = balance_page(
        88,
        777,
        vec![
            (name(1, 101), note_a),
            // Duplicate note name with identical payload should dedupe.
            (name(2, 102), note_b),
        ],
    );

    let (merged_page, normalized) = Wallet::union_balance_pages(vec![page_one, page_two])
        .expect("union should succeed")
        .expect("expected merged snapshot");

    assert_eq!(merged_page.height, BlockHeight(Belt(88)));
    assert_eq!(merged_page.block_id, hash(777));
    assert_eq!(merged_page.notes.0.len(), 2);
    assert_eq!(normalized.candidates.len(), 2);
}

#[test]
fn sync_filters_untracked_v1_notes_before_wallet_state_update() {
    let tracked = note_v1(1, 101, 10, 5);
    let untracked = note_v1(2, 102, 10, 9);
    let page = balance_page(
        88,
        777,
        vec![(name(1, 101), tracked), (name(2, 102), untracked)],
    );

    let filtered = Wallet::filter_untracked_v1_notes_for_tests(page, vec![hash(1)]);
    assert_eq!(filtered.notes.0.len(), 1);
    assert_eq!(filtered.notes.0[0].0, name(1, 101));
}

#[test]
fn planner_signer_candidates_include_no_signer_and_sorted_unique_signers() {
    let candidates = Wallet::planner_signer_candidates(vec![hash(2), hash(1), hash(2)]);
    assert_eq!(candidates, vec![None, Some(hash(1)), Some(hash(2))]);
}

#[test]
fn planner_signer_candidates_still_try_no_signer_when_tracked_set_is_empty() {
    let candidates = Wallet::planner_signer_candidates(Vec::new());
    assert_eq!(candidates, vec![None]);
}

#[test]
fn signing_key_lock_matcher_accepts_simple_lock_for_matching_signer() {
    let signer = hash(7);
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(7)]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(
        nockchain_types::tx_engine::v1::tx::Pkh::new(1, vec![signer]),
    )]);
    let first_name = spend_condition
        .first_name()
        .expect("simple first-name should compute")
        .into_hash();

    assert!(matcher.matches(&first_name, &spend_condition));
    assert!(!matcher.matches(&hash(999), &spend_condition));
}

#[test]
fn signing_key_lock_matcher_rejects_when_signer_hash_not_present() {
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(1)]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(
        nockchain_types::tx_engine::v1::tx::Pkh::new(1, vec![hash(2)]),
    )]);
    let first_name = spend_condition
        .first_name()
        .expect("simple first-name should compute")
        .into_hash();

    assert!(!matcher.matches(&first_name, &spend_condition));
}

#[test]
fn signing_key_lock_matcher_rejects_threshold_lock_when_single_signer_cannot_meet_m() {
    let signer = hash(5);
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(5)]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(
        nockchain_types::tx_engine::v1::tx::Pkh::new(2, vec![hash(9), signer]),
    )]);

    assert!(!matcher.matches(&hash(1234), &spend_condition));
}

#[test]
fn signing_key_lock_matcher_rejects_multisig_lock_even_when_single_sig_threshold_is_one() {
    let signer = hash(5);
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(5)]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(
        nockchain_types::tx_engine::v1::tx::Pkh::new(1, vec![hash(9), signer]),
    )]);

    assert!(!matcher.matches(&hash(1234), &spend_condition));
}

#[test]
fn signing_key_lock_matcher_rejects_multisig_lock_when_signers_meet_threshold() {
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(5), signer_key(7)]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(
        nockchain_types::tx_engine::v1::tx::Pkh::new(2, vec![hash(5), hash(6), hash(7)]),
    )]);

    assert!(!matcher.matches(&hash(1234), &spend_condition));
}

#[test]
fn signing_key_lock_matcher_accepts_coinbase_shape_for_matching_signer() {
    let signer = hash(8);
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(8)]);
    let spend_condition = SpendCondition::new(vec![
        LockPrimitive::Pkh(nockchain_types::tx_engine::v1::tx::Pkh::new(
            1,
            vec![signer],
        )),
        LockPrimitive::Tim(nockchain_types::tx_engine::v1::tx::LockTim {
            rel: TimelockRangeRelative::new(Some(BlockHeightDelta(Belt(1))), None),
            abs: TimelockRangeAbsolute::none(),
        }),
    ]);
    let first_name = spend_condition
        .first_name()
        .expect("coinbase first-name should compute")
        .into_hash();

    assert!(matcher.matches(&first_name, &spend_condition));
    assert!(!matcher.matches(&hash(82), &spend_condition));
}

#[test]
fn signing_key_lock_matcher_rejects_non_pkh_locks() {
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(1)]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Burn]);
    assert!(!matcher.matches(&hash(10), &spend_condition));
}

#[test]
fn signing_key_lock_matcher_rejects_unsupported_primitive_even_with_matching_signer() {
    let signer = hash(1);
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(1)]);
    let spend_condition = SpendCondition::new(vec![
        LockPrimitive::Pkh(nockchain_types::tx_engine::v1::tx::Pkh::new(
            1,
            vec![signer],
        )),
        LockPrimitive::Burn,
    ]);
    assert!(!matcher.matches(&hash(10), &spend_condition));
}

#[test]
fn planner_recipient_outputs_match_hoon_lock_root_vectors() {
    const EXPECTED_PKH_ROOT_B58: &str = "DKrgXqE8bXR1uBZ3t4vU13m2KquGCDbnn1PeoPL7dxSHTucGPFDPt53";
    const EXPECTED_MULTISIG_2_OF_2_ROOT_B58: &str =
        "4eMAT3BuhLPjYFronoYJ9RSLVSgveCL3nQB7RHSLZzjBTiYCxEzkzEH";
    const ADDRESS_A_B58: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";
    const ADDRESS_B_B58: &str = "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt";

    let address_a = Hash::from_base58(ADDRESS_A_B58).expect("address a should parse");
    let address_b = Hash::from_base58(ADDRESS_B_B58).expect("address b should parse");
    let bridge_address =
        nockchain_types::EthAddress::from_hex_str("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .expect("bridge address should parse");
    let recipients = vec![
        RecipientSpec::P2pkh {
            address: address_a.clone(),
            amount: 5,
        },
        RecipientSpec::Multisig {
            threshold: 2,
            addresses: vec![address_a.clone(), address_b],
            amount: 7,
        },
        RecipientSpec::BridgeDeposit {
            evm_address: bridge_address,
            amount: 9,
        },
    ];

    let outputs = planner_recipient_outputs(&recipients, true).expect("recipient outputs");
    assert_eq!(outputs.len(), 3);
    assert_eq!(outputs[0].lock_root.to_base58(), EXPECTED_PKH_ROOT_B58);
    assert_eq!(
        outputs[1].lock_root.to_base58(),
        EXPECTED_MULTISIG_2_OF_2_ROOT_B58
    );
    assert_eq!(
        outputs[2].lock_root.to_base58(),
        BRIDGE_LOCK_ROOT_DEFAULT_B58
    );
    assert_eq!(outputs[0].note_data.len(), 1);
    assert_eq!(outputs[0].note_data[0].key, "lock");
    assert_eq!(outputs[1].note_data.len(), 1);
    assert_eq!(outputs[1].note_data[0].key, "lock");
    assert_eq!(outputs[2].note_data.len(), 1);
    assert_eq!(outputs[2].note_data[0].key, "bridge");
}

#[test]
fn planner_recipient_outputs_respect_include_data_for_p2pkh_but_not_multisig_or_bridge() {
    const ADDRESS_A_B58: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";
    const ADDRESS_B_B58: &str = "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt";

    let address_a = Hash::from_base58(ADDRESS_A_B58).expect("address a should parse");
    let address_b = Hash::from_base58(ADDRESS_B_B58).expect("address b should parse");
    let bridge_address =
        nockchain_types::EthAddress::from_hex_str("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .expect("bridge address should parse");
    let recipients = vec![
        RecipientSpec::P2pkh {
            address: address_a.clone(),
            amount: 5,
        },
        RecipientSpec::Multisig {
            threshold: 2,
            addresses: vec![address_a.clone(), address_b],
            amount: 7,
        },
        RecipientSpec::BridgeDeposit {
            evm_address: bridge_address,
            amount: 9,
        },
    ];

    let outputs = planner_recipient_outputs(&recipients, false).expect("recipient outputs");
    assert_eq!(outputs[0].note_data.len(), 0);
    assert_eq!(outputs[1].note_data.len(), 1);
    assert_eq!(outputs[1].note_data[0].key, "lock");
    assert_eq!(outputs[2].note_data.len(), 1);
    assert_eq!(outputs[2].note_data[0].key, "bridge");
}

#[test]
fn planner_refund_output_template_includes_lock_note_data_when_enabled() {
    let signer = hash(1234);
    let with_data =
        planner_refund_output_template(None, &signer, true).expect("refund output with data");
    assert_eq!(with_data.amount, 0);
    assert_eq!(with_data.note_data.len(), 1);
    assert_eq!(with_data.note_data[0].key, "lock");

    let without_data =
        planner_refund_output_template(None, &signer, false).expect("refund output without data");
    assert_eq!(without_data.amount, 0);
    assert_eq!(without_data.note_data.len(), 0);
}

#[test]
fn planner_constants_decode_from_dedicated_peek_shape() {
    let constants = PlannerBlockchainConstantsNoun {
        _v1_phase: 40_000,
        bythos_phase: 54_000,
        data: PlannerNoteDataConstantsNoun {
            _max_size: 2_048,
            min_fee: 256,
        },
        base_fee: 128,
        input_fee_divisor: 4,
        _legacy_constants: D(0),
    };
    let wrapped = Some(Some(constants));

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = wrapped.to_noun(&mut slab);
    let decoded: Option<Option<PlannerBlockchainConstantsNoun>> =
        Option::from_noun(&noun).expect("peek payload should decode");
    let parsed = decoded.flatten().expect("payload should be present");
    assert_eq!(parsed.bythos_phase, 54_000);
    assert_eq!(parsed.base_fee, 128);
    assert_eq!(parsed.input_fee_divisor, 4);
    assert_eq!(parsed.data.min_fee, 256);
}

#[test]
fn planner_constants_extract_coinbase_timelock_min_from_payload() {
    let constants = default_fakenet_blockchain_constants();
    let wrapped = Some(Some(constants.clone()));

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = wrapped.to_noun(&mut slab);
    let decoded: Option<Option<PlannerBlockchainConstantsNoun>> =
        Option::from_noun(&noun).expect("peek payload should decode");
    let parsed = decoded.flatten().expect("payload should be present");

    assert_eq!(
        parsed
            .coinbase_timelock_min()
            .expect("coinbase timelock min should decode"),
        constants.coinbase_timelock_min
    );
}

#[tokio::test]
async fn fakenet_mode_sets_low_bythos_phase_in_wallet_constants() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let expected_after = default_fakenet_blockchain_constants();

    assert!(!wallet.is_fakenet().await?);

    let before = peek_wallet_blockchain_constants(&mut wallet).await?;
    assert_eq!(before._v1_phase, 39_000);
    assert_eq!(before.bythos_phase, 54_000);
    assert_eq!(before.base_fee, 16_384);
    assert_eq!(before.input_fee_divisor, 4);
    assert_eq!(before.data.min_fee, 256);
    assert_eq!(before.coinbase_timelock_min()?, 100);

    wallet.set_fakenet().await?;
    assert!(wallet.is_fakenet().await?);

    let after = peek_wallet_blockchain_constants(&mut wallet).await?;
    assert_eq!(after._v1_phase, expected_after.v1_phase);
    assert_eq!(after.bythos_phase, expected_after.bythos_phase);
    assert_eq!(after.base_fee, expected_after.base_fee);
    assert_eq!(after.input_fee_divisor, expected_after.input_fee_divisor);
    assert_eq!(after.data.min_fee, expected_after.note_data.min_fee);
    assert_eq!(
        after.coinbase_timelock_min()?,
        expected_after.coinbase_timelock_min
    );

    Ok(())
}

#[tokio::test]
async fn fakenet_create_tx_accepts_discounted_fee_schedule() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    wallet.set_fakenet().await?;

    let signer_pkh = peek_master_signing_key(&mut wallet).await?;
    let simple = SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        1,
        vec![signer_pkh.clone()],
    ))]);
    let note_name = Name::new(
        simple
            .first_name()
            .expect("simple first-name should compute")
            .into_hash(),
        hash(9_999),
    );
    let note = note_v1_with_lock(
        note_name.clone(),
        1,
        10_000,
        simple_pkh_lock(signer_pkh.clone()),
    );
    apply_balance_update(
        &mut wallet,
        balance_page(1, 777, vec![(note_name.clone(), note)]),
    )
    .await?;

    let recipient = RecipientSpec::P2pkh {
        address: signer_pkh,
        amount: 4_000,
    };
    let (noun, _) = Wallet::create_tx_command_for_tests(
        format_note_names(std::slice::from_ref(&note_name)),
        vec![recipient],
        3_584,
        false,
        None,
        Vec::new(),
        true,
        false,
        NoteSelectionStrategyCli::Ascending,
    )?;
    let wire = WalletWire::Command(Commands::CreateTx {
        names: Some(String::new()),
        recipients: Vec::new(),
        fee: Some(3_584),
        allow_low_fee: false,
        refund_pkh: None,
        index: None,
        hardened: false,
        include_data: true,
        sign_keys: Vec::new(),
        save_raw_tx: false,
        note_selection_strategy: NoteSelectionStrategyCli::Ascending,
    })
    .to_wire();

    let result = wallet.app.poke(wire, noun).await?;
    assert!(
        result.len() > 1,
        "fakenet create-tx should emit transaction effects, got {result:?}"
    );

    Ok(())
}

#[tokio::test]
async fn signing_keys_support_rust_first_name_reconstruction_in_fakenet() -> Result<(), NockAppError>
{
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    wallet.set_fakenet().await?;

    let constants = peek_wallet_blockchain_constants(&mut wallet).await?;
    let relative_min = constants.coinbase_timelock_min()?;
    let signer_pkh = peek_master_signing_key(&mut wallet).await?;
    let simple = SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        1,
        vec![signer_pkh.clone()],
    ))]);
    let coinbase = SpendCondition::new(vec![
        LockPrimitive::Pkh(Pkh::new(1, vec![signer_pkh.clone()])),
        LockPrimitive::Tim(nockchain_types::tx_engine::v1::tx::LockTim {
            rel: TimelockRangeRelative::new(Some(BlockHeightDelta(Belt(relative_min))), None),
            abs: TimelockRangeAbsolute::none(),
        }),
    ]);
    let rust_simple = simple
        .first_name()
        .expect("simple first-name should compute");
    let rust_coinbase = coinbase
        .first_name()
        .expect("coinbase first-name should compute");

    assert_ne!(
        rust_simple,
        rust_coinbase,
        "simple and coinbase first-name should differ for signer {}",
        signer_pkh.to_base58()
    );

    Ok(())
}

#[tokio::test]
async fn migrate_v0_notes_per_signer_writes_tx_for_v0_master_bucket() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 0).await?;
    wallet.set_fakenet().await?;

    let destination = peek_master_signing_key(&mut wallet).await?.to_base58();
    let signer_pubkey = peek_master_signing_pubkey(&mut wallet).await?;

    let v0_note_name = name(51, 5_151);
    let v0_note = note_v0_with_lock(
        v0_note_name.clone(),
        1,
        25_000,
        simple_v0_lock(signer_pubkey.clone()),
    );
    apply_balance_update(
        &mut wallet,
        balance_page(1, 888, vec![(v0_note_name, v0_note)]),
    )
    .await?;

    let summary = wallet
        .migrate_v0_notes_per_signer_for_tests(None, destination, data_dir.path())
        .await?;
    assert_eq!(summary.examined_signers, 1);
    assert_eq!(summary.created_count, 1);
    assert_eq!(summary.skipped_count, 0);

    let signer_summary = summary
        .signers
        .iter()
        .find(|signer| signer.tx_path.is_some())
        .expect("master signer migration should create a tx");
    assert!(signer_summary.signer.child_index.is_none());
    assert!(std::path::Path::new(
        signer_summary
            .tx_path
            .as_ref()
            .expect("created migration should emit a tx path")
    )
    .exists());

    Ok(())
}

#[tokio::test]
async fn active_signers_peek_lists_master_and_children() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    derive_child_key(&mut wallet, 0, false).await?;
    derive_child_key(&mut wallet, 1, true).await?;

    let signers = peek_active_signers(&mut wallet).await?;

    assert_eq!(signers.len(), 3, "expected master plus two child signers");
    assert!(signers[0].child_index.is_none(), "master should sort first");
    assert_eq!(signers[1].child_index, Some(0));
    assert!(!signers[1].hardened);
    assert_eq!(signers[1].absolute_index, Some(0));
    assert_eq!(signers[2].child_index, Some(1));
    assert!(signers[2].hardened);
    assert_eq!(signers[2].absolute_index, Some((1u64 << 31) + 1));

    Ok(())
}

#[tokio::test]
async fn active_signers_peek_lists_v0_hardened_and_unhardened_children_for_v0_master(
) -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 0).await?;
    derive_child_key(&mut wallet, 0, false).await?;
    derive_child_key(&mut wallet, 1, true).await?;

    let signers = peek_active_signers(&mut wallet).await?;

    assert_eq!(signers.len(), 3, "expected master plus two child signers");
    assert!(signers.iter().all(|signer| signer.version == 0));

    let unhardened_child = signers
        .iter()
        .find(|signer| signer.child_index == Some(0))
        .expect("unhardened v0 child signer should exist");
    assert!(!unhardened_child.hardened);
    assert_eq!(unhardened_child.absolute_index, Some(0));

    let hardened_child = signers
        .iter()
        .find(|signer| signer.child_index == Some(1))
        .expect("hardened v0 child signer should exist");
    assert!(hardened_child.hardened);
    assert_eq!(hardened_child.absolute_index, Some((1u64 << 31) + 1));

    Ok(())
}

#[tokio::test]
async fn migrate_v0_notes_per_signer_creates_txs_for_v0_child_buckets() -> Result<(), NockAppError>
{
    init_tracing();
    let (mut wallet, data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    wallet.set_fakenet().await?;
    derive_child_key(&mut wallet, 1, true).await?;
    derive_child_key(&mut wallet, 2, true).await?;

    let destination = peek_master_signing_key(&mut wallet).await?.to_base58();
    let signers = peek_active_signers(&mut wallet).await?;
    let child_signer_1 = signers
        .iter()
        .find(|signer| signer.child_index == Some(1) && signer.hardened && signer.version == 0)
        .expect("first v0 child signer should exist")
        .clone();
    let child_signer_2 = signers
        .iter()
        .find(|signer| signer.child_index == Some(2) && signer.hardened && signer.version == 0)
        .expect("second v0 child signer should exist")
        .clone();

    let child_note_name_1 = name(71, 7_171);
    let child_note_name_2 = name(72, 7_272);
    let child_note_1 = note_v0_with_lock(
        child_note_name_1.clone(),
        1,
        25_000,
        simple_v0_lock(child_signer_1.pubkey.clone()),
    );
    let child_note_2 = note_v0_with_lock(
        child_note_name_2.clone(),
        1,
        25_000,
        simple_v0_lock(child_signer_2.pubkey.clone()),
    );
    apply_balance_update(
        &mut wallet,
        balance_page(
            1,
            1_717,
            vec![(child_note_name_1, child_note_1), (child_note_name_2, child_note_2)],
        ),
    )
    .await?;

    let summary = wallet
        .migrate_v0_notes_per_signer_for_tests(None, destination, data_dir.path())
        .await?;

    assert_eq!(summary.examined_signers, 2);
    assert_eq!(summary.created_count, 2);
    assert_eq!(summary.skipped_count, 0);
    assert_eq!(summary.signers.len(), 2);
    for signer in &summary.signers {
        assert_eq!(signer.signer.version, 0);
        assert_eq!(signer.note_count, 1);
        let tx_path = signer
            .tx_path
            .as_ref()
            .expect("created migration should emit a tx path");
        assert!(
            std::path::Path::new(tx_path).exists(),
            "expected tx file to be written at {tx_path}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn migrate_v0_notes_per_signer_creates_txs_for_mixed_hardened_v0_child_buckets(
) -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 0).await?;
    wallet.set_fakenet().await?;
    derive_child_key(&mut wallet, 0, false).await?;
    derive_child_key(&mut wallet, 1, true).await?;

    let destination = hash(8_808).to_base58();
    let signers = peek_active_signers(&mut wallet).await?;
    let unhardened_child = signers
        .iter()
        .find(|signer| signer.child_index == Some(0) && !signer.hardened && signer.version == 0)
        .expect("unhardened v0 child signer should exist")
        .clone();
    let hardened_child = signers
        .iter()
        .find(|signer| signer.child_index == Some(1) && signer.hardened && signer.version == 0)
        .expect("hardened v0 child signer should exist")
        .clone();

    let unhardened_note_name = name(73, 7_373);
    let hardened_note_name = name(74, 7_474);
    let unhardened_note = note_v0_with_lock(
        unhardened_note_name.clone(),
        1,
        25_000,
        simple_v0_lock(unhardened_child.pubkey.clone()),
    );
    let hardened_note = note_v0_with_lock(
        hardened_note_name.clone(),
        1,
        25_000,
        simple_v0_lock(hardened_child.pubkey.clone()),
    );
    apply_balance_update(
        &mut wallet,
        balance_page(
            1,
            1_818,
            vec![(unhardened_note_name, unhardened_note), (hardened_note_name, hardened_note)],
        ),
    )
    .await?;

    let summary = wallet
        .migrate_v0_notes_per_signer_for_tests(None, destination, data_dir.path())
        .await?;

    assert_eq!(summary.examined_signers, 3);
    assert_eq!(summary.created_count, 2);
    assert_eq!(summary.skipped_count, 1);

    let created = summary
        .signers
        .iter()
        .filter(|signer| signer.tx_path.is_some())
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 2, "expected one tx per child bucket");

    let created_unhardened = created
        .iter()
        .find(|signer| signer.signer.child_index == Some(0))
        .expect("unhardened child summary should exist");
    assert!(!created_unhardened.signer.hardened);
    assert_eq!(created_unhardened.signer.version, 0);
    assert_eq!(created_unhardened.note_count, 1);

    let created_hardened = created
        .iter()
        .find(|signer| signer.signer.child_index == Some(1))
        .expect("hardened child summary should exist");
    assert!(created_hardened.signer.hardened);
    assert_eq!(created_hardened.signer.version, 0);
    assert_eq!(created_hardened.note_count, 1);

    for signer in created {
        let tx_path = signer
            .tx_path
            .as_ref()
            .expect("created migration should emit a tx path");
        assert!(
            std::path::Path::new(tx_path).exists(),
            "expected tx file to be written at {tx_path}"
        );
    }

    let skipped_master = summary
        .signers
        .iter()
        .find(|signer| signer.signer.child_index.is_none())
        .expect("master signer summary should exist");
    assert_eq!(
        skipped_master.skip_reason.as_deref(),
        Some("no_eligible_v0_notes")
    );

    Ok(())
}

#[tokio::test]
async fn migrate_v0_notes_per_signer_generated_txs_validate_via_send_tx_for_mixed_v0_children(
) -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 0).await?;
    wallet.set_fakenet().await?;
    derive_child_key(&mut wallet, 0, false).await?;
    derive_child_key(&mut wallet, 1, true).await?;

    let destination = hash(9_909).to_base58();
    let signers = peek_active_signers(&mut wallet).await?;
    let unhardened_child = signers
        .iter()
        .find(|signer| signer.child_index == Some(0) && !signer.hardened && signer.version == 0)
        .expect("unhardened v0 child signer should exist")
        .clone();
    let hardened_child = signers
        .iter()
        .find(|signer| signer.child_index == Some(1) && signer.hardened && signer.version == 0)
        .expect("hardened v0 child signer should exist")
        .clone();

    apply_balance_update(
        &mut wallet,
        balance_page(
            1,
            1_919,
            vec![
                (
                    name(75, 7_575),
                    note_v0_with_lock(
                        name(75, 7_575),
                        1,
                        25_000,
                        simple_v0_lock(unhardened_child.pubkey.clone()),
                    ),
                ),
                (
                    name(76, 7_676),
                    note_v0_with_lock(
                        name(76, 7_676),
                        1,
                        25_000,
                        simple_v0_lock(hardened_child.pubkey.clone()),
                    ),
                ),
            ],
        ),
    )
    .await?;

    let summary = wallet
        .migrate_v0_notes_per_signer_for_tests(None, destination, data_dir.path())
        .await?;

    let tx_paths = summary
        .signers
        .iter()
        .filter_map(|signer| signer.tx_path.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(tx_paths.len(), 2, "expected one tx per child bucket");

    for tx_path in tx_paths {
        let (noun, _) = Wallet::send_tx(tx_path)?;
        let wire = WalletWire::Command(Commands::SendTx {
            transaction: tx_path.to_string(),
        })
        .to_wire();
        let effects = wallet.app.poke(wire, noun).await?;

        assert_eq!(
            effect_exit_code(&effects),
            Some(0),
            "send-tx should validate and exit successfully for {tx_path}"
        );
        assert!(
            effects
                .iter()
                .any(|effect| effect_tag(effect).as_deref() == Some("nockchain-grpc")),
            "send-tx should emit a nockchain-grpc send effect for {tx_path}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn create_tx_with_planner_accepts_manual_all_v0_notes() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    wallet.set_fakenet().await?;

    let destination = peek_master_signing_key(&mut wallet).await?;
    let signer_pubkey = peek_master_signing_pubkey(&mut wallet).await?;

    let v0_note_name = name(52, 5_252);
    let v0_note = note_v0_with_lock(
        v0_note_name.clone(),
        1,
        25_000,
        simple_v0_lock(signer_pubkey.clone()),
    );
    apply_balance_update(
        &mut wallet,
        balance_page(1, 889, vec![(v0_note_name.clone(), v0_note)]),
    )
    .await?;

    let (noun, _) = wallet
        .create_tx_with_planner(
            None,
            Some(format_note_names(std::slice::from_ref(&v0_note_name))),
            None,
            vec![RecipientSpec::P2pkh {
                address: destination.clone(),
                amount: 20_000,
            }],
            false,
            Some(destination.to_base58()),
            Vec::new(),
            true,
            false,
            NoteSelectionStrategyCli::Ascending,
        )
        .await?;
    let result = wallet.app.poke(OnePunchWire::Poke.to_wire(), noun).await?;
    assert!(
        result.len() > 1,
        "manual all-v0 create-tx should emit transaction effects, got {result:?}"
    );

    Ok(())
}

#[tokio::test]
async fn migrate_v0_notes_wallet_tx_matches_planner_word_and_fee_counts() -> Result<(), NockAppError>
{
    init_tracing();
    let (mut wallet, data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 0).await?;
    wallet.set_fakenet().await?;

    let destination = peek_master_signing_key(&mut wallet).await?.to_base58();
    let destination_hash = Hash::from_base58(&destination).expect("destination should parse");
    let signer_key = peek_master_signing_key(&mut wallet).await?;
    let signer_pubkey = peek_master_signing_pubkey(&mut wallet).await?;

    let v0_note_name = name(61, 6_161);
    let v0_note = note_v0_with_lock(
        v0_note_name.clone(),
        1,
        25_000,
        simple_v0_lock(signer_pubkey.clone()),
    );
    apply_balance_update(
        &mut wallet,
        balance_page(1, 999, vec![(v0_note_name, v0_note)]),
    )
    .await?;

    let balance = peek_balance_state(&mut wallet).await?;
    let snapshot = normalize_balance_pages(&[balance])
        .map_err(|err| NockAppError::OtherError(format!("snapshot normalization failed: {err}")))?;
    let mut destination_outputs = planner_recipient_outputs(
        &[RecipientSpec::P2pkh {
            address: destination_hash,
            amount: 0,
        }],
        true,
    )?;
    let destination_output = destination_outputs
        .pop()
        .expect("single migration destination should yield one output");
    let planner_constants = peek_wallet_blockchain_constants(&mut wallet).await?;
    let coinbase_relative_min = planner_constants.coinbase_timelock_min()?;
    let request = PlanRequest {
        planning_mode: PlanningMode::V0MigrationSweep {
            destination_output: destination_output.clone(),
        },
        selection_mode: SelectionMode::Auto,
        order_direction: SelectionOrder::Ascending,
        include_data: true,
        chain_context: ChainContext {
            height: snapshot.metadata.height.clone(),
            bythos_phase: BlockHeight(Belt(planner_constants.bythos_phase)),
            base_fee: planner_constants.base_fee,
            input_fee_divisor: planner_constants.input_fee_divisor,
            min_fee: planner_constants.data.min_fee,
        },
        signer_pkh: None,
        candidate_version_policy: CandidateVersionPolicy::V0Only,
        candidates: snapshot.candidates.clone(),
        recipient_outputs: Vec::new(),
        refund_output: destination_output,
        coinbase_relative_min: Some(coinbase_relative_min),
        v0_migration_signer_pubkeys: vec![signer_pubkey.clone()],
    };
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key]);
    let plan = plan_create_tx(&request, &matcher)
        .map_err(|err| NockAppError::OtherError(format!("planner failed: {err}")))?;

    let summary = wallet
        .migrate_v0_notes_per_signer_for_tests(None, destination.clone(), data_dir.path())
        .await?;
    assert_eq!(summary.created_count, 1);
    assert_eq!(summary.skipped_count, 0);
    let tx_path = summary
        .signers
        .iter()
        .find_map(|signer| signer.tx_path.as_deref())
        .expect("migration should create one tx");
    let spends = decode_saved_transaction_spends_from_path(tx_path)?;

    assert_eq!(spends.0.len(), 1, "migration should build one spend");
    assert!(
        matches!(spends.0.first(), Some((_, v1::Spend::Legacy(_)))),
        "migration should build a legacy v0 spend"
    );

    let hoon_seed_words = merged_seed_word_count(&spends);
    let hoon_witness_words = witness_word_count(&spends);
    let hoon_fee = total_paid_fee(&spends);
    let hoon_gift = total_seed_gift(&spends);
    let computed_fee = compute_minimum_fee(FeeInputs {
        seed_words: hoon_seed_words,
        witness_words: hoon_witness_words,
        base_fee: request.chain_context.base_fee,
        input_fee_divisor: request.chain_context.input_fee_divisor,
        min_fee: request.chain_context.min_fee,
        height: request.chain_context.height,
        bythos_phase: request.chain_context.bythos_phase,
    });

    // A one-input v0 migration on fakenet should emit one merged destination seed
    // note-data payload and one legacy signature map witness.
    assert_eq!(hoon_seed_words, 14);
    assert_eq!(hoon_witness_words, 31);
    assert_eq!(computed_fee.minimum_fee, 2_784);
    assert_eq!(hoon_fee, computed_fee.minimum_fee);
    assert_eq!(hoon_gift, 22_216);

    assert_eq!(plan.word_counts.seed_words, hoon_seed_words);
    assert_eq!(plan.word_counts.witness_words, hoon_witness_words);
    assert_eq!(plan.final_fee, computed_fee.minimum_fee);
    assert_eq!(plan.outputs.len(), 1);
    assert_eq!(plan.outputs[0].amount, hoon_gift);
    assert_eq!(plan.selected_total, hoon_fee + hoon_gift);

    Ok(())
}

#[test]
fn master_signing_key_decodes_from_hash_payload_shape() {
    let wrapped = Some(Some(hash(3)));

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = wrapped.to_noun(&mut slab);
    let decoded: Option<Option<Hash>> =
        Option::from_noun(&noun).expect("master signing key payload should decode");
    let parsed = decoded.flatten().expect("payload should be present");

    assert_eq!(parsed, hash(3));
}

#[test]
fn migrate_v0_notes_summary_includes_send_tx_command_for_saved_transactions() {
    let v0_address =
        "2cPnE4Z9RevhTv9is9Hmc1amFubEFbUxzCV2Fxb9GxevJstV5VG92oYt6Sai3d3NjLFcsuVXSLx9hikMbD1agv9M267TVw3hV9MCpMfEnGo5LYtjJ7jPyHg8SERPjJRCWTgZ";
    let summary = MigrateV0NotesSummary {
        destination: "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt".to_string(),
        block_id: "block-id".to_string(),
        height: 33,
        examined_signers: 1,
        created_count: 1,
        skipped_count: 0,
        signers: vec![MigrateV0SignerSummary {
            signer: ActiveSignerEntryNoun {
                child_index: Some(0),
                hardened: false,
                absolute_index: Some(0),
                version: 0,
                pubkey: SchnorrPubkey::from_base58(v0_address)
                    .expect("sample v0 signer pubkey should parse"),
                address_b58: v0_address.to_string(),
            },
            note_count: 1,
            selected_total: 25_000,
            fee: Some(2_784),
            migrated_amount: Some(22_216),
            tx_path: Some("./txs/example.tx".to_string()),
            skip_reason: None,
        }],
    };

    let rendered = Wallet::format_migrate_v0_notes_summary(&summary);

    assert!(rendered.contains("- tx path: `./txs/example.tx`"));
    assert!(rendered.contains("- submit with: `nockchain-wallet send-tx \"./txs/example.tx\"`"));
}

#[test]
fn migrate_v0_notes_summary_mentions_when_no_batch_poke_was_emitted() {
    let summary = MigrateV0NotesSummary {
        destination: "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt".to_string(),
        block_id: "block-id".to_string(),
        height: 33,
        examined_signers: 1,
        created_count: 0,
        skipped_count: 1,
        signers: vec![MigrateV0SignerSummary {
            signer: ActiveSignerEntryNoun {
                child_index: None,
                hardened: false,
                absolute_index: None,
                version: 0,
                pubkey: SchnorrPubkey::from_base58(
                    "2cPnE4Z9RevhTv9is9Hmc1amFubEFbUxzCV2Fxb9GxevJstV5VG92oYt6Sai3d3NjLFcsuVXSLx9hikMbD1agv9M267TVw3hV9MCpMfEnGo5LYtjJ7jPyHg8SERPjJRCWTgZ",
                )
                .expect("sample v0 signer pubkey should parse"),
                address_b58: "2cPnE4Z9RevhTv9is9Hmc1amFubEFbUxzCV2Fxb9GxevJstV5VG92oYt6Sai3d3NjLFcsuVXSLx9hikMbD1agv9M267TVw3hV9MCpMfEnGo5LYtjJ7jPyHg8SERPjJRCWTgZ"
                    .to_string(),
            },
            note_count: 0,
            selected_total: 0,
            fee: Some(2_784),
            migrated_amount: None,
            tx_path: None,
            skip_reason: Some("no_eligible_v0_notes".to_string()),
        }],
    };

    let rendered = Wallet::format_migrate_v0_notes_summary(&summary);

    assert!(rendered
        .contains("- batch create poke: not emitted because every signer bucket was skipped"));
}

#[test]
fn collect_signing_keys_prefers_explicit_entries() {
    let entries = vec!["0:true".to_string(), "1:false".to_string()];
    let keys = Wallet::collect_signing_keys(Some(5), false, &entries).expect("valid explicit keys");
    assert_eq!(keys, vec![(0, true), (1, false)]);
}

#[test]
fn collect_signing_keys_falls_back_to_index() {
    let keys = Wallet::collect_signing_keys(Some(3), true, &[]).expect("valid");
    assert_eq!(keys, vec![(3, true)]);
}

#[test]
fn collect_signing_keys_defaults_to_master() {
    let keys = Wallet::collect_signing_keys(None, false, &[]).expect("valid");
    assert!(keys.is_empty());
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_keygen() -> Result<(), NockAppError> {
    init_tracing();
    let cli = BootCli::parse_from(&["--new"]);

    let prover_hot_state = produce_prover_hot_state();
    let nockapp = boot::setup(
        KERNEL,
        cli.clone(),
        prover_hot_state.as_slice(),
        "wallet",
        None,
    )
    .await
    .map_err(|e| CrownError::Unknown(e.to_string()))?;
    let mut wallet = Wallet::new(nockapp);
    let mut entropy = [0u8; 32];
    let mut salt = [0u8; 16];
    getrandom::fill(&mut entropy).map_err(|e| CrownError::Unknown(e.to_string()))?;
    getrandom::fill(&mut salt).map_err(|e| CrownError::Unknown(e.to_string()))?;
    let (noun, op) = Wallet::keygen(&entropy, &salt)?;

    let wire = WalletWire::Command(Commands::Keygen).to_wire();

    let keygen_result = wallet.app.poke(wire, noun.clone()).await?;

    println!("keygen result: {:?}", keygen_result);
    assert!(
        keygen_result.len() == 2,
        "Expected keygen result to be a list of 2 noun slabs - markdown and exit"
    );
    let exit_cause = unsafe { keygen_result[1].root() };
    let code = exit_cause.as_cell()?.tail();
    assert!(unsafe { code.raw_equals(&D(0)) }, "Expected exit code 0");

    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_derive_child() -> Result<(), NockAppError> {
    init_tracing();
    let cli = BootCli::parse_from(&["--new"]);

    let prover_hot_state = produce_prover_hot_state();
    let nockapp = boot::setup(
        KERNEL,
        cli.clone(),
        prover_hot_state.as_slice(),
        "wallet",
        None,
    )
    .await
    .map_err(|e| CrownError::Unknown(e.to_string()))?;
    let mut wallet = Wallet::new(nockapp);

    // Generate a new key pair
    let mut entropy = [0u8; 32];
    let mut salt = [0u8; 16];
    let (noun, op) = Wallet::keygen(&entropy, &salt)?;
    let wire = WalletWire::Command(Commands::Keygen).to_wire();
    let _ = wallet.app.poke(wire, noun.clone()).await?;

    // Derive a child key
    let index = 0;
    let hardened = true;
    let label = None;
    let (noun, op) = Wallet::derive_child(index, hardened, &label)?;

    let wire = WalletWire::Command(Commands::DeriveChild {
        index,
        hardened,
        label,
    })
    .to_wire();

    let derive_result = wallet.app.poke(wire, noun.clone()).await?;

    assert!(
        derive_result.len() == 2,
        "Expected derive result to be a list of 2 noun slabs - markdown and exit"
    );

    let exit_cause = unsafe { derive_result[1].root() };
    let code = exit_cause.as_cell()?.tail();
    assert!(unsafe { code.raw_equals(&D(0)) }, "Expected exit code 0");

    Ok(())
}

// Tests for Cold Side Commands
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_gen_master_privkey() -> Result<(), NockAppError> {
    init_tracing();
    let cli = BootCli::parse_from(&[""]);
    let nockapp = boot::setup(KERNEL, cli.clone(), &[], "wallet", None)
        .await
        .map_err(|e| CrownError::Unknown(e.to_string()))?;
    let mut wallet = Wallet::new(nockapp);
    let seedphrase = "correct horse battery staple";
    let version = 1;
    let (noun, op) = Wallet::import_seed_phrase(seedphrase, version)?;
    println!("privkey_slab: {:?}", noun);
    let wire = WalletWire::Command(Commands::ImportKeys {
        file: None,
        key: None,
        seedphrase: Some(seedphrase.to_string()),
        version: Some(version),
    })
    .to_wire();
    let privkey_result = wallet.app.poke(wire, noun.clone()).await?;
    println!("privkey_result: {:?}", privkey_result);
    Ok(())
}

// Tests for Hot Side Commands
// TODO: fix this test by adding a real key file
#[tokio::test]
#[ignore]
async fn test_import_keys() -> Result<(), NockAppError> {
    init_tracing();
    let cli = BootCli::parse_from(&["--new"]);
    let nockapp = boot::setup(KERNEL, cli.clone(), &[], "wallet", None)
        .await
        .map_err(|e| CrownError::Unknown(e.to_string()))?;
    let mut wallet = Wallet::new(nockapp);

    // Create test key file
    let test_path = "test_keys.jam";
    let test_data = vec![0u8; 32]; // TODO: Use real jammed key data
    fs::write(test_path, &test_data).expect(&format!(
        "Called `expect()` at {}:{} (git sha: {})",
        file!(),
        line!(),
        option_env!("GIT_SHA").unwrap_or("unknown")
    ));

    let (noun, op) = Wallet::import_keys(test_path)?;
    let wire = WalletWire::Command(Commands::ImportKeys {
        file: Some(test_path.to_string()),
        key: None,
        seedphrase: None,
        version: None,
    })
    .to_wire();
    let import_result = wallet.app.poke(wire, noun.clone()).await?;

    fs::remove_file(test_path).expect(&format!(
        "Called `expect()` at {}:{} (git sha: {})",
        file!(),
        line!(),
        option_env!("GIT_SHA").unwrap_or("unknown")
    ));

    println!("import result: {:?}", import_result);
    assert!(
        !import_result.is_empty(),
        "Expected non-empty import result"
    );

    Ok(())
}

// TODO: fix this test
#[tokio::test]
#[ignore]
async fn test_spend_multisig_format() -> Result<(), NockAppError> {
    // TODO: replace with an end-to-end test that exercises multisig recipient specs.
    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_spend_single_sig_format() -> Result<(), NockAppError> {
    // TODO: replace with an end-to-end test for PKH recipients once fixtures exist.
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_list_notes() -> Result<(), NockAppError> {
    init_tracing();
    let cli = BootCli::parse_from(&[""]);
    let nockapp = boot::setup(KERNEL, cli.clone(), &[], "wallet", None)
        .await
        .map_err(|e| CrownError::Unknown(e.to_string()))?;
    let mut wallet = Wallet::new(nockapp);

    // Test listing notes
    let (noun, op) = Wallet::list_notes()?;
    let wire = WalletWire::Command(Commands::ListNotes {}).to_wire();
    let list_result = wallet.app.poke(wire, noun.clone()).await?;
    println!("list_result: {:?}", list_result);

    Ok(())
}

// TODO: fix this test by adding a real draft
#[tokio::test]
#[ignore]
async fn test_make_tx_from_draft() -> Result<(), NockAppError> {
    init_tracing();
    let cli = BootCli::parse_from(&[""]);
    let nockapp = boot::setup(KERNEL, cli.clone(), &[], "wallet", None)
        .await
        .map_err(|e| CrownError::Unknown(e.to_string()))?;
    let mut wallet = Wallet::new(nockapp);

    // use the transaction in txs/
    let transaction_path = "txs/test_transaction.tx";
    let test_data = vec![0u8; 32]; // TODO: Use real transaction data
    fs::write(transaction_path, &test_data).expect(&format!(
        "Called `expect()` at {}:{} (git sha: {})",
        file!(),
        line!(),
        option_env!("GIT_SHA").unwrap_or("unknown")
    ));

    let (noun, op) = Wallet::send_tx(transaction_path)?;
    let wire = WalletWire::Command(Commands::SendTx {
        transaction: transaction_path.to_string(),
    })
    .to_wire();
    let tx_result = wallet.app.poke(wire, noun.clone()).await?;

    fs::remove_file(transaction_path).expect(&format!(
        "Called `expect()` at {}:{} (git sha: {})",
        file!(),
        line!(),
        option_env!("GIT_SHA").unwrap_or("unknown")
    ));

    println!("transaction result: {:?}", tx_result);
    assert!(
        !tx_result.is_empty(),
        "Expected non-empty transaction result"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_show_tx() -> Result<(), NockAppError> {
    init_tracing();
    let cli = BootCli::parse_from(&[""]);
    let nockapp = boot::setup(KERNEL, cli.clone(), &[], "wallet", None)
        .await
        .map_err(|e| CrownError::Unknown(e.to_string()))?;
    let mut wallet = Wallet::new(nockapp);

    // Create a temporary transaction file
    let transaction_path = "test_show_transaction.tx";
    let test_data = vec![0u8; 32]; // TODO: Use real transaction data
    fs::write(transaction_path, &test_data).expect(&format!(
        "Called `expect()` at {}:{} (git sha: {})",
        file!(),
        line!(),
        option_env!("GIT_SHA").unwrap_or("unknown")
    ));

    let (noun, op) = Wallet::show_tx(transaction_path)?;
    let wire = WalletWire::Command(Commands::ShowTx {
        transaction: transaction_path.to_string(),
    })
    .to_wire();
    let show_result = wallet.app.poke(wire, noun.clone()).await?;

    fs::remove_file(transaction_path).expect(&format!(
        "Called `expect()` at {}:{} (git sha: {})",
        file!(),
        line!(),
        option_env!("GIT_SHA").unwrap_or("unknown")
    ));

    println!("show-tx result: {:?}", show_result);
    assert!(!show_result.is_empty(), "Expected non-empty show-tx result");

    Ok(())
}

#[test]
fn domain_hash_from_base58_accepts_valid_id() {
    let tx_id = "3giXkwW4zbFhoyJu27RbP6VNiYgR6yaTfk2AYnEHvxtVaGbmcVD6jb9";
    Hash::from_base58(tx_id).expect("expected valid base58 hash");
}

#[test]
fn domain_hash_from_base58_rejects_invalid_id() {
    let invalid_tx_id = "not-a-valid-hash";
    assert!(Hash::from_base58(invalid_tx_id).is_err());
}
