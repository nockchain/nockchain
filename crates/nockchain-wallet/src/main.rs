#![allow(clippy::doc_overindented_list_items)]

use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use nockapp::utils::bytes::Byts;
use nockapp::wire::SystemWire;
use nockapp::{system_data_dir, CrownError, NockApp, NockAppError, ToBytesExt};
use nockapp_grpc::client::NockAppGrpcClient;
use nockapp_grpc::driver::grpc_listener_driver;
use nockvm::jets::cold::Nounable;
use nockvm::noun::{Atom, Cell, IndirectAtom, Noun, D, NO, SIG, T, YES};
use tokio::fs as tokio_fs;
use tracing::{error, info};
use zkvm_jetpack::hot::produce_prover_hot_state;

mod error;

use kernels::wallet::KERNEL;
use nockapp::driver::*;
use nockapp::kernel::boot::{self, Cli as BootCli};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::wire::{Wire, WireRepr};
use nockapp::{exit_driver, file_driver, markdown_driver, one_punch_driver};

/// Represents a timelock range with optional min and max page numbers
#[derive(Debug, Clone)]
pub struct TimelockRange {
    pub min: Option<u64>,
    pub max: Option<u64>,
}

impl TimelockRange {
    pub fn new(min: Option<u64>, max: Option<u64>) -> Self {
        Self { min, max }
    }

    pub fn none() -> Self {
        Self {
            min: None,
            max: None,
        }
    }

    /// Convert to noun representation: [min=(unit page-number) max=(unit page-number)]
    pub fn to_noun(&self, slab: &mut NounSlab) -> Noun {
        let min_noun = match self.min {
            Some(val) => T(slab, &[D(0), D(val)]), // unit: [~ value]
            None => D(0),                          // unit: ~
        };
        let max_noun = match self.max {
            Some(val) => T(slab, &[D(0), D(val)]), // unit: [~ value]
            None => D(0),                          // unit: ~
        };
        T(slab, &[min_noun, max_noun])
    }
}

/// Represents a timelock intent - optional constraint for output notes
#[derive(Debug, Clone)]
pub struct TimelockIntent {
    pub absolute: Option<TimelockRange>,
    pub relative: Option<TimelockRange>,
}

impl TimelockIntent {
    pub fn new(absolute: Option<TimelockRange>, relative: Option<TimelockRange>) -> Self {
        Self { absolute, relative }
    }

    pub fn none() -> Self {
        Self {
            absolute: None,
            relative: None,
        }
    }

    pub fn absolute_only(range: TimelockRange) -> Self {
        Self {
            absolute: Some(range),
            relative: Some(TimelockRange::none()),
        }
    }

    pub fn relative_only(range: TimelockRange) -> Self {
        Self {
            absolute: Some(TimelockRange::none()),
            relative: Some(range),
        }
    }

    /// Convert to noun representation: (unit [absolute=timelock-range relative=timelock-range])
    pub fn to_noun(&self, slab: &mut NounSlab) -> Noun {
        match (&self.absolute, &self.relative) {
            (None, None) => D(0), // unit: ~ (no intent)
            (abs, rel) => {
                let default_abs_range = TimelockRange::none();
                let default_rel_range = TimelockRange::none();
                let abs_range = abs.as_ref().unwrap_or(&default_abs_range);
                let rel_range = rel.as_ref().unwrap_or(&default_rel_range);

                let abs_noun = abs_range.to_noun(slab);
                let rel_noun = rel_range.to_noun(slab);
                let content = T(slab, &[abs_noun, rel_noun]);
                T(slab, &[D(0), content]) // unit: [~ content]
            }
        }
    }
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct WalletCli {
    #[command(flatten)]
    boot: BootCli,

    #[command(subcommand)]
    command: Commands,

    #[arg(
        long,
        value_name = "GRPC_ADDRESS",
        default_value = "http://localhost:5555"
    )]
    pub grpc_address: String,
}

#[derive(Debug)]
pub enum WalletWire {
    ListNotes,
    UpdateBalance,
    UpdateBlock,
    Exit,
    Command(Commands),
}

impl Wire for WalletWire {
    const VERSION: u64 = 1;
    const SOURCE: &str = "wallet";

    fn to_wire(&self) -> WireRepr {
        let tags = match self {
            WalletWire::ListNotes => vec!["list-notes".into()],
            WalletWire::UpdateBalance => vec!["update-balance".into()],
            WalletWire::UpdateBlock => vec!["update-block".into()],
            WalletWire::Exit => vec!["exit".into()],
            WalletWire::Command(command) => {
                vec!["command".into(), command.as_wire_tag().into()]
            }
        };
        WireRepr::new(WalletWire::SOURCE, WalletWire::VERSION, tags)
    }
}

/// Represents a Noun that the wallet kernel can handle
type CommandNoun<T> = Result<(T, Operation), NockAppError>;

fn validate_label(s: &str) -> Result<String, String> {
    if s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        Ok(s.to_string())
    } else {
        Err("Label must contain only lowercase letters, numbers, and hyphens".to_string())
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Generate a new key pair
    Keygen,

    /// Derive child key (pub, private or both) from the current master key
    DeriveChild {
        /// Index of the child key to derive, should be in range [0, 2^31)
        #[arg(value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: u64,

        /// Hardened or unhardened child key
        #[arg(short, long)]
        hardened: bool,

        /// Label for the child key
        #[arg(short, long, value_parser = validate_label, default_value = None)]
        label: Option<String>,
    },

    /// Import keys from a file, extended key, seed phrase, or master private key
    #[command(group = clap::ArgGroup::new("import_source").required(true).args(&["file", "key", "seedphrase", "master_privkey"]))]
    ImportKeys {
        /// Path to the jammed keys file
        #[arg(short = 'f', long = "file", value_name = "FILE")]
        file: Option<String>,

        /// Extended key string (e.g., "zprv..." or "zpub...")
        #[arg(short = 'k', long = "key", value_name = "EXTENDED_KEY")]
        key: Option<String>,

        /// Seed phrase to generate master private key
        #[arg(short = 's', long = "seedphrase", value_name = "SEEDPHRASE")]
        seedphrase: Option<String>,

        /// Master private key (base58-encoded) - requires --chain-code
        #[arg(short = 'm', long = "master-privkey", value_name = "MASTER_PRIVKEY")]
        master_privkey: Option<String>,

        /// Chain code (base58-encoded) - required with --master-privkey
        #[arg(short = 'c', long = "chain-code", value_name = "CHAIN_CODE")]
        chain_code: Option<String>,
    },

    /// Export keys to a file
    ExportKeys,

    /// Perform a simple scan of the blockchain
    Scan {
        /// Master public key to scan for
        #[arg(short, long)]
        master_pubkey: String,
        /// Optional search depth (default 100)
        #[arg(short, long, default_value = "100")]
        search_depth: u64,
        /// Include timelocks in scan
        #[arg(long, default_value = "false")]
        include_timelocks: bool,
        /// Include multisig in scan
        #[arg(long, default_value = "false")]
        include_multisig: bool,
    },

    /// List all notes in the wallet
    ListNotes,

    /// List notes by public key
    ListNotesByPubkey {
        /// Optional public key to filter notes
        pubkey: Option<String>,
    },

    /// List notes by public key in CSV format
    ListNotesByPubkeyCsv {
        /// Public key to filter notes
        pubkey: String,
    },

    /// Create a transaction from a transaction file
    SendTx {
        /// Transaction file to create transaction from
        transaction: String,
    },

    /// Display a transaction file contents
    ShowTx {
        /// Transaction file to display
        transaction: String,
    },

    /// Signs a transaction (for multisigs only)
    SignTx {
        /// Path to input bundle file
        transaction: String,

        /// Optional key index to use for signing [0, 2^31)
        #[arg(short, long, value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: Option<u64>,
        /// Hardened or unhardened child key
        #[arg(short, long, default_value = "false")]
        hardened: bool,
    },

    /// Create a transaction
    CreateTx {
        /// Names of notes to spend (comma-separated)
        #[arg(long)]
        names: String,
        /// Recipient addresses (comma-separated)
        #[arg(long)]
        recipients: String,
        /// Amounts to send (comma-separated)
        #[arg(long)]
        gifts: String,
        /// Transaction fee
        #[arg(long)]
        fee: u64,
        /// Optional key index to use for signing [0, 2^31), if not provided, we use the master key
        #[arg(short, long, value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: Option<u64>,
        /// Type of timelock intent: "absolute", "relative", or "none"
        #[arg(long, default_value = "none")]
        timelock_intent: String,
        /// Minimum block height for absolute timelock or relative delay in blocks
        #[arg(long)]
        timelock_min: Option<u64>,
        /// Hardened or unhardened child key
        #[arg(short, long, default_value = "false")]
        hardened: bool,
    },

    /// Export a master public key
    ExportMasterPubkey,

    /// Import a master public key
    ImportMasterPubkey {
        // Path to keys file generated from export-master-pubkey
        key_path: String,
    },

    /// Lists all public keys in the wallet
    ListPubkeys,

    /// Show the seed phrase for the current master key
    ShowSeedphrase,

    /// Show the master public key
    ShowMasterPubkey,

    /// Show the master private key
    ShowMasterPrivkey,

    /// Sign an arbitrary message
    #[command(group = clap::ArgGroup::new("message_source").required(true).args(&["message", "message_file", "message_pos"]))]
    SignMessage {
        /// Message to sign (raw string)
        #[arg(short = 'm', long = "message", group = "message_source")]
        message: Option<String>,

        /// Path to file containing raw bytes to sign
        #[arg(short = 'f', long = "message-file", group = "message_source")]
        message_file: Option<String>,

        /// Positional message to sign (equivalent to --message)
        #[arg(value_name = "MESSAGE", group = "message_source")]
        message_pos: Option<String>,

        /// Optional key index to use for signing [0, 2^31)
        #[arg(short, long, value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: Option<u64>,
        /// Hardened or unhardened child key
        #[arg(short, long, default_value = "false")]
        hardened: bool,
    },

    /// Sign an already-computed tip5 hash (base58)
    SignHash {
        /// Positional base58-encoded tip5 hash to sign
        #[arg(value_name = "HASH")]
        hash_b58: String,

        /// Optional key index to use for signing [0, 2^31)
        #[arg(short, long, value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: Option<u64>,
        /// Hardened or unhardened child key
        #[arg(short, long, default_value = "false")]
        hardened: bool,
    },

    /// Verify an arbitrary message signature
    VerifyMessage {
        /// Message to verify (raw string)
        #[arg(short = 'm', long = "message")]
        message: Option<String>,

        /// Path to file containing raw bytes of message to verify
        #[arg(short = 'f', long = "message-file")]
        message_file: Option<String>,

        /// Positional message to verify (equivalent to --message)
        #[arg(value_name = "MESSAGE", conflicts_with_all = ["message", "message_file"])]
        message_pos: Option<String>,

        /// Path to jammed signature file produced by sign-message
        #[arg(short = 's', long = "signature")]
        signature_path: Option<String>,

        /// Positional signature path (equivalent to --signature)
        #[arg(value_name = "SIGNATURE_FILE")]
        signature_pos: Option<String>,

        /// Base58-encoded schnorr public key
        #[arg(short = 'p', long = "pubkey")]
        pubkey: Option<String>,

        /// Positional public key (equivalent to --pubkey)
        #[arg(value_name = "PUBKEY")]
        pubkey_pos: Option<String>,
    },

    /// Verify a signature against an already-computed tip5 hash (base58)
    VerifyHash {
        /// Positional base58-encoded tip5 hash
        #[arg(value_name = "HASH")]
        hash_b58: String,

        /// Path to jammed signature file produced by signing
        #[arg(short = 's', long = "signature")]
        signature_path: Option<String>,
        /// Positional signature path
        #[arg(value_name = "SIGNATURE_FILE")]
        signature_pos: Option<String>,

        /// Base58-encoded schnorr public key
        #[arg(short = 'p', long = "pubkey")]
        pubkey: Option<String>,
        /// Positional public key
        #[arg(value_name = "PUBKEY")]
        pubkey_pos: Option<String>,
    },
}

impl Commands {
    fn as_wire_tag(&self) -> &'static str {
        match self {
            Commands::Keygen => "keygen",
            Commands::DeriveChild { .. } => "derive-child",
            Commands::ImportKeys { .. } => "import-keys",
            Commands::ExportKeys => "export-keys",
            Commands::SignTx { .. } => "sign-tx",
            Commands::Scan { .. } => "scan",
            Commands::ListNotes => "list-notes",
            Commands::ListNotesByPubkey { .. } => "list-notes-by-pubkey",
            Commands::ListNotesByPubkeyCsv { .. } => "list-notes-by-pubkey-csv",
            Commands::CreateTx { .. } => "create-tx",
            Commands::SendTx { .. } => "send-tx",
            Commands::ShowTx { .. } => "show-tx",
            Commands::ExportMasterPubkey => "export-master-pubkey",
            Commands::ImportMasterPubkey { .. } => "import-master-pubkey",
            Commands::ListPubkeys => "list-pubkeys",
            Commands::ShowSeedphrase => "show-seedphrase",
            Commands::ShowMasterPubkey => "show-master-pubkey",
            Commands::ShowMasterPrivkey => "show-master-privkey",
            Commands::SignMessage { .. } => "sign-message",
            Commands::VerifyMessage { .. } => "verify-message",
            Commands::SignHash { .. } => "sign-hash",
            Commands::VerifyHash { .. } => "verify-hash",
        }
    }
}

pub struct Wallet {
    app: NockApp,
}

#[derive(Debug, Clone)]
pub enum KeyType {
    Pub,
    Prv,
}

impl KeyType {
    pub fn to_string(&self) -> &'static str {
        match self {
            KeyType::Pub => "pub",
            KeyType::Prv => "prv",
        }
    }
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

    /// Generates a new key pair.
    ///
    /// # Arguments
    ///
    /// * `entropy` - The entropy to use for key generation.
    fn keygen(entropy: &[u8; 32], sal: &[u8; 16]) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let ent: Byts = Byts::new(entropy.to_vec());
        let ent_noun = ent.into_noun(&mut slab);
        let sal: Byts = Byts::new(sal.to_vec());
        let sal_noun = sal.into_noun(&mut slab);
        Self::wallet("keygen", &[ent_noun, sal_noun], Operation::Poke, &mut slab)
    }

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
            None => D(0),
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
            None => D(0),
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
            None => D(0),
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

    /// Generates a master private key from a seed phrase.
    ///
    /// # Arguments
    ///
    /// * `seedphrase` - The seed phrase to generate the master private key from.
    fn gen_master_privkey(seedphrase: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let seedphrase_noun = make_tas(&mut slab, seedphrase).as_noun();
        Self::wallet(
            "gen-master-privkey",
            &[seedphrase_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Generates a master public key from a master private key and chain code.
    ///
    /// # Arguments
    ///
    /// * `master_privkey` - The master private key (base58-encoded)
    /// * `chain_code` - The chain code (base58-encoded)
    fn gen_master_pubkey(master_privkey: &str, chain_code: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let key_noun = make_tas(&mut slab, master_privkey).as_noun();
        let chain_code_noun = make_tas(&mut slab, chain_code).as_noun();
        Self::wallet(
            "gen-master-pubkey",
            &[key_noun, chain_code_noun],
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

    /// Exports keys to a file.
    fn export_keys() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("export-keys", &[], Operation::Poke, &mut slab)
    }

    /// Performs a simple scan of the blockchain.
    ///
    /// # Arguments
    ///
    /// * `master_pubkey` - The master public key to scan for.
    /// * `search_depth` - How many addresses to scan (default 100)
    fn scan(
        master_pubkey: &str,
        search_depth: u64,
        include_timelocks: bool,
        include_multisig: bool,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let master_pubkey_noun = make_tas(&mut slab, master_pubkey).as_noun();
        let search_depth_noun = D(search_depth);
        let include_timelocks_noun = D(include_timelocks as u64);
        let include_multisig_noun = D(include_multisig as u64);

        Self::wallet(
            "scan",
            &[
                master_pubkey_noun, search_depth_noun, include_timelocks_noun,
                include_multisig_noun,
            ],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Creates a transaction by building transaction inputs from notes.
    ///
    /// Takes a list of note names, recipient addresses, and gift amounts to create
    /// transaction inputs. The fee is subtracted from the first note that has sufficient
    /// assets to cover both the fee and its corresponding gift amount.
    ///
    /// # Arguments
    ///
    /// * `names` - Comma-separated list of note name pairs in format "[first last]"
    ///             Example: "[first1 last1],[first2 last2]"
    ///
    /// * `recipients` - Comma-separated list of recipient $locks
    ///                 Example: "[1 pk1],[2 pk2,pk3,pk4]"
    ///                 A simple comma-separated list is also supported: "pk1,pk2,pk3",
    ///                 where it is presumed that all recipients are single-signature,
    ///                 that is to say, it is the same as "[1 pk1],[1 pk2],[1 pk3]"
    ///
    /// * `gifts` - Comma-separated list of amounts to send to each recipient
    ///             Example: "100,200"
    ///
    /// * `fee` - Transaction fee to be subtracted from one of the input notes
    ///
    /// # Returns
    ///
    /// Returns a `CommandNoun` containing:
    /// - A `NounSlab` with the encoded create-tx command
    /// - The `Operation` type (Poke)
    ///
    /// # Errors
    ///
    /// Returns `NockAppError` if:
    /// - Name pairs are not properly formatted as "[first last]"
    /// - Number of names, recipients, and gifts don't match
    /// - Any input parsing fails
    ///
    /// # Example
    ///
    /// ```no_run
    /// let names = "[first1 last1],[first2 last2]";
    /// let recipients = "[1 pk1],[2 pk2,pk3,pk4]";
    /// let gifts = "100,200";
    /// let fee = 10;
    /// wallet.create_tx(names.to_string(), recipients.to_string(), gifts.to_string(), fee)?;
    /// ```
    fn create_tx(
        names: String,
        recipients: String,
        gifts: String,
        fee: u64,
        index: Option<u64>,
        hardened: bool,
        timelock_intents: Vec<TimelockIntent>,
    ) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();

        // Split the comma-separated inputs
        // Each name should be in format "[first last]"
        let names_vec: Vec<(String, String)> = names
            .split(',')
            .filter_map(|pair| {
                let pair = pair.trim();
                if pair.starts_with('[') && pair.ends_with(']') {
                    let inner = &pair[1..pair.len() - 1];
                    let parts: Vec<&str> = inner.split_whitespace().collect();
                    if parts.len() == 2 {
                        Some((parts[0].to_string(), parts[1].to_string()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Convert recipients to list of [number pubkeys] pairs
        let recipients_vec: Vec<(u64, Vec<String>)> = if recipients.contains('[') {
            // Parse complex format: "[1 pk1],[2 pk2,pk3,pk4]"
            recipients
                .split(',')
                .filter_map(|pair| {
                    let pair = pair.trim();
                    if pair.starts_with('[') && pair.ends_with(']') {
                        let inner = &pair[1..pair.len() - 1];
                        let mut parts = inner.splitn(2, ' ');

                        // Parse the number
                        let number = parts.next()?.parse().ok()?;

                        // Parse the pubkeys
                        let pubkeys = parts
                            .next()?
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .collect();

                        Some((number, pubkeys))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            // Parse simple format: "pk1,pk2,pk3"
            recipients
                .split(',')
                .map(|addr| (1, vec![addr.trim().to_string()]))
                .collect()
        };

        let gifts_vec: Vec<u64> = gifts.split(',').filter_map(|s| s.parse().ok()).collect();

        // Verify lengths based on single vs multiple mode
        if recipients_vec.len() == 1 && gifts_vec.len() == 1 {
            // Single mode: can spend from multiple notes to single recipient
            // No additional validation needed - any number of names is allowed
        } else {
            // Multiple mode: all lengths must match
            if names_vec.len() != recipients_vec.len() || names_vec.len() != gifts_vec.len() {
                return Err(CrownError::Unknown(
                    "Multiple recipient mode requires names, recipients, and gifts to have the same length"
                        .to_string(),
                )
                .into());
            }
        }

        // Use the first timelock intent if provided, or a default one
        let timelock_intent = if timelock_intents.is_empty() {
            TimelockIntent::none()
        } else {
            timelock_intents
                .into_iter()
                .next()
                .unwrap_or(TimelockIntent::none())
        };

        // Convert names to list of pairs
        let names_noun = names_vec
            .into_iter()
            .rev()
            .fold(D(0), |acc, (first, last)| {
                // Create a tuple [first_name last_name] for each name pair
                let first_noun = make_tas(&mut slab, &first).as_noun();
                let last_noun = make_tas(&mut slab, &last).as_noun();
                let name_pair = T(&mut slab, &[first_noun, last_noun]);
                Cell::new(&mut slab, name_pair, acc).as_noun()
            });

        let fee_noun = D(fee);

        // Format information about signing key
        let sign_key_noun = match index {
            Some(i) => {
                let inner = D(i);
                let hardened_noun = if hardened { YES } else { NO };
                T(&mut slab, &[D(0), inner, hardened_noun])
            }
            None => D(0),
        };

        // Create the order noun - use single or multiple mode based on input
        let order_noun = if recipients_vec.len() == 1 && gifts_vec.len() == 1 {
            // Single mode: [%single recipient_data gift_amount]
            let single_tag = make_tas(&mut slab, "single").as_noun();
            let single_recipient = recipients_vec.into_iter().next().unwrap();
            let single_gift = gifts_vec.into_iter().next().unwrap();

            // Create the recipient data [number pubkeys_list] for single case
            let pubkeys_noun = single_recipient
                .1
                .into_iter()
                .rev()
                .fold(D(0), |acc, pubkey| {
                    let pubkey_noun = make_tas(&mut slab, &pubkey).as_noun();
                    Cell::new(&mut slab, pubkey_noun, acc).as_noun()
                });
            let recipient_data = T(&mut slab, &[D(single_recipient.0), pubkeys_noun]);

            T(&mut slab, &[single_tag, recipient_data, D(single_gift)])
        } else {
            // Multiple mode: [%multiple recipients_list gifts_list]
            let multiple_tag = make_tas(&mut slab, "multiple").as_noun();

            // Convert recipients to list
            let recipients_noun =
                recipients_vec
                    .into_iter()
                    .rev()
                    .fold(D(0), |acc, (num, pubkeys)| {
                        // Create the inner list of pubkeys
                        let pubkeys_noun = pubkeys.into_iter().rev().fold(D(0), |acc, pubkey| {
                            let pubkey_noun = make_tas(&mut slab, &pubkey).as_noun();
                            Cell::new(&mut slab, pubkey_noun, acc).as_noun()
                        });

                        // Create the pair of [number pubkeys_list]
                        let pair = T(&mut slab, &[D(num), pubkeys_noun]);
                        Cell::new(&mut slab, pair, acc).as_noun()
                    });

            // Convert gifts to list
            let gifts_noun = gifts_vec.into_iter().rev().fold(D(0), |acc, amount| {
                Cell::new(&mut slab, D(amount), acc).as_noun()
            });

            T(&mut slab, &[multiple_tag, recipients_noun, gifts_noun])
        };

        // Convert timelock intent to noun
        let timelock_intent_noun = timelock_intent.to_noun(&mut slab);

        Self::wallet(
            "create-tx",
            &[names_noun, order_noun, fee_noun, sign_key_noun, timelock_intent_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    async fn update_balance_grpc(
        client: &mut nockapp_grpc::client::NockAppGrpcClient,
    ) -> Result<CommandNoun<NounSlab>, NockAppError> {
        let mut slab = NounSlab::new();
        let path = vec![String::from("current-balance")];
        let response = client.peek(1, path).await.map_err(|e| {
            NockAppError::OtherError(format!("Failed to peek current balance: {}", e))
        })?;
        let balance = slab.cue_into(response.as_bytes()?)?;
        Ok(Self::wallet(
            "update-balance-grpc",
            &[balance],
            Operation::Poke,
            &mut slab,
        ))
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

    /// Lists all public keys in the wallet.
    fn list_pubkeys() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("list-pubkeys", &[], Operation::Poke, &mut slab)
    }

    /// Lists notes by public key
    fn list_notes_by_pubkey(pubkey: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let pubkey_noun = make_tas(&mut slab, pubkey).as_noun();
        Self::wallet(
            "list-notes-by-pubkey",
            &[pubkey_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Lists notes by public key in CSV format
    fn list_notes_by_pubkey_csv(pubkey: &str) -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        let pubkey_noun = make_tas(&mut slab, pubkey).as_noun();
        Self::wallet(
            "list-notes-by-pubkey-csv",
            &[pubkey_noun],
            Operation::Poke,
            &mut slab,
        )
    }

    /// Shows the seed phrase for the current master key.
    fn show_seedphrase() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("show-seedphrase", &[], Operation::Poke, &mut slab)
    }

    /// Shows the master public key.
    fn show_master_pubkey() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("show-master-pubkey", &[], Operation::Poke, &mut slab)
    }

    /// Shows the master private key.
    fn show_master_privkey() -> CommandNoun<NounSlab> {
        let mut slab = NounSlab::new();
        Self::wallet("show-master-privkey", &[], Operation::Poke, &mut slab)
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

#[tokio::main]
async fn main() -> Result<(), NockAppError> {
    let cli = WalletCli::parse();
    boot::init_default_tracing(&cli.boot.clone()); // Init tracing early

    let prover_hot_state = produce_prover_hot_state();
    let data_dir = wallet_data_dir().await?;

    let kernel = boot::setup(
        KERNEL,
        Some(cli.boot.clone()),
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
        | Commands::SignTx { .. }
        | Commands::SignMessage { .. }
        | Commands::VerifyMessage { .. }
        | Commands::SignHash { .. }
        | Commands::VerifyHash { .. }
        | Commands::ExportMasterPubkey
        | Commands::ImportMasterPubkey { .. }
        | Commands::ListPubkeys
        | Commands::ShowSeedphrase
        | Commands::ShowMasterPubkey
        | Commands::ShowMasterPrivkey
        | Commands::CreateTx { .. }
        | Commands::ShowTx { .. } => false,

        // All other commands DO need sync
        _ => true,
    };

    // Generate the command noun and operation
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
        Commands::SignTx {
            transaction,
            index,
            hardened,
        } => Wallet::sign_tx(transaction, *index, *hardened),
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
            master_privkey,
            chain_code,
        } => {
            if let Some(file_path) = file {
                Wallet::import_keys(file_path)
            } else if let Some(extended_key) = key {
                Wallet::import_extended(extended_key)
            } else if let Some(seed) = seedphrase {
                // normalize seedphrase to have exactly one space between words
                let normalized_seed = seed.split_whitespace().collect::<Vec<&str>>().join(" ");
                Wallet::gen_master_privkey(&normalized_seed)
            } else if let (Some(privkey), Some(chain)) = (master_privkey, chain_code) {
                Wallet::gen_master_pubkey(&privkey, &chain)
            } else if master_privkey.is_some() && chain_code.is_none() {
                return Err(CrownError::Unknown(
                    "--master-privkey requires --chain-code to be provided".to_string(),
                )
                .into());
            } else if chain_code.is_some() && master_privkey.is_none() {
                return Err(CrownError::Unknown(
                    "--chain-code requires --master-privkey to be provided".to_string(),
                )
                .into());
            } else {
                return Err(CrownError::Unknown(
                    "One of --file, --key, --seedphrase, or --master-privkey must be provided for import-keys".to_string(),
                )
                .into());
            }
        }
        Commands::ExportKeys => Wallet::export_keys(),
        Commands::Scan {
            master_pubkey,
            search_depth,
            include_timelocks,
            include_multisig,
        } => Wallet::scan(
            master_pubkey, *search_depth, *include_timelocks, *include_multisig,
        ),
        Commands::ListNotes => Wallet::list_notes(),
        Commands::ListNotesByPubkey { pubkey } => {
            if let Some(pk) = pubkey {
                Wallet::list_notes_by_pubkey(pk)
            } else {
                return Err(CrownError::Unknown("Public key is required".into()).into());
            }
        }
        Commands::ListNotesByPubkeyCsv { pubkey } => Wallet::list_notes_by_pubkey_csv(pubkey),
        Commands::CreateTx {
            names,
            recipients,
            gifts,
            fee,
            index,
            hardened,
            timelock_intent,
            timelock_min,
        } => {
            let parsed_timelock_intent = match timelock_intent.as_str() {
                "absolute" => {
                    TimelockIntent::absolute_only(TimelockRange::new(*timelock_min, None))
                }
                "relative" => {
                    TimelockIntent::relative_only(TimelockRange::new(*timelock_min, None))
                }
                "none" => TimelockIntent::none(),
                _ => {
                    return Err(CrownError::Unknown(format!(
                        "Unknown timelock intent: {}",
                        timelock_intent
                    ))
                    .into())
                }
            };

            Wallet::create_tx(
                names.clone(),
                recipients.clone(),
                gifts.clone(),
                *fee,
                *index,
                *hardened,
                vec![parsed_timelock_intent],
            )
        }
        Commands::SendTx { transaction } => Wallet::send_tx(transaction),
        Commands::ShowTx { transaction } => Wallet::show_tx(transaction),
        Commands::ExportMasterPubkey => Wallet::export_master_pubkey(),
        Commands::ImportMasterPubkey { key_path } => Wallet::import_master_pubkey(key_path),
        Commands::ListPubkeys => Wallet::list_pubkeys(),
        Commands::ShowSeedphrase => Wallet::show_seedphrase(),
        Commands::ShowMasterPubkey => Wallet::show_master_pubkey(),
        Commands::ShowMasterPrivkey => Wallet::show_master_privkey(),
    }?;

    // If this command requires sync, update the balance using a synchronous poke
    if requires_sync {
        info!(
            "Command requires syncing the current balance, connecting to Nockchain gRPC server..."
        );
        let mut client = NockAppGrpcClient::connect(cli.grpc_address.clone())
            .await
            .map_err(|_e| {
                NockAppError::OtherError(format!("Failed to connect to Nockchain gRPC server, make sure you have a nockchain node running locally and the grpc_address matches"))
            })?;
        info!("Connected to NockApp gRPC server");
        let poke = Wallet::update_balance_grpc(&mut client).await??;
        let _ = wallet.app.poke(SystemWire.to_wire(), poke.0).await?;
    }

    wallet
        .app
        .add_io_driver(one_punch_driver(poke.0, poke.1))
        .await;
    wallet
        .app
        .add_io_driver(grpc_listener_driver(cli.grpc_address.clone()))
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
    use tokio::sync::mpsc;

    use super::*;

    static INIT: Once = Once::new();

    fn init_tracing() {
        INIT.call_once(|| {
            let cli = boot::default_boot_cli(true);
            boot::init_default_tracing(&cli);
        });
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_keygen() -> Result<(), NockAppError> {
        init_tracing();
        let cli = BootCli::parse_from(&["--new"]);

        let prover_hot_state = produce_prover_hot_state();
        let nockapp = boot::setup(
            KERNEL,
            Some(cli.clone()),
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
            Some(cli.clone()),
            prover_hot_state.as_slice(),
            "wallet",
            None,
        )
        .await
        .map_err(|e| CrownError::Unknown(e.to_string()))?;
        let mut wallet = Wallet::new(nockapp);
        let key_type = KeyType::Prv;

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

    // TODO make this a real test by creating and signing a real draft
    #[tokio::test]
    #[ignore]
    async fn test_sign_tx() -> Result<(), NockAppError> {
        init_tracing();
        let cli = BootCli::parse_from(&[""]);
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
            .await
            .map_err(|e| CrownError::Unknown(e.to_string()))?;
        let mut wallet = Wallet::new(nockapp);

        // Create a temporary input bundle file
        let bundle_path = "test_bundle.jam";
        let test_data = vec![0u8; 32]; // TODO make this a real input bundle
        fs::write(bundle_path, &test_data).map_err(|e| NockAppError::IoError(e))?;

        let wire = WalletWire::Command(Commands::SignTx {
            transaction: bundle_path.to_string(),
            index: None,
            hardened: false,
        })
        .to_wire();

        // Test signing with valid indices
        let (noun, op) = Wallet::sign_tx(bundle_path, None, false)?;
        let sign_result = wallet.app.poke(wire, noun.clone()).await?;

        println!("sign_result: {:?}", sign_result);

        let wire = WalletWire::Command(Commands::SignTx {
            transaction: bundle_path.to_string(),
            index: Some(1),
            hardened: false,
        })
        .to_wire();

        let (noun, op) = Wallet::sign_tx(bundle_path, Some(1), false)?;
        let sign_result = wallet.app.poke(wire, noun.clone()).await?;

        println!("sign_result: {:?}", sign_result);

        let wire = WalletWire::Command(Commands::SignTx {
            transaction: bundle_path.to_string(),
            index: Some(255),
            hardened: false,
        })
        .to_wire();

        let (noun, op) = Wallet::sign_tx(bundle_path, Some(255), false)?;
        let sign_result = wallet.app.poke(wire, noun.clone()).await?;

        println!("sign_result: {:?}", sign_result);

        // Cleanup
        fs::remove_file(bundle_path).map_err(|e| NockAppError::IoError(e))?;
        Ok(())
    }

    // Tests for Cold Side Commands
    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_gen_master_privkey() -> Result<(), NockAppError> {
        init_tracing();
        let cli = BootCli::parse_from(&[""]);
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
            .await
            .map_err(|e| CrownError::Unknown(e.to_string()))?;
        let mut wallet = Wallet::new(nockapp);
        let seedphrase = "correct horse battery staple";
        let (noun, op) = Wallet::gen_master_privkey(seedphrase)?;
        println!("privkey_slab: {:?}", noun);
        let wire = WalletWire::Command(Commands::ImportKeys {
            file: None,
            key: None,
            seedphrase: Some(seedphrase.to_string()),
            master_privkey: None,
            chain_code: None,
        })
        .to_wire();
        let privkey_result = wallet.app.poke(wire, noun.clone()).await?;
        println!("privkey_result: {:?}", privkey_result);
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_gen_master_pubkey() -> Result<(), NockAppError> {
        init_tracing();
        // Start with a fresh wallet (--new flag)
        let cli = BootCli::parse_from(&["--new"]);
        let prover_hot_state = produce_prover_hot_state();
        let nockapp = boot::setup(
            KERNEL,
            Some(cli.clone()),
            prover_hot_state.as_slice(),
            "wallet",
            None,
        )
        .await
        .map_err(|e| CrownError::Unknown(e.to_string()))?;
        let mut wallet = Wallet::new(nockapp);

        let master_privkey = "5crSXzcevKKieL7VTZW2hy3guFgT6sserEXm3pWHZAnQ";
        let chain_code = "yr3PWpcne3t6ByqtHSmybAJkGqyHB41WNifc5qwNfWA";

        // Generate master public key from the private key and chain code
        let (noun, op) = Wallet::gen_master_pubkey(master_privkey, chain_code)?;
        let wire = WalletWire::Command(Commands::ImportKeys {
            file: None,
            key: None,
            seedphrase: None,
            master_privkey: Some(master_privkey.to_string()),
            chain_code: Some(chain_code.to_string()),
        })
        .to_wire();
        let pubkey_result = wallet.app.poke(wire, noun.clone()).await?;
        println!("pubkey_result: {:?}", pubkey_result);

        assert!(
            pubkey_result.len() == 2,
            "Expected pubkey result to be a list of 2 noun slabs - markdown and exit"
        );
        let exit_cause = unsafe { pubkey_result[1].root() };
        let code = exit_cause.as_cell()?.tail();
        assert!(unsafe { code.raw_equals(&D(0)) }, "Expected exit code 0");

        // Now show the master private key to verify it matches our input
        let (show_noun, show_op) = Wallet::show_master_privkey()?;
        let show_wire = WalletWire::Command(Commands::ShowMasterPrivkey).to_wire();
        let show_result = wallet.app.poke(show_wire, show_noun).await?;
        println!("show_master_privkey result: {:?}", show_result);

        // Verify the show command succeeded
        assert!(
            show_result.len() == 2,
            "Expected show result to be a list of 2 noun slabs - markdown and exit"
        );
        let show_exit_cause = unsafe { show_result[1].root() };
        let show_code = show_exit_cause.as_cell()?.tail();
        assert!(
            unsafe { show_code.raw_equals(&D(0)) },
            "Expected show exit code 0"
        );

        // Parse the markdown output to extract the private key
        let markdown_slab = &show_result[0];
        let markdown_root = unsafe { markdown_slab.root() };
        let markdown_cell = markdown_root.as_cell()?;
        let markdown_content_atom = markdown_cell.tail().as_atom()?;

        let markdown_text =
            String::from_utf8_lossy(&markdown_content_atom.to_bytes_until_nul()?).to_string();

        println!("Markdown content: {}", markdown_text);

        // Extract the private key from the markdown - it should be on a line with "- private key: "
        let extracted_privkey_line = markdown_text
            .lines()
            .find(|line| line.trim().contains("Private Key: "))
            .ok_or_else(|| {
                CrownError::Unknown("Private key not found in markdown output".to_string())
            })?
            .trim();

        // remove the "- private key: " prefix and get the base58 value directly
        let extracted_privkey_b58 = extracted_privkey_line
            .trim_start_matches("- Private Key: ")
            .trim()
            .to_string();

        // Verify the extracted private key matches our input
        assert_eq!(
            extracted_privkey_b58, master_privkey,
            "Extracted private key '{}' does not match input private key '{}'",
            extracted_privkey_b58, master_privkey
        );

        println!("✓ Verification successful: Private key in output matches input");
        println!("  Input:     {}", master_privkey);
        println!("  Retrieved: {}", extracted_privkey_b58);

        Ok(())
    }

    // Tests for Hot Side Commands
    // TODO: fix this test by adding a real key file
    #[tokio::test]
    #[ignore]
    async fn test_import_keys() -> Result<(), NockAppError> {
        init_tracing();
        let cli = BootCli::parse_from(&["--new"]);
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
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
            master_privkey: None,
            chain_code: None,
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

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_simple_scan() -> Result<(), NockAppError> {
        init_tracing();
        let cli = BootCli::parse_from(&[""]);
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
            .await
            .map_err(|e| CrownError::Unknown(e.to_string()))?;
        let mut wallet = Wallet::new(nockapp);
        let master_pubkey = "pubkey123";
        let (noun, op) = Wallet::scan(master_pubkey, 100, false, false)?;
        let wire = WalletWire::Command(Commands::Scan {
            master_pubkey: master_pubkey.to_string(),
            search_depth: 100,
            include_timelocks: false,
            include_multisig: false,
        })
        .to_wire();
        let scan_result = wallet.app.poke(wire, noun.clone()).await?;
        println!("scan_result: {:?}", scan_result);
        Ok(())
    }

    // TODO: fix this test
    #[tokio::test]
    #[ignore]
    async fn test_spend_multisig_format() -> Result<(), NockAppError> {
        init_tracing();
        let cli = BootCli::parse_from(&[""]);
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
            .await
            .map_err(|e| CrownError::Unknown(e.to_string()))?;
        let mut wallet = Wallet::new(nockapp);

        let names = "[first1 last1],[first2 last2]".to_string();
        let recipients = "[1 pk1],[2 pk2,pk3,pk4]".to_string();
        let gifts = "1,2".to_string();
        let fee = 1;

        let (noun, op) = Wallet::create_tx(
            names.clone(),
            recipients.clone(),
            gifts.clone(),
            fee,
            None,
            false,
            vec![TimelockIntent::none()],
        )?;
        let wire = WalletWire::Command(Commands::CreateTx {
            names: names.clone(),
            recipients: recipients.clone(),
            gifts: gifts.clone(),
            fee: fee.clone(),
            index: None,
            hardened: false,
            timelock_intent: "none".to_string(),
            timelock_min: None,
        })
        .to_wire();
        let spend_result = wallet.app.poke(wire, noun.clone()).await?;
        println!("spend_result: {:?}", spend_result);

        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_spend_single_sig_format() -> Result<(), NockAppError> {
        let cli = BootCli::parse_from(&[""]);
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
            .await
            .map_err(|e| CrownError::Unknown(e.to_string()))?;
        init_tracing();
        let mut wallet = Wallet::new(nockapp);

        // these should be valid names of notes in the wallet balance
        let names = "[Amt4GcpYievY4PXHfffiWriJ1sYfTXFkyQsGzbzwMVzewECWDV3Ad8Q BJnaDB3koU7ruYVdWCQqkFYQ9e3GXhFsDYjJ1vSmKFdxzf6Y87DzP4n]".to_string();
        let recipients = "3HKKp7xZgCw1mhzk4iw735S2ZTavCLHc8YDGRP6G9sSTrRGsaPBu1AqJ8cBDiw2LwhRFnQG7S3N9N9okc28uBda6oSAUCBfMSg5uC9cefhrFrvXVGomoGcRvcFZTWuJzm3ch".to_string();

        let gifts = "0".to_string();
        let fee = 0;

        // generate keys
        let (genkey_noun, genkey_op) = Wallet::gen_master_privkey("correct horse battery staple")?;
        let (spend_noun, spend_op) = Wallet::create_tx(
            names.clone(),
            recipients.clone(),
            gifts.clone(),
            fee,
            None,
            false,
            vec![TimelockIntent::none()],
        )?;

        let wire1 = WalletWire::Command(Commands::ImportKeys {
            file: None,
            key: None,
            seedphrase: Some("correct horse battery staple".to_string()),
            master_privkey: None,
            chain_code: None,
        })
        .to_wire();
        let genkey_result = wallet.app.poke(wire1, genkey_noun.clone()).await?;
        println!("genkey_result: {:?}", genkey_result);

        let wire2 = WalletWire::Command(Commands::CreateTx {
            names: names.clone(),
            recipients: recipients.clone(),
            gifts: gifts.clone(),
            fee: fee.clone(),
            index: None,
            hardened: false,
            timelock_intent: "none".to_string(),
            timelock_min: None,
        })
        .to_wire();
        let spend_result = wallet.app.poke(wire2, spend_noun.clone()).await?;
        println!("spend_result: {:?}", spend_result);

        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_list_notes() -> Result<(), NockAppError> {
        init_tracing();
        let cli = BootCli::parse_from(&[""]);
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
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
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
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
        let nockapp = boot::setup(KERNEL, Some(cli.clone()), &[], "wallet", None)
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
}
