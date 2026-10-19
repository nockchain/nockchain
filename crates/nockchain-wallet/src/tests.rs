#![allow(warnings)]
// TODO: all these tests need to also validate the results and not
// just ensure that the wallet can be poked with the expected noun.

use std::collections::BTreeMap;
use std::sync::Once;

use nockapp::kernel::boot::{self, ephemeral_test_boot_cli, Cli as BootCli};
use nockapp::wire::SystemWire;
use nockapp::{exit_driver, AtomExt, Bytes};
use nockchain_math::belt::Belt;
use nockchain_math::zoon::zmap::ZMap;
use nockchain_types::default_fakenet_blockchain_constants;
use nockchain_types::tx_engine::common::{BlockHeight, BlockHeightDelta, Nicks, Signature};
use nockchain_types::tx_engine::v1::note::{NoteData, NoteDataEntry};
use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh, SpendCondition};
use nockchain_types::tx_engine::{v0, v1};
use nockvm::noun::{NounAllocator, T};
use noun_serde::{NounDecode, NounEncode};
use tempfile::TempDir;
use tokio::sync::mpsc;
use wallet_tx_builder::adapter::normalize_balance_pages;
use wallet_tx_builder::fee::{compute_minimum_fee, FeeInputs};
use wallet_tx_builder::planner::plan_create_tx;
use wallet_tx_builder::types::{
    CandidateVersionPolicy, ChainContext, CreateTxPlanningMode, PlanRequest, RawNoteDataEntry,
    SelectionMode, SelectionOrder,
};

use super::*;
use crate::command::WalletWire;
use crate::create_tx::{
    decode_fund_spend_policy, ensure_manual_planner_parity, fund_candidate_height,
    ActiveSignerEntryNoun, FundSpendPolicyNoun, MigrateV0NotesSummary, MigrateV0SignerSummary,
    PlannerBlockchainConstantsNoun, PlannerNoteDataConstantsNoun, SigningKeyLockMatcher,
};
use crate::recipient::{planner_recipient_outputs, RecipientSpec, BRIDGE_LOCK_ROOT_DEFAULT_B58};

static INIT: Once = Once::new();

fn init_tracing() {
    INIT.call_once(|| {
        let cli = ephemeral_test_boot_cli(true);
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
            .map(|entry| {
                NoteDataEntry::from_raw_blob(entry.key, entry.blob)
                    .expect("fixture raw note-data should decode")
            })
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

fn merged_seed_word_count(spends: &v1::Spends) -> u64 {
    let mut merged_by_lock_root = BTreeMap::<
        [u64; 5],
        BTreeMap<String, nockchain_types::tx_engine::v1::note::NoteDataValue>,
    >::new();

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
                merged.insert(note_data_entry.key.clone(), note_data_entry.value.clone());
            }
        }
    }

    merged_by_lock_root
        .into_values()
        .map(|merged| {
            let entries = merged
                .into_iter()
                .map(|(key, value)| NoteDataEntry::new(key, value))
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
            let space = effect.noun_space();
            let noun = unsafe { effect.root() };
            let cell = noun.in_space(&space).as_cell().ok()?;
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
    let space = slab.noun_space();
    let transaction_cell = transaction_noun.in_space(&space).as_cell().map_err(|err| {
        NockAppError::OtherError(format!("transaction jam root not a cell: {err}"))
    })?;
    let version =
        <u64 as NounDecode>::from_noun(&transaction_cell.head().noun(), &space).map_err(|err| {
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
    let mut spends =
        v1::Spends::from_noun(&spends_and_rest.head().noun(), &space).map_err(|err| {
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
    let witness_tag =
        <u64 as NounDecode>::from_noun(&witness_cell.head().noun(), &space).map_err(|err| {
            NockAppError::OtherError(format!("witness-data tag did not decode: {err}"))
        })?;
    match witness_tag {
        0 => {
            let signatures =
                ZMap::<Name, Signature>::from_noun(&witness_cell.tail().noun(), &space).map_err(
                    |err| {
                        NockAppError::OtherError(format!(
                            "legacy witness-data signature map did not decode: {err}"
                        ))
                    },
                )?;
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
            let witnesses = ZMap::<Name, v1::Witness>::from_noun(
                &witness_cell.tail().noun(),
                &space,
            )
            .map_err(|err| {
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
    let space = effect.noun_space();
    let noun = unsafe { effect.root() };
    let cell = noun.in_space(&space).as_cell().ok()?;
    cell.head().as_atom().ok()?.into_string().ok()
}

fn effect_exit_code(effects: &[NounSlab]) -> Option<u64> {
    effects.iter().find_map(|effect| {
        if effect_tag(effect).as_deref() != Some("exit") {
            return None;
        }
        let space = effect.noun_space();
        let noun = unsafe { effect.root() };
        let cell = noun.in_space(&space).as_cell().ok()?;
        <u64 as NounDecode>::from_noun(&cell.tail().noun(), &space).ok()
    })
}

fn format_note_names(names: &[Name]) -> String {
    names
        .iter()
        .map(|name| format!("[{} {}]", name.first.to_base58(), name.last.to_base58()))
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_option_handle<'a>(
    noun: nockvm::noun::NounHandle<'a>,
) -> Result<Option<nockvm::noun::NounHandle<'a>>, NockAppError> {
    if let Ok(atom) = noun.as_atom() {
        let tag = atom
            .as_u64()
            .map_err(|err| NockAppError::OtherError(format!("option tag did not decode: {err}")))?;
        return match tag {
            1 => Ok(None),
            other => Err(NockAppError::OtherError(format!(
                "unexpected option atom tag {other}"
            ))),
        };
    }

    let cell = noun
        .as_cell()
        .map_err(|err| NockAppError::OtherError(format!("option payload not a cell: {err}")))?;
    let tag = cell
        .head()
        .as_atom()
        .map_err(|err| NockAppError::OtherError(format!("option head not an atom: {err}")))?
        .as_u64()
        .map_err(|err| NockAppError::OtherError(format!("option head did not decode: {err}")))?;
    if tag != 0 {
        return Err(NockAppError::OtherError(format!(
            "unexpected option cell tag {tag}"
        )));
    }
    Ok(Some(cell.tail()))
}

async fn peek_master_signing_key(wallet: &mut Wallet) -> Result<Hash, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "master-signing-key").as_noun();
    let path = T(&mut slab, &[tag, SIG]);
    slab.set_root(path);

    let result = wallet.app.peek(slab).await?;
    let space = result.noun_space();
    let decoded: Option<Option<Hash>> = unsafe { Option::from_noun(result.root(), &space)? };
    if let Some(master_signing_key) = decoded.flatten() {
        return Ok(master_signing_key);
    }

    let signing_keys = peek_signing_keys(wallet).await?;
    if let Some(signing_key) = signing_keys.first() {
        return Ok(signing_key.clone());
    }

    let active_signers = peek_active_signers(wallet).await?;
    if let Some(master_signer) = active_signers
        .iter()
        .find(|signer| signer.child_index.is_none())
    {
        return Hash::from_base58(&master_signer.address_b58).map_err(|err| {
            NockAppError::OtherError(format!(
                "wallet active master signer address did not decode as a pubkey hash: {err}"
            ))
        });
    }
    active_signers
        .first()
        .map(|signer| {
            Hash::from_base58(&signer.address_b58).map_err(|err| {
                NockAppError::OtherError(format!(
                    "wallet active signer address did not decode as a pubkey hash: {err}"
                ))
            })
        })
        .transpose()?
        .ok_or_else(|| {
            NockAppError::OtherError("wallet signer key peek returned no payload".to_string())
        })
}

async fn peek_signing_keys(wallet: &mut Wallet) -> Result<Vec<Hash>, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "signing-keys").as_noun();
    let path = T(&mut slab, &[tag, SIG]);
    slab.set_root(path);

    let result = wallet.app.peek(slab).await?;
    let space = result.noun_space();
    let decoded: Option<Option<Vec<Hash>>> = unsafe { Option::from_noun(result.root(), &space)? };
    Ok(decoded.flatten().unwrap_or_default())
}

async fn peek_master_signing_pubkey(wallet: &mut Wallet) -> Result<SchnorrPubkey, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "master-signing-pubkey").as_noun();
    let path = T(&mut slab, &[tag, SIG]);
    slab.set_root(path);

    let result = wallet.app.peek(slab).await?;
    let space = result.noun_space();
    let decoded: Option<Option<SchnorrPubkey>> =
        unsafe { Option::from_noun(result.root(), &space)? };
    if let Some(master_signing_pubkey) = decoded.flatten() {
        return Ok(master_signing_pubkey);
    }

    let active_signers = peek_active_signers(wallet).await?;
    if let Some(master_signer) = active_signers
        .iter()
        .find(|signer| signer.child_index.is_none())
    {
        return Ok(master_signer.pubkey.clone());
    }
    active_signers
        .first()
        .map(|signer| signer.pubkey.clone())
        .ok_or_else(|| {
            NockAppError::OtherError("wallet signer pubkey peek returned no payload".to_string())
        })
}

async fn peek_active_signers(
    wallet: &mut Wallet,
) -> Result<Vec<ActiveSignerEntryNoun>, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "active-signers").as_noun();
    let path = T(&mut slab, &[tag, SIG]);
    slab.set_root(path);

    let result = wallet.app.peek(slab).await?;
    let space = result.noun_space();
    let decoded: Option<Option<Vec<ActiveSignerEntryNoun>>> =
        unsafe { Option::from_noun(result.root(), &space)? };
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
        key_file: None,
        seedphrase: true,
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

async fn derive_child_address(
    wallet: &mut Wallet,
    index: u64,
    hardened: bool,
) -> Result<String, NockAppError> {
    let label = None;
    let (noun, _) = Wallet::derive_child(index, hardened, &label)?;
    let wire = WalletWire::Command(Commands::DeriveChild {
        index,
        hardened,
        label,
    })
    .to_wire();
    let effects = wallet.app.poke(wire, noun).await?;
    Wallet::derived_address_from_effects(&effects)
}

async fn boot_wallet_with(
    cli: BootCli,
    hot_state: &[nockvm::jets::hot::HotEntry],
) -> Result<(Wallet, TempDir), NockAppError> {
    let data_dir = tempfile::tempdir().map_err(NockAppError::IoError)?;
    let nockapp = boot::setup(
        KERNEL,
        cli.clone(),
        hot_state,
        "wallet",
        Some(data_dir.path().to_path_buf()),
    )
    .await
    .map_err(|e| CrownError::Unknown(e.to_string()))?;
    Ok((Wallet::new(nockapp), data_dir))
}

async fn boot_test_wallet() -> Result<(Wallet, TempDir), NockAppError> {
    let cli = ephemeral_test_boot_cli(true);
    let prover_hot_state = produce_prover_hot_state();
    boot_wallet_with(cli, prover_hot_state.as_slice()).await
}

async fn peek_wallet_blockchain_constants(
    wallet: &mut Wallet,
) -> Result<PlannerBlockchainConstantsNoun, NockAppError> {
    let mut slab = NounSlab::new();
    let state_tag = make_tas(&mut slab, "state").as_noun();
    slab.modify(|_| vec![state_tag, SIG]);

    let result = wallet.app.peek(slab).await?;
    let space = result.noun_space();
    let root = unsafe { *result.root() }.in_space(&space);
    let state = decode_option_handle(root)?
        .and_then(|inner| decode_option_handle(inner).transpose())
        .transpose()?
        .ok_or_else(|| NockAppError::OtherError("missing wallet state payload".to_string()))?;
    let constants = state.slot(31).map_err(|err| {
        NockAppError::OtherError(format!("wallet state missing blockchain constants: {err}"))
    })?;
    PlannerBlockchainConstantsNoun::from_noun(&constants.noun(), &space).map_err(|err| {
        NockAppError::OtherError(format!("decode blockchain constants failed: {err}"))
    })
}

async fn peek_balance_state(wallet: &mut Wallet) -> Result<v1::BalanceUpdate, NockAppError> {
    let mut slab = NounSlab::new();
    let balance_tag = make_tas(&mut slab, "balance").as_noun();
    let path = T(&mut slab, &[balance_tag, SIG]);
    slab.set_root(path);

    let result = wallet.app.peek(slab).await?;
    let space = result.noun_space();
    let maybe_balance: Option<Option<v1::BalanceUpdate>> =
        unsafe { <Option<Option<v1::BalanceUpdate>>>::from_noun(result.root(), &space)? };
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
fn signing_key_lock_matcher_accepts_single_sig_multisig_when_signer_is_present() {
    let signer = hash(5);
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[signer_key(5)]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(
        nockchain_types::tx_engine::v1::tx::Pkh::new(1, vec![hash(9), signer]),
    )]);
    let first_name = spend_condition
        .first_name()
        .expect("multisig first-name should compute")
        .into_hash();

    assert!(matcher.matches(&first_name, &spend_condition));
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
fn signing_key_lock_matcher_resolves_simple_lock_for_non_master_signer_reconstruction() {
    let master = signer_key(1);
    let child = signer_key(8);
    let matcher = SigningKeyLockMatcher::from_signer_keys(&[master.clone(), child.clone()]);
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(
        nockchain_types::tx_engine::v1::tx::Pkh::new(1, vec![child.clone()]),
    )]);
    let first_name = spend_condition
        .first_name()
        .expect("simple first-name should compute")
        .into_hash();
    let decoded = wallet_tx_builder::note_data::DecodedNoteData(Vec::new());

    let resolution = matcher.resolve_lock(ResolveLockRequest {
        note_first_name: &first_name,
        decoded_note_data: &decoded,
        signer_pkh: Some(&master),
        coinbase_relative_min: Some(1),
    });

    assert_eq!(
        resolution.source,
        LockResolutionSource::ReconstructedSimplePkh
    );
    assert_eq!(resolution.spend_condition, Some(spend_condition));
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
            root: Hash::from_base58(BRIDGE_LOCK_ROOT_DEFAULT_B58).expect("default bridge root"),
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
            root: Hash::from_base58(BRIDGE_LOCK_ROOT_DEFAULT_B58).expect("default bridge root"),
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
fn fund_policy_rejects_absent_malformed_and_mismatched_responses() {
    for absent in [None, Some(None::<FundSpendPolicyNoun>)] {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = absent.to_noun(&mut slab);
        assert!(decode_fund_spend_policy(&noun, &slab.noun_space(), 12).is_err());
    }

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let malformed = Some(Some(42u64)).to_noun(&mut slab);
    assert!(decode_fund_spend_policy(&malformed, &slab.noun_space(), 12).is_err());
    let invalid_minimum = Some(Some((12u64, hash(1), hash(2), 7u64))).to_noun(&mut slab);
    assert!(decode_fund_spend_policy(&invalid_minimum, &slab.noun_space(), 12).is_err());
    let noun = Some(Some(FundSpendPolicyNoun {
        candidate_height: 11,
        fund_lock_root: hash(1),
        fund_first_name: hash(2),
        minimum: None,
    }))
    .to_noun(&mut slab);
    assert!(decode_fund_spend_policy(&noun, &slab.noun_space(), 12).is_err());
    assert!(fund_candidate_height(u64::MAX).is_err());
}

/// Public protocol-fund reveal only; no signing key or wallet mutation is used.
fn public_fund_spend_condition() -> SpendCondition {
    SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        3,
        [
            "7pGXggKU1AWk3d3wqX2kpKUatTqT68Cv8SQfGzGRQvJvYnQBvagSSjT",
            "8Mc1U7kdujhPoEwog1BfNsFDtRp8St8UQCHk84iaLdhP4cX9a2CT1MU",
            "DAvp9ffoyNTBqAudZN29qc6s8GZfvvvAGvAfEFrqQsCVgSSSkg1SaSm",
            "9LK7wEcQsmRpEot4qFaV9bwjSE9ZD6tB1kWbgNsFkDa2LEpvBV9WGY3",
        ]
        .into_iter()
        .map(|pkh| Hash::from_base58(pkh).expect("public fund participant")),
    ))])
}

#[tokio::test]
async fn fund_policy_runtime_selects_mature_notes_without_changing_reveal(
) -> Result<(), NockAppError> {
    use wallet_tx_builder::lock_resolver::{FundMaturity, LockRootLockMatcher};
    use wallet_tx_builder::types::PlannedOutput;

    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    // Far beyond activation without mirroring a consensus phase in Rust.
    // The Hoon wallet tests derive and exercise the exact shared phase boundary.
    let tip = u64::MAX / 2;
    let candidate_height = fund_candidate_height(tip)?;
    let policy = wallet.peek_fund_spend_policy(candidate_height).await?;
    let minimum = policy.minimum.expect("active fund recovery policy");
    assert!(minimum > 0, "default runtime has a positive minimum");
    let condition = public_fund_spend_condition();
    assert_eq!(condition.hash().unwrap(), policy.fund_lock_root);

    let note = |last, origin| {
        let name = Name::new(policy.fund_first_name.clone(), hash(last));
        let note = v1::Note::V1(v1::NoteV1::new(
            BlockHeight(Belt(origin)),
            name.clone(),
            NoteData::new(Vec::new()),
            Nicks(10_000_000),
        ));
        (name, note)
    };
    let immature = note(1, candidate_height - minimum + 1);
    let mature = note(2, candidate_height - minimum);
    let snapshot =
        normalize_balance_pages(&[balance_page(tip, 1, vec![immature.clone(), mature.clone()])])
            .expect("consistent synthetic snapshot");
    let policy = wallet
        .peek_fund_spend_policy(fund_candidate_height((snapshot.metadata.height.0).0)?)
        .await?;
    // The wallet kernel remains at its empty stored balance: the requested
    // candidate comes from the supplied normalized snapshot instead.
    assert_eq!((peek_balance_state(&mut wallet).await?.height.0).0, 0);
    let matcher = LockRootLockMatcher::from_lock_root(&policy.fund_lock_root)
        .unwrap()
        .with_spend_condition(condition.clone())
        .with_fund_notes(
            &policy.fund_lock_root,
            &policy.fund_first_name,
            FundMaturity {
                candidate_height: policy.candidate_height,
                minimum: policy.minimum,
            },
        );
    let constants = peek_wallet_blockchain_constants(&mut wallet).await?;
    let mut request = PlanRequest {
        planning_mode: CreateTxPlanningMode::Standard,
        selection_mode: SelectionMode::Auto,
        order_direction: SelectionOrder::Ascending,
        include_data: false,
        chain_context: ChainContext {
            height: snapshot.metadata.height,
            bythos_phase: BlockHeight(Belt(constants.bythos_phase)),
            base_fee: constants.base_fee,
            input_fee_divisor: constants.input_fee_divisor,
            min_fee: constants.data.min_fee,
        },
        signer_pkh: None,
        candidate_version_policy: CandidateVersionPolicy::V1Only,
        candidates: snapshot.candidates,
        recipient_outputs: vec![PlannedOutput {
            lock_root: hash(99),
            amount: 100_000,
            note_data: Vec::new(),
        }],
        refund_output: planner_refund_output_template(None, &hash(99), false)?,
        coinbase_relative_min: Some(constants.coinbase_timelock_min()?),
        v0_migration_signer_pubkeys: Vec::new(),
    };
    let plan = plan_create_tx(&request, &matcher).expect("mature note funds the plan");
    assert_eq!(plan.selected.len(), 1);
    assert_eq!(plan.selected[0].name, mature.0);
    let planned_names = plan
        .selected
        .iter()
        .map(|input| input.name.clone())
        .collect::<Vec<_>>();
    assert!(
        ensure_manual_planner_parity(&[immature.0.clone(), mature.0.clone()], &planned_names,)
            .is_err()
    );
    request.selection_mode = SelectionMode::Manual {
        note_names: vec![immature.0.clone()],
    };
    assert!(plan_create_tx(&request, &matcher).is_err());

    request.selection_mode = SelectionMode::Auto;
    request
        .candidates
        .retain(|candidate| candidate.identity().name == mature.0);
    // An inactive policy is a present payload, not an absent scry.
    let inactive = wallet.peek_fund_spend_policy(0).await?;
    assert!(inactive.minimum.is_none());
    let inactive_matcher = LockRootLockMatcher::from_lock_root(&inactive.fund_lock_root)
        .unwrap()
        .with_spend_condition(condition.clone())
        .with_fund_notes(
            &inactive.fund_lock_root,
            &inactive.fund_first_name,
            FundMaturity {
                candidate_height: inactive.candidate_height,
                minimum: inactive.minimum,
            },
        );
    let inactive_plan = plan_create_tx(&request, &inactive_matcher).expect("inactive mature plan");
    assert_eq!(plan.selected, inactive_plan.selected);
    assert_eq!(plan.final_fee, inactive_plan.final_fee);
    assert_eq!(plan.word_counts, inactive_plan.word_counts);
    let candidate = &request.candidates[0];
    let resolution = matcher.select_v1_candidate(ResolveLockRequest {
        note_first_name: &candidate.identity().name.first,
        decoded_note_data: candidate.decoded_note_data(),
        signer_pkh: None,
        coinbase_relative_min: request.coinbase_relative_min,
    });
    assert_eq!(resolution.spend_condition.unwrap(), condition);
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let zero = Some(Some(FundSpendPolicyNoun {
        minimum: Some(0),
        ..policy
    }))
    .to_noun(&mut slab);
    let zero = decode_fund_spend_policy(&zero, &slab.noun_space(), candidate_height)?;
    let zero_matcher = LockRootLockMatcher::from_lock_root(&zero.fund_lock_root)
        .unwrap()
        .with_spend_condition(condition)
        .with_fund_notes(
            &zero.fund_lock_root,
            &zero.fund_first_name,
            FundMaturity {
                candidate_height: zero.candidate_height,
                minimum: zero.minimum,
            },
        );
    request.candidates[0].identity_mut().origin_page = BlockHeight(Belt(candidate_height + 1));
    assert!(plan_create_tx(&request, &zero_matcher).is_err());
    assert!(plan_create_tx(&request, &inactive_matcher).is_ok());
    // A backwards query must return an inactive policy after an active query.
    assert!(wallet.peek_fund_spend_policy(0).await?.minimum.is_none());
    Ok(())
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
        coinbase_timelock_min: 99,
    };
    let wrapped = Some(Some(constants));

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = wrapped.to_noun(&mut slab);
    let space = slab.noun_space();
    let decoded: Option<Option<PlannerBlockchainConstantsNoun>> =
        Option::from_noun(&noun, &space).expect("peek payload should decode");
    let parsed = decoded.flatten().expect("payload should be present");
    assert_eq!(parsed.bythos_phase, 54_000);
    assert_eq!(parsed.base_fee, 128);
    assert_eq!(parsed.input_fee_divisor, 4);
    assert_eq!(parsed.data.min_fee, 256);
    assert_eq!(parsed.coinbase_timelock_min().unwrap(), 99);
}

#[test]
fn planner_constants_extract_coinbase_timelock_min_from_payload() {
    let constants = default_fakenet_blockchain_constants();
    let wrapped = Some(Some(constants.clone()));

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = wrapped.to_noun(&mut slab);
    let space = slab.noun_space();
    let decoded: Option<Option<PlannerBlockchainConstantsNoun>> =
        Option::from_noun(&noun, &space).expect("peek payload should decode");
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
        to: Vec::new(),
        amounts: Vec::new(),
        amounts_nicks: Vec::new(),
        bridge_deposit: None,
        to_evm_address: None,
        fee: None,
        fee_nicks: Some(3_584),
        allow_low_fee: false,
        refund_pkh: None,
        index: None,
        hardened: false,
        include_data: true,
        sign_keys: Vec::new(),
        save_raw_tx: false,
        note_selection_strategy: NoteSelectionStrategyCli::Ascending,
        notes_csv: None,
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

    import_seed_phrase(&mut wallet, seedphrase, 0).await?;
    wallet.set_fakenet().await?;

    let destination = peek_master_signing_key(&mut wallet).await?;
    let child_address = derive_child_address(&mut wallet, 0, false).await?;
    let signer_pubkey = SchnorrPubkey::from_base58(&child_address)
        .expect("derived v0 child address should decode as a Schnorr pubkey");

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
            vec![(0, false)],
            true,
            false,
            NoteSelectionStrategyCli::Ascending,
            None,
            None,
            &mut None,
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
        planning_mode: CreateTxPlanningMode::V0MigrationSweep {
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

    let hoon_encoded_seed_words = merged_seed_word_count(&spends);
    let hoon_witness_words = witness_word_count(&spends);
    let hoon_fee = total_paid_fee(&spends);
    let hoon_gift = total_seed_gift(&spends);
    let computed_fee = compute_minimum_fee(FeeInputs {
        seed_words: plan.word_counts.seed_words,
        witness_words: hoon_witness_words,
        base_fee: request.chain_context.base_fee,
        input_fee_divisor: request.chain_context.input_fee_divisor,
        min_fee: request.chain_context.min_fee,
        height: request.chain_context.height,
        bythos_phase: request.chain_context.bythos_phase,
    });

    // A one-input v0 migration on fakenet should emit one merged destination seed
    // note-data payload and one legacy signature map witness. Fee accounting charges the
    // tx-engine z-map `rep` shape, which is one leaf smaller than the saved note-data noun.
    assert_eq!(hoon_encoded_seed_words, 14);
    assert_eq!(plan.word_counts.seed_words, 13);
    assert_eq!(hoon_witness_words, 31);
    assert_eq!(computed_fee.minimum_fee, 2_656);
    assert_eq!(hoon_fee, computed_fee.minimum_fee);
    assert_eq!(hoon_gift, 22_344);

    assert_eq!(plan.word_counts.witness_words, hoon_witness_words);
    assert_eq!(plan.final_fee, computed_fee.minimum_fee);
    assert_eq!(plan.outputs.len(), 1);
    assert_eq!(plan.outputs[0].amount, hoon_gift);
    assert_eq!(plan.selected_total, hoon_fee + hoon_gift);

    Ok(())
}

#[tokio::test]
async fn create_tx_planner_accepts_child_sign_key_for_lock_reconstruction(
) -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";

    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    wallet.set_fakenet().await?;

    let master_signer = peek_master_signing_key(&mut wallet).await?;
    let child_address = derive_child_address(&mut wallet, 0, false).await?;
    let child_pkh = Hash::from_base58(&child_address)
        .expect("derived child address should decode as pubkey hash");
    let note_name = Name::new(
        SpendCondition::simple_pkh(child_pkh.clone())
            .first_name()
            .expect("simple child lock should compute first-name")
            .into_hash(),
        hash(4_242),
    );
    let child_note = v1::Note::V1(v1::NoteV1::new(
        BlockHeight(Belt(1)),
        note_name.clone(),
        NoteData::new(Vec::new()),
        Nicks(10_000),
    ));
    apply_balance_update(
        &mut wallet,
        balance_page(7, 888, vec![(note_name.clone(), child_note)]),
    )
    .await?;

    let recipient = RecipientSpec::P2pkh {
        address: master_signer,
        amount: 4_000,
    };

    let _ = wallet
        .create_tx_with_planner(
            None,
            Some(format_note_names(std::slice::from_ref(&note_name))),
            None,
            vec![recipient],
            false,
            None,
            vec![(0, false)],
            true,
            false,
            NoteSelectionStrategyCli::Ascending,
            None,
            None,
            &mut None,
        )
        .await?;

    Ok(())
}

#[tokio::test]
async fn keygen_create_tx_uses_tracked_signing_keys() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let entropy = [7u8; 32];
    let salt = [9u8; 16];

    let (noun, _) = Wallet::keygen(&entropy, &salt)?;
    let wire = WalletWire::Command(Commands::Keygen).to_wire();
    let _ = wallet.app.poke(wire, noun).await?;
    wallet.set_fakenet().await?;

    let signing_keys = peek_signing_keys(&mut wallet).await?;
    let signer_pkh = signing_keys
        .first()
        .cloned()
        .expect("keygen should expose a tracked signing key");
    let note_name = Name::new(
        SpendCondition::simple_pkh(signer_pkh.clone())
            .first_name()
            .expect("simple signer lock should compute first-name")
            .into_hash(),
        hash(7_777),
    );
    let note = v1::Note::V1(v1::NoteV1::new(
        BlockHeight(Belt(1)),
        note_name.clone(),
        NoteData::new(Vec::new()),
        Nicks(10_000),
    ));
    apply_balance_update(
        &mut wallet,
        balance_page(9, 999, vec![(note_name.clone(), note)]),
    )
    .await?;

    let _ = wallet
        .create_tx_with_planner(
            None,
            Some(format_note_names(std::slice::from_ref(&note_name))),
            None,
            vec![RecipientSpec::P2pkh {
                address: signer_pkh,
                amount: 4_000,
            }],
            false,
            None,
            Vec::new(),
            true,
            false,
            NoteSelectionStrategyCli::Ascending,
            None,
            None,
            &mut None,
        )
        .await?;

    Ok(())
}

#[test]
fn master_signing_key_decodes_from_hash_payload_shape() {
    let wrapped = Some(Some(hash(3)));

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = wrapped.to_noun(&mut slab);
    let space = slab.noun_space();
    let decoded: Option<Option<Hash>> =
        Option::from_noun(&noun, &space).expect("master signing key payload should decode");
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
    let cli = ephemeral_test_boot_cli(true);
    let prover_hot_state = produce_prover_hot_state();
    let (mut wallet, _data_dir) = boot_wallet_with(cli, prover_hot_state.as_slice()).await?;
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
    let exit_space = keygen_result[1].noun_space();
    let exit_cause = unsafe { *keygen_result[1].root() }.in_space(&exit_space);
    let code = exit_cause.as_cell()?.tail();
    assert_eq!(code.as_atom()?.as_u64()?, 0, "Expected exit code 0");

    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_derive_child() -> Result<(), NockAppError> {
    init_tracing();
    let cli = ephemeral_test_boot_cli(true);
    let prover_hot_state = produce_prover_hot_state();
    let (mut wallet, _data_dir) = boot_wallet_with(cli, prover_hot_state.as_slice()).await?;

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

    let exit_space = derive_result[1].noun_space();
    let exit_cause = unsafe { *derive_result[1].root() }.in_space(&exit_space);
    let code = exit_cause.as_cell()?.tail();
    assert_eq!(code.as_atom()?.as_u64()?, 0, "Expected exit code 0");

    Ok(())
}

fn key_output_field(effects: &[NounSlab], field: &str) -> String {
    effects
        .iter()
        .filter_map(|effect| Wallet::markdown_text_from_effect(effect).unwrap())
        .find_map(|text| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix(field).map(str::to_owned))
        })
        .expect("expected key field in kernel output")
}

fn kernel_exit_code(effects: &[NounSlab]) -> u64 {
    effects
        .iter()
        .find_map(|effect| {
            let space = effect.noun_space();
            let cell = unsafe { effect.root() }.in_space(&space).as_cell().ok()?;
            (cell.head().as_atom().ok()?.into_string().ok()? == "exit")
                .then(|| cell.tail().as_atom().unwrap().as_u64().unwrap())
        })
        .expect("kernel exit effect")
}

fn kernel_file_bytes(effects: &[NounSlab], expected_path: &str) -> Option<Vec<u8>> {
    effects.iter().find_map(|effect| {
        let space = effect.noun_space();
        let cell = unsafe { effect.root() }.in_space(&space).as_cell().ok()?;
        if cell.head().as_atom().ok()?.into_string().ok()? != "file" {
            return None;
        }
        let operation = cell.tail().as_cell().ok()?;
        if operation.head().as_atom().ok()?.into_string().ok()? != "write" {
            return None;
        }
        let write = operation.tail().as_cell().ok()?;
        if write.head().as_atom().ok()?.into_string().ok()? != expected_path {
            return None;
        }
        let atom = write.tail().as_atom().ok()?;
        let bytes = atom.as_ne_bytes();
        let length = bytes
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |i| i + 1);
        Some(bytes[..length].to_vec())
    })
}

async fn wallet_state_jam(wallet: &mut Wallet) -> Result<Vec<u8>, NockAppError> {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "state").as_noun();
    slab.modify(|_| vec![tag, SIG]);
    Ok(wallet.app.peek(slab).await?.jam().as_ref().to_vec())
}

async fn derived_key_output(
    wallet: &mut Wallet,
    index: u64,
    hardened: bool,
) -> Result<Vec<NounSlab>, NockAppError> {
    let (poke, _) = Wallet::derive_child(index, hardened, &None)?;
    wallet
        .app
        .poke(
            WalletWire::Command(Commands::DeriveChild {
                index,
                hardened,
                label: None,
            })
            .to_wire(),
            poke,
        )
        .await
}

async fn recover_child_output(
    wallet: &mut Wallet,
    index: u64,
    hardened: bool,
    retained: &str,
) -> Result<Vec<NounSlab>, NockAppError> {
    let (poke, _) = Wallet::recover_child(index, hardened, retained)?;
    wallet
        .app
        .poke(
            WalletWire::Command(Commands::RecoverChild {
                index,
                hardened,
                key_file: PathBuf::from("unused-in-kernel-test"),
            })
            .to_wire(),
            poke,
        )
        .await
}

// Keep the checksum valid when testing independent key-material controls.
fn extended_key_with_payload_change(key: &str, change: impl FnOnce(&mut [u8])) -> String {
    use sha2::{Digest, Sha256};

    let mut bytes = bs58::decode(key).into_vec().expect("extended key base58");
    let payload_length = bytes.len() - 4;
    change(&mut bytes[..payload_length]);
    let first = Sha256::digest(&bytes[..payload_length]);
    let first_nonzero = first
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(first.len());
    let second = Sha256::digest(&first[first_nonzero..]);
    bytes[payload_length..].copy_from_slice(&second[..4]);
    bs58::encode(bytes).into_string()
}

fn extended_key_with_changed_byte(key: &str, offset: usize) -> String {
    extended_key_with_payload_change(key, |payload| payload[offset] ^= 1)
}

fn significant_bytes(bytes: &[u8]) -> &[u8] {
    &bytes[bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len())..]
}

fn assert_child_exports_match_stored_material(
    exported: &[u8],
    absolute_index: u64,
    private: &[u8],
    public: &[u8],
) {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::copy_from_slice(exported))
        .expect("decode wallet keys export");
    let space = slab.noun_space();
    let mut entries = noun.in_space(&space);
    let mut matched = Vec::new();
    while let Ok(list) = entries.as_cell() {
        let entry = list.head().as_cell().expect("keys export entry");
        let mut path = entry.head();
        let mut child_index = None;
        while let Ok(segments) = path.as_cell() {
            child_index = segments.head().as_cell().ok().and_then(|segment| {
                match segment
                    .head()
                    .as_atom()
                    .unwrap()
                    .into_string()
                    .unwrap()
                    .as_str()
                {
                    "ud" => Some(segment.tail().as_atom().unwrap().as_u64().unwrap()),
                    _ => None,
                }
            });
            path = segments.tail();
        }
        let metadata = entry.tail().as_cell().expect("key metadata");
        if child_index == Some(absolute_index)
            && metadata.head().as_atom().unwrap().into_string().unwrap() == "coil"
        {
            let coil = metadata.tail().as_cell().expect("stored coil");
            let data = coil.tail().as_cell().expect("stored coil data");
            let key = data.head().as_cell().expect("stored key");
            let kind = key.head().as_atom().unwrap().into_string().unwrap();
            let (expected, key_offset) = match kind.as_str() {
                "prv" => (private, 47),
                "pub" => (public, 46),
                _ => panic!("unexpected stored key kind"),
            };
            assert_eq!(
                coil.head().as_atom().unwrap().as_u64().unwrap(),
                expected[4] as u64
            );
            let stored_key = key.tail().as_atom().unwrap().to_be_bytes();
            let stored_chain = data.tail().as_atom().unwrap().to_be_bytes();
            assert_eq!(
                significant_bytes(&stored_key),
                significant_bytes(&expected[key_offset..expected.len() - 4]),
            );
            assert_eq!(
                significant_bytes(&stored_chain),
                significant_bytes(&expected[14..46])
            );
            matched.push(kind);
        }
        entries = list.tail();
    }
    matched.sort();
    assert_eq!(matched, ["prv", "pub"]);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn child_exports_preserve_stored_protocol_and_signing_identity() -> Result<(), NockAppError> {
    init_tracing();
    let phrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";
    for parent_version in [0, 1] {
        let (mut source, _source_dir) = boot_test_wallet().await?;
        import_seed_phrase(&mut source, phrase, parent_version).await?;
        let (mut target, _target_dir) = boot_test_wallet().await?;
        import_seed_phrase(&mut target, phrase, parent_version).await?;
        let master = peek_master_signing_key(&mut target).await?;
        for (index, hardened) in [(3, false), (7, true)] {
            let original = derived_key_output(&mut source, index, hardened).await?;
            let private = key_output_field(&original, "- Extended Private Key: ");
            let public = key_output_field(&original, "- Extended Public Key: ");
            let private_bytes = bs58::decode(&private)
                .into_vec()
                .expect("private extended key");
            let public_bytes = bs58::decode(&public)
                .into_vec()
                .expect("public extended key");
            assert_eq!(private_bytes.len(), 83);
            assert_eq!(public_bytes.len(), 147);
            assert_eq!(private_bytes[46], 0);
            let stored = peek_active_signers(&mut source)
                .await?
                .into_iter()
                .find(|signer| signer.child_index == Some(index) && signer.hardened == hardened)
                .expect("stored child signer");
            assert_eq!(stored.version, if hardened { 0 } else { parent_version });
            let absolute_index = index + if hardened { 1 << 31 } else { 0 };
            for bytes in [&private_bytes, &public_bytes] {
                assert_eq!(bytes[4] as u64, stored.version);
                assert_eq!(bytes[5], 1);
                assert_eq!(
                    u32::from_be_bytes(bytes[10..14].try_into().unwrap()) as u64,
                    absolute_index
                );
            }
            let mut printed_versions = Vec::new();
            for effect in &original {
                if let Some(text) = Wallet::markdown_text_from_effect(effect)? {
                    for line in text.lines() {
                        if let Some(version) = line.trim().strip_prefix("- Version: ") {
                            printed_versions.push(version.parse::<u64>().expect("printed version"));
                        }
                    }
                }
            }
            assert_eq!(printed_versions, vec![stored.version; 2]);
            assert_eq!(
                Wallet::derived_address_from_effects(&original)?,
                stored.address_b58
            );
            let public_key = bs58::encode(&public_bytes[46..143]).into_string();
            assert_eq!(
                SchnorrPubkey::from_base58(&public_key).unwrap(),
                stored.pubkey
            );
            let (export, _) = Wallet::export_keys()?;
            let exported = source
                .app
                .poke(OnePunchWire::Poke.to_wire(), export)
                .await?;
            let exported = kernel_file_bytes(&exported, "keys.export").expect("keys export");
            assert_child_exports_match_stored_material(
                &exported, absolute_index, &private_bytes, &public_bytes,
            );

            let before = wallet_state_jam(&mut target).await?;
            for retained in [&private, &public] {
                let wrong_version = extended_key_with_changed_byte(retained, 4);
                let rejected =
                    recover_child_output(&mut target, index, hardened, &wrong_version).await?;
                assert_eq!(kernel_exit_code(&rejected), 1);
                assert_eq!(wallet_state_jam(&mut target).await?, before);
            }
            let recovered = recover_child_output(&mut target, index, hardened, &private).await?;
            assert_eq!(kernel_exit_code(&recovered), 0);
            assert_eq!(
                key_output_field(&recovered, "- Extended Private Key: "),
                private
            );
            assert_eq!(
                key_output_field(&recovered, "- Extended Public Key: "),
                public
            );
            assert_eq!(
                Wallet::derived_address_from_effects(&recovered)?,
                stored.address_b58
            );
            assert_eq!(peek_master_signing_key(&mut target).await?, master);
            let recovered_public =
                recover_child_output(&mut target, index, hardened, &public).await?;
            assert_eq!(kernel_exit_code(&recovered_public), 0);

            let message = b"stored child identity";
            let (sign, _) = Wallet::sign_message(message, Some(index), hardened)?;
            let signed = target.app.poke(OnePunchWire::Poke.to_wire(), sign).await?;
            let signature = kernel_file_bytes(&signed, "message.sig").expect("child signature");
            let (verify, _) = Wallet::verify_message(message, &signature, &public_key)?;
            let verified = source
                .app
                .poke(OnePunchWire::Poke.to_wire(), verify)
                .await?;
            assert_eq!(kernel_exit_code(&verified), 0);
        }
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn child_recovery_matches_material_before_writing() -> Result<(), NockAppError> {
    init_tracing();
    let phrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";
    let (mut source, _source_dir) = boot_test_wallet().await?;
    import_seed_phrase(&mut source, phrase, 1).await?;
    let (mut target, _target_dir) = boot_test_wallet().await?;
    import_seed_phrase(&mut target, phrase, 1).await?;
    derive_child_key(&mut target, 5, false).await?;
    let master = peek_master_signing_key(&mut target).await?;
    let untouched_address = peek_active_signers(&mut target)
        .await?
        .into_iter()
        .find(|signer| signer.child_index == Some(5))
        .expect("unrelated child")
        .address_b58;

    for (index, hardened) in [(3, false), (7, true)] {
        let original = derived_key_output(&mut source, index, hardened).await?;
        let private = key_output_field(&original, "- Extended Private Key: ");
        let public = key_output_field(&original, "- Extended Public Key: ");
        let address = Wallet::derived_address_from_effects(&original)?;
        let before = wallet_state_jam(&mut target).await?;
        // Byte 4 is the protocol version; byte 14 begins the chain code.
        for (retained, requested_index, requested_hardening) in [
            (private.clone(), index + 1, hardened),
            (private.clone(), index, !hardened),
            (extended_key_with_changed_byte(&private, 4), index, hardened),
            (
                extended_key_with_changed_byte(&private, 14),
                index,
                hardened,
            ),
            (extended_key_with_changed_byte(&public, 14), index, hardened),
        ] {
            let effects =
                recover_child_output(&mut target, requested_index, requested_hardening, &retained)
                    .await?;
            assert_eq!(kernel_exit_code(&effects), 1);
            assert_eq!(wallet_state_jam(&mut target).await?, before);
            assert!(effects.iter().all(|effect| {
                Wallet::markdown_text_from_effect(effect)
                    .unwrap()
                    .is_none_or(|text| !text.contains("Extended Private Key:"))
            }));
        }
        let recovered = recover_child_output(&mut target, index, hardened, &private).await?;
        assert_eq!(kernel_exit_code(&recovered), 0);
        assert_eq!(
            key_output_field(&recovered, "- Extended Private Key: "),
            private
        );
        assert_eq!(
            key_output_field(&recovered, "- Extended Public Key: "),
            public
        );
        assert_eq!(Wallet::derived_address_from_effects(&recovered)?, address);
        assert_eq!(peek_master_signing_key(&mut target).await?, master);
        let recovered_again = recover_child_output(&mut target, index, hardened, &public).await?;
        assert_eq!(kernel_exit_code(&recovered_again), 0);
        assert!(
            peek_active_signers(&mut target)
                .await?
                .iter()
                .any(|signer| signer.child_index == Some(5)
                    && signer.address_b58 == untouched_address)
        );

        let pubkey = peek_active_signers(&mut source)
            .await?
            .into_iter()
            .find(|signer| signer.child_index == Some(index) && signer.hardened == hardened)
            .expect("source child signer")
            .pubkey
            .to_base58()
            .expect("child public key base58");
        let message = b"child identity";
        let (sign_poke, _) = Wallet::sign_message(message, Some(index), hardened)?;
        let signed = target
            .app
            .poke(OnePunchWire::Poke.to_wire(), sign_poke)
            .await?;
        assert_eq!(kernel_exit_code(&signed), 0);
        let signature = kernel_file_bytes(&signed, "message.sig").expect("signature file effect");
        for (candidate_message, expected_exit) in
            [(message.as_slice(), 0), (b"other identity".as_slice(), 1)]
        {
            let (verify_poke, _) = Wallet::verify_message(candidate_message, &signature, &pubkey)?;
            let verified = source
                .app
                .poke(OnePunchWire::Poke.to_wire(), verify_poke)
                .await?;
            assert_eq!(kernel_exit_code(&verified), expected_exit);
        }
    }

    let (mut wrong_parent, _wrong_dir) = boot_test_wallet().await?;
    let (keygen, _) = Wallet::keygen(&[7u8; 32], &[9u8; 16])?;
    wrong_parent
        .app
        .poke(OnePunchWire::Poke.to_wire(), keygen)
        .await?;
    let original = derived_key_output(&mut source, 3, false).await?;
    let retained = key_output_field(&original, "- Extended Private Key: ");
    let before = wallet_state_jam(&mut wrong_parent).await?;
    let rejected = recover_child_output(&mut wrong_parent, 3, false, &retained).await?;
    assert_eq!(kernel_exit_code(&rejected), 1);
    assert_eq!(wallet_state_jam(&mut wrong_parent).await?, before);
    let (mut empty, _empty_dir) = boot_test_wallet().await?;
    let before = wallet_state_jam(&mut empty).await?;
    let rejected = recover_child_output(&mut empty, 3, false, &retained).await?;
    assert_eq!(kernel_exit_code(&rejected), 1);
    assert_eq!(wallet_state_jam(&mut empty).await?, before);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn public_parent_recovery_keeps_watch_only_authority() -> Result<(), NockAppError> {
    init_tracing();
    let (mut source, _source_dir) = boot_test_wallet().await?;
    let (keygen, _) = Wallet::keygen(&[7u8; 32], &[9u8; 16])?;
    source
        .app
        .poke(OnePunchWire::Poke.to_wire(), keygen)
        .await?;
    let parent_public = {
        let (poke, _) = Wallet::show_master_pubkey()?;
        let shown = source.app.poke(OnePunchWire::Poke.to_wire(), poke).await?;
        key_output_field(&shown, "- Extended Public Key: ")
            .split_whitespace()
            .next()
            .expect("root public key")
            .to_owned()
    };
    let (mut target, _target_dir) = boot_test_wallet().await?;
    let (import, _) = Wallet::import_extended(&parent_public)?;
    let imported = target
        .app
        .poke(OnePunchWire::Poke.to_wire(), import)
        .await?;
    assert_eq!(kernel_exit_code(&imported), 0);
    let original = derived_key_output(&mut source, 3, false).await?;
    let public = key_output_field(&original, "- Extended Public Key: ");
    let private = key_output_field(&original, "- Extended Private Key: ");
    let before = wallet_state_jam(&mut target).await?;
    for (index, hardened, retained) in [(3, false, &private), (3, true, &public)] {
        let rejected = recover_child_output(&mut target, index, hardened, retained).await?;
        assert_eq!(kernel_exit_code(&rejected), 1);
        assert_eq!(wallet_state_jam(&mut target).await?, before);
    }
    let recovered = recover_child_output(&mut target, 3, false, &public).await?;
    assert_eq!(kernel_exit_code(&recovered), 0);
    assert_eq!(
        key_output_field(&recovered, "- Extended Public Key: "),
        public
    );
    assert!(recovered.iter().all(|effect| {
        Wallet::markdown_text_from_effect(effect)
            .unwrap()
            .is_none_or(|text| !text.contains("Extended Private Key:"))
    }));
    let before = wallet_state_jam(&mut target).await?;
    let (sign, _) = Wallet::sign_message(b"watch only", Some(3), false)?;
    let result = target.app.poke(OnePunchWire::Poke.to_wire(), sign).await;
    if let Ok(effects) = result {
        assert!(kernel_file_bytes(&effects, "message.sig").is_none());
    }
    assert!(peek_active_signers(&mut target).await?.is_empty());
    assert_eq!(wallet_state_jam(&mut target).await?, before);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn standalone_key_import_preserves_signing_identity() -> Result<(), NockAppError> {
    init_tracing();
    let (mut source, _source_dir) = boot_test_wallet().await?;
    let (keygen, _) = Wallet::keygen(&[7u8; 32], &[9u8; 16])?;
    source
        .app
        .poke(OnePunchWire::Poke.to_wire(), keygen)
        .await?;
    let public_identity = peek_master_signing_pubkey(&mut source).await?;
    let public_b58 = public_identity.to_base58().expect("public key base58");
    let (show_private, _) = Wallet::show_master_privkey()?;
    let shown = source
        .app
        .poke(OnePunchWire::Poke.to_wire(), show_private)
        .await?;
    let private = key_output_field(&shown, "- Extended Private Key: ")
        .split_whitespace()
        .next()
        .expect("root private key")
        .to_owned();
    let (show_public, _) = Wallet::show_master_pubkey()?;
    let shown = source
        .app
        .poke(OnePunchWire::Poke.to_wire(), show_public)
        .await?;
    let public = key_output_field(&shown, "- Extended Public Key: ")
        .split_whitespace()
        .next()
        .expect("root public key")
        .to_owned();
    for (key, can_sign) in [(&private, true), (&public, false)] {
        let (mut isolated, isolated_dir) = boot_test_wallet().await?;
        let path = isolated_dir.path().join("standalone.key");
        fs::write(&path, format!("{key}\n")).map_err(NockAppError::IoError)?;
        let input = Wallet::read_extended_key_file(&path)?;
        let (import, _) = Wallet::import_extended_checked(input.trim())?;
        let imported = isolated
            .app
            .poke(OnePunchWire::Poke.to_wire(), import)
            .await?;
        assert_eq!(kernel_exit_code(&imported), 0);
        let (show, _) = Wallet::show_master_pubkey()?;
        let shown = isolated
            .app
            .poke(OnePunchWire::Poke.to_wire(), show)
            .await?;
        assert_eq!(
            key_output_field(&shown, "- Extended Public Key: ")
                .split_whitespace()
                .next()
                .expect("imported public key"),
            public,
        );
        let before = wallet_state_jam(&mut isolated).await?;
        let (sign, _) = Wallet::sign_message(b"standalone identity", None, false)?;
        let result = isolated.app.poke(OnePunchWire::Poke.to_wire(), sign).await;
        if can_sign {
            let signed = result?;
            assert_eq!(kernel_exit_code(&signed), 0);
            let signature = kernel_file_bytes(&signed, "message.sig").expect("signature output");
            let (verify, _) =
                Wallet::verify_message(b"standalone identity", &signature, &public_b58)?;
            let verified = source
                .app
                .poke(OnePunchWire::Poke.to_wire(), verify)
                .await?;
            assert_eq!(kernel_exit_code(&verified), 0);
        } else {
            if let Ok(effects) = result {
                assert!(kernel_file_bytes(&effects, "message.sig").is_none());
            }
            assert!(peek_active_signers(&mut isolated).await?.is_empty());
        }
        assert_eq!(wallet_state_jam(&mut isolated).await?, before);
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn key_material_rejection_preserves_state_and_terminates() -> Result<(), NockAppError> {
    init_tracing();
    let phrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";
    let (mut wallet, _wallet_dir) = boot_test_wallet().await?;
    import_seed_phrase(&mut wallet, phrase, 1).await?;
    let child = derived_key_output(&mut wallet, 3, false).await?;
    let private = key_output_field(&child, "- Extended Private Key: ");
    let public = key_output_field(&child, "- Extended Public Key: ");
    let mut bad_checksum = bs58::decode(&private).into_vec().expect("private base58");
    let checksum_end = bad_checksum.len() - 1;
    bad_checksum[checksum_end] ^= 1;
    let inputs = [
        String::new(),
        "not-a-valid-extended-key".to_owned(),
        "0OIl".to_owned(),
        bs58::encode(bad_checksum).into_string(),
        extended_key_with_payload_change(&private, |payload| payload[47..].fill(0)),
        extended_key_with_payload_change(&private, |payload| payload[47..].fill(0xff)),
        extended_key_with_payload_change(&public, |payload| payload[46..].fill(0)),
        format!("zprv{}", "1".repeat(256)),
    ];
    let before = wallet_state_jam(&mut wallet).await?;
    for input in &inputs {
        let recovered = recover_child_output(&mut wallet, 3, false, input).await?;
        assert_eq!(kernel_exit_code(&recovered), 1);
        assert_eq!(wallet_state_jam(&mut wallet).await?, before);
        let (import, _) = Wallet::import_extended_checked(input)?;
        let imported = wallet
            .app
            .poke(OnePunchWire::Poke.to_wire(), import)
            .await?;
        assert_eq!(kernel_exit_code(&imported), 1);
        assert_eq!(wallet_state_jam(&mut wallet).await?, before);
        for effects in [&recovered, &imported] {
            assert!(effects.iter().all(|effect| {
                let space = effect.noun_space();
                let cell = unsafe { effect.root() }.in_space(&space).as_cell().unwrap();
                cell.head().as_atom().unwrap().into_string().unwrap() != "file"
            }));
        }
    }
    // Exercise the same one-shot consumer and exit driver used by these CLI paths.
    for checked_import in [false, true] {
        let (mut consumer, _consumer_dir) = boot_test_wallet().await?;
        import_seed_phrase(&mut consumer, phrase, 1).await?;
        let (poke, _) = if checked_import {
            Wallet::import_extended_checked("not-a-valid-extended-key")?
        } else {
            Wallet::recover_child(3, false, "not-a-valid-extended-key")?
        };
        consumer.app.add_io_driver(key_material_driver(poke)).await;
        consumer.app.add_io_driver(exit_driver()).await;
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), consumer.app.run())
            .await
            .expect("rejected key operation must terminate");
        assert!(matches!(result, Err(NockAppError::Exit(1))));
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn full_key_export_roundtrip_preserves_namespaced_material() -> Result<(), NockAppError> {
    init_tracing();
    let (mut source, source_dir) = boot_test_wallet().await?;
    let phrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";
    import_seed_phrase(&mut source, phrase, 1).await?;
    let master = peek_master_signing_key(&mut source).await?;
    let label = Some("retained-child".to_owned());
    let (derive, _) = Wallet::derive_child(7, true, &label)?;
    source
        .app
        .poke(OnePunchWire::Poke.to_wire(), derive)
        .await?;
    derive_child_key(&mut source, 3, false).await?;
    let (watch, _) = Wallet::watch_address(&hash(42).to_base58())?;
    source.app.poke(OnePunchWire::Poke.to_wire(), watch).await?;
    let (other_root, _) = Wallet::keygen(&[7u8; 32], &[9u8; 16])?;
    source
        .app
        .poke(OnePunchWire::Poke.to_wire(), other_root)
        .await?;
    assert_eq!(peek_master_signing_key(&mut source).await?, master);
    let (export, _) = Wallet::export_keys()?;
    let exported = source
        .app
        .poke(OnePunchWire::Poke.to_wire(), export)
        .await?;
    let bytes = kernel_file_bytes(&exported, "keys.export").expect("keys export effect");
    let path = source_dir.path().join("roundtrip.keys");
    fs::write(&path, &bytes).map_err(NockAppError::IoError)?;
    let (mut restored, _restored_dir) = boot_test_wallet().await?;
    let (import, _) = Wallet::import_keys(path.to_str().expect("temporary export path"))?;
    let imported = restored
        .app
        .poke(OnePunchWire::Poke.to_wire(), import)
        .await?;
    assert_eq!(kernel_exit_code(&imported), 0);
    let (export, _) = Wallet::export_keys()?;
    let reexported = restored
        .app
        .poke(OnePunchWire::Poke.to_wire(), export)
        .await?;
    assert_eq!(
        kernel_file_bytes(&reexported, "keys.export").expect("restored export effect"),
        bytes,
        "all namespaces, private/public coils, labels, seeds, and watch entries must survive",
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn importing_exported_child_keeps_active_master() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";
    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    let original_master = peek_master_signing_key(&mut wallet).await?;

    let (derive_poke, _) = Wallet::derive_child(7, true, &None)?;
    let derive_wire = WalletWire::Command(Commands::DeriveChild {
        index: 7,
        hardened: true,
        label: None,
    })
    .to_wire();
    let derive_effects = wallet.app.poke(derive_wire, derive_poke).await?;
    let mut child_extended_key = None;
    for effect in &derive_effects {
        if let Some(markdown) = Wallet::markdown_text_from_effect(effect)? {
            for line in markdown.lines() {
                if let Some(key) = line.trim().strip_prefix("- Extended Private Key: ") {
                    child_extended_key = Some(key.trim().to_owned());
                }
            }
        }
    }
    let child_extended_key = child_extended_key.expect("derived child private key in effect");
    let (import_poke, _) = Wallet::import_extended(&child_extended_key)?;
    let import_wire = WalletWire::Command(Commands::ImportKeys {
        file: None,
        key: Some(child_extended_key),
        key_file: None,
        seedphrase: false,
        version: None,
    })
    .to_wire();
    let import_effects = wallet.app.poke(import_wire, import_poke).await?;
    let exit_space = import_effects
        .last()
        .expect("import exit effect")
        .noun_space();
    let exit = unsafe { *import_effects.last().unwrap().root() }.in_space(&exit_space);
    assert_eq!(exit.as_cell()?.tail().as_atom()?.as_u64()?, 0);
    assert_eq!(peek_master_signing_key(&mut wallet).await?, original_master);
    assert_eq!(peek_active_signers(&mut wallet).await?.len(), 2);

    let (mut other_wallet, _other_data_dir) = boot_test_wallet().await?;
    let (keygen_poke, _) = Wallet::keygen(&[7u8; 32], &[9u8; 16])?;
    other_wallet
        .app
        .poke(WalletWire::Command(Commands::Keygen).to_wire(), keygen_poke)
        .await?;
    assert_ne!(
        peek_master_signing_key(&mut other_wallet).await?,
        original_master
    );
    let (show_poke, _) = Wallet::show_master_privkey()?;
    let show_effects = other_wallet
        .app
        .poke(
            WalletWire::Command(Commands::ShowMasterZPrv).to_wire(),
            show_poke,
        )
        .await?;
    let mut other_root_key = None;
    for effect in &show_effects {
        if let Some(markdown) = Wallet::markdown_text_from_effect(effect)? {
            for line in markdown.lines() {
                if let Some(key) = line.trim().strip_prefix("- Extended Private Key: ") {
                    other_root_key = key.split_whitespace().next().map(str::to_owned);
                }
            }
        }
    }
    let other_root_key = other_root_key.expect("other wallet root private key in effect");
    let (root_import_poke, _) = Wallet::import_extended(&other_root_key)?;
    let root_import_effects = wallet
        .app
        .poke(
            WalletWire::Command(Commands::ImportKeys {
                file: None,
                key: Some(other_root_key),
                key_file: None,
                seedphrase: false,
                version: None,
            })
            .to_wire(),
            root_import_poke,
        )
        .await?;
    assert!(root_import_effects.iter().any(|effect| {
        Wallet::markdown_text_from_effect(effect)
            .ok()
            .flatten()
            .is_some_and(|text| text.contains("Active master key unchanged"))
    }));
    assert_eq!(peek_master_signing_key(&mut wallet).await?, original_master);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn planner_uses_only_the_keys_the_kernel_will_sign_with() -> Result<(), NockAppError> {
    init_tracing();
    let (mut wallet, _data_dir) = boot_test_wallet().await?;
    let seedphrase = "route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm";
    import_seed_phrase(&mut wallet, seedphrase, 1).await?;
    let master = peek_master_signing_key(&mut wallet).await?;
    let child_address = derive_child_address(&mut wallet, 3, false).await?;
    let child = Hash::from_base58(&child_address).expect("v1 child address");
    let second_child_address = derive_child_address(&mut wallet, 5, false).await?;
    let second_child = Hash::from_base58(&second_child_address).expect("v1 child address");

    assert_eq!(wallet.resolve_planner_signer_keys(&[]).await?, vec![master]);
    assert_eq!(
        wallet.resolve_planner_signer_keys(&[(3, false)]).await?,
        vec![child.clone()]
    );
    assert_eq!(
        wallet
            .resolve_planner_signer_keys(&[(5, false), (3, false), (5, false)])
            .await?,
        vec![second_child, child],
        "explicit sender order must match the wallet kernel"
    );
    Ok(())
}

// Tests for Cold Side Commands
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_gen_master_privkey() -> Result<(), NockAppError> {
    init_tracing();
    let cli = ephemeral_test_boot_cli(true);
    let (mut wallet, _data_dir) = boot_wallet_with(cli, &[]).await?;
    let seedphrase = "correct horse battery staple";
    let version = 1;
    let (noun, op) = Wallet::import_seed_phrase(seedphrase, version)?;
    println!("privkey_slab: {:?}", noun);
    let wire = WalletWire::Command(Commands::ImportKeys {
        file: None,
        key: None,
        key_file: None,
        seedphrase: true,
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
    let cli = ephemeral_test_boot_cli(true);
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
        key_file: None,
        seedphrase: false,
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
    let cli = ephemeral_test_boot_cli(true);
    let (mut wallet, _data_dir) = boot_wallet_with(cli, &[]).await?;

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
    let cli = ephemeral_test_boot_cli(true);
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
    let cli = ephemeral_test_boot_cli(true);
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

#[test]
fn normalize_seed_phrase_collapses_whitespace_into_zeroizing_buffer() {
    // The prompted phrase and its normalized copy must both live in
    // `Zeroizing` buffers. Wiping on drop is not observable from safe Rust, so
    // this pins the type and the normalization rule.
    let normalized: zeroize::Zeroizing<String> =
        normalize_seed_phrase("  abandon\tability\n\n able  about ");
    assert_eq!(normalized.as_str(), "abandon ability able about");
    assert_eq!(normalize_seed_phrase("").as_str(), "");
    assert_eq!(normalize_seed_phrase("single").as_str(), "single");
}
