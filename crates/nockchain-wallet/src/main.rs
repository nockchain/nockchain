#![allow(clippy::doc_overindented_list_items)]

mod command;
mod connection;
mod error;
mod recipient;

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::Parser;
#[cfg(test)]
use command::TimelockRangeCli;
#[cfg(test)]
use command::WalletWire;
use command::{ClientType, CommandNoun, Commands, WalletCli, WatchSubcommand};
use kernels::wallet::KERNEL;
use nockapp::driver::*;
use nockapp::kernel::boot::{self, NockStackSize};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::utils::bytes::Byts;
use nockapp::utils::make_tas;
use nockapp::wire::{SystemWire, Wire};
use nockapp::{
    exit_driver, file_driver, markdown_driver, one_punch_driver, system_data_dir, CrownError,
    NockApp, NockAppError, ToBytesExt,
};
use nockapp_grpc::pb::common::v1::Base58Hash as PbBase58Hash;
use nockapp_grpc::pb::public::v2::transaction_accepted_response;
use nockapp_grpc::{private_nockapp, public_nockchain};
use nockchain_types::common::{Hash, SchnorrPubkey, TimelockRangeAbsolute, TimelockRangeRelative};
use nockchain_types::{v0, v1};
use nockvm::jets::cold::Nounable;
use nockvm::noun::{Atom, Cell, IndirectAtom, Noun, D, NO, SIG, T, YES};
use noun_serde::prelude::*;
use noun_serde::NounDecodeError;
use recipient::{recipient_tokens_to_specs, RecipientSpec};
use termimad::MadSkin;
use tokio::fs as tokio_fs;
use tracing::{error, info, warn};
use zkvm_jetpack::hot::produce_prover_hot_state;

use crate::public_nockchain::v2::client::BalanceRequest;

#[tokio::main]
async fn main() -> Result<(), NockAppError> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("default provider already set elsewhere");

    let mut cli = WalletCli::parse();
    // Use a smaller stack size for the wallet
    cli.boot.stack_size = NockStackSize::Tiny;
    boot::init_default_tracing(&cli.boot.clone()); // Init tracing early

    if let Commands::TxAccepted { tx_id } = &cli.command {
        return run_transaction_accepted(&cli.connection, tx_id).await;
    }

    let prover_hot_state = produce_prover_hot_state();
    let data_dir = wallet_data_dir().await?;

    let kernel = boot::setup(
        KERNEL,
        cli.boot.clone(),
        prover_hot_state.as_slice(),
        "wallet",
        Some(data_dir),
    )
    .await
    .map_err(|e| CrownError::Unknown(format!("Kernel setup failed: {}", e)))?;

    let mut wallet = Wallet::new(kernel);

    // Determine if this command requires chain synchronization

    let requires_sync = match &cli.command {
        // Commands that DON'T need syncing either because they don't sync
        // or they don't interact with the chain
        Commands::Keygen
        | Commands::DeriveChild { .. }
        | Commands::ImportKeys { .. }
        | Commands::ExportKeys
        | Commands::SignMessage { .. }
        | Commands::VerifyMessage { .. }
        | Commands::SignHash { .. }
        | Commands::VerifyHash { .. }
        | Commands::ExportMasterPubkey
        | Commands::ImportMasterPubkey { .. }
        | Commands::ListActiveAddresses
        | Commands::SetActiveMasterAddress { .. }
        | Commands::ListMasterAddresses
        | Commands::ShowSeedphrase
        | Commands::ShowMasterZPub
        | Commands::ShowMasterZPrv
        | Commands::ShowKeyTree { .. }
        | Commands::ShowTx { .. }
        | Commands::SignMultisigTx { .. }
        | Commands::Watch { .. }
        | Commands::TxAccepted { .. } => false,

        // All other commands DO need sync
        _ => true,
    };

    let poke = match &cli.command {
        Commands::Keygen => {
            let mut entropy = [0u8; 32];
            let mut salt = [0u8; 16];
            getrandom::fill(&mut entropy).map_err(|e| CrownError::Unknown(e.to_string()))?;
            getrandom::fill(&mut salt).map_err(|e| CrownError::Unknown(e.to_string()))?;
            Wallet::keygen(&entropy, &salt)
        }
        Commands::DeriveChild {
            index,
            hardened,
            label,
        } => Wallet::derive_child(*index, *hardened, label),
        Commands::SignMessage {
            message,
            message_file,
            message_pos,
            index,
            hardened,
        } => {
            let bytes = if let Some(m) = message.clone().or(message_pos.clone()) {
                m.as_bytes().to_vec()
            } else if let Some(path) = message_file {
                fs::read(path).map_err(|e| {
                    CrownError::Unknown(format!("Failed to read message file: {}", e))
                })?
            } else {
                return Err(CrownError::Unknown(
                    "either --message or --message-file must be provided".into(),
                )
                .into());
            };
            Wallet::sign_message(&bytes, *index, *hardened)
        }
        Commands::SignHash {
            hash_b58,
            index,
            hardened,
        } => Wallet::sign_hash(hash_b58, *index, *hardened),
        Commands::VerifyMessage {
            message,
            message_file,
            message_pos,
            signature_path,
            signature_pos,
            pubkey,
            pubkey_pos,
        } => {
            let msg_bytes = if let Some(m) = message.clone().or(message_pos.clone()) {
                m.as_bytes().to_vec()
            } else if let Some(path) = message_file {
                fs::read(path).map_err(|e| {
                    CrownError::Unknown(format!("Failed to read message file: {}", e))
                })?
            } else {
                return Err(CrownError::Unknown(
                    "either --message or --message-file must be provided".into(),
                )
                .into());
            };
            let sig_path = signature_path
                .clone()
                .or(signature_pos.clone())
                .ok_or_else(|| {
                    NockAppError::from(CrownError::Unknown(
                        "--signature or SIGNATURE_FILE positional is required".into(),
                    ))
                })?;
            let pk_b58 = pubkey.clone().or(pubkey_pos.clone()).ok_or_else(|| {
                NockAppError::from(CrownError::Unknown(
                    "--pubkey or PUBKEY positional is required".into(),
                ))
            })?;

            let sig_bytes = fs::read(sig_path)
                .map_err(|e| CrownError::Unknown(format!("Failed to read signature: {}", e)))?;
            Wallet::verify_message(&msg_bytes, &sig_bytes, &pk_b58)
        }
        Commands::VerifyHash {
            hash_b58,
            signature_path,
            signature_pos,
            pubkey,
            pubkey_pos,
        } => {
            let sig_path = signature_path
                .clone()
                .or(signature_pos.clone())
                .ok_or_else(|| {
                    NockAppError::from(CrownError::Unknown(
                        "--signature or SIGNATURE_FILE positional is required".into(),
                    ))
                })?;
            let pk_b58 = pubkey.clone().or(pubkey_pos.clone()).ok_or_else(|| {
                NockAppError::from(CrownError::Unknown(
                    "--pubkey or PUBKEY positional is required".into(),
                ))
            })?;
            let sig_bytes = fs::read(sig_path)
                .map_err(|e| CrownError::Unknown(format!("Failed to read signature: {}", e)))?;
            Wallet::verify_hash(hash_b58, &sig_bytes, &pk_b58)
        }
        Commands::ImportKeys {
            file,
            key,
            seedphrase,
            version,
        } => {
            if let Some(file_path) = file {
                Wallet::import_keys(file_path)
            } else if let Some(extended_key) = key {
                Wallet::import_extended(extended_key)
            } else if let Some(seed) = seedphrase {
                let version = version.ok_or_else(|| {
                    NockAppError::from(CrownError::Unknown(
                        "--version is required when using --seedphrase".into(),
                    ))
                })?;
                // normalize seedphrase to have exactly one space between words
                let normalized_seed = seed.split_whitespace().collect::<Vec<&str>>().join(" ");
                Wallet::import_seed_phrase(&normalized_seed, version)
            } else {
                return Err(CrownError::Unknown(
                    "One of --file, --key, --seedphrase, or --master-privkey must be provided for import-keys".to_string(),
                )
                .into());
            }
        }
        Commands::Watch { subcommand } => match subcommand {
            WatchSubcommand::Address { address } => match normalize_watch_address(address.clone())?
            {
                Some(normalized) => Wallet::watch_address(&normalized),
                None => {
                    return Err(
                        CrownError::Unknown("Invalid watch identifier provided".into()).into(),
                    );
                }
            },
            WatchSubcommand::Pubkey { pubkey } => match normalize_watch_address(pubkey.clone())? {
                Some(normalized) => Wallet::watch_address(&normalized),
                None => {
                    return Err(CrownError::Unknown("Invalid pubkey provided".into()).into());
                }
            },
            //WatchSubcommand::FirstName { first_name } => {
            //    match normalize_first_name(first_name.clone())? {
            //        Some(name) => Wallet::watch_first_name(&name),
            //        None => {
            //            return Err(
            //                CrownError::Unknown("Invalid first name provided".into()).into()
            //            );
            //        }
            //    }
            //}
            WatchSubcommand::Multisig {
                threshold,
                participants,
            } => Wallet::watch_multisig(*threshold, participants),
        },
        Commands::ExportKeys => Wallet::export_keys(),
        Commands::ListNotes => Wallet::list_notes(),
        Commands::ListNotesByAddress { address } => {
            if let Some(pk) = address {
                Wallet::list_notes_by_address(pk)
            } else {
                return Err(CrownError::Unknown("Address is required".into()).into());
            }
        }
        Commands::ListNotesByAddressCsv { address } => Wallet::list_notes_by_address_csv(address),
        Commands::CreateTx {
            names,
            recipients,
            fee,
            refund_pkh,
            index,
            hardened,
            include_data,
            sign_keys,
            save_raw_tx,
        } => {
            let recipient_specs = recipient_tokens_to_specs(recipients.clone())?;
            let signing_keys = Wallet::collect_signing_keys(*index, *hardened, sign_keys)?;
            Wallet::create_tx(
                names.clone(),
                recipient_specs,
                *fee,
                refund_pkh.clone(),
                signing_keys,
                *include_data,
                *save_raw_tx,
            )
        }
        Commands::SignMultisigTx {
            transaction,
            sign_keys,
        } => Wallet::sign_multisig_tx(transaction, sign_keys.as_deref()),
        Commands::SendTx { transaction } => Wallet::send_tx(transaction),
        Commands::ShowTx { transaction } => Wallet::show_tx(transaction),
        Commands::ShowBalance => Wallet::show_balance(),
        Commands::ExportMasterPubkey => Wallet::export_master_pubkey(),
        Commands::ImportMasterPubkey { key_path } => Wallet::import_master_pubkey(key_path),
        Commands::ListActiveAddresses => Wallet::list_active_addresses(),
        Commands::SetActiveMasterAddress { address_b58 } => {
            Wallet::set_active_master_address(address_b58)
        }
        Commands::ListMasterAddresses => Wallet::list_master_addresses(),
        Commands::ShowSeedphrase => Wallet::show_seed_phrase(),
        Commands::ShowMasterZPub => Wallet::show_master_pubkey(),
        Commands::ShowMasterZPrv => Wallet::show_master_privkey(),
        Commands::ShowKeyTree { include_values } => Wallet::show_key_tree(*include_values),
        Commands::TxAccepted { .. } => {
            unreachable!("transaction-accepted handled earlier")
        }
    }?;

    // If this command requires sync, update the balance using a synchronous poke
    if requires_sync {
        info!(
            "Command requires syncing the current balance, connecting to Nockchain gRPC server..."
        );
        let mut pubkey_peek_slab = NounSlab::new();
        let tracked_tag = make_tas(&mut pubkey_peek_slab, "tracked-pubkeys").as_noun();
        let path = T(&mut pubkey_peek_slab, &[tracked_tag, SIG]);
        pubkey_peek_slab.set_root(path);
        let pubkey_slab = wallet.app.peek_handle(pubkey_peek_slab).await?;

        let mut first_name_peek_slab = NounSlab::new();
        let tracked_tag = make_tas(&mut first_name_peek_slab, "tracked-names").as_noun();
        let path = T(&mut first_name_peek_slab, &[tracked_tag, SIG]);
        first_name_peek_slab.set_root(path);
        let first_name_slab = wallet.app.peek_handle(first_name_peek_slab).await?;

        let pubkeys = if let Some(pubkey_slab) = pubkey_slab {
            pubkey_slab
                .to_vec()
                .iter()
                .map(|key| String::from_noun(unsafe { key.root() }))
                .collect::<Result<Vec<String>, NounDecodeError>>()?
                .into_iter()
                .filter_map(|value| match normalize_watch_address(value) {
                    Ok(Some(normalized)) => Some(Ok(normalized)),
                    Ok(None) => None,
                    Err(err) => Some(Err(err)),
                })
                .collect::<Result<Vec<String>, NockAppError>>()?
        } else {
            Vec::new()
        };

        let first_names: Vec<String> = if let Some(name_slab) = first_name_slab {
            let names_noun = unsafe { name_slab.root() };
            <Vec<String>>::from_noun(names_noun)?
        } else {
            Vec::new()
        };

        let connection_target = cli.connection.target();
        let pokes =
            connection::sync_wallet_balance(&mut wallet, &connection_target, pubkeys, first_names)
                .await?;

        for poke in pokes {
            let _ = wallet.app.poke(SystemWire.to_wire(), poke).await.unwrap();
        }
    }

    wallet
        .app
        .add_io_driver(one_punch_driver(poke.0, poke.1))
        .await;
    wallet.app.add_io_driver(file_driver()).await;
    wallet.app.add_io_driver(markdown_driver()).await;
    wallet.app.add_io_driver(exit_driver()).await;

    match wallet.app.run().await {
        Ok(_) => {
            info!("Command executed successfully");
            Ok(())
        }
        Err(e) => {
            error!("Command failed: {}", e);
            Err(e)
        }
    }
}

#[allow(dead_code)]
fn validate_label(s: &str) -> Result<String, String> {
    if s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        Ok(s.to_string())
    } else {
        Err("Label must contain only lowercase letters, numbers, and hyphens".to_string())
    }
}

pub struct Wallet {
    app: NockApp,
}

impl Wallet {
    /// Creates a new `Wallet` instance with the given kernel.
    ///
    /// This wraps the kernel in a NockApp, which exposes a substrate
    /// for kernel interaction with IO driver semantics.
    ///
    /// # Arguments
    ///
    /// * `kernel` - The kernel to initialize the wallet with.
    ///
    /// # Returns
    ///
    /// A new `Wallet` instance with the kernel initialized
    /// as a NockApp.
    fn new(nockapp: NockApp) -> Self {
        Wallet { app: nockapp }
    }

    /// Prepares a wallet command for execution.
    ///
    /// # Arguments
    ///
    /// * `command` - The command to execute.
    /// * `args` - The arguments for the command.
    /// * `operation` - The operation type (Poke or Peek).
    /// * `slab` - The NounSlab to use for the command.
    ///
    /// # Returns
    ///
    /// A `CommandNoun` containing the prepared NounSlab and operation.
    fn wallet(
        command: &str,
        args: &[Noun],
        operation: Operation,
        slab: &mut NounSlab,
    ) -> CommandNoun<NounSlab> {
        let head = make_tas(slab, command).as_noun();

        let tail = match args.len() {
            0 => D(0),
            1 => args[0],
            _ => T(slab, args),
        };

        let full = T(slab, &[head, tail]);

        slab.set_root(full);
        Ok((slab.clone(), operation))
    }

    /// Generates a new key pair. Will be a version 0 key until the wallet supports v1 transactions
    ///
    /// # Arguments
    ///
    /// * `entropy` - The entropy to use for key generation.
    /// * `sal` - The salt to use for key generation.
    fn keygen(entropy: &[u8; 32], sal: &[u8; 16]) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let ent: Byts = Byts::new(entropy.to_vec());
        let ent_noun = ent.into_noun(&mut slab);
        let sal: Byts = Byts::new(sal.to_vec());
        let sal_noun = sal.into_noun(&mut slab);
        Self::wallet("keygen", &[ent_noun, sal_noun], Operation::Poke, &mut slab)
    }

    ///// Updates the keys in the wallet.
    /////
    ///// # Arguments
    /////
    ///// * `entropy` - The entropy to use for key generation.
    ///// * `salt` - The salt to use for key generation.
    //fn upgrade_keys(entropy: &[u8; 32], salt: &[u8; 16]) -> CommandNoun<NounSlab> {
    //    let mut slab = NounSlab::new();
    //    let ent: Byts = Byts::new(entropy.to_vec());
    //    let ent_noun = ent.into_noun(&mut slab);
    //    let sal: Byts = Byts::new(salt.to_vec());
    //    let sal_noun = sal.into_noun(&mut slab);
    //    Self::wallet(
    //        "upgrade-keys-v2",
    //        &[ent_noun, sal_noun],
    //        Operation::Poke,
    //        &mut slab,
    //    )
    //}

    // Derives a child key from current master key.
    //
    // # Arguments
    //
    // * `index` - The index of the child key to derive
    // * `hardened` - Whether the child key should be hardened
    // * `label` - Optional label for the child key
    fn derive_child(index: u64, hardened: bool, label: &Option<String>) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let index_noun = D(index);
        let hardened_noun = if hardened { YES } else { NO };
        let label_noun = label.as_ref().map_or(SIG, |l| {
            let label_noun = l.into_noun(&mut slab);
            T(&mut slab, &[SIG, label_noun])
        });

        Self::wallet(
            "derive-child",
            &[index_noun, hardened_noun, label_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Signs a transaction.
    ///
    /// # Arguments
    ///
    /// * `transaction_path` - Path to the transaction file
    /// * `index` - Optional index of the key to use for signing
    fn sign_tx(
        transaction_path: &str,
        index: Option<u64>,
        hardened: bool,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        // Validate index is within range (though clap should prevent this)
        if let Some(idx) = index {
            if idx >= 2 << 31 {
                return Err(
                    CrownError::Unknown("Key index must not exceed 2^31 - 1".into()).into(),
                );
            }
        }

        // Read and decode the input bundle
        let transaction_data = fs::read(transaction_path)
            .map_err(|e| CrownError::Unknown(format!("Failed to read transaction: {}", e)))?;

        // Convert the bundle data into a noun using cue
        let transaction_noun = slab
            .cue_into(transaction_data.as_bytes()?)
            .map_err(|e| CrownError::Unknown(format!("Failed to decode transaction: {}", e)))?;

        // Format information about signing key
        let sign_key_noun = match index {
            Some(i) => {
                let inner = D(i);
                let hardened_noun = if hardened { YES } else { NO };
                T(&mut slab, &[D(0), inner, hardened_noun])
            }
            None => SIG,
        };

        // Generate random entropy
        let mut entropy_bytes = [0u8; 32];
        getrandom::fill(&mut entropy_bytes).map_err(|e| CrownError::Unknown(e.to_string()))?;
        let entropy = from_bytes(&mut slab, &entropy_bytes).as_noun();

        Self::wallet(
            "sign-tx",
            &[transaction_noun, sign_key_noun, entropy],
            Operation::Poke,
            &mut slab,
        )
    }

    fn sign_message(
        message_bytes: &[u8],
        index: Option<u64>,
        hardened: bool,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        if let Some(idx) = index {
            if idx >= 2 << 31 {
                return Err(
                    CrownError::Unknown("Key index must not exceed 2^31 - 1".into()).into(),
                );
            }
        }

        let msg_atom = from_bytes(&mut slab, message_bytes).as_noun();

        let sign_key_noun = match index {
            Some(i) => {
                let inner = D(i);
                let hardened_noun = if hardened { YES } else { NO };
                T(&mut slab, &[D(0), inner, hardened_noun])
            }
            None => SIG,
        };

        Self::wallet(
            "sign-message",
            &[msg_atom, sign_key_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    fn verify_message(
        message_bytes: &[u8],
        signature_jam: &[u8],
        pubkey_b58: &str,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let msg_atom = from_bytes(&mut slab, message_bytes).as_noun();
        let sig_atom = from_bytes(&mut slab, signature_jam).as_noun();
        let pk_noun = make_tas(&mut slab, pubkey_b58).as_noun();

        Self::wallet(
            "verify-message",
            &[msg_atom, sig_atom, pk_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    fn sign_hash(hash_b58: &str, index: Option<u64>, hardened: bool) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        if let Some(idx) = index {
            if idx >= 2 << 31 {
                return Err(
                    CrownError::Unknown("Key index must not exceed 2^31 - 1".into()).into(),
                );
            }
        }

        let hash_noun = make_tas(&mut slab, hash_b58).as_noun();
        let sign_key_noun = match index {
            Some(i) => {
                let inner = D(i);
                let hardened_noun = if hardened { YES } else { NO };
                T(&mut slab, &[D(0), inner, hardened_noun])
            }
            None => SIG,
        };

        Self::wallet(
            "sign-hash",
            &[hash_noun, sign_key_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    fn verify_hash(
        hash_b58: &str,
        signature_jam: &[u8],
        pubkey_b58: &str,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let hash_noun = make_tas(&mut slab, hash_b58).as_noun();
        let sig_atom = from_bytes(&mut slab, signature_jam).as_noun();
        let pk_noun = make_tas(&mut slab, pubkey_b58).as_noun();

        Self::wallet(
            "verify-hash",
            &[hash_noun, sig_atom, pk_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Imports keys from a seed phrase.
    ///
    /// # Arguments
    ///
    /// * `seed_phrase` - The seed phrase to generate the master private key from.
    /// * `version` - The version tag to attach to the generated master key.
    fn import_seed_phrase(seed_phrase: &str, version: u64) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let seed_phrase_noun = make_tas(&mut slab, seed_phrase).as_noun();
        let version_noun = D(version);
        Self::wallet(
            "import-seed-phrase",
            &[seed_phrase_noun, version_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Imports keys.
    ///
    /// # Arguments
    ///
    /// * `input_path` - Path to jammed keys file
    fn import_keys(input_path: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        let key_data = fs::read(input_path)
            .map_err(|e| CrownError::Unknown(format!("Failed to read master pubkeys: {}", e)))?;

        let pubkey_noun = slab
            .cue_into(key_data.as_bytes()?)
            .map_err(|e| CrownError::Unknown(format!("Failed to decode master pubkeys: {}", e)))?;

        Self::wallet("import-keys", &[pubkey_noun], Operation::Poke, &mut slab)
    }

    /// Imports an extended key.
    ///
    /// # Arguments
    ///
    /// * `extended_key` - Extended key string (e.g., "zprv..." or "zpub...")
    fn import_extended(extended_key: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let key_noun = make_tas(&mut slab, extended_key).as_noun();
        Self::wallet("import-extended", &[key_noun], Operation::Poke, &mut slab)
    }

    /// Imports a watch-only public key.
    ///
    /// # Arguments
    ///
    /// * `watch_address` - Watch-only b58 encoded address. Can be v1 or v0.
    fn watch_address(watch_address: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let address_noun = make_tas(&mut slab, watch_address).as_noun();
        Self::wallet("watch-address", &[address_noun], Operation::Poke, &mut slab)
    }

    /// Imports a watch-only first name.
    ///
    /// # Arguments
    ///
    /// * `first_name` - Base58-encoded first name hash.
    fn watch_first_name(first_name: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let first_name_noun = make_tas(&mut slab, first_name).as_noun();
        let lock_noun = SIG; // unit: no known lock provided
        Self::wallet(
            "watch-first-name",
            &[first_name_noun, lock_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Imports a watch-only multisig lock by its parameters.
    ///
    /// # Arguments
    ///
    /// * `m` - The M value of the multisig.
    /// * `pubkeys_str` - Comma-separated list of base58 pubkey hashes.
    fn watch_multisig(m: u64, pubkeys_str: &str) -> CommandNoun<NounSlab> {
        if m == 0 {
            return Err(
                CrownError::Unknown("m must be greater than 0 for multisig watch".into()).into(),
            );
        }

        let pubkey_hashes = Self::parse_pubkey_hashes(pubkeys_str)?;

        if m as usize > pubkey_hashes.len() {
            return Err(CrownError::Unknown(format!(
                "m ({}) cannot exceed number of pubkeys ({})",
                m,
                pubkey_hashes.len()
            ))
            .into());
        }

        let mut slab = NounSlab::new();
        let m_noun = D(m);
        let pubkeys_noun = pubkey_hashes.into_iter().rev().fold(D(0), |acc, hash| {
            let hash_b58 = hash.to_base58();
            let hash_noun = make_tas(&mut slab, &hash_b58).as_noun();
            Cell::new(&mut slab, hash_noun, acc).as_noun()
        });

        Self::wallet(
            "watch-address-multisig",
            &[m_noun, pubkeys_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Exports keys to a file.
    fn export_keys() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("export-keys", &[], Operation::Poke, &mut slab)
    }

    #[allow(dead_code)]
    fn timelock_intent_from_ranges(
        absolute: Option<TimelockRangeAbsolute>,
        relative: Option<TimelockRangeRelative>,
    ) -> Option<v0::TimelockIntent> {
        if absolute.is_none() && relative.is_none() {
            None
        } else {
            Some(v0::TimelockIntent {
                absolute: absolute.unwrap_or_else(TimelockRangeAbsolute::none),
                relative: relative.unwrap_or_else(TimelockRangeRelative::none),
            })
        }
    }

    fn parse_note_names(raw: &str) -> Result<Vec<(String, String)>, NockAppError> {
        let mut names = Vec::new();

        for piece in raw.split(',') {
            let trimmed = piece.trim();
            if trimmed.is_empty() {
                continue;
            }

            if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
                return Err(CrownError::Unknown(format!(
                    "Invalid note name '{}', expected [first last]",
                    trimmed
                ))
                .into());
            }

            let inner = &trimmed[1..trimmed.len() - 1];
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.len() != 2 {
                return Err(CrownError::Unknown(format!(
                    "Invalid note name '{}', expected exactly two components",
                    trimmed
                ))
                .into());
            }

            let first = parts[0].to_string();
            let last = parts[1].to_string();
            names.push((first, last));
        }

        if names.is_empty() {
            return Err(
                CrownError::Unknown("At least one note name must be provided".to_string()).into(),
            );
        }

        Ok(names)
    }

    fn collect_signing_keys(
        index: Option<u64>,
        hardened: bool,
        sign_keys: &[String],
    ) -> Result<Vec<(u64, bool)>, NockAppError> {
        if !sign_keys.is_empty() {
            sign_keys
                .iter()
                .map(|entry| Self::parse_sign_key_entry(entry))
                .collect()
        } else if let Some(idx) = index {
            Ok(vec![(idx, hardened)])
        } else {
            Ok(Vec::new())
        }
    }

    fn parse_sign_key_entry(entry: &str) -> Result<(u64, bool), NockAppError> {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Err(CrownError::Unknown("Sign key entries cannot be empty".to_string()).into());
        }

        let (index_part, hardened_part) = trimmed
            .split_once(':')
            .map(|(index, hardened)| (index, Some(hardened)))
            .unwrap_or((trimmed, None));
        Self::parse_sign_key_components(index_part, hardened_part)
    }

    /// Creates a transaction. Use `--refund-pkh` when spending legacy v0 notes so the kernel
    /// knows where to return change. When spending v1 notes the refund automatically
    /// defaults back to the note owner, so `--refund-pkh` can be omitted.
    fn create_tx(
        names: String,
        recipients: Vec<RecipientSpec>,
        fee: u64,
        refund_pkh: Option<String>,
        sign_keys: Vec<(u64, bool)>,
        include_data: bool,
        save_raw_tx: bool,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        let names_vec = Self::parse_note_names(&names)?;
        let names_noun = names_vec
            .into_iter()
            .rev()
            .fold(D(0), |acc, (first, last)| {
                let first_noun = make_tas(&mut slab, &first).as_noun();
                let last_noun = make_tas(&mut slab, &last).as_noun();
                let name_pair = T(&mut slab, &[first_noun, last_noun]);
                Cell::new(&mut slab, name_pair, acc).as_noun()
            });

        let fee_noun = D(fee);
        let order_noun = recipients.to_noun(&mut slab);
        let sign_key_noun = Wallet::encode_sign_keys(&mut slab, sign_keys);

        let refund_noun = if let Some(refund) = refund_pkh {
            let refund_hash = Hash::from_base58(&refund).map_err(|err| {
                NockAppError::from(CrownError::Unknown(format!(
                    "Invalid refund pubkey hash '{}': {}",
                    refund, err
                )))
            })?;
            let refund_atom = refund_hash.to_noun(&mut slab);
            T(&mut slab, &[SIG, refund_atom])
        } else {
            SIG
        };
        let include_data_noun = include_data.to_noun(&mut slab);
        let save_raw_tx_noun = save_raw_tx.to_noun(&mut slab);

        Self::wallet(
            "create-tx",
            &[
                names_noun, order_noun, fee_noun, sign_key_noun, refund_noun, include_data_noun,
                save_raw_tx_noun,
            ],
            Operation::Poke,
            &mut slab,
        )
    }

    fn encode_sign_keys(slab: &mut NounSlab, keys: Vec<(u64, bool)>) -> Noun {
        if keys.is_empty() {
            SIG
        } else {
            Some(keys).to_noun(slab)
        }
    }

    async fn update_balance_grpc_public(
        client: &mut public_nockchain::PublicNockchainGrpcClient,
        pubkeys: Vec<String>,
        first_names: Vec<String>,
    ) -> Result<Vec<NounSlab>, NockAppError> {
        let mut results = Vec::new();

        for first_name in first_names {
            let mut slab = NounSlab::new(); // Define slab - adjust as needed
            let response = client
                .wallet_get_balance(&BalanceRequest::FirstName(first_name))
                .await
                .map_err(|e| {
                    NockAppError::OtherError(format!("Failed to request current balance: {}", e))
                })?;
            let balance_update = v1::BalanceUpdate::try_from(response).map_err(|e| {
                NockAppError::OtherError(format!("Failed to parse balance update: {}", e))
            })?;
            let wrapped_balance = Some(Some(balance_update));
            let balance_noun = wrapped_balance.to_noun(&mut slab);
            let head = make_tas(&mut slab, "update-balance-grpc").as_noun();
            let full = T(&mut slab, &[head, balance_noun]);
            slab.set_root(full);
            results.push(slab);
        }

        for (_index, key) in pubkeys.iter().enumerate() {
            let mut slab = NounSlab::new(); // Define slab - adjust as needed
            let response = client
                .wallet_get_balance(&BalanceRequest::Address(key.to_owned()))
                .await
                .map_err(|e| {
                    NockAppError::OtherError(format!("Failed to request current balance: {}", e))
                })?;
            let balance_update = v1::BalanceUpdate::try_from(response).map_err(|e| {
                NockAppError::OtherError(format!("Failed to parse balance update: {}", e))
            })?;
            let wrapped_balance = Some(Some(balance_update));
            let balance_noun = wrapped_balance.to_noun(&mut slab);
            let head = make_tas(&mut slab, "update-balance-grpc").as_noun();
            let full = T(&mut slab, &[head, balance_noun]);
            slab.set_root(full);
            results.push(slab);
        }

        Ok(results)
    }

    async fn update_balance_grpc_private(
        client: &mut private_nockapp::PrivateNockAppGrpcClient,
        mut pubkeys: Vec<String>,
        mut first_names: Vec<String>,
    ) -> Result<Vec<NounSlab>, NockAppError> {
        first_names.sort();
        first_names.dedup();
        pubkeys.sort();
        pubkeys.dedup();

        let mut request_index: i32 = 0;
        let mut results = Vec::new();

        for first_name in first_names {
            let mut slab = NounSlab::new();

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
            let head = make_tas(&mut slab, "update-balance-grpc").as_noun();
            let full = T(&mut slab, &[head, balance]);
            slab.set_root(full);
            results.push(slab);
        }

        for key in pubkeys {
            let mut slab = NounSlab::new();
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
            let head = make_tas(&mut slab, "update-balance-grpc").as_noun();
            let full = T(&mut slab, &[head, balance]);
            slab.set_root(full);
            results.push(slab);
        }

        Ok(results)
    }

    /// Lists all notes in the wallet.
    ///
    /// Retrieves and displays all notes from the wallet's balance, sorted by assets.
    fn list_notes() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("list-notes", &[], Operation::Poke, &mut slab)
    }

    /// Exports the master public key.
    ///
    /// # Returns
    ///
    /// Retrieves and displays master public key and chaincode.
    fn export_master_pubkey() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("export-master-pubkey", &[], Operation::Poke, &mut slab)
    }

    /// Imports a master public key.
    ///
    /// # Arguments
    ///
    /// * `key` - Base58-encoded public key
    /// * `chain_code` - Base58-encoded chain code
    fn import_master_pubkey(input_path: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        let key_data = fs::read(input_path)
            .map_err(|e| CrownError::Unknown(format!("Failed to read master pubkeys: {}", e)))?;

        let pubkey_noun = slab
            .cue_into(key_data.as_bytes()?)
            .map_err(|e| CrownError::Unknown(format!("Failed to decode master pubkeys: {}", e)))?;

        Self::wallet(
            "import-master-pubkey",
            &[pubkey_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Creates a transaction from a transaction file.
    ///
    /// # Arguments
    ///
    /// * `transaction_path` - Path to the transaction file to create transaction from
    fn send_tx(transaction_path: &str) -> CommandNoun<NounSlab> {
        // Read and decode the transaction file
        let transaction_data = fs::read(transaction_path)
            .map_err(|e| CrownError::Unknown(format!("Failed to read transaction file: {}", e)))?;

        let mut slab = NounSlab::new();
        let transaction_noun = slab.cue_into(transaction_data.as_bytes()?).map_err(|e| {
            CrownError::Unknown(format!("Failed to decode transaction data: {}", e))
        })?;

        Self::wallet("send-tx", &[transaction_noun], Operation::Poke, &mut slab)
    }

    /// Displays a transaction file contents.
    ///
    /// # Arguments
    ///
    /// * `transaction_path` - Path to the transaction file to display
    fn show_tx(transaction_path: &str) -> CommandNoun<NounSlab> {
        // Read and decode the transaction file
        let transaction_data = fs::read(transaction_path)
            .map_err(|e| CrownError::Unknown(format!("Failed to read transaction file: {}", e)))?;

        let mut slab = NounSlab::new();
        let transaction_noun = slab.cue_into(transaction_data.as_bytes()?).map_err(|e| {
            CrownError::Unknown(format!("Failed to decode transaction data: {}", e))
        })?;

        Self::wallet("show-tx", &[transaction_noun], Operation::Poke, &mut slab)
    }

    /// Lists all addresses nested under the active master address.
    fn list_active_addresses() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("list-active-addresses", &[], Operation::Poke, &mut slab)
    }

    /// Sets the active master address.
    fn set_active_master_address(address_b58: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let address_noun = make_tas(&mut slab, address_b58).as_noun();
        Self::wallet(
            "set-active-master-address",
            &[address_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Lists known master addresses.
    fn list_master_addresses() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("list-master-addresses", &[], Operation::Poke, &mut slab)
    }

    /// Lists notes by public key
    fn list_notes_by_address(pubkey: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let pubkey_noun = make_tas(&mut slab, pubkey).as_noun();
        Self::wallet(
            "list-notes-by-address",
            &[pubkey_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Lists notes by public key in CSV format
    fn list_notes_by_address_csv(pubkey: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let pubkey_noun = make_tas(&mut slab, pubkey).as_noun();
        Self::wallet(
            "list-notes-by-address-csv",
            &[pubkey_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Shows the aggregate wallet balance summary.
    fn show_balance() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        let balance_tag = make_tas(&mut slab, "balance").as_noun();
        let path_noun = Cell::new(&mut slab, balance_tag, D(0)).as_noun();

        Self::wallet("show", &[path_noun], Operation::Poke, &mut slab)
    }

    /// Shows the seed phrase for the current master key.
    fn show_seed_phrase() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("show-seed-phrase", &[], Operation::Poke, &mut slab)
    }

    /// Shows the master public key.
    fn show_master_pubkey() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("show-master-zpub", &[], Operation::Poke, &mut slab)
    }

    /// Shows the master private key.
    fn show_master_privkey() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("show-master-zprv", &[], Operation::Poke, &mut slab)
    }

    /// Shows the key tree structure.
    fn show_key_tree(include_values: bool) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let include_values_noun = if include_values { YES } else { NO };
        Self::wallet(
            "show-key-tree",
            &[include_values_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    fn parse_sign_key_components(
        index_str: &str,
        hardened_str: Option<&str>,
    ) -> Result<(u64, bool), NockAppError> {
        let index = index_str.trim().parse::<u64>().map_err(|err| {
            CrownError::Unknown(format!("Invalid key index '{}': {}", index_str.trim(), err))
        })?;
        if index >= 2 << 31 {
            return Err(CrownError::Unknown("Key index must not exceed 2^31 - 1".into()).into());
        }
        let hardened = if let Some(flag) = hardened_str {
            Self::parse_boolish(flag)?
        } else {
            false
        };
        Ok((index, hardened))
    }

    fn parse_boolish(flag: &str) -> Result<bool, NockAppError> {
        match flag {
            "true" | "t" | "1" | "yes" | "y" => Ok(true),
            "false" | "f" | "0" | "no" | "n" => Ok(false),
            _ => Err(CrownError::Unknown(format!(
                "Invalid hardened value '{}', expected true/false",
                flag
            ))
            .into()),
        }
    }

    fn parse_sign_keys(sign_keys_str: &str) -> Result<Vec<(u64, bool)>, NockAppError> {
        let mut sign_keys = Vec::new();
        for piece in sign_keys_str.split(',') {
            let trimmed = piece.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parts: Vec<&str> = trimmed.split(':').collect();
            if parts.len() != 2 {
                return Err(CrownError::Unknown(format!(
                    "Invalid sign key '{}', expected index:hardened",
                    trimmed
                ))
                .into());
            }
            sign_keys.push(Self::parse_sign_key_components(parts[0], Some(parts[1]))?);
        }
        if sign_keys.is_empty() {
            return Err(
                CrownError::Unknown("At least one sign key must be provided".to_string()).into(),
            );
        }
        Ok(sign_keys)
    }

    fn parse_pubkey_hashes(pubkeys_str: &str) -> Result<Vec<Hash>, NockAppError> {
        let pubkeys: Vec<Hash> = pubkeys_str
            .split(',')
            .map(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Err(NockAppError::from(CrownError::Unknown(
                        "Empty pubkey hash provided in list".into(),
                    )));
                }
                Hash::from_base58(trimmed).map_err(|err| {
                    NockAppError::from(CrownError::Unknown(format!(
                        "Invalid pubkey hash '{}': {}",
                        trimmed, err
                    )))
                })
            })
            .collect::<Result<Vec<Hash>, NockAppError>>()?;

        if pubkeys.is_empty() {
            return Err(
                CrownError::Unknown("At least one pubkey hash must be provided".into()).into(),
            );
        }

        Ok(pubkeys)
    }

    fn sign_multisig_tx(
        transaction_path: &str,
        sign_keys_str: Option<&str>,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        let transaction_data = fs::read(transaction_path)
            .map_err(|e| CrownError::Unknown(format!("Failed to read transaction file: {}", e)))?;

        let transaction_noun = slab.cue_into(transaction_data.as_bytes()?).map_err(|e| {
            CrownError::Unknown(format!("Failed to decode transaction data: {}", e))
        })?;

        let sign_keys_noun = if let Some(sign_keys_str) = sign_keys_str {
            let sign_keys = Self::parse_sign_keys(sign_keys_str)?;
            sign_keys
                .into_iter()
                .rev()
                .fold(D(0), |acc, (index, hardened)| {
                    let index_noun = D(index);
                    let hardened_noun = if hardened { YES } else { NO };
                    let pair = T(&mut slab, &[index_noun, hardened_noun]);
                    Cell::new(&mut slab, pair, acc).as_noun()
                })
        } else {
            SIG
        };

        Self::wallet(
            "sign-multisig-tx",
            &[transaction_noun, sign_keys_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    fn show_multisig_tx(transaction_path: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        let transaction_data = fs::read(transaction_path)
            .map_err(|e| CrownError::Unknown(format!("Failed to read transaction file: {}", e)))?;

        let transaction_noun = slab.cue_into(transaction_data.as_bytes()?).map_err(|e| {
            CrownError::Unknown(format!("Failed to decode transaction data: {}", e))
        })?;

        Self::wallet(
            "show-multisig-tx",
            &[transaction_noun],
            Operation::Poke,
            &mut slab,
        )
    }
}

pub async fn wallet_data_dir() -> Result<PathBuf, NockAppError> {
    let wallet_data_dir = system_data_dir().join("wallet");
    if !wallet_data_dir.exists() {
        tokio_fs::create_dir_all(&wallet_data_dir)
            .await
            .map_err(|e| {
                CrownError::Unknown(format!("Failed to create wallet data directory: {}", e))
            })?;
    }
    Ok(wallet_data_dir)
}

#[allow(dead_code)]
fn confirm_upper_bound_warning() -> Result<(), NockAppError> {
    println!(
        "Warning: specifying an upper timelock bound will make the output unspendable after that height. Only use this feature if you know what you're doing."
    );
    print!("Type 'YES' to continue: ");
    io::stdout()
        .flush()
        .map_err(|e| CrownError::Unknown(format!("Failed to flush stdout: {}", e)))?;
    let mut response = String::new();
    io::stdin()
        .read_line(&mut response)
        .map_err(|e| CrownError::Unknown(format!("Failed to read confirmation: {}", e)))?;

    if response.trim() == "YES" {
        Ok(())
    } else {
        Err(CrownError::Unknown(
            "Aborted create-tx because upper bound was not confirmed with YES".into(),
        )
        .into())
    }
}

fn normalize_watch_address(value: String) -> Result<Option<String>, NockAppError> {
    if value.as_bytes().len() >= SchnorrPubkey::BYTES_BASE58 {
        match SchnorrPubkey::from_base58(&value) {
            Ok(pubkey) => pubkey
                .to_base58()
                .map(Some)
                .map_err(|err| NockAppError::OtherError(err.to_string())),
            Err(err) => {
                warn!(
                    "Skipping invalid watch-only schnorr pubkey '{}': {}",
                    value, err
                );
                Ok(None)
            }
        }
    } else {
        match Hash::from_base58(&value) {
            Ok(hash) => Ok(Some(hash.to_base58())),
            Err(err) => {
                warn!("Skipping invalid watch-only hash '{}': {}", value, err);
                Ok(None)
            }
        }
    }
}

fn normalize_first_name(value: String) -> Result<Option<String>, NockAppError> {
    match Hash::from_base58(&value) {
        Ok(hash) => Ok(Some(hash.to_base58())),
        Err(err) => {
            warn!("Skipping invalid first name '{}': {}", value, err);
            Ok(None)
        }
    }
}

async fn run_transaction_accepted(
    connection: &connection::ConnectionCli,
    tx_id: &str,
) -> Result<(), NockAppError> {
    if connection.client != ClientType::Public {
        return Err(NockAppError::OtherError(
            "transaction-accepted command requires the public client (--client public)".to_string(),
        ));
    }

    let endpoint = connection.public_grpc_server_addr.to_string();
    let mut client = public_nockchain::PublicNockchainGrpcClient::connect(endpoint.clone())
        .await
        .map_err(|err| {
            NockAppError::OtherError(format!(
                "Failed to connect to public Nockchain gRPC server at {}: {}",
                endpoint, err
            ))
        })?;

    Hash::from_base58(tx_id).map_err(|_| {
        NockAppError::OtherError(format!(
            "Invalid transaction ID (expected base58-encoded hash): {}",
            tx_id
        ))
    })?;

    let request = PbBase58Hash {
        hash: tx_id.to_string(),
    };

    let response = client.transaction_accepted(request).await.map_err(|err| {
        NockAppError::OtherError(format!(
            "Transaction accepted query failed for {}: {}",
            tx_id, err
        ))
    })?;

    let accepted = match response.result {
        Some(transaction_accepted_response::Result::Accepted(value)) => value,
        Some(transaction_accepted_response::Result::Error(err)) => {
            return Err(NockAppError::OtherError(format!(
                "Transaction accepted query returned error code {}: {}",
                err.code, err.message
            )))
        }
        None => {
            return Err(NockAppError::OtherError(
                "Transaction accepted query returned an empty result".to_string(),
            ))
        }
    };

    let markdown = format_transaction_accepted_markdown(tx_id, accepted);
    let skin = MadSkin::default_dark();
    println!("{}", skin.term_text(&markdown));

    Ok(())
}

fn format_transaction_accepted_markdown(tx_id: &str, accepted: bool) -> String {
    let status_line = if accepted {
        "- status: **accepted by node**"
    } else {
        "- status: **not yet accepted**"
    };

    [
        "## Transaction Acceptance".to_string(),
        format!("- tx id: `{}`", tx_id),
        status_line.to_string(),
    ]
    .join("\n")
}

pub fn from_bytes(stack: &mut NounSlab, bytes: &[u8]) -> Atom {
    unsafe {
        let mut tas_atom = IndirectAtom::new_raw_bytes(stack, bytes.len(), bytes.as_ptr());
        tas_atom.normalize_as_atom()
    }
}

// TODO: all these tests need to also validate the results and not
// just ensure that the wallet can be poked with the expected noun.
#[allow(warnings)]
#[cfg(test)]
mod tests {
    use std::sync::Once;

    use nockapp::kernel::boot::{self, Cli as BootCli};
    use nockapp::wire::SystemWire;
    use nockapp::{exit_driver, AtomExt, Bytes};
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::{BlockHeight, BlockHeightDelta};
    use nockchain_types::tx_engine::v0;
    use tokio::sync::mpsc;

    use super::*;

    static INIT: Once = Once::new();

    fn init_tracing() {
        INIT.call_once(|| {
            let cli = boot::default_boot_cli(true);
            boot::init_default_tracing(&cli);
        });
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
    fn collect_signing_keys_prefers_explicit_entries() {
        let entries = vec!["0:true".to_string(), "1:false".to_string()];
        let keys =
            Wallet::collect_signing_keys(Some(5), false, &entries).expect("valid explicit keys");
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
}
