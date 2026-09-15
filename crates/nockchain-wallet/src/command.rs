use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use nockapp::driver::Operation;
use nockapp::kernel::boot::Cli as BootCli;
use nockapp::wire::{Wire, WireRepr};
use nockapp::NockAppError;
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::v0;

use crate::connection::ConnectionCli;
use crate::recipient::{parse_recipient_arg, RecipientSpecToken};

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
            .is_some_and(TimelockRangeCli::has_upper_bound)
            || self
                .relative
                .as_ref()
                .is_some_and(TimelockRangeCli::has_upper_bound)
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

/// CLI-facing note selection strategy for create-tx ordering.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum NoteSelectionStrategyCli {
    Ascending,
    Descending,
}

impl NoteSelectionStrategyCli {
    pub fn tas_label(&self) -> &'static str {
        match self {
            NoteSelectionStrategyCli::Ascending => "asc",
            NoteSelectionStrategyCli::Descending => "desc",
        }
    }
}

/// Top-level wallet CLI definition.
#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct WalletCli {
    #[command(flatten)]
    pub boot: BootCli,

    #[arg(long, default_value = "false")]
    pub fakenet: bool,

    #[arg(long, requires = "fakenet")]
    pub fakenet_v1_phase: Option<u64>,

    #[arg(long, requires = "fakenet")]
    pub fakenet_bythos_phase: Option<u64>,

    #[command(flatten)]
    pub connection: ConnectionCli,

    #[command(subcommand)]
    pub command: Commands,
}

/// Supported watch subcommands for addresses and lock forms.
#[derive(Subcommand, Debug, Clone)]
pub enum WatchSubcommand {
    /// Add a watch-only address (base58 pkh or schnorr pubkey)
    Address {
        /// Base58-encoded address or schnorr pubkey
        #[arg(value_name = "address")]
        address: String,
    },
    /// Add a watch-only schnorr pubkey
    Pubkey {
        /// Base58-encoded schnorr pubkey
        #[arg(value_name = "pubkey")]
        pubkey: String,
    },
    /// Add a watch-only first name (base58 hash)
    //FirstName {
    //    /// Base58-encoded first name hash
    //    #[arg(value_name = "first-name")]
    //    first_name: String,
    //},
    /// Import a multisig lock for watch-only tracking
    Multisig {
        /// Threshold (m) value for the m-of-n multisig
        #[arg(short = 't', long = "threshold")]
        threshold: u64,
        /// Comma-separated list of base58 pubkey hashes for the multisig
        #[arg(long)]
        participants: String,
    },
    /// Import many watch-only multisig locks from a manifest file
    MultisigBatch {
        /// Threshold (m) value for every manifest entry
        #[arg(short = 't', long = "threshold")]
        threshold: u64,
        /// Path to a newline-delimited manifest of comma-separated participant hashes
        #[arg(long, value_name = "FILE")]
        manifest: String,
    },
}

/// gRPC client mode used for wallet network operations.
#[derive(clap::ValueEnum, Debug, Clone, PartialEq, Eq)]
pub enum ClientType {
    Public,
    Private,
}

#[derive(Debug)]
#[allow(dead_code)]
// The `Command` variant carries the full CLI `Commands` enum, which is large by
// nature (every subcommand's args). A `WalletWire` is constructed once per
// command invocation and immediately lowered via `to_wire()`, never stored in
// bulk or on a hot path, so boxing it would only add indirection without value.
#[allow(clippy::large_enum_variant)]
/// Internal wallet event wires used for nockapp routing.
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

/// Validates label strings accepted by key-derivation CLI paths.
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
/// Wallet command surface for key, note, and transaction operations.
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

    /// Derive a contiguous batch of child keys from the current master key
    DeriveChildBatch {
        /// Starting child index, should be in range [0, 2^31)
        #[arg(long = "start-index", value_parser = clap::value_parser!(u64).range(0..(1u64 << 31)))]
        start_index: u64,

        /// Number of child keys to derive
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..(1u64 << 31)))]
        count: u64,

        /// Hardened or unhardened child keys
        #[arg(long, default_value = "false")]
        hardened: bool,

        /// Optional label prefix. Derived labels become `<prefix>-<index>`.
        #[arg(long = "label-prefix", value_parser = validate_label, default_value = None)]
        label_prefix: Option<String>,

        /// Optional output CSV path. Writes `index,address` rows when provided.
        #[arg(long = "out", value_name = "FILE")]
        out: Option<String>,
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

    /// Watch addresses, pubkeys, multisigs, or first-names
    Watch {
        #[command(subcommand)]
        subcommand: WatchSubcommand,
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

    /// List notes in an already-watched multisig in CSV format
    ListNotesByMultisigCsv {
        /// Base58 first-name of the watched multisig (printed by `watch multisig`)
        #[arg(value_name = "first-name")]
        first_name: String,
    },

    /// Show the aggregate balance of an already-watched multisig
    ShowBalanceMultisig {
        /// Base58 first-name of the watched multisig (printed by `watch multisig`)
        #[arg(value_name = "first-name")]
        first_name: String,
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

    /// Show a transaction's lifecycle status: confirmed (with block height +
    /// confirmations), pending in the mempool, or unknown to the node.
    TxStatus {
        /// Base58-encoded transaction ID
        #[arg(value_name = "TX_ID")]
        tx_id: String,

        /// Poll until the tx is confirmed (or the timeout elapses) instead of
        /// reporting once and exiting.
        #[arg(long)]
        wait: bool,

        /// With --wait, give up after this many seconds (default 600).
        #[arg(long, value_name = "SECS", default_value_t = 600)]
        timeout_secs: u64,
    },

    /// Create a transaction (use --refund-pkh when spending legacy v0 notes)
    #[command(
        name = "create-tx",
        override_usage = r#"nockchain-wallet create-tx [--names <NAMES>] --to <P2PKH_B58> (--amount <NOCKS> | --amount-nicks <NICKS>) [OPTIONS]
nockchain-wallet create-tx [--names <NAMES>] --bridge-deposit <NOCKS> --to-evm-address <EVM_ADDR> [OPTIONS]
nockchain-wallet create-tx [--names <NAMES>] --recipient <ADVANCED_RECIPIENT>... [OPTIONS]

Common examples:
  # Pay one address in whole nocks; spendable v1 notes and the fee are selected automatically.
  nockchain-wallet create-tx --to <p2pkh-b58> --amount 100

  # Pay multiple addresses and override the fee in whole nocks.
  nockchain-wallet create-tx \
    --to <p2pkh-a> --amount 100 \
    --to <p2pkh-b> --amount 5 \
    --fee 1

  # Manually select notes while using the same simple payment flags.
  nockchain-wallet create-tx \
    --names "[first1 last1],[first2 last2]" \
    --to <p2pkh-b58> --amount 100

  # Deposit the 100,000-nock minimum onto Base.
  nockchain-wallet create-tx \
    --bridge-deposit 100000 \
    --to-evm-address 0xabcdef0123456789abcdef0123456789abcdef01

  # Reuse a downloaded notes CSV instead of re-syncing.
  nockchain-wallet create-tx \
    --notes-csv notes-<p2pkh-b58>.csv \
    --to <p2pkh-b58> --amount 100

Amounts and --fee use whole nocks. Use --amount-nicks or --fee-nicks for raw nicks; each pair is mutually exclusive.
Repeat --to/--amount pairs to create multiple p2pkh outputs.
Advanced --recipient forms remain available for multisig outputs, explicit raw-nick specifications, and compatibility.
When --names is omitted, the planner selects spendable v1 notes. Manual selection can spend an all-v1 or all-v0 set, but not a mixed set; legacy v0 spends require --refund-pkh.
--include-data defaults to true. --notes-csv requires a prior wallet sync and removes spent notes from the CSV after transaction creation."#
    )]
    CreateTx {
        /// Optional names of notes to spend (comma-separated) for manual selection.
        #[arg(long)]
        names: Option<String>,
        /// Advanced output specification. Repeat for multisig outputs, explicit
        /// raw-nick specifications, or compatibility with older commands.
        /// Accepts `<p2pkh>:<amount>` or JSON with kind `p2pkh`, `multisig`, or
        /// `bridge-deposit`.
        #[arg(
            long = "recipient",
            value_name = "RECIPIENT",
            value_parser = parse_recipient_arg,
            action = ArgAction::Append
        )]
        recipients: Vec<RecipientSpecToken>,
        /// Pay a p2pkh recipient (base58). Pair each --to with one --amount in
        /// whole nocks, or one --amount-nicks in raw nicks. Repeat the pair for
        /// multiple outputs.
        #[arg(long = "to", value_name = "P2PKH_B58", action = ArgAction::Append)]
        to: Vec<String>,
        /// Amount in whole nocks (1 nock = 65536 nicks) for the paired --to
        /// recipient. Repeat --to/--amount for multiple outputs.
        #[arg(long = "amount", value_name = "NOCKS", action = ArgAction::Append)]
        amounts: Vec<u64>,
        /// Amount in nicks (raw protocol unit) for the paired --to recipient.
        /// Mutually exclusive with --amount; use a single amount unit per command.
        #[arg(
            long = "amount-nicks",
            value_name = "NICKS",
            action = ArgAction::Append,
            conflicts_with = "amounts"
        )]
        amounts_nicks: Vec<u64>,
        /// Base bridge deposit amount in whole nocks (1 nock = 65536 nicks).
        /// Pair with --to-evm-address. Only one bridge deposit is allowed per
        /// transaction.
        #[arg(
            long = "bridge-deposit",
            value_name = "NOCKS",
            requires = "to_evm_address"
        )]
        bridge_deposit: Option<u64>,
        /// Base (EVM) destination address for the paired --bridge-deposit
        /// (0x-prefixed 20-byte hex, 40 hex chars; the 0x prefix is optional).
        #[arg(
            long = "to-evm-address",
            value_name = "EVM_ADDR",
            requires = "bridge_deposit"
        )]
        to_evm_address: Option<String>,
        /// Optional transaction fee override, in whole nocks (1 nock = 65536 nicks).
        #[arg(long, value_name = "NOCKS")]
        fee: Option<u64>,
        /// Optional transaction fee override, in nicks.
        /// Mutually exclusive with --fee, which is denominated in nocks.
        #[arg(long = "fee-nicks", value_name = "NICKS", conflicts_with = "fee")]
        fee_nicks: Option<u64>,
        /// Allow fees below the estimated minimum (unsafe, testing only)
        #[arg(long, default_value = "false")]
        allow_low_fee: bool,
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
        /// Additional signing keys. Accepts `index` or `index:hardened`.
        #[arg(long = "sign-key", value_name = "INDEX[:HARDENED]", action = ArgAction::Append)]
        sign_keys: Vec<String>,
        /// For debugging purposes. If true, the raw-tx jam will be saved in the
        /// txs-debug folder in the current working directory.
        #[arg(long, default_value = "false")]
        save_raw_tx: bool,
        /// Note selection strategy (ascending selects smallest notes first)
        #[arg(long = "note-selection", value_enum, default_value = "ascending")]
        note_selection_strategy: NoteSelectionStrategyCli,
        /// Select candidate notes from a notes CSV (as written by
        /// `list-notes-by-address-csv`) instead of downloading the balance. The
        /// wallet must already hold the listed notes from a prior sync, since
        /// the note data needed to build the spend comes from local state. The
        /// notes actually spent are removed from the CSV once the tx is created.
        #[arg(long = "notes-csv", value_name = "PATH")]
        notes_csv: Option<PathBuf>,
    },

    #[command(
        name = "create-multisig-tx",
        override_usage = r#"nockchain-wallet create-multisig-tx --threshold <M> --participants <PKH,...> --to <P2PKH_B58> (--amount <NOCKS> | --amount-nicks <NICKS>) [OPTIONS]
nockchain-wallet create-multisig-tx --threshold <M> --participants <PKH,...> --bridge-deposit <NOCKS> --to-evm-address <EVM_ADDR> [OPTIONS]
nockchain-wallet create-multisig-tx --threshold <M> --participants <PKH,...> --recipient <ADVANCED_RECIPIENT>... [OPTIONS]

Common example:
  nockchain-wallet create-multisig-tx \
    --threshold 2 \
    --participants <pkh-a>,<pkh-b>,<pkh-c> \
    --to <p2pkh-b58> \
    --amount 100

--threshold and --participants reconstruct the multisig lock being spent. Omit --names to select its spendable notes automatically.
Change returns to the multisig lock unless --refund-pkh is supplied.
The command writes an unsigned or partially signed transaction under ./txs; collect remaining signatures with sign-multisig-tx before send-tx.
Amounts and --fee use whole nocks. Use --amount-nicks or --fee-nicks for raw nicks.
Advanced --recipient forms remain available for multisig outputs, explicit raw-nick specifications, and compatibility.
--notes-csv uses list-notes-by-multisig-csv output and requires a prior wallet sync."#
    )]
    CreateMultisigTx {
        /// Threshold (m) value for the m-of-n multisig being spent.
        #[arg(short = 't', long = "threshold")]
        threshold: u64,
        /// Comma-separated list of base58 pubkey hashes that define the multisig lock.
        #[arg(long)]
        participants: String,
        /// Optional names of notes to spend (comma-separated) for manual selection.
        #[arg(long)]
        names: Option<String>,
        /// Advanced output specification. Repeat for multisig outputs, explicit
        /// raw-nick specifications, or compatibility with older commands.
        /// Accepts `<p2pkh>:<amount>` or JSON with kind `p2pkh`, `multisig`, or
        /// `bridge-deposit`.
        #[arg(
            long = "recipient",
            value_name = "RECIPIENT",
            value_parser = parse_recipient_arg,
            action = ArgAction::Append
        )]
        recipients: Vec<RecipientSpecToken>,
        /// Pay a p2pkh recipient (base58). Pair each --to with one --amount in
        /// whole nocks, or one --amount-nicks in raw nicks. Repeat the pair for
        /// multiple outputs.
        #[arg(long = "to", value_name = "P2PKH_B58", action = ArgAction::Append)]
        to: Vec<String>,
        /// Amount in whole nocks (1 nock = 65536 nicks) for the paired --to
        /// recipient. Repeat --to/--amount for multiple outputs.
        #[arg(long = "amount", value_name = "NOCKS", action = ArgAction::Append)]
        amounts: Vec<u64>,
        /// Amount in nicks (raw protocol unit) for the paired --to recipient.
        /// Mutually exclusive with --amount; use a single amount unit per command.
        #[arg(
            long = "amount-nicks",
            value_name = "NICKS",
            action = ArgAction::Append,
            conflicts_with = "amounts"
        )]
        amounts_nicks: Vec<u64>,
        /// Base bridge deposit amount in whole nocks (1 nock = 65536 nicks).
        /// Pair with --to-evm-address. Only one bridge deposit is allowed per
        /// transaction.
        #[arg(
            long = "bridge-deposit",
            value_name = "NOCKS",
            requires = "to_evm_address"
        )]
        bridge_deposit: Option<u64>,
        /// Base (EVM) destination address for the paired --bridge-deposit
        /// (0x-prefixed 20-byte hex, 40 hex chars; the 0x prefix is optional).
        #[arg(
            long = "to-evm-address",
            value_name = "EVM_ADDR",
            requires = "bridge_deposit"
        )]
        to_evm_address: Option<String>,
        /// Optional transaction fee override, in whole nocks (1 nock = 65536 nicks).
        #[arg(long, value_name = "NOCKS")]
        fee: Option<u64>,
        /// Optional transaction fee override, in nicks.
        /// Mutually exclusive with --fee, which is denominated in nocks.
        #[arg(long = "fee-nicks", value_name = "NICKS", conflicts_with = "fee")]
        fee_nicks: Option<u64>,
        /// Allow fees below the estimated minimum (unsafe, testing only)
        #[arg(long, default_value = "false")]
        allow_low_fee: bool,
        /// Optional refund recipient pubkey hash (base58). When omitted, change returns to the multisig lock.
        #[arg(long = "refund-pkh", value_name = "REFUND_PKH")]
        refund_pkh: Option<String>,
        /// Optional key index to use for the initial signature [0, 2^31), if not provided, we use the master key
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
        /// Additional signing keys. Accepts `index` or `index:hardened`.
        #[arg(long = "sign-key", value_name = "INDEX[:HARDENED]", action = ArgAction::Append)]
        sign_keys: Vec<String>,
        /// For debugging purposes. If true, the raw-tx jam will be saved in the
        /// txs-debug folder in the current working directory.
        #[arg(long, default_value = "false")]
        save_raw_tx: bool,
        /// Note selection strategy (ascending selects smallest notes first)
        #[arg(long = "note-selection", value_enum, default_value = "ascending")]
        note_selection_strategy: NoteSelectionStrategyCli,
        /// Select candidate notes from a notes CSV (as written by
        /// `list-notes-by-multisig-csv`) instead of downloading the balance. The
        /// wallet must already hold the listed notes from a prior sync, since
        /// the note data needed to build the spend comes from local state. The
        /// notes actually spent are removed from the CSV once the tx is created.
        #[arg(long = "notes-csv", value_name = "PATH")]
        notes_csv: Option<PathBuf>,
    },

    /// Sweep all spendable legacy v0 notes into one v1 destination address.
    #[command(name = "migrate-v0-notes")]
    MigrateV0Notes {
        /// Base58-encoded v1 pay-to-pubkey-hash address that receives the migrated funds.
        #[arg(long = "destination", value_name = "DESTINATION")]
        destination: String,
    },

    /// Sign a multisig transaction
    SignMultisigTx {
        /// Path to transaction file
        transaction: String,
        /// Comma-separated list of key indices to sign with (format: index:hardened). If not provided, uses master key.
        #[arg(long)]
        sign_keys: Option<String>,
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

    /// Show the raw master private key as base58
    #[command(name = "show-master-prv")]
    ShowMasterPrv,

    /// Show the key tree structure
    #[command(name = "show-key-tree")]
    ShowKeyTree {
        /// Include values at each path
        #[arg(long)]
        include_values: bool,
    },

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
            Commands::DeriveChildBatch { .. } => "derive-child-batch",
            Commands::ImportKeys { .. } => "import-keys",
            Commands::ExportKeys => "export-keys",
            Commands::ListNotes => "list-notes",
            Commands::ListNotesByAddress { .. } => "list-notes-by-address",
            Commands::ListNotesByAddressCsv { .. } => "list-notes-by-address-csv",
            Commands::ListNotesByMultisigCsv { .. } => "list-notes-by-multisig-csv",
            Commands::ShowBalanceMultisig { .. } => "show-balance-multisig",
            Commands::SetActiveMasterAddress { .. } => "set-active-master-address",
            Commands::CreateTx { .. } => "create-tx",
            Commands::CreateMultisigTx { .. } => "create-tx",
            Commands::MigrateV0Notes { .. } => "migrate-v0-notes",
            Commands::SignMultisigTx { .. } => "sign-multisig-tx",
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
            Commands::ShowMasterPrv => "show-master-prv",
            Commands::ShowKeyTree { .. } => "show-key-tree",
            Commands::SignMessage { .. } => "sign-message",
            Commands::VerifyMessage { .. } => "verify-message",
            Commands::SignHash { .. } => "sign-hash",
            Commands::VerifyHash { .. } => "verify-hash",
            Commands::TxAccepted { .. } => "tx-accepted",
            Commands::TxStatus { .. } => "tx-status",
            Commands::Watch { subcommand } => match subcommand {
                WatchSubcommand::Address { .. } => "watch-address",
                WatchSubcommand::Pubkey { .. } => "watch-address",
                //WatchSubcommand::FirstName { .. } => "watch-first-name",
                WatchSubcommand::Multisig { .. } => "watch-address-multisig",
                WatchSubcommand::MultisigBatch { .. } => "watch-address-multisig",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_P2PKH: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";

    #[test]
    fn create_tx_defaults_to_ascending_note_selection() {
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "1",
        ])
        .expect("create-tx CLI should parse");

        let Commands::CreateTx {
            note_selection_strategy,
            ..
        } = cli.command
        else {
            panic!("expected create-tx command");
        };

        assert!(matches!(
            note_selection_strategy,
            NoteSelectionStrategyCli::Ascending
        ));
    }

    #[test]
    fn create_tx_accepts_descending_note_selection_override() {
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "1",
            "--note-selection", "descending",
        ])
        .expect("create-tx CLI should parse");

        let Commands::CreateTx {
            note_selection_strategy,
            ..
        } = cli.command
        else {
            panic!("expected create-tx command");
        };

        assert!(matches!(
            note_selection_strategy,
            NoteSelectionStrategyCli::Descending
        ));
    }

    #[test]
    fn create_tx_accepts_paired_to_amount_flags() {
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "100", "--to",
            SAMPLE_P2PKH, "--amount", "5",
        ])
        .expect("create-tx with --to/--amount should parse");

        let Commands::CreateTx {
            to,
            amounts,
            amounts_nicks,
            recipients,
            ..
        } = cli.command
        else {
            panic!("expected create-tx command");
        };
        assert_eq!(to, vec![SAMPLE_P2PKH.to_string(), SAMPLE_P2PKH.to_string()]);
        assert_eq!(amounts, vec![100u64, 5u64]);
        assert!(amounts_nicks.is_empty());
        assert!(recipients.is_empty());
    }

    #[test]
    fn create_tx_rejects_decimal_amount() {
        let result = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "5.5",
        ]);
        assert!(result.is_err(), "decimal --amount must be rejected");
    }

    #[test]
    fn create_tx_accepts_amount_nicks() {
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount-nicks", "6553600",
        ])
        .expect("create-tx with --amount-nicks should parse");
        let Commands::CreateTx {
            amounts,
            amounts_nicks,
            ..
        } = cli.command
        else {
            panic!("expected create-tx command");
        };
        assert!(amounts.is_empty());
        assert_eq!(amounts_nicks, vec![6_553_600u64]);
    }

    #[test]
    fn create_tx_amount_and_amount_nicks_are_mutually_exclusive() {
        let result = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "1",
            "--amount-nicks", "65536",
        ]);
        assert!(
            result.is_err(),
            "--amount and --amount-nicks must not be accepted together"
        );
    }

    #[test]
    fn create_multisig_tx_accepts_to_amount_flags() {
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet",
            "create-multisig-tx",
            "--threshold",
            "2",
            "--participants",
            &format!("{SAMPLE_P2PKH},{SAMPLE_P2PKH}"),
            "--to",
            SAMPLE_P2PKH,
            "--amount",
            "100",
        ])
        .expect("create-multisig-tx with --to/--amount should parse");

        let Commands::CreateMultisigTx { to, amounts, .. } = cli.command else {
            panic!("expected create-multisig-tx command");
        };
        assert_eq!(to, vec![SAMPLE_P2PKH.to_string()]);
        assert_eq!(amounts, vec![100u64]);
    }

    #[test]
    fn create_tx_fee_and_fee_nicks_are_mutually_exclusive() {
        let result = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "1", "--fee", "10",
            "--fee-nicks", "1",
        ]);
        assert!(
            result.is_err(),
            "--fee and --fee-nicks must not be accepted together"
        );
    }

    #[test]
    fn create_tx_fee_is_nocks_and_fee_nicks_is_nicks() {
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "1", "--fee", "2",
        ])
        .expect("create-tx with --fee (nocks) should parse");
        let Commands::CreateTx { fee, fee_nicks, .. } = cli.command else {
            panic!("expected create-tx command");
        };
        assert_eq!(fee, Some(2u64));
        assert_eq!(fee_nicks, None);

        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--to", SAMPLE_P2PKH, "--amount", "1", "--fee-nicks",
            "3",
        ])
        .expect("create-tx with --fee-nicks should parse");
        let Commands::CreateTx { fee, fee_nicks, .. } = cli.command else {
            panic!("expected create-tx command");
        };
        assert_eq!(fee, None);
        assert_eq!(fee_nicks, Some(3u64));
    }

    #[test]
    fn create_tx_accepts_bridge_deposit_pair() {
        let evm = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "create-tx", "--bridge-deposit", "100000", "--to-evm-address", evm,
        ])
        .expect("create-tx with bridge-deposit pair should parse");
        let Commands::CreateTx {
            bridge_deposit,
            to_evm_address,
            ..
        } = cli.command
        else {
            panic!("expected create-tx command");
        };
        assert_eq!(bridge_deposit, Some(100_000u64));
        assert_eq!(to_evm_address, Some(evm.to_string()));
    }

    #[test]
    fn create_tx_bridge_deposit_flags_require_each_other() {
        // --bridge-deposit without --to-evm-address (and vice versa) is rejected
        // by clap's `requires` relationship.
        assert!(
            WalletCli::try_parse_from([
                "nockchain-wallet", "create-tx", "--bridge-deposit", "100000",
            ])
            .is_err(),
            "--bridge-deposit alone must be rejected"
        );
        assert!(
            WalletCli::try_parse_from([
                "nockchain-wallet", "create-tx", "--to-evm-address",
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ])
            .is_err(),
            "--to-evm-address alone must be rejected"
        );
    }

    #[test]
    fn migrate_v0_notes_requires_destination() {
        let cli = WalletCli::try_parse_from([
            "nockchain-wallet", "migrate-v0-notes", "--destination", SAMPLE_P2PKH,
        ])
        .expect("migrate-v0-notes CLI should parse");

        let Commands::MigrateV0Notes { destination } = cli.command else {
            panic!("expected migrate-v0-notes command");
        };

        assert_eq!(destination, SAMPLE_P2PKH);
    }
}
