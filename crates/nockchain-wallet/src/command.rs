use std::str::FromStr;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Parser, Subcommand};
use nockapp::driver::Operation;
use nockapp::kernel::boot::Cli as BootCli;
use nockapp::wire::{Wire, WireRepr};
use nockapp::NockAppError;
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::v0;

use crate::connection::ConnectionCli;

/// CLI helper that captures optional lower and upper bounds for timelocks.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TimelockRangeCli {
    min: Option<u64>,
    max: Option<u64>,
}

#[allow(dead_code)]
impl TimelockRangeCli {
    pub fn absolute(&self) -> v0::TimelockRangeAbsolute {
        v0::TimelockRangeAbsolute::new(
            self.min.map(|value| v0::BlockHeight(Belt(value))),
            self.max.map(|value| v0::BlockHeight(Belt(value))),
        )
    }

    pub fn relative(&self) -> v0::TimelockRangeRelative {
        v0::TimelockRangeRelative::new(
            self.min.map(|value| v0::BlockHeightDelta(Belt(value))),
            self.max.map(|value| v0::BlockHeightDelta(Belt(value))),
        )
    }

    pub fn has_upper_bound(&self) -> bool {
        self.max.is_some()
    }

    pub fn from_bounds(min: Option<u64>, max: Option<u64>) -> Result<Self, String> {
        if let (Some(lo), Some(hi)) = (min, max) {
            if lo > hi {
                return Err(format!(
                    "timelock range must have min <= max, got {}..{}",
                    lo, hi
                ));
            }
        }

        Ok(Self { min, max })
    }

    fn parse_bound(component: &str) -> Result<Option<u64>, String> {
        let trimmed = component.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            trimmed
                .parse::<u64>()
                .map(Some)
                .map_err(|err| format!("invalid timelock bound '{}': {}", trimmed, err))
        }
    }
}

/// Optional timelock constraints are specified with a single flag: `--timelock <SPEC>`, where `SPEC` is a comma-separated list of `absolute=<range>` and/or `relative=<range>`.
///   - Ranges use the `min..max` syntax. (`10..`, `..500`, `0..1`).
///   - Providing only a range (without `absolute=`) is shorthand for `absolute=<range>`.
///   - Supplying both components gives a combined intent.
///
/// For now, all the seeds in a transaction constructed by the wallet will share the same
/// intent. So for all "intents" and purposes, the timelock intent is functionally the same
/// as a timelock.

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TimelockIntentCli {
    absolute: Option<TimelockRangeCli>,
    relative: Option<TimelockRangeCli>,
}

#[allow(dead_code)]
impl TimelockIntentCli {
    pub fn absolute_range(&self) -> Option<v0::TimelockRangeAbsolute> {
        self.absolute.as_ref().map(|range| range.absolute())
    }

    pub fn relative_range(&self) -> Option<v0::TimelockRangeRelative> {
        self.relative.as_ref().map(|range| range.relative())
    }

    pub fn has_upper_bound(&self) -> bool {
        self.absolute
            .as_ref()
            .map_or(false, TimelockRangeCli::has_upper_bound)
            || self
                .relative
                .as_ref()
                .map_or(false, TimelockRangeCli::has_upper_bound)
    }
}

impl FromStr for TimelockIntentCli {
    type Err = String;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            return Err("timelock spec cannot be empty".into());
        }

        let mut intent = TimelockIntentCli::default();
        for part in trimmed.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            if let Some(rest) = part.strip_prefix("absolute=") {
                if intent.absolute.is_some() {
                    return Err("absolute timelock specified more than once".into());
                }
                intent.absolute = Some(rest.parse()?);
            } else if let Some(rest) = part.strip_prefix("relative=") {
                if intent.relative.is_some() {
                    return Err("relative timelock specified more than once".into());
                }
                intent.relative = Some(rest.parse()?);
            } else {
                if intent.absolute.is_some() {
                    return Err(
                        "ambiguous timelock spec; prefix additional ranges with 'absolute=' or 'relative='"
                            .into(),
                    );
                }
                intent.absolute = Some(part.parse()?);
            }
        }

        if intent.absolute.is_none() && intent.relative.is_none() {
            return Err(
                "timelock spec must include an absolute=... or relative=... component".into(),
            );
        }

        Ok(intent)
    }
}

impl FromStr for TimelockRangeCli {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("timelock range cannot be empty".into());
        }

        if let Some((min_str, max_str)) = trimmed.split_once("..") {
            let min = Self::parse_bound(min_str)?;
            let max = Self::parse_bound(max_str)?;
            TimelockRangeCli::from_bounds(min, max)
        } else {
            // Single value -> lower bound only
            let min = Self::parse_bound(trimmed)?;
            TimelockRangeCli::from_bounds(min, None)
        }
    }
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct WalletCli {
    #[command(flatten)]
    pub boot: BootCli,

    #[command(flatten)]
    pub connection: ConnectionCli,

    /// Include watch-only pubkeys when synchronizing wallet balance
    #[arg(long, global = true, default_value_t = false)]
    pub include_watch_only: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(clap::ValueEnum, Debug, Clone, PartialEq, Eq)]
pub enum ClientType {
    Public,
    Private,
}

#[derive(Debug)]
#[allow(dead_code)]
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
pub type CommandNoun<T> = Result<(T, Operation), NockAppError>;

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
    /// Generates a new version 1 key pair
    Keygen,

    /// Derive child key (pub, private or both) from the current master key
    DeriveChild {
        /// Index of the child key to derive, should be in range [0, 2^31)
        #[arg(value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: u64,

        /// Hardened or unhardened child key
        #[arg(long)]
        hardened: bool,

        /// Label for the child key
        #[arg(short, long, value_parser = validate_label, default_value = None)]
        label: Option<String>,
    },

    /// Import keys from a file, extended key, seed phrase, or master private key
    #[command(group = clap::ArgGroup::new("import_source").required(true).args(&["file", "key", "seedphrase"]))]
    ImportKeys {
        /// Path to the jammed keys file
        #[arg(short = 'f', long = "file", value_name = "FILE")]
        file: Option<String>,

        /// Extended key string (e.g., "zprv..." or "zpub...")
        #[arg(short = 'k', long = "key", value_name = "EXTENDED_KEY")]
        key: Option<String>,

        /// Seed phrase to generate master private key, requires version. If your key was generated prior to
        /// the release of the v1 protocol upgrade on October 15, 2025, it is mostly likely version 0.
        /// If it was generated after that date, it is likely version 1.
        #[arg(short = 's', long = "seedphrase", value_name = "SEEDPHRASE")]
        seedphrase: Option<String>,

        /// Master key version to use when generating from seed phrase
        #[arg(long = "version", value_name = "VERSION", requires = "seedphrase")]
        version: Option<u64>,
    },

    /// Add a watch-only address or pubkey to the wallet
    WatchAddress {
        /// Public key hash (v1 address) or schnorr pubkey (v0 address). base58 encoded.
        #[arg(value_name = "address")]
        address: String,
    },

    /// Export keys to a file
    ExportKeys,

    /// List all notes in the wallet
    ListNotes,

    /// List notes by public key
    ListNotesByAddress {
        /// Optional public key to filter notes
        address: Option<String>,
    },

    /// List notes by public key in CSV format
    ListNotesByAddressCsv {
        /// Public key to filter notes
        address: String,
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

    /// Summarize the wallet balance
    ShowBalance,

    /// Query whether a transaction was accepted by the node
    TxAccepted {
        /// Base58-encoded transaction ID
        #[arg(value_name = "TX_ID")]
        tx_id: String,
    },

    /// Signs a transaction (for multisigs only)
    SignTx {
        /// Path to input bundle file
        transaction: String,

        /// Optional key index to use for signing [0, 2^31)
        #[arg(short, long, value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: Option<u64>,
        /// Hardened or unhardened child key
        #[arg(long, default_value = "false")]
        hardened: bool,
    },

    /// Create a transaction (use --refund-pkh when spending legacy v0 notes)
    #[command(
        name = "create-tx",
        override_usage = "nockchain-wallet create-tx --names <NAMES> --recipient <RECIPIENT> --fee <FEE> [--refund-pkh <REFUND_PKH>] [--include-data <BOOL>] [--save-raw-tx] \n\n# NOTE: --refund-pkh is required when spending from v0 notes. For v1 notes, the refund defaults to the note owner. --include-data defaults to true (pass 'false' to exclude note data). \n\nExamples:\n  # Send to a single recipient\n  nockchain-wallet create-tx \\\n    --names \"[first1 last1],[first2 last2]\" \\\n    --recipient \"<pkh-b58>:<amount>\" \\\n    --fee 10 \\\n    --refund-pkh <pkh-b58>"
    )]
    CreateTx {
        /// Names of notes to spend (comma-separated)
        #[arg(long)]
        names: String,
        /// Transaction output, formatted as "<recipient>:<amount>"
        #[arg(long = "recipient")]
        recipients: Vec<String>,
        /// Transaction fee
        #[arg(long)]
        fee: u64,
        /// Optional refund recipient pubkey hash (base58). Required for legacy v0 notes; v1 notes default to the note owner.
        #[arg(long = "refund-pkh", value_name = "REFUND_PKH")]
        refund_pkh: Option<String>,
        /// Optional key index to use for signing [0, 2^31), if not provided, we use the master key
        #[arg(short, long, value_parser = clap::value_parser!(u64).range(0..2 << 31))]
        index: Option<u64>,
        /// Hardened or unhardened child key
        #[arg(long, default_value = "false")]
        hardened: bool,
        /// Include note data in output note
        #[arg(
            long,
            action = ArgAction::Set,
            value_parser = BoolishValueParser::new(),
            default_value_t = true
        )]
        include_data: bool,
        /// For debugging purposes. If true, the raw-tx jam will be saved in the
        /// txs-debug folder in the current working directory.
        #[arg(long, default_value = "false")]
        save_raw_tx: bool,
    },

    /// Export a master public key
    ExportMasterPubkey,

    /// Import a master public key
    ImportMasterPubkey {
        // Path to keys file generated from export-master-pubkey
        key_path: String,
    },

    /// Set the active master address. Any child keys derived from that address will also become active.
    SetActiveMasterAddress {
        /// Base58-encoded address to promote to master
        #[arg(value_name = "ADDRESS_B58")]
        address_b58: String,
    },

    /// Lists all addresses in the wallet under the active master address, including child addresses
    ListActiveAddresses,

    /// Lists all master addresses
    ListMasterAddresses,

    /// Show the seed phrase for the current master key
    ShowSeedphrase,

    /// Show the master zpub extended public key
    #[command(name = "show-master-zpub")]
    ShowMasterZPub,

    /// Show the master zprv extended private key
    #[command(name = "show-master-zprv")]
    ShowMasterZPrv,

    /// Fetch confirmation depth for a transaction ID
    // Confirmations {
    //     /// Base58-encoded transaction ID
    //     #[arg(value_name = "TX_ID")]
    //     tx_id: String,
    // },

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
        #[arg(long, default_value = "false")]
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
        #[arg(long, default_value = "false")]
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
            Commands::ListNotes => "list-notes",
            Commands::ListNotesByAddress { .. } => "list-notes-by-address",
            Commands::ListNotesByAddressCsv { .. } => "list-notes-by-address-csv",
            Commands::SetActiveMasterAddress { .. } => "set-active-master-address",
            Commands::CreateTx { .. } => "create-tx",
            Commands::SendTx { .. } => "send-tx",
            Commands::ShowTx { .. } => "show-tx",
            Commands::ShowBalance => "show",
            Commands::ExportMasterPubkey => "export-master-pubkey",
            Commands::ImportMasterPubkey { .. } => "import-master-pubkey",
            Commands::ListActiveAddresses => "list-active-addresses",
            Commands::ListMasterAddresses => "list-master-addresses",
            Commands::ShowSeedphrase => "show-seed-phrase",
            Commands::ShowMasterZPub => "show-master-zpub",
            Commands::ShowMasterZPrv => "show-master-zprv",
            Commands::SignMessage { .. } => "sign-message",
            Commands::VerifyMessage { .. } => "verify-message",
            Commands::SignHash { .. } => "sign-hash",
            Commands::VerifyHash { .. } => "verify-hash",
            Commands::TxAccepted { .. } => "tx-accepted",
            Commands::WatchAddress { .. } => "watch-address",
        }
    }
}
