use std::path::{Path, PathBuf};

use nockapp::Bytes;
use nockchain_math::zoon::zmap::ZMap;
use nockchain_types::tx_engine::common::Signature;
use wallet_tx_builder::types::CandidateNote;

use super::*;

pub(crate) fn ensure_manual_planner_parity(
    requested_names: &[Name],
    planned_names: &[Name],
) -> Result<(), String> {
    let mut normalized_requested = requested_names
        .iter()
        .map(|name| (name.first.to_array(), name.last.to_array()))
        .collect::<Vec<_>>();
    let mut normalized_planned = planned_names
        .iter()
        .map(|name| (name.first.to_array(), name.last.to_array()))
        .collect::<Vec<_>>();
    normalized_requested.sort_unstable();
    normalized_planned.sort_unstable();

    if normalized_planned != normalized_requested {
        let planned_names_arg = Wallet::format_note_names_for_create_tx(planned_names);
        let requested_names_arg = Wallet::format_note_names_for_create_tx(requested_names);
        return Err(format!(
            "planner parity mismatch: selected names differ from user-provided manual names (planned='{}', requested='{}')",
            planned_names_arg, requested_names_arg
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
/// Subset of chain note-data constants consumed by planner fee logic.
pub(crate) struct PlannerNoteDataConstantsNoun {
    pub(crate) _max_size: u64,
    pub(crate) min_fee: u64,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
/// Blockchain constants payload extracted from wallet state for planning.
pub(crate) struct PlannerBlockchainConstantsNoun {
    pub(crate) _v1_phase: u64,
    pub(crate) bythos_phase: u64,
    pub(crate) data: PlannerNoteDataConstantsNoun,
    pub(crate) base_fee: u64,
    pub(crate) input_fee_divisor: u64,
    pub(crate) _legacy_constants: Noun,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
/// Embedded v0 constants payload carried inside wallet blockchain constants.
struct PlannerV0BlockchainConstantsNoun {
    _max_block_size: Noun,
    _blocks_per_epoch: Noun,
    _target_epoch_duration: Noun,
    _update_candidate_timestamp_interval: Noun,
    _max_future_timestamp: Noun,
    _min_past_blocks: Noun,
    _genesis_target_atom: Noun,
    _max_target_atom: Noun,
    _check_pow_flag: bool,
    coinbase_timelock_min: u64,
    _pow_len: Noun,
    _max_coinbase_split: Noun,
    _first_month_coinbase_min: Noun,
}

#[derive(Debug, Clone, NounEncode, NounDecode, PartialEq, Eq)]
pub(crate) struct ActiveSignerEntryNoun {
    pub(crate) child_index: Option<u64>,
    pub(crate) hardened: bool,
    pub(crate) absolute_index: Option<u64>,
    pub(crate) version: u64,
    pub(crate) pubkey: SchnorrPubkey,
    pub(crate) address_b58: String,
}

impl ActiveSignerEntryNoun {
    fn is_master(&self) -> bool {
        self.child_index.is_none()
    }

    fn sign_keys(&self) -> Vec<(u64, bool)> {
        self.child_index
            .map(|index| vec![(index, self.hardened)])
            .unwrap_or_default()
    }

    fn sort_key(&self) -> (u8, u64, String) {
        (
            if self.is_master() { 0 } else { 1 },
            self.absolute_index.unwrap_or(0),
            self.address_b58.clone(),
        )
    }

    fn label(&self) -> String {
        match self.child_index {
            Some(index) => {
                let hardened = if self.hardened {
                    "hardened"
                } else {
                    "unhardened"
                };
                format!("child({index}:{hardened})")
            }
            None => "master".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MigrateV0SignerSummary {
    pub(crate) signer: ActiveSignerEntryNoun,
    pub(crate) note_count: usize,
    pub(crate) selected_total: u64,
    pub(crate) fee: Option<u64>,
    pub(crate) migrated_amount: Option<u64>,
    pub(crate) tx_path: Option<String>,
    pub(crate) skip_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MigrateV0NotesSummary {
    pub(crate) destination: String,
    pub(crate) block_id: String,
    pub(crate) height: u64,
    pub(crate) examined_signers: usize,
    pub(crate) created_count: usize,
    pub(crate) skipped_count: usize,
    pub(crate) signers: Vec<MigrateV0SignerSummary>,
}

#[cfg(test)]
#[derive(Debug, Clone, NounDecode)]
struct BatchWriteRequestEntry {
    path: String,
    contents: Bytes,
}

#[cfg(test)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AppliedWalletEffects {
    tx_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TxFileSnapshot {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WrittenTxSnapshot(BTreeMap<PathBuf, TxFileSnapshot>);

#[derive(Debug, Clone)]
struct CreateTxRequest {
    names: String,
    recipients: Vec<RecipientSpec>,
    fee: u64,
    allow_low_fee: bool,
    refund_pkh: Option<String>,
    sign_keys: Vec<(u64, bool)>,
    include_data: bool,
    save_raw_tx: bool,
    note_selection: NoteSelectionStrategyCli,
}

#[derive(Debug, Clone)]
struct PendingMigrationTx {
    summary_index: usize,
    planned_names: Vec<Name>,
    request: CreateTxRequest,
}

pub(crate) struct PreparedMigrateV0Notes {
    pub(crate) summary: MigrateV0NotesSummary,
    poke: Option<(NounSlab, Operation)>,
    pending_txs: Vec<PendingMigrationTx>,
}

impl PreparedMigrateV0Notes {
    pub(crate) fn take_poke(&mut self) -> Option<(NounSlab, Operation)> {
        self.poke.take()
    }

    fn normalized_name_key(names: &[Name]) -> Vec<([u64; 5], [u64; 5])> {
        let mut key = names
            .iter()
            .map(|name| (name.first.to_array(), name.last.to_array()))
            .collect::<Vec<_>>();
        key.sort_unstable();
        key
    }

    fn assign_tx_paths(&mut self, tx_paths: Vec<String>) -> Result<(), NockAppError> {
        if tx_paths.len() != self.pending_txs.len() {
            return Err(NockAppError::OtherError(format!(
                "migrate-v0-notes expected {} saved transaction files, but found {}",
                self.pending_txs.len(),
                tx_paths.len()
            )));
        }

        let mut expected_by_name_set = BTreeMap::<Vec<([u64; 5], [u64; 5])>, usize>::new();
        for pending in &self.pending_txs {
            let key = Self::normalized_name_key(&pending.planned_names);
            if expected_by_name_set
                .insert(key, pending.summary_index)
                .is_some()
            {
                return Err(NockAppError::OtherError(
                    "migrate-v0-notes found duplicate planned note sets while matching saved transactions".to_string(),
                ));
            }
        }

        let mut assigned = BTreeMap::<usize, String>::new();
        for tx_path in tx_paths {
            let spends = Wallet::decode_transaction_spends_from_path(&tx_path)?;
            let tx_name_key = Self::normalized_name_key(
                &spends
                    .0
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>(),
            );
            let Some(summary_index) = expected_by_name_set.get(&tx_name_key).copied() else {
                return Err(NockAppError::OtherError(format!(
                    "migrate-v0-notes could not match saved transaction '{}' to any planned signer batch",
                    tx_path
                )));
            };
            if assigned.insert(summary_index, tx_path.clone()).is_some() {
                return Err(NockAppError::OtherError(format!(
                    "migrate-v0-notes matched more than one saved transaction to signer summary index {}",
                    summary_index
                )));
            }
        }

        for pending in &self.pending_txs {
            let Some(tx_path) = assigned.remove(&pending.summary_index) else {
                return Err(NockAppError::OtherError(format!(
                    "migrate-v0-notes did not find a saved transaction for signer summary index {}",
                    pending.summary_index
                )));
            };
            self.summary.signers[pending.summary_index].tx_path = Some(tx_path);
        }

        Ok(())
    }

    pub(crate) fn finalize(
        mut self,
        tx_paths: Vec<String>,
    ) -> Result<MigrateV0NotesSummary, NockAppError> {
        if !self.pending_txs.is_empty() {
            self.assign_tx_paths(tx_paths)?;
        }
        Ok(self.summary)
    }
}

impl PlannerBlockchainConstantsNoun {
    /// Returns the consensus coinbase relative timelock minimum.
    pub(crate) fn coinbase_timelock_min(&self) -> Result<u64, NockAppError> {
        let parsed = PlannerV0BlockchainConstantsNoun::from_noun(&self._legacy_constants).map_err(
            |err| {
                NockAppError::OtherError(format!(
                    "wallet blockchain-constants payload missing coinbase timelock min: {err}"
                ))
            },
        )?;
        Ok(parsed.coinbase_timelock_min)
    }
}

#[derive(Debug, Clone, Default)]
/// Lock matcher for simple single-signer PKH lock resolution.
///
/// This matcher is intentionally scoped to single-signer PKH spend conditions
/// that can be satisfied by locally held signer keys.
/// Multisig or otherwise complex lock forms are intentionally not matched here.
pub(crate) struct SigningKeyLockMatcher {
    signer_pkhs: std::collections::BTreeSet<[u64; 5]>,
}

impl SigningKeyLockMatcher {
    /// Builds a matcher from signer pubkey-hashes.
    pub(crate) fn from_signer_keys(signer_keys: &[Hash]) -> Self {
        let signer_pkhs = signer_keys
            .iter()
            .map(Hash::to_array)
            .collect::<std::collections::BTreeSet<_>>();
        Self { signer_pkhs }
    }
}

impl LockMatcher for SigningKeyLockMatcher {
    fn matches(&self, note_first_name: &Hash, spend_condition: &SpendCondition) -> bool {
        let mut primitive_count = 0usize;
        let mut tim_primitive_count = 0usize;
        let mut signer_pkh_primitive = None;
        for primitive in spend_condition.iter() {
            primitive_count = primitive_count.saturating_add(1);
            match primitive {
                LockPrimitive::Pkh(pkh) => {
                    if signer_pkh_primitive.is_some() {
                        return false;
                    }
                    signer_pkh_primitive = Some(pkh);
                }
                LockPrimitive::Tim(_) => {
                    tim_primitive_count = tim_primitive_count.saturating_add(1);
                }
                _ => return false,
            }
        }
        let Some(pkh) = signer_pkh_primitive else {
            return false;
        };
        if pkh.m != 1 || pkh.hashes.len() != 1 {
            return false;
        }
        let Some(hash) = pkh.hashes.first() else {
            return false;
        };
        if !self.signer_pkhs.contains(&hash.to_array()) {
            return false;
        }
        let is_simple_shape = tim_primitive_count == 0 && primitive_count == 1;
        let is_coinbase_shape = tim_primitive_count == 1 && primitive_count == 2;
        if !is_simple_shape && !is_coinbase_shape {
            return false;
        }
        let Ok(reconstructed_first_name) = spend_condition.first_name() else {
            return false;
        };
        note_first_name.to_array() == reconstructed_first_name.as_hash().to_array()
    }
}

impl Wallet {
    fn parse_note_names_as_hashes(raw: &str) -> Result<Vec<Name>, NockAppError> {
        Self::parse_note_names(raw)?
            .into_iter()
            .map(|(first, last)| {
                let first_hash = Hash::from_base58(&first).map_err(|err| {
                    NockAppError::from(CrownError::Unknown(format!(
                        "Invalid note first-name hash '{}': {}",
                        first, err
                    )))
                })?;
                let last_hash = Hash::from_base58(&last).map_err(|err| {
                    NockAppError::from(CrownError::Unknown(format!(
                        "Invalid note last-name hash '{}': {}",
                        last, err
                    )))
                })?;
                Ok(Name::new(first_hash, last_hash))
            })
            .collect()
    }

    /// Formats selected names into the canonical create-tx `--names` argument.
    fn format_note_names_for_create_tx(names: &[Name]) -> String {
        names
            .iter()
            .map(|name| format!("[{} {}]", name.first.to_base58(), name.last.to_base58()))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Determines whether a manual note set is all-v1 or all-v0.
    /// Missing notes are ignored here so planner manual-mode errors can report them.
    fn manual_candidate_version_policy(
        note_names: &[Name],
        candidates: &[CandidateNote],
    ) -> Result<CandidateVersionPolicy, String> {
        if note_names.is_empty() {
            return Err("manual mode requires at least one note name".to_string());
        }

        let mut found_v0 = false;
        let mut found_v1 = false;

        for name in note_names {
            let Some(candidate) = candidates
                .iter()
                .find(|candidate| candidate.identity().name == *name)
            else {
                return Err(format!(
                    "manual mode references unknown note {}/{}",
                    name.first.to_base58(),
                    name.last.to_base58()
                ));
            };

            match candidate.version() {
                nockchain_types::tx_engine::common::Version::V0 => found_v0 = true,
                _ => found_v1 = true,
            }
        }

        match (found_v0, found_v1) {
            (true, false) => Ok(CandidateVersionPolicy::V0Only),
            (false, true) => Ok(CandidateVersionPolicy::V1Only),
            (false, false) => Err("manual mode requires at least one note name".to_string()),
            (true, true) => Err(
                "manual create-tx cannot mix v0 and v1 notes; select notes from only one version"
                    .to_string(),
            ),
        }
    }

    /// Maps CLI ordering strategy onto planner selection order semantics.
    fn planner_order_direction(strategy: NoteSelectionStrategyCli) -> SelectionOrder {
        match strategy {
            NoteSelectionStrategyCli::Ascending => SelectionOrder::Ascending,
            NoteSelectionStrategyCli::Descending => SelectionOrder::Descending,
        }
    }

    /// Reads the latest synced balance snapshot from wallet state.
    async fn peek_balance_state(&mut self) -> Result<v1::BalanceUpdate, NockAppError> {
        let mut slab = NounSlab::new();
        let balance_tag = make_tas(&mut slab, "balance").as_noun();
        let path = T(&mut slab, &[balance_tag, SIG]);
        slab.set_root(path);

        let result = self.app.peek(slab).await?;
        let maybe_balance: Option<Option<v1::BalanceUpdate>> =
            unsafe { <Option<Option<v1::BalanceUpdate>>>::from_noun(result.root())? };
        match maybe_balance {
            Some(Some(balance)) => Ok(balance),
            _ => Err(NockAppError::OtherError(
                "wallet balance peek returned no balance payload".to_string(),
            )),
        }
    }

    /// Reads blockchain constants from wallet state so the planner uses live fee policy.
    async fn peek_planner_blockchain_constants(
        &mut self,
    ) -> Result<PlannerBlockchainConstantsNoun, NockAppError> {
        let mut slab = NounSlab::new();
        let constants_tag = make_tas(&mut slab, "blockchain-constants").as_noun();
        let path = T(&mut slab, &[constants_tag, SIG]);
        slab.set_root(path);

        let result = self.app.peek(slab).await?;
        let maybe_constants: Option<Option<PlannerBlockchainConstantsNoun>> =
            unsafe { <Option<Option<PlannerBlockchainConstantsNoun>>>::from_noun(result.root())? };
        let Some(constants) = maybe_constants.flatten() else {
            return Err(NockAppError::OtherError(
                "wallet blockchain-constants peek returned no payload".to_string(),
            ));
        };
        Ok(constants)
    }

    /// Reads the master signer pubkey-hash from wallet tracked state for lock matching.
    async fn peek_master_signing_key(&mut self) -> Result<Hash, NockAppError> {
        let mut slab = NounSlab::new();
        let tracked_tag = make_tas(&mut slab, "master-signing-key").as_noun();
        let path = T(&mut slab, &[tracked_tag, SIG]);
        slab.set_root(path);

        let result = self.app.peek(slab).await?;
        let maybe_signing_key: Option<Option<Hash>> =
            unsafe { <Option<Option<Hash>>>::from_noun(result.root())? };
        maybe_signing_key.flatten().ok_or_else(|| {
            NockAppError::OtherError(
                "wallet master-signing-key peek returned no payload".to_string(),
            )
        })
    }

    async fn peek_master_signing_pubkey(&mut self) -> Result<SchnorrPubkey, NockAppError> {
        let mut slab = NounSlab::new();
        let tracked_tag = make_tas(&mut slab, "master-signing-pubkey").as_noun();
        let path = T(&mut slab, &[tracked_tag, SIG]);
        slab.set_root(path);

        let result = self.app.peek(slab).await?;
        let maybe_signing_pubkey: Option<Option<SchnorrPubkey>> =
            unsafe { <Option<Option<SchnorrPubkey>>>::from_noun(result.root())? };
        maybe_signing_pubkey.flatten().ok_or_else(|| {
            NockAppError::OtherError(
                "wallet master-signing-pubkey peek returned no payload".to_string(),
            )
        })
    }

    async fn peek_active_signers(&mut self) -> Result<Vec<ActiveSignerEntryNoun>, NockAppError> {
        let mut slab = NounSlab::new();
        let tracked_tag = make_tas(&mut slab, "active-signers").as_noun();
        let path = T(&mut slab, &[tracked_tag, SIG]);
        slab.set_root(path);

        let result = self.app.peek(slab).await?;
        let maybe_signers: Option<Option<Vec<ActiveSignerEntryNoun>>> =
            unsafe { <Option<Option<Vec<ActiveSignerEntryNoun>>>>::from_noun(result.root())? };
        let mut signers = maybe_signers.flatten().unwrap_or_default();
        signers.sort_by_key(ActiveSignerEntryNoun::sort_key);
        signers.dedup_by(|left, right| {
            left.child_index == right.child_index
                && left.hardened == right.hardened
                && left.absolute_index == right.absolute_index
                && left.address_b58 == right.address_b58
        });
        Ok(signers)
    }

    #[cfg(test)]
    fn resolve_effect_write_path(path: &str, output_path: Option<&Path>) -> PathBuf {
        let raw_path = Path::new(path);
        match output_path {
            Some(base_path) if !raw_path.is_absolute() => base_path.join(raw_path),
            _ => raw_path.to_path_buf(),
        }
    }

    #[cfg(test)]
    async fn apply_wallet_effects_locally(
        effects: Vec<NounSlab>,
        output_path: Option<&Path>,
    ) -> Result<AppliedWalletEffects, NockAppError> {
        let mut applied = AppliedWalletEffects::default();

        for effect in effects {
            let noun = unsafe { effect.root() };
            let Ok(cell) = noun.as_cell() else {
                continue;
            };
            let Ok(tag) = <String>::from_noun(&cell.head()) else {
                continue;
            };

            match tag.as_str() {
                "file" => {
                    let file_cell = cell.tail().as_cell().map_err(|err| {
                        NockAppError::OtherError(format!(
                            "wallet file effect payload did not decode as a cell: {err}"
                        ))
                    })?;
                    let operation = <String>::from_noun(&file_cell.head())?;
                    match operation.as_str() {
                        "write" => {
                            let (path, contents): (String, Bytes) =
                                <(String, Bytes)>::from_noun(&file_cell.tail())?;
                            let resolved_path = Self::resolve_effect_write_path(&path, output_path);
                            if let Some(parent) = resolved_path.parent() {
                                tokio_fs::create_dir_all(parent)
                                    .await
                                    .map_err(NockAppError::IoError)?;
                            }
                            tokio_fs::write(&resolved_path, contents.as_ref())
                                .await
                                .map_err(NockAppError::IoError)?;
                            if resolved_path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| ext == "tx")
                            {
                                applied.tx_paths.push(resolved_path.display().to_string());
                            }
                        }
                        "batch-write" => {
                            let entries: Vec<BatchWriteRequestEntry> =
                                Vec::from_noun(&file_cell.tail())?;
                            for entry in entries {
                                let resolved_path =
                                    Self::resolve_effect_write_path(&entry.path, output_path);
                                if let Some(parent) = resolved_path.parent() {
                                    tokio_fs::create_dir_all(parent)
                                        .await
                                        .map_err(NockAppError::IoError)?;
                                }
                                tokio_fs::write(&resolved_path, entry.contents.as_ref())
                                    .await
                                    .map_err(NockAppError::IoError)?;
                                if resolved_path
                                    .extension()
                                    .and_then(|ext| ext.to_str())
                                    .is_some_and(|ext| ext == "tx")
                                {
                                    applied.tx_paths.push(resolved_path.display().to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                "exit" => {
                    let code = <u64 as NounDecode>::from_noun(&cell.tail())?;
                    if code != 0 {
                        return Err(NockAppError::OtherError(format!(
                            "wallet command exited with code {code} while running migrate-v0-notes"
                        )));
                    }
                }
                _ => {}
            }
        }

        Ok(applied)
    }

    pub(crate) async fn snapshot_written_txs(
        tx_dir: &Path,
    ) -> Result<WrittenTxSnapshot, NockAppError> {
        let mut snapshots = BTreeMap::new();
        if !tx_dir.exists() {
            return Ok(WrittenTxSnapshot(snapshots));
        }

        let mut entries = tokio_fs::read_dir(tx_dir)
            .await
            .map_err(NockAppError::IoError)?;
        while let Some(entry) = entries.next_entry().await.map_err(NockAppError::IoError)? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("tx") {
                continue;
            }
            let metadata = entry.metadata().await.map_err(NockAppError::IoError)?;
            let modified = metadata.modified().ok();
            snapshots.insert(
                path,
                TxFileSnapshot {
                    modified,
                    len: metadata.len(),
                },
            );
        }

        Ok(WrittenTxSnapshot(snapshots))
    }

    pub(crate) fn detect_written_tx_paths(
        before: &WrittenTxSnapshot,
        after: &WrittenTxSnapshot,
    ) -> Result<Vec<String>, NockAppError> {
        let changed = after
            .0
            .iter()
            .filter_map(|(path, metadata)| match before.0.get(path) {
                Some(previous) if previous == metadata => None,
                _ => Some(path.display().to_string()),
            })
            .collect::<Vec<_>>();

        if changed.is_empty() {
            return Err(NockAppError::OtherError(
                "migrate-v0-notes expected create-tx-batch to write at least one transaction file, but no tx files changed".to_string(),
            ));
        }

        Ok(changed)
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
                let witnesses = ZMap::<Name, v1::Witness>::from_noun(&witness_cell.tail())
                    .map_err(|err| {
                        NockAppError::OtherError(format!(
                            "v1 witness-data map did not decode: {err}"
                        ))
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

    fn decode_transaction_spends_from_path(
        transaction_path: &str,
    ) -> Result<v1::Spends, NockAppError> {
        let tx_bytes = std::fs::read(transaction_path).map_err(|err| {
            NockAppError::OtherError(format!("failed to read transaction file: {err}"))
        })?;
        Self::decode_transaction_spends_from_bytes(&tx_bytes)
    }

    #[cfg(test)]
    /// Builds deterministic signer candidate list used by tests.
    pub(crate) fn planner_signer_candidates(mut tracked_signers: Vec<Hash>) -> Vec<Option<Hash>> {
        tracked_signers.sort_by_key(|signer| signer.to_array());
        tracked_signers.dedup_by(|a, b| a.to_array() == b.to_array());
        let mut candidates = Vec::with_capacity(tracked_signers.len() + 1);
        candidates.push(None);
        candidates.extend(tracked_signers.into_iter().map(Some));
        candidates
    }

    /// Plans create-tx inputs/fee and dispatches final hoon create-tx poke.
    pub(crate) async fn create_tx_with_planner(
        &mut self,
        synced_snapshot: Option<NormalizedSnapshot>,
        names: Option<String>,
        fee: Option<u64>,
        recipients: Vec<RecipientSpec>,
        allow_low_fee: bool,
        refund_pkh: Option<String>,
        sign_keys: Vec<(u64, bool)>,
        include_data: bool,
        save_raw_tx: bool,
        note_selection: NoteSelectionStrategyCli,
    ) -> CommandNoun<NounSlab> {
        let planner_error = |reason: String| -> CommandNoun<NounSlab> {
            Err(CrownError::Unknown(format!("create-tx planner failed: {}", reason)).into())
        };

        let snapshot = if let Some(snapshot) = synced_snapshot {
            snapshot
        } else {
            let balance = match self.peek_balance_state().await {
                Ok(balance) => balance,
                Err(err) => {
                    return planner_error(format!(
                        "unable to read synced balance from wallet state: {err}"
                    ));
                }
            };
            match normalize_balance_pages(&[balance]) {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    return planner_error(format!(
                        "candidate normalization failed for wallet balance snapshot: {err}"
                    ));
                }
            }
        };
        let v1_candidate_count = snapshot
            .candidates
            .iter()
            .filter(|candidate| {
                candidate.version() == nockchain_types::tx_engine::common::Version::V1
            })
            .count();
        let candidate_preview = snapshot
            .candidates
            .iter()
            .take(5)
            .map(|candidate| {
                let identity = candidate.identity();
                format!(
                    "{}/{}",
                    identity.name.first.to_base58(),
                    identity.name.last.to_base58()
                )
            })
            .collect::<Vec<_>>();
        info!(
            "create-tx planner snapshot block={} height={:?} candidates_total={} candidates_v1={} preview={:?}",
            snapshot.metadata.block_id.to_base58(),
            snapshot.metadata.height,
            snapshot.candidates.len(),
            v1_candidate_count,
            candidate_preview
        );

        let manual_note_names = match names.as_deref() {
            Some(raw_names) => match Self::parse_note_names_as_hashes(raw_names) {
                Ok(note_names) => Some(note_names),
                Err(err) => {
                    return planner_error(format!("unable to parse manual note names: {err}"));
                }
            },
            None => None,
        };
        let selection_mode = match &manual_note_names {
            Some(note_names) => SelectionMode::Manual {
                note_names: note_names.clone(),
            },
            None => SelectionMode::Auto,
        };
        let parsed_refund_pkh = if let Some(refund) = refund_pkh.as_ref() {
            match Hash::from_base58(refund) {
                Ok(hash) => Some(hash),
                Err(err) => {
                    return planner_error(format!(
                        "invalid refund pubkey hash '{}': {}",
                        refund, err
                    ));
                }
            }
        } else {
            None
        };
        let candidate_version_policy = match &manual_note_names {
            Some(note_names) => {
                match Self::manual_candidate_version_policy(note_names, &snapshot.candidates) {
                    Ok(policy) => policy,
                    Err(err) => {
                        return planner_error(err);
                    }
                }
            }
            None => CandidateVersionPolicy::V1Only,
        };
        if candidate_version_policy == CandidateVersionPolicy::V0Only && parsed_refund_pkh.is_none()
        {
            return planner_error(
                "manual create-tx spending legacy v0 notes requires --refund-pkh".to_string(),
            );
        }
        if !sign_keys.is_empty() {
            info!(
                "create-tx planner spendability matching currently uses only the wallet master key"
            );
        }
        let master_signer_pkh = match self.peek_master_signing_key().await {
            Ok(key) => key,
            Err(err) => {
                warn!(
                    "create-tx planner could not read master signing key from wallet state: {}",
                    err
                );
                return planner_error(
                    "wallet has no signer keys for create-tx planner".to_string(),
                );
            }
        };
        info!(
            "create-tx planner master-signer-pkh={}",
            master_signer_pkh.to_base58()
        );
        let legacy_signer_pubkeys = if candidate_version_policy == CandidateVersionPolicy::V0Only {
            let master_signer_pubkey = match self.peek_master_signing_pubkey().await {
                Ok(key) => key,
                Err(err) => {
                    return planner_error(format!(
                        "unable to read master signing pubkey from wallet state: {err}"
                    ));
                }
            };
            vec![master_signer_pubkey]
        } else {
            Vec::new()
        };
        // Today lock matching is constrained to the master signer key only.
        // We can expand this matcher input to include additional signing keys later.
        let matcher_signer_keys = vec![master_signer_pkh.clone()];
        let recipient_outputs = match planner_recipient_outputs(&recipients, include_data) {
            Ok(outputs) => outputs,
            Err(err) => {
                return planner_error(format!(
                    "unable to derive planner recipient lock roots from recipients: {err}"
                ));
            }
        };
        let refund_output_template = match planner_refund_output_template(
            parsed_refund_pkh.as_ref(),
            &master_signer_pkh,
            include_data,
        ) {
            Ok(output) => output,
            Err(err) => {
                return planner_error(format!(
                    "unable to derive planner refund output template from signer/refund context: {err}"
                ));
            }
        };
        let planner_constants = match self.peek_planner_blockchain_constants().await {
            Ok(constants) => constants,
            Err(err) => {
                return planner_error(format!(
                    "unable to read blockchain constants from wallet state: {err}"
                ));
            }
        };
        let coinbase_relative_min = match planner_constants.coinbase_timelock_min() {
            Ok(min) => min,
            Err(err) => {
                return planner_error(format!(
                    "unable to resolve coinbase timelock min from blockchain constants: {err}"
                ));
            }
        };
        info!(
            "create-tx planner constants bythos_phase={} base_fee={} input_fee_divisor={} min_fee={} coinbase_relative_min={}",
            planner_constants.bythos_phase,
            planner_constants.base_fee,
            planner_constants.input_fee_divisor,
            planner_constants.data.min_fee,
            coinbase_relative_min
        );
        let order_direction = Self::planner_order_direction(note_selection);

        let request = PlanRequest {
            planning_mode: PlanningMode::Standard,
            selection_mode: selection_mode.clone(),
            order_direction,
            include_data,
            chain_context: ChainContext {
                height: snapshot.metadata.height.clone(),
                bythos_phase: nockchain_types::tx_engine::common::BlockHeight(
                    nockchain_math::belt::Belt(planner_constants.bythos_phase),
                ),
                base_fee: planner_constants.base_fee,
                input_fee_divisor: planner_constants.input_fee_divisor,
                min_fee: planner_constants.data.min_fee,
            },
            signer_pkh: Some(master_signer_pkh.clone()),
            candidate_version_policy,
            candidates: snapshot.candidates,
            recipient_outputs,
            refund_output: refund_output_template,
            coinbase_relative_min: Some(coinbase_relative_min),
            v0_migration_signer_pubkeys: legacy_signer_pubkeys,
        };

        let matcher = SigningKeyLockMatcher::from_signer_keys(&matcher_signer_keys);
        let plan = match plan_create_tx(&request, &matcher) {
            Ok(found_plan) => {
                info!(
                    "create-tx planner using master signer {} for lock spendability checks",
                    master_signer_pkh.to_base58()
                );
                found_plan
            }
            Err(err @ PlanError::CandidateVersionDisabled { .. }) => {
                return Err(CrownError::Unknown(format!(
                    "create-tx planner rejected the manual note set because it does not match the selected note version policy ({})",
                    err
                ))
                .into());
            }
            Err(err) => {
                return planner_error(format!("planner returned an error: {err}"));
            }
        };

        for trace in &plan.debug_trace {
            info!("create-tx planner trace: {}", trace);
        }

        let planned_names = plan
            .selected
            .iter()
            .map(|selected| selected.name.clone())
            .collect::<Vec<_>>();
        if let SelectionMode::Manual { note_names } = &selection_mode {
            if let Err(reason) = ensure_manual_planner_parity(note_names, &planned_names) {
                return planner_error(reason);
            }
        }
        let planned_names_arg = Self::format_note_names_for_create_tx(&planned_names);
        let planned_fee = plan.final_fee;
        let final_fee = if let Some(requested_fee) = fee {
            if requested_fee < planned_fee && !allow_low_fee {
                return Err(CrownError::Unknown(format!(
                    "requested --fee {} is below planner minimum {} (pass --allow-low-fee to override)",
                    requested_fee, planned_fee
                ))
                .into());
            }
            if requested_fee != planned_fee {
                info!(
                    "create-tx planner fee override requested_fee={} planned_fee={}",
                    requested_fee, planned_fee
                );
            }
            requested_fee
        } else {
            planned_fee
        };

        Self::create_tx(CreateTxRequest {
            names: planned_names_arg,
            recipients,
            fee: final_fee,
            allow_low_fee,
            refund_pkh,
            sign_keys,
            include_data,
            save_raw_tx,
            note_selection,
        })
    }

    pub(crate) fn format_migrate_v0_notes_summary(summary: &MigrateV0NotesSummary) -> String {
        let mut lines = vec![
            "## V0 Migration Sweep".to_string(),
            format!("- destination: `{}`", summary.destination),
            format!("- block id: `{}`", summary.block_id),
            format!("- height: `{}`", summary.height),
            format!(
                "- active signing keys examined: `{}`",
                summary.examined_signers
            ),
            format!("- migration txs created: `{}`", summary.created_count),
            format!("- signing keys skipped: `{}`", summary.skipped_count),
        ];

        if summary.created_count == 0 {
            lines.push(
                "- batch create poke: not emitted because every signer bucket was skipped"
                    .to_string(),
            );
        }

        for signer_summary in &summary.signers {
            lines.push(String::new());
            lines.push(format!("### {}", signer_summary.signer.label()));
            lines.push(format!(
                "- signer address: `{}`",
                signer_summary.signer.address_b58
            ));
            lines.push(format!(
                "- signer version: `{}`",
                signer_summary.signer.version
            ));
            lines.push(format!("- selected notes: `{}`", signer_summary.note_count));
            lines.push(format!(
                "- selected total: `{}`",
                signer_summary.selected_total
            ));
            match (&signer_summary.migrated_amount, &signer_summary.tx_path) {
                (Some(migrated_amount), Some(tx_path)) => {
                    lines.push("- result: `created`".to_string());
                    lines.push(format!(
                        "- fee: `{}`",
                        signer_summary.fee.unwrap_or_default()
                    ));
                    lines.push(format!("- migrated amount: `{}`", migrated_amount));
                    lines.push(format!("- tx path: `{}`", tx_path));
                    lines.push(format!(
                        "- submit with: `nockchain-wallet send-tx \"{}\"`",
                        tx_path
                    ));
                }
                _ => {
                    lines.push("- result: `skipped`".to_string());
                    if let Some(fee) = signer_summary.fee {
                        lines.push(format!("- fee estimate: `{}`", fee));
                    }
                    if let Some(reason) = &signer_summary.skip_reason {
                        lines.push(format!("- skip reason: `{}`", reason));
                    }
                }
            }
        }

        lines.join("\n")
    }

    /// Plans one v0 migration transaction per active local v0 signer.
    ///
    /// Arguments:
    /// - `synced_snapshot`: optional pre-normalized balance snapshot from the caller. When
    ///   `None`, the helper reads the current synced balance from wallet state and normalizes it.
    /// - `destination`: base58-encoded v1 destination address that receives each migrated output.
    pub(crate) async fn prepare_migrate_v0_notes_per_signer(
        &mut self,
        synced_snapshot: Option<NormalizedSnapshot>,
        destination: String,
    ) -> Result<PreparedMigrateV0Notes, NockAppError> {
        let destination_hash = Hash::from_base58(&destination).map_err(|err| {
            CrownError::Unknown(format!(
                "migrate-v0-notes planner failed: invalid migration destination '{}' : {}",
                destination, err
            ))
        })?;
        let snapshot = if let Some(snapshot) = synced_snapshot {
            snapshot
        } else {
            let balance = self.peek_balance_state().await.map_err(|err| {
                CrownError::Unknown(format!(
                    "migrate-v0-notes planner failed: unable to read synced balance from wallet state: {err}"
                ))
            })?;
            normalize_balance_pages(&[balance]).map_err(|err| {
                CrownError::Unknown(format!(
                    "migrate-v0-notes planner failed: candidate normalization failed for wallet balance snapshot: {err}"
                ))
            })?
        };
        let active_signers = self.peek_active_signers().await.map_err(|err| {
            CrownError::Unknown(format!(
                "migrate-v0-notes planner failed: unable to read active signer entries from wallet state: {err}"
            ))
        })?;
        let active_signers = active_signers
            .into_iter()
            .filter(|signer| signer.version == 0)
            .collect::<Vec<_>>();
        if active_signers.is_empty() {
            return Err(CrownError::Unknown(
                "migrate-v0-notes planner failed: wallet has no active local v0 signing keys under the active master".to_string(),
            )
            .into());
        }

        let planner_constants = self.peek_planner_blockchain_constants().await.map_err(|err| {
            CrownError::Unknown(format!(
                "migrate-v0-notes planner failed: unable to read blockchain constants from wallet state: {err}"
            ))
        })?;
        let coinbase_relative_min = planner_constants.coinbase_timelock_min().map_err(|err| {
            CrownError::Unknown(format!(
                "migrate-v0-notes planner failed: unable to resolve coinbase timelock min from blockchain constants: {err}"
            ))
        })?;
        let mut destination_outputs = planner_recipient_outputs(
            &[RecipientSpec::P2pkh {
                address: destination_hash.clone(),
                amount: 0,
            }],
            true,
        )
        .map_err(|err| {
            CrownError::Unknown(format!(
                "migrate-v0-notes planner failed: unable to derive migration destination output from recipient: {err}"
            ))
        })?;
        let destination_output = destination_outputs
            .pop()
            .expect("single migration recipient should yield one planner output");
        let refund_output =
            planner_refund_output_template(Some(&destination_hash), &destination_hash, true)
                .expect("p2pkh migration refund template should build");
        let chain_context = ChainContext {
            height: snapshot.metadata.height.clone(),
            bythos_phase: nockchain_types::tx_engine::common::BlockHeight(
                nockchain_math::belt::Belt(planner_constants.bythos_phase),
            ),
            base_fee: planner_constants.base_fee,
            input_fee_divisor: planner_constants.input_fee_divisor,
            min_fee: planner_constants.data.min_fee,
        };

        let mut signer_summaries = Vec::with_capacity(active_signers.len());
        let mut pending_txs = Vec::<PendingMigrationTx>::new();
        let mut skipped_count = 0usize;

        for signer in active_signers {
            let request = PlanRequest {
                planning_mode: PlanningMode::V0MigrationSweep {
                    destination_output: destination_output.clone(),
                },
                selection_mode: SelectionMode::Auto,
                order_direction: SelectionOrder::Ascending,
                include_data: true,
                chain_context: chain_context.clone(),
                signer_pkh: None,
                candidate_version_policy: CandidateVersionPolicy::V0Only,
                candidates: snapshot.candidates.clone(),
                recipient_outputs: Vec::new(),
                refund_output: refund_output.clone(),
                coinbase_relative_min: Some(coinbase_relative_min),
                v0_migration_signer_pubkeys: vec![signer.pubkey.clone()],
            };

            match plan_create_tx(&request, &SigningKeyLockMatcher::default()) {
                Ok(plan) => {
                    for trace in &plan.debug_trace {
                        info!(
                            "migrate-v0-notes planner trace signer={} {}",
                            signer.label(),
                            trace
                        );
                    }

                    let note_count = plan.selected.len();
                    let selected_total = plan.selected_total;
                    let fee = Some(plan.final_fee);
                    let migrated_amount = plan.outputs.first().map(|output| output.amount);
                    let planned_names = plan
                        .selected
                        .iter()
                        .map(|selected| selected.name.clone())
                        .collect::<Vec<_>>();
                    let Some(migrated_amount) = migrated_amount else {
                        skipped_count = skipped_count.saturating_add(1);
                        signer_summaries.push(MigrateV0SignerSummary {
                            signer,
                            note_count,
                            selected_total,
                            fee,
                            migrated_amount: None,
                            tx_path: None,
                            skip_reason: Some("planner_returned_no_destination_output".to_string()),
                        });
                        continue;
                    };

                    let summary_index = signer_summaries.len();
                    signer_summaries.push(MigrateV0SignerSummary {
                        signer: signer.clone(),
                        note_count,
                        selected_total,
                        fee,
                        migrated_amount: Some(migrated_amount),
                        tx_path: None,
                        skip_reason: None,
                    });
                    pending_txs.push(PendingMigrationTx {
                        summary_index,
                        planned_names: planned_names.clone(),
                        request: CreateTxRequest {
                            names: Self::format_note_names_for_create_tx(&planned_names),
                            recipients: vec![RecipientSpec::P2pkh {
                                address: destination_hash.clone(),
                                amount: migrated_amount,
                            }],
                            fee: plan.final_fee,
                            allow_low_fee: false,
                            refund_pkh: Some(destination_hash.to_base58()),
                            sign_keys: signer.sign_keys(),
                            include_data: true,
                            save_raw_tx: false,
                            note_selection: NoteSelectionStrategyCli::Ascending,
                        },
                    });
                }
                Err(PlanError::V0MigrationProducesZeroValue {
                    selected_total,
                    fee,
                }) => {
                    skipped_count = skipped_count.saturating_add(1);
                    let skip_reason = if selected_total == 0 {
                        "no_eligible_v0_notes"
                    } else {
                        "zero_value_after_fees"
                    };
                    signer_summaries.push(MigrateV0SignerSummary {
                        signer,
                        note_count: 0,
                        selected_total,
                        fee: Some(fee),
                        migrated_amount: None,
                        tx_path: None,
                        skip_reason: Some(skip_reason.to_string()),
                    });
                }
                Err(err) => {
                    skipped_count = skipped_count.saturating_add(1);
                    signer_summaries.push(MigrateV0SignerSummary {
                        signer,
                        note_count: 0,
                        selected_total: 0,
                        fee: None,
                        migrated_amount: None,
                        tx_path: None,
                        skip_reason: Some(format!("planner_error:{err}")),
                    });
                }
            }
        }

        let poke = if pending_txs.is_empty() {
            None
        } else {
            Some(Self::create_tx_batch(
                &pending_txs
                    .iter()
                    .map(|pending| pending.request.clone())
                    .collect::<Vec<_>>(),
            )?)
        };

        let created_count = pending_txs.len();

        Ok(PreparedMigrateV0Notes {
            summary: MigrateV0NotesSummary {
                destination,
                block_id: snapshot.metadata.block_id.to_base58(),
                height: (snapshot.metadata.height.0).0,
                examined_signers: signer_summaries.len(),
                created_count,
                skipped_count,
                signers: signer_summaries,
            },
            poke,
            pending_txs,
        })
    }

    #[cfg(test)]
    pub(crate) async fn migrate_v0_notes_per_signer_for_tests(
        &mut self,
        synced_snapshot: Option<NormalizedSnapshot>,
        destination: String,
        output_path: &Path,
    ) -> Result<MigrateV0NotesSummary, NockAppError> {
        let mut prepared = self
            .prepare_migrate_v0_notes_per_signer(synced_snapshot, destination)
            .await?;
        let tx_paths = if let Some((poke, _operation)) = prepared.take_poke() {
            let effects = self.app.poke(OnePunchWire::Poke.to_wire(), poke).await?;
            Self::apply_wallet_effects_locally(effects, Some(output_path))
                .await?
                .tx_paths
        } else {
            Vec::new()
        };
        prepared.finalize(tx_paths)
    }

    /// Creates a transaction. Use `--refund-pkh` when spending legacy v0 notes so the kernel
    /// knows where to return change. When spending v1 notes the refund automatically
    /// defaults back to the note owner, so `--refund-pkh` can be omitted.
    fn encode_create_tx_request(
        slab: &mut NounSlab,
        request: &CreateTxRequest,
    ) -> Result<Noun, NockAppError> {
        let names_vec = Self::parse_note_names(&request.names)?;
        let names_noun = names_vec
            .into_iter()
            .rev()
            .fold(D(0), |acc, (first, last)| {
                let first_noun = make_tas(slab, &first).as_noun();
                let last_noun = make_tas(slab, &last).as_noun();
                let name_pair = T(slab, &[first_noun, last_noun]);
                Cell::new(slab, name_pair, acc).as_noun()
            });

        let fee_noun = D(request.fee);
        let order_noun = request.recipients.to_noun(slab);
        let sign_key_noun = Wallet::encode_sign_keys(slab, request.sign_keys.clone());

        let refund_noun = if let Some(refund) = request.refund_pkh.as_ref() {
            let refund_hash = Hash::from_base58(refund).map_err(|err| {
                NockAppError::from(CrownError::Unknown(format!(
                    "Invalid refund pubkey hash '{}': {}",
                    refund, err
                )))
            })?;
            let refund_atom = refund_hash.to_noun(slab);
            T(slab, &[SIG, refund_atom])
        } else {
            SIG
        };
        let include_data_noun = request.include_data.to_noun(slab);
        let allow_low_fee_noun = request.allow_low_fee.to_noun(slab);
        let save_raw_tx_noun = request.save_raw_tx.to_noun(slab);
        let note_selection_noun = make_tas(slab, request.note_selection.tas_label()).as_noun();

        Ok(T(
            slab,
            &[
                names_noun, order_noun, fee_noun, allow_low_fee_noun, sign_key_noun, refund_noun,
                include_data_noun, save_raw_tx_noun, note_selection_noun,
            ],
        ))
    }

    fn create_tx(request: CreateTxRequest) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let request_noun = Self::encode_create_tx_request(&mut slab, &request)?;

        Self::wallet("create-tx", &[request_noun], Operation::Poke, &mut slab)
    }

    fn create_tx_batch(requests: &[CreateTxRequest]) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let mut request_nouns = Vec::with_capacity(requests.len());
        for request in requests {
            request_nouns.push(Self::encode_create_tx_request(&mut slab, request)?);
        }
        let requests_noun = request_nouns
            .into_iter()
            .rev()
            .fold(D(0), |acc, request_noun| {
                Cell::new(&mut slab, request_noun, acc).as_noun()
            });

        Self::wallet(
            "create-tx-batch",
            &[requests_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    #[cfg(test)]
    pub(crate) fn create_tx_command_for_tests(
        names: String,
        recipients: Vec<RecipientSpec>,
        fee: u64,
        allow_low_fee: bool,
        refund_pkh: Option<String>,
        sign_keys: Vec<(u64, bool)>,
        include_data: bool,
        save_raw_tx: bool,
        note_selection: NoteSelectionStrategyCli,
    ) -> CommandNoun<NounSlab> {
        Self::create_tx(CreateTxRequest {
            names,
            recipients,
            fee,
            allow_low_fee,
            refund_pkh,
            sign_keys,
            include_data,
            save_raw_tx,
            note_selection,
        })
    }

    /// Encodes optional sign-key tuples for wallet kernel create-tx commands.
    fn encode_sign_keys(slab: &mut NounSlab, keys: Vec<(u64, bool)>) -> Noun {
        if keys.is_empty() {
            SIG
        } else {
            Some(keys).to_noun(slab)
        }
    }

    /// Builds one `update-balance-grpc` poke from a fully assembled balance snapshot.
    fn update_balance_grpc_poke(balance_update: v1::BalanceUpdate) -> NounSlab {
        let mut slab = NounSlab::new();
        let wrapped_balance = Some(Some(balance_update));
        let balance_noun = wrapped_balance.to_noun(&mut slab);
        let head = make_tas(&mut slab, "update-balance-grpc").as_noun();
        let full = T(&mut slab, &[head, balance_noun]);
        slab.set_root(full);
        slab
    }

    #[cfg(test)]
    pub(crate) fn update_balance_grpc_poke_for_tests(
        balance_update: v1::BalanceUpdate,
    ) -> NounSlab {
        Self::update_balance_grpc_poke(balance_update)
    }

    /// Merges fetched balance pages into one consistent deduplicated snapshot.
    pub(crate) fn union_balance_pages(
        pages: Vec<v1::BalanceUpdate>,
    ) -> Result<Option<(v1::BalanceUpdate, NormalizedSnapshot)>, NormalizeSnapshotError> {
        if pages.is_empty() {
            return Ok(None);
        }

        let normalized = normalize_balance_pages(&pages)?;

        let mut deduped_notes = BTreeMap::<([u64; 5], [u64; 5]), (Name, v1::Note)>::new();
        for page in pages {
            for (name, note) in page.notes.0 {
                let key = (name.first.to_array(), name.last.to_array());
                deduped_notes.entry(key).or_insert((name, note));
            }
        }

        let merged = v1::BalanceUpdate {
            height: normalized.metadata.height.clone(),
            block_id: normalized.metadata.block_id.clone(),
            notes: v1::Balance(deduped_notes.into_values().collect()),
        };
        Ok(Some((merged, normalized)))
    }

    #[cfg(test)]
    /// Removes v1 notes that do not match tracked first-name filters.
    ///
    /// Some balance endpoints can return broader result sets than requested.
    /// This keeps wallet state aligned with tracked keys/watch lists by
    /// admitting only v1 notes whose first-name matches a tracked query.
    fn filter_untracked_v1_notes_from_balance_update(
        mut balance_update: v1::BalanceUpdate,
        tracked_first_names: &std::collections::BTreeSet<[u64; 5]>,
    ) -> v1::BalanceUpdate {
        if tracked_first_names.is_empty() {
            return balance_update;
        }

        let before = balance_update.notes.0.len();
        balance_update.notes.0.retain(|(name, note)| match note {
            v1::Note::V1(_) => tracked_first_names.contains(&name.first.to_array()),
            v1::Note::V0(_) => true,
        });
        let removed = before.saturating_sub(balance_update.notes.0.len());
        if removed > 0 {
            info!(
                "wallet balance sync dropped {} untracked v1 notes from one page",
                removed
            );
        }
        balance_update
    }

    /// Builds one `update-balance-grpc` poke from a private-api peek payload.
    fn update_balance_grpc_poke_from_payload(
        payload: Option<Option<v1::BalanceUpdate>>,
    ) -> NounSlab {
        let mut slab = NounSlab::new();
        let payload_noun = payload.to_noun(&mut slab);
        let head = make_tas(&mut slab, "update-balance-grpc").as_noun();
        let full = T(&mut slab, &[head, payload_noun]);
        slab.set_root(full);
        slab
    }

    #[cfg(test)]
    /// Test helper for filtering one balance update against tracked first names.
    pub(crate) fn filter_untracked_v1_notes_for_tests(
        balance_update: v1::BalanceUpdate,
        tracked_first_names: Vec<Hash>,
    ) -> v1::BalanceUpdate {
        let tracked = tracked_first_names
            .into_iter()
            .map(|hash| hash.to_array())
            .collect::<std::collections::BTreeSet<_>>();
        Self::filter_untracked_v1_notes_from_balance_update(balance_update, &tracked)
    }

    /// Collects one page set from the public API balance endpoints.
    async fn fetch_balance_pages_grpc_public(
        client: &mut public_nockchain::PublicNockchainGrpcClient,
        pubkeys: &[String],
        first_names: &[String],
    ) -> Result<Vec<v1::BalanceUpdate>, NockAppError> {
        let mut pages = Vec::<v1::BalanceUpdate>::new();

        for first_name in first_names {
            let response = client
                .wallet_get_balance(&BalanceRequest::FirstName(first_name.clone()))
                .await
                .map_err(|e| {
                    NockAppError::OtherError(format!(
                        "Failed to request current balance for first name {}: {}",
                        first_name, e
                    ))
                })?;
            let balance_update = v1::BalanceUpdate::try_from(response).map_err(|e| {
                NockAppError::OtherError(format!(
                    "Failed to parse balance update for first name {}: {}",
                    first_name, e
                ))
            })?;
            pages.push(balance_update);
        }

        for key in pubkeys {
            let response = client
                .wallet_get_balance(&BalanceRequest::Address(key.clone()))
                .await
                .map_err(|e| {
                    NockAppError::OtherError(format!(
                        "Failed to request current balance for pubkey {}: {}",
                        key, e
                    ))
                })?;
            let balance_update = v1::BalanceUpdate::try_from(response).map_err(|e| {
                NockAppError::OtherError(format!(
                    "Failed to parse balance update for pubkey {}: {}",
                    key, e
                ))
            })?;
            pages.push(balance_update);
        }

        Ok(pages)
    }

    /// Fetches balances via public gRPC and emits one merged wallet update snapshot.
    pub(crate) async fn update_balance_grpc_public(
        client: &mut public_nockchain::PublicNockchainGrpcClient,
        mut pubkeys: Vec<String>,
        mut first_names: Vec<String>,
    ) -> Result<connection::BalanceSyncResult, NockAppError> {
        first_names.sort();
        first_names.dedup();
        pubkeys.sort();
        pubkeys.dedup();

        const SNAPSHOT_DRIFT_MAX_RETRIES: usize = 2;
        let mut attempt = 0usize;
        let (merged_balance, normalized_snapshot) = loop {
            attempt = attempt.saturating_add(1);
            let pages =
                Self::fetch_balance_pages_grpc_public(client, &pubkeys, &first_names).await?;

            match Self::union_balance_pages(pages) {
                Ok(Some((merged_balance, normalized_snapshot))) => {
                    break (merged_balance, normalized_snapshot);
                }
                Ok(None) => {
                    return Ok(connection::BalanceSyncResult {
                        pokes: Vec::new(),
                        normalized_snapshot: None,
                    });
                }
                Err(
                    NormalizeSnapshotError::Snapshot(SnapshotConsistencyError::HeightDrift)
                    | NormalizeSnapshotError::Snapshot(SnapshotConsistencyError::BlockIdDrift),
                ) if attempt <= SNAPSHOT_DRIFT_MAX_RETRIES => {
                    continue;
                }
                Err(err) => {
                    return Err(NockAppError::OtherError(format!(
                        "Failed to normalize fetched wallet balance pages into one snapshot: {}",
                        err
                    )));
                }
            }
        };

        Ok(connection::BalanceSyncResult {
            pokes: vec![Self::update_balance_grpc_poke(merged_balance)],
            normalized_snapshot: Some(normalized_snapshot),
        })
    }

    /// Fetches balances via private gRPC peek paths and wraps updates as wallet pokes.
    pub(crate) async fn update_balance_grpc_private(
        client: &mut private_nockapp::PrivateNockAppGrpcClient,
        mut pubkeys: Vec<String>,
        mut first_names: Vec<String>,
    ) -> Result<connection::BalanceSyncResult, NockAppError> {
        first_names.sort();
        first_names.dedup();
        pubkeys.sort();
        pubkeys.dedup();

        let mut request_index: i32 = 0;
        let mut results = Vec::new();

        for first_name in first_names {
            let mut slab: NounSlab<NockJammer> = NounSlab::new();

            let mut path_slab = NounSlab::<NockJammer>::new();
            let path_noun = vec!["balance-by-first-name".to_string(), first_name.clone()]
                .to_noun(&mut path_slab);
            path_slab.set_root(path_noun);
            let path_bytes = path_slab.jam().to_vec();

            let response = client.peek(request_index, path_bytes).await.map_err(|e| {
                NockAppError::OtherError(format!(
                    "Failed to peek balance for first name {first_name}: {e}"
                ))
            })?;
            request_index = request_index.wrapping_add(1);

            let balance = slab.cue_into(response.as_bytes()?)?;
            let payload: Option<Option<v1::BalanceUpdate>> =
                <Option<Option<v1::BalanceUpdate>>>::from_noun(&balance)?;
            results.push(Self::update_balance_grpc_poke_from_payload(payload));
        }

        for key in pubkeys {
            let mut slab: NounSlab<NockJammer> = NounSlab::new();
            let mut path_slab = NounSlab::<NockJammer>::new();
            let path_noun =
                vec!["balance-by-pubkey".to_string(), key.clone()].to_noun(&mut path_slab);
            path_slab.set_root(path_noun);
            let path_bytes = path_slab.jam().to_vec();

            let response = client.peek(request_index, path_bytes).await.map_err(|e| {
                NockAppError::OtherError(format!("Failed to peek balance for pubkey {key}: {e}"))
            })?;
            request_index = request_index.wrapping_add(1);

            let balance = slab.cue_into(response.as_bytes()?)?;
            let payload: Option<Option<v1::BalanceUpdate>> =
                <Option<Option<v1::BalanceUpdate>>>::from_noun(&balance)?;
            results.push(Self::update_balance_grpc_poke_from_payload(payload));
        }

        Ok(connection::BalanceSyncResult {
            pokes: results,
            normalized_snapshot: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_math::crypto::cheetah::A_GEN;
    use nockchain_types::tx_engine::common::{BlockHeight, Nicks, SchnorrPubkey};
    use nockchain_types::tx_engine::v0::Lock as V0Lock;
    use wallet_tx_builder::note_data::DecodedNoteData;
    use wallet_tx_builder::types::{
        CandidateIdentity, CandidateV0Note, CandidateV1Note, CandidateVersionPolicy,
    };

    use super::*;

    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    fn name(first: u64, last: u64) -> Name {
        Name::new(hash(first), hash(last))
    }

    fn candidate_v0(first: u64, last: u64) -> CandidateNote {
        CandidateNote::V0(CandidateV0Note {
            identity: CandidateIdentity {
                name: name(first, last),
                origin_page: BlockHeight(Belt(1)),
            },
            assets: Nicks(1),
            lock: V0Lock {
                keys_required: 1,
                pubkeys: vec![SchnorrPubkey(A_GEN)],
            },
            timelock: None,
        })
    }

    fn candidate_v1(first: u64, last: u64) -> CandidateNote {
        CandidateNote::V1(CandidateV1Note {
            identity: CandidateIdentity {
                name: name(first, last),
                origin_page: BlockHeight(Belt(1)),
            },
            assets: Nicks(1),
            raw_note_data: Vec::new(),
            decoded_note_data: DecodedNoteData(Vec::new()),
        })
    }

    #[test]
    fn manual_candidate_version_policy_returns_v0_only_for_all_v0_manual_sets() {
        let note_names = vec![name(1, 10), name(2, 20)];
        let candidates = vec![candidate_v0(1, 10), candidate_v0(2, 20), candidate_v1(3, 30)];

        let policy =
            Wallet::manual_candidate_version_policy(&note_names, &candidates).expect("policy");

        assert_eq!(policy, CandidateVersionPolicy::V0Only);
    }

    #[test]
    fn manual_candidate_version_policy_returns_v1_only_for_all_v1_manual_sets() {
        let note_names = vec![name(3, 30)];
        let candidates = vec![candidate_v0(1, 10), candidate_v1(3, 30)];

        let policy =
            Wallet::manual_candidate_version_policy(&note_names, &candidates).expect("policy");

        assert_eq!(policy, CandidateVersionPolicy::V1Only);
    }

    #[test]
    fn manual_candidate_version_policy_rejects_mixed_manual_sets() {
        let note_names = vec![name(1, 10), name(3, 30)];
        let candidates = vec![candidate_v0(1, 10), candidate_v1(3, 30)];

        let err = Wallet::manual_candidate_version_policy(&note_names, &candidates)
            .expect_err("mixed version note set should error");

        assert_eq!(
            err,
            "manual create-tx cannot mix v0 and v1 notes; select notes from only one version"
        );
    }

    #[test]
    fn manual_candidate_version_policy_rejects_missing_manual_notes() {
        let missing = name(9, 90);
        let note_names = vec![missing.clone()];
        let candidates = vec![candidate_v0(1, 10), candidate_v1(3, 30)];

        let err = Wallet::manual_candidate_version_policy(&note_names, &candidates)
            .expect_err("missing note should error");

        assert_eq!(
            err,
            format!(
                "manual mode references unknown note {}/{}",
                missing.first.to_base58(),
                missing.last.to_base58()
            )
        );
    }
}
