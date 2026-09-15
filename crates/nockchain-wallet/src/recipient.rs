use std::collections::BTreeSet;

use nockchain_types::common::Hash;
use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh, SpendCondition};
use nockchain_types::{EthAddress, EthAddressParseError};
use noun_serde::{NounDecode, NounEncode};
use serde::Deserialize;
use wallet_tx_builder::types::{PlannedOutput, RawNoteDataEntry};

use crate::{CrownError, NockAppError};

pub const BRIDGE_LOCK_ROOT_DEFAULT_B58: &str =
    "AcsPkuhXQoGeEsF91yynpm1kcW17PQ2Z1MEozgx7YnDPkZwrtzLuuqd";

/// 1 nock = 2^16 nicks. Protocol amounts are nicks; the ergonomic `--amount`
/// and `--fee` flags take whole nocks, so they are converted with exact integer
/// math before use.
pub const NICKS_PER_NOCK: u64 = 1 << 16;

/// Converts a whole-nock amount to nicks with exact integer math.
///
/// Nocks are integers only — there is deliberately no sub-nock / decimal input
/// anywhere in the CLI. Rejects zero and any value that overflows the `u64`
/// nicks range used by the protocol.
///
/// This is the sole soundness-sensitive primitive behind the nocks-denominated
/// `--amount` / `--fee` flags: the resulting nick count must equal what a user
/// would have hand-entered via the nicks-based `--recipient` JSON / `--fee-nicks`
/// forms.
pub fn nocks_to_nicks(nocks: u64) -> Result<u64, String> {
    if nocks == 0 {
        return Err("amount must be greater than zero".into());
    }
    nocks
        .checked_mul(NICKS_PER_NOCK)
        .ok_or_else(|| format!("amount '{nocks}' nocks overflows the u64 nicks range"))
}

/// Builds `RecipientSpecToken::P2pkh` values from paired `--to` addresses and
/// their amounts already resolved to nicks. The two slices are zipped by
/// position and must have equal length; the produced token is identical to the
/// nicks-based `--recipient` JSON form for the same address and nick amount.
pub fn to_amount_pairs_to_tokens(
    tos: &[String],
    amounts_nicks: &[u64],
) -> Result<Vec<RecipientSpecToken>, String> {
    if tos.len() != amounts_nicks.len() {
        return Err(format!(
            "each --to must be paired with one --amount/--amount-nicks \
             ({} --to vs {} amount(s))",
            tos.len(),
            amounts_nicks.len()
        ));
    }
    tos.iter()
        .zip(amounts_nicks.iter())
        .map(|(address, &amount)| {
            let address = address.trim();
            if address.is_empty() {
                return Err("--to recipient address cannot be empty".to_string());
            }
            if amount == 0 {
                return Err("--amount/--amount-nicks must be greater than zero".to_string());
            }
            Ok(RecipientSpecToken::P2pkh {
                address: address.to_string(),
                amount,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "lowercase")]
pub enum RecipientSpecToken {
    P2pkh {
        address: String,
        amount: u64,
    },
    Multisig {
        threshold: u64,
        addresses: Vec<String>,
        amount: u64,
    },
    #[serde(rename = "bridge-deposit")]
    BridgeDeposit {
        #[serde(default)]
        root: Option<String>,
        #[serde(rename = "evm-address")]
        evm_address: String,
        amount: u64,
    },
}

#[derive(Debug, Clone, NounEncode, NounDecode, PartialEq)]
pub enum RecipientSpec {
    #[noun(tag = "pkh")]
    P2pkh { address: Hash, amount: u64 },
    #[noun(tag = "multisig")]
    Multisig {
        threshold: u64,
        addresses: Vec<Hash>,
        amount: u64,
    },
    #[noun(tag = "bridge-deposit")]
    BridgeDeposit {
        root: Hash,
        evm_address: EthAddress,
        amount: u64,
    },
}

impl RecipientSpecToken {
    pub fn from_cli_arg(raw: &str) -> Result<Self, CrownError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CrownError::Unknown(
                "Recipient specification cannot be empty".into(),
            ));
        }
        if trimmed.starts_with('{') {
            return Self::from_json(trimmed);
        }
        Self::from_legacy(trimmed)
    }

    fn from_json(raw: &str) -> Result<Self, CrownError> {
        serde_json::from_str(raw).map_err(|err| {
            CrownError::Unknown(format!("Failed to parse recipient JSON '{raw}': {err}"))
        })
    }

    fn from_legacy(raw: &str) -> Result<Self, CrownError> {
        let (address, amount_str) = raw.split_once(':').ok_or_else(|| {
            CrownError::Unknown("Legacy recipient must be formatted as <p2pkh>:<amount>".into())
        })?;
        let p2pkh = address.trim();
        if p2pkh.is_empty() {
            return Err(CrownError::Unknown(
                "Legacy recipient p2pkh cannot be empty".into(),
            ));
        }
        let amount_raw = amount_str.trim();
        let amount = amount_raw.parse::<u64>().map_err(|err| {
            CrownError::Unknown(format!(
                "Invalid amount '{}' in legacy recipient: {err}",
                amount_raw
            ))
        })?;
        if amount == 0 {
            return Err(CrownError::Unknown(
                "Legacy recipient amount must be greater than zero".into(),
            ));
        }
        Ok(RecipientSpecToken::P2pkh {
            address: p2pkh.to_string(),
            amount,
        })
    }

    pub fn into_recipient_spec(self) -> Result<RecipientSpec, NockAppError> {
        match self {
            RecipientSpecToken::P2pkh { address, amount } => {
                if amount == 0 {
                    return Err(CrownError::Unknown(
                        "Recipient amount must be greater than zero".into(),
                    )
                    .into());
                }
                let recipient = Hash::from_base58(&address).map_err(|err| {
                    NockAppError::from(CrownError::Unknown(format!(
                        "Invalid recipient address '{address}': {err}"
                    )))
                })?;
                Ok(RecipientSpec::P2pkh {
                    address: recipient,
                    amount,
                })
            }
            RecipientSpecToken::Multisig {
                threshold,
                addresses,
                amount,
            } => {
                if amount == 0 {
                    return Err(CrownError::Unknown(
                        "Recipient amount must be greater than zero".into(),
                    )
                    .into());
                }
                if threshold == 0 {
                    return Err(CrownError::Unknown(
                        "Multisig threshold must be greater than zero".into(),
                    )
                    .into());
                }
                if addresses.is_empty() {
                    return Err(CrownError::Unknown(
                        "Multisig recipient must include at least one address".into(),
                    )
                    .into());
                }
                let mut unique = BTreeSet::new();
                let parsed = addresses
                    .into_iter()
                    .map(|pkh| {
                        if !unique.insert(pkh.clone()) {
                            return Err(NockAppError::from(CrownError::Unknown(
                                "Multisig recipients cannot include duplicate addresses".into(),
                            )));
                        }
                        Hash::from_base58(&pkh).map_err(|err| {
                            NockAppError::from(CrownError::Unknown(format!(
                                "Invalid multisig address '{pkh}': {err}"
                            )))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if threshold as usize > parsed.len() {
                    return Err(
                        CrownError::Unknown(format!(
                            "Multisig threshold ({threshold}) cannot exceed the number of addresses ({})",
                            parsed.len()
                        ))
                        .into(),
                    );
                }
                Ok(RecipientSpec::Multisig {
                    threshold,
                    addresses: parsed,
                    amount,
                })
            }
            RecipientSpecToken::BridgeDeposit {
                root,
                evm_address,
                amount,
            } => {
                if amount == 0 {
                    return Err(CrownError::Unknown(
                        "Recipient amount must be greater than zero".into(),
                    )
                    .into());
                }
                let parsed = EthAddress::from_hex_str(&evm_address).map_err(|err| {
                    NockAppError::from(CrownError::Unknown(format!(
                        "Invalid EVM address '{}': {}",
                        evm_address,
                        format_eth_addr_error(err)
                    )))
                })?;
                let parsed_root = resolve_bridge_lock_root(root.as_deref())?;
                Ok(RecipientSpec::BridgeDeposit {
                    root: parsed_root,
                    evm_address: parsed,
                    amount,
                })
            }
        }
    }
}

fn format_eth_addr_error(err: EthAddressParseError) -> String {
    match err {
        EthAddressParseError::Empty => "address cannot be empty".into(),
        EthAddressParseError::WrongLength(len) => {
            format!("expected 40 hex chars (20 bytes), got length {}", len)
        }
        EthAddressParseError::InvalidCharacters => "contains non-hex characters".into(),
        EthAddressParseError::InvalidHex(msg) => msg,
    }
}

pub fn parse_recipient_arg(raw: &str) -> Result<RecipientSpecToken, String> {
    RecipientSpecToken::from_cli_arg(raw).map_err(|err| err.to_string())
}

pub fn recipient_tokens_to_specs(
    tokens: Vec<RecipientSpecToken>,
) -> Result<Vec<RecipientSpec>, NockAppError> {
    if tokens.is_empty() {
        return Err(CrownError::Unknown(
            "At least one output must be provided with --to/--amount, \
             --bridge-deposit/--to-evm-address, or --recipient"
                .into(),
        )
        .into());
    }
    tokens
        .into_iter()
        .map(|token| token.into_recipient_spec())
        .collect()
}

fn pkh_lock(threshold: u64, addresses: &[Hash]) -> Lock {
    Lock::SpendCondition(SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        threshold,
        addresses.to_vec(),
    ))]))
}

/// Reconstructed m-of-n multisig lock used when spending multisig notes.
///
/// Carries both the canonical `SpendCondition` (needed to plan/seed the lock
/// matcher and, in the robust path, to supply the input lock to the kernel) and
/// the derived `lock_root` (whose first-name identifies the multisig's notes).
#[derive(Debug, Clone)]
pub struct MultisigLockContext {
    pub spend_condition: SpendCondition,
    pub lock_root: Hash,
    pub threshold: u64,
    pub participants: Vec<Hash>,
}

/// Parses `--threshold` plus a comma-separated list of base58 participant pubkey
/// hashes into a canonical multisig spend-condition and its lock root.
///
/// Validation mirrors the multisig recipient rules: threshold must be non-zero,
/// at least one participant is required, participants must be unique, and the
/// threshold cannot exceed the number of participants. The participant order is
/// irrelevant to the resulting lock root because the underlying `Pkh` stores the
/// hashes in a canonical `ZSet`.
pub fn multisig_lock_from_participants(
    threshold: u64,
    participants_csv: &str,
) -> Result<MultisigLockContext, NockAppError> {
    if threshold == 0 {
        return Err(
            CrownError::Unknown("Multisig threshold must be greater than zero".into()).into(),
        );
    }
    let mut unique = BTreeSet::new();
    let mut participants = Vec::new();
    for raw in participants_csv.split(',') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let pkh = Hash::from_base58(trimmed).map_err(|err| {
            NockAppError::from(CrownError::Unknown(format!(
                "Invalid multisig participant '{trimmed}': {err}"
            )))
        })?;
        if !unique.insert(pkh.to_array()) {
            return Err(CrownError::Unknown(format!(
                "Multisig participants cannot include duplicate address '{trimmed}'"
            ))
            .into());
        }
        participants.push(pkh);
    }
    if participants.is_empty() {
        return Err(CrownError::Unknown(
            "Multisig spend requires at least one --participants address".into(),
        )
        .into());
    }
    if threshold as usize > participants.len() {
        return Err(CrownError::Unknown(format!(
            "Multisig threshold ({threshold}) cannot exceed the number of participants ({})",
            participants.len()
        ))
        .into());
    }
    let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        threshold,
        participants.clone(),
    ))]);
    let lock = Lock::SpendCondition(spend_condition.clone());
    let lock_root = lock_root(&lock)?;
    Ok(MultisigLockContext {
        spend_condition,
        lock_root,
        threshold,
        participants,
    })
}

/// Builds the planner refund output that returns change to the multisig itself.
///
/// This mirrors the tx-builder's default refund behavior (`refund-lock` falls
/// back to the multisig lock when no explicit refund pubkey hash is supplied),
/// so the fee estimate accounts for a multisig change note that carries its own
/// lock data.
pub fn multisig_refund_output_template(ctx: &MultisigLockContext) -> PlannedOutput {
    PlannedOutput {
        lock_root: ctx.lock_root.clone(),
        amount: 0,
        note_data: vec![RawNoteDataEntry::from_lock(Lock::SpendCondition(
            ctx.spend_condition.clone(),
        ))],
    }
}

fn lock_root(lock: &Lock) -> Result<Hash, NockAppError> {
    lock.hash()
        .map_err(|err| CrownError::Unknown(format!("unable to derive lock root: {err}")).into())
}

fn evm_address_to_based(evm_address: EthAddress) -> [u64; 3] {
    let mut be = [0_u8; 32];
    be[12..].copy_from_slice(evm_address.as_slice());
    let limbs = Hash::from_be_bytes(&be).to_array();
    [limbs[0], limbs[1], limbs[2]]
}

fn default_bridge_lock_root() -> Result<Hash, NockAppError> {
    Hash::from_base58(BRIDGE_LOCK_ROOT_DEFAULT_B58).map_err(|err| {
        NockAppError::from(CrownError::Unknown(format!(
            "Invalid bridge lock root constant '{}': {}",
            BRIDGE_LOCK_ROOT_DEFAULT_B58, err
        )))
    })
}

fn resolve_bridge_lock_root(raw_root: Option<&str>) -> Result<Hash, NockAppError> {
    let Some(root) = raw_root.map(str::trim).filter(|value| !value.is_empty()) else {
        return default_bridge_lock_root();
    };
    Hash::from_base58(root).map_err(|err| {
        NockAppError::from(CrownError::Unknown(format!(
            "Invalid bridge deposit lock root '{}': {}",
            root, err
        )))
    })
}

/// Converts CLI recipient specs into planner outputs with tx-builder-compatible note-data.
pub fn planner_recipient_outputs(
    recipients: &[RecipientSpec],
    include_data: bool,
) -> Result<Vec<PlannedOutput>, NockAppError> {
    recipients
        .iter()
        .map(|recipient| planner_recipient_output(recipient, include_data))
        .collect()
}

/// Builds one planner output from a recipient, including deterministic lock root + note-data.
pub fn planner_recipient_output(
    recipient: &RecipientSpec,
    include_data: bool,
) -> Result<PlannedOutput, NockAppError> {
    match recipient {
        RecipientSpec::P2pkh { address, amount } => {
            let lock = pkh_lock(1, std::slice::from_ref(address));
            let note_data = if include_data {
                vec![RawNoteDataEntry::from_lock(lock.clone())]
            } else {
                Vec::new()
            };
            Ok(PlannedOutput {
                lock_root: lock_root(&lock)?,
                amount: *amount,
                note_data,
            })
        }
        RecipientSpec::Multisig {
            threshold,
            addresses,
            amount,
        } => {
            let lock = pkh_lock(*threshold, addresses);
            Ok(PlannedOutput {
                lock_root: lock_root(&lock)?,
                amount: *amount,
                // Hoon always includes lock note-data for multisig outputs.
                note_data: vec![RawNoteDataEntry::from_lock(lock.clone())],
            })
        }
        RecipientSpec::BridgeDeposit {
            root,
            evm_address,
            amount,
        } => Ok(PlannedOutput {
            lock_root: root.clone(),
            amount: *amount,
            note_data: vec![RawNoteDataEntry::from_bridge_deposit(evm_address_to_based(
                *evm_address,
            ))],
        }),
    }
}

pub fn planner_refund_output_template(
    refund_pkh: Option<&Hash>,
    signer_pkh: &Hash,
    include_data: bool,
) -> Result<PlannedOutput, NockAppError> {
    let refund_owner = refund_pkh.unwrap_or(signer_pkh).clone();
    let refund_lock = pkh_lock(1, std::slice::from_ref(&refund_owner));
    Ok(PlannedOutput {
        lock_root: lock_root(&refund_lock)?,
        amount: 0,
        note_data: if include_data {
            vec![RawNoteDataEntry::from_lock(refund_lock.clone())]
        } else {
            Vec::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockvm::noun::NounAllocator;
    use noun_serde::NounDecode;

    use super::*;

    const SAMPLE_P2PKH: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";
    const SAMPLE_P2PKH_ALT: &str = "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt";
    const SAMPLE_EVM_ADDRESS: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn parse_recipient_arg_accepts_json_p2pkh() {
        let raw = format!(
            "{{\"kind\":\"p2pkh\",\"address\":\"{}\",\"amount\":42}}",
            SAMPLE_P2PKH
        );
        let token = RecipientSpecToken::from_cli_arg(&raw).expect("json p2pkh parses");
        assert!(matches!(token, RecipientSpecToken::P2pkh { amount, .. } if amount == 42));
    }

    #[test]
    fn parse_recipient_arg_accepts_json_multisig() {
        let raw = format!(
            "{{\"kind\":\"multisig\",\"threshold\":2,\"addresses\":[\"{}\",\"{}\"],\"amount\":9000}}",
            SAMPLE_P2PKH, SAMPLE_P2PKH_ALT
        );
        let token = RecipientSpecToken::from_cli_arg(&raw).expect("json multisig parses");
        assert!(matches!(
            token,
            RecipientSpecToken::Multisig {
                threshold, amount, ..
            } if threshold == 2 && amount == 9000
        ));
    }

    #[test]
    fn parse_recipient_arg_accepts_legacy() {
        let token = RecipientSpecToken::from_cli_arg(&format!("{SAMPLE_P2PKH}:7"))
            .expect("legacy recipient parses");
        assert!(matches!(
            token,
            RecipientSpecToken::P2pkh { amount, .. } if amount == 7
        ));
    }

    #[test]
    fn parse_recipient_arg_accepts_bridge_deposit() {
        let raw = format!(
            "{{\"kind\":\"bridge-deposit\",\"evm-address\":\"{}\",\"amount\":123456}}",
            SAMPLE_EVM_ADDRESS
        );
        let token = RecipientSpecToken::from_cli_arg(&raw).expect("bridge deposit parses");
        assert!(matches!(
            token,
            RecipientSpecToken::BridgeDeposit { amount, .. } if amount == 123456
        ));
    }

    #[test]
    fn bridge_deposit_rejects_bad_address() {
        let raw = "{\"kind\":\"bridge-deposit\",\"evm-address\":\"0xdeadbeef\",\"amount\":10}";
        let token =
            RecipientSpecToken::from_cli_arg(raw).expect("json parsing should succeed initially");
        let err = token
            .into_recipient_spec()
            .expect_err("invalid bridge deposit should fail conversion");
        assert!(
            format!("{err}").contains("EVM address"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn bridge_deposit_rejects_unknown_json_field() {
        let raw = format!(
            "{{\"kind\":\"bridge-deposit\",\"evm-address\":\"{}\",\"amount\":10,\"unexpected\":true}}",
            SAMPLE_EVM_ADDRESS
        );
        let err = RecipientSpecToken::from_cli_arg(&raw)
            .expect_err("bridge deposit with unknown field should fail");
        assert!(format!("{err}").contains("unknown field"));
    }

    #[test]
    fn parse_recipient_arg_rejects_empty() {
        let err = RecipientSpecToken::from_cli_arg("   ").expect_err("empty spec should fail");
        assert!(format!("{err}").contains("cannot be empty"));
    }

    #[test]
    fn recipient_tokens_to_specs_builds_structs() {
        let tokens = vec![
            RecipientSpecToken::P2pkh {
                address: SAMPLE_P2PKH.to_string(),
                amount: 1000,
            },
            RecipientSpecToken::Multisig {
                threshold: 1,
                addresses: vec![SAMPLE_P2PKH_ALT.to_string(), SAMPLE_P2PKH.to_string()],
                amount: 5,
            },
            RecipientSpecToken::BridgeDeposit {
                root: None,
                evm_address: SAMPLE_EVM_ADDRESS.to_string(),
                amount: 9,
            },
        ];
        let specs = recipient_tokens_to_specs(tokens).expect("tokens -> specs");
        assert_eq!(specs.len(), 3);
        match &specs[0] {
            RecipientSpec::P2pkh { address, amount } => {
                assert_eq!(*amount, 1000);
                assert_eq!(
                    address,
                    &Hash::from_base58(SAMPLE_P2PKH).expect("sample p2pkh hash")
                );
            }
            _ => panic!("first spec should be p2pkh"),
        }
        match &specs[1] {
            RecipientSpec::Multisig {
                threshold,
                addresses,
                amount,
            } => {
                assert_eq!(*threshold, 1);
                assert_eq!(*amount, 5);
                assert_eq!(addresses.len(), 2);
                assert_eq!(
                    addresses[0],
                    Hash::from_base58(SAMPLE_P2PKH_ALT).expect("sample alt hash")
                );
                assert_eq!(
                    addresses[1],
                    Hash::from_base58(SAMPLE_P2PKH).expect("sample alt hash")
                );
            }
            _ => panic!("second spec should be multisig"),
        }
        match &specs[2] {
            RecipientSpec::BridgeDeposit {
                evm_address,
                amount,
                ..
            } => {
                assert_eq!(*amount, 9);
                assert_eq!(
                    evm_address,
                    &EthAddress::from_hex_str(SAMPLE_EVM_ADDRESS).expect("sample evm address")
                );
            }
            _ => panic!("third spec should be bridge deposit"),
        }
    }

    #[test]
    fn recipient_tokens_to_specs_rejects_empty() {
        let err = recipient_tokens_to_specs(vec![]).expect_err("missing recipients");
        assert!(format!("{err}").contains("At least one output must be provided"));
    }

    #[test]
    fn multisig_lock_from_participants_matches_canonical_pkh_lock() {
        let csv = format!("{SAMPLE_P2PKH},{SAMPLE_P2PKH_ALT}");
        let ctx = multisig_lock_from_participants(2, &csv).expect("multisig lock");
        assert_eq!(ctx.threshold, 2);
        assert_eq!(ctx.participants.len(), 2);

        // The reconstructed lock root must equal the canonical pkh_lock path that
        // builds multisig *outputs*, so a note sent to this multisig is spendable
        // by this context (identical lock root => identical note first-name).
        let addresses = vec![
            Hash::from_base58(SAMPLE_P2PKH).expect("p2pkh hash"),
            Hash::from_base58(SAMPLE_P2PKH_ALT).expect("alt hash"),
        ];
        let expected_root = lock_root(&pkh_lock(2, &addresses)).expect("canonical lock root");
        assert_eq!(ctx.lock_root, expected_root);

        // The carried spend-condition must hash to the same lock root.
        let sc_root = Lock::SpendCondition(ctx.spend_condition.clone())
            .hash()
            .expect("spend-condition lock root");
        assert_eq!(ctx.lock_root, sc_root);
    }

    #[test]
    fn multisig_lock_from_participants_is_participant_order_independent() {
        let a = multisig_lock_from_participants(2, &format!("{SAMPLE_P2PKH},{SAMPLE_P2PKH_ALT}"))
            .expect("order a");
        let b = multisig_lock_from_participants(2, &format!("{SAMPLE_P2PKH_ALT},{SAMPLE_P2PKH}"))
            .expect("order b");
        assert_eq!(a.lock_root, b.lock_root);
    }

    #[test]
    fn multisig_lock_from_participants_validates_inputs() {
        assert!(
            multisig_lock_from_participants(0, SAMPLE_P2PKH).is_err(),
            "zero threshold must be rejected"
        );
        assert!(
            multisig_lock_from_participants(1, "   ,  ").is_err(),
            "empty participant set must be rejected"
        );
        assert!(
            multisig_lock_from_participants(3, &format!("{SAMPLE_P2PKH},{SAMPLE_P2PKH_ALT}"))
                .is_err(),
            "threshold greater than participant count must be rejected"
        );
        assert!(
            multisig_lock_from_participants(1, &format!("{SAMPLE_P2PKH},{SAMPLE_P2PKH}")).is_err(),
            "duplicate participants must be rejected"
        );
    }

    #[test]
    fn nocks_to_nicks_converts_whole_nocks() {
        assert_eq!(nocks_to_nicks(1).unwrap(), 65536);
        assert_eq!(nocks_to_nicks(100).unwrap(), 6_553_600);
        assert_eq!(nocks_to_nicks(2).unwrap(), 131072);
    }

    #[test]
    fn nocks_to_nicks_rejects_zero_and_overflow() {
        assert!(nocks_to_nicks(0).is_err());
        // u64::MAX nocks * 65536 overflows the u64 nicks range.
        assert!(nocks_to_nicks(u64::MAX).is_err());
        assert!(nocks_to_nicks(u64::MAX / 65536 + 1).is_err());
        // The largest representable whole-nock amount still converts.
        assert_eq!(
            nocks_to_nicks(u64::MAX / 65536).unwrap(),
            (u64::MAX / 65536) * 65536
        );
    }

    #[test]
    fn to_amount_pairs_match_nicks_based_json() {
        // The ergonomic --to/--amount form must produce a RecipientSpec that is
        // byte-identical to the nicks-based --recipient JSON path for the same
        // address and equivalent nick amount. This is the core equivalence
        // guarantee that lets us treat the new flag as pure sugar. Amounts here
        // are already resolved to nicks (100 and 5 whole nocks).
        let tokens = to_amount_pairs_to_tokens(
            &[SAMPLE_P2PKH.to_string(), SAMPLE_P2PKH_ALT.to_string()],
            &[nocks_to_nicks(100).unwrap(), nocks_to_nicks(5).unwrap()],
        )
        .expect("pairs -> tokens");
        let ergonomic = recipient_tokens_to_specs(tokens).expect("ergonomic specs");

        let json_a = format!(
            "{{\"kind\":\"p2pkh\",\"address\":\"{SAMPLE_P2PKH}\",\"amount\":{}}}",
            100u64 * 65536
        );
        let json_b = format!(
            "{{\"kind\":\"p2pkh\",\"address\":\"{SAMPLE_P2PKH_ALT}\",\"amount\":{}}}",
            5u64 * 65536
        );
        let json = recipient_tokens_to_specs(vec![
            RecipientSpecToken::from_cli_arg(&json_a).unwrap(),
            RecipientSpecToken::from_cli_arg(&json_b).unwrap(),
        ])
        .expect("json specs");

        assert_eq!(ergonomic, json);
    }

    #[test]
    fn to_amount_pairs_reject_length_mismatch_empty_address_and_zero() {
        let err = to_amount_pairs_to_tokens(&[SAMPLE_P2PKH.to_string()], &[1, 2])
            .expect_err("count mismatch");
        assert!(err.contains("paired"), "unexpected: {err}");

        let err = to_amount_pairs_to_tokens(&["   ".to_string()], &[1]).expect_err("empty address");
        assert!(err.contains("cannot be empty"), "unexpected: {err}");

        let err =
            to_amount_pairs_to_tokens(&[SAMPLE_P2PKH.to_string()], &[0]).expect_err("zero amount");
        assert!(err.contains("greater than zero"), "unexpected: {err}");
    }

    #[test]
    fn recipient_spec_roundtrips_via_noun() {
        let specs = vec![
            RecipientSpec::P2pkh {
                address: Hash::from_base58(SAMPLE_P2PKH).expect("p2pkh hash"),
                amount: 10,
            },
            RecipientSpec::Multisig {
                threshold: 1,
                addresses: vec![
                    Hash::from_base58(SAMPLE_P2PKH_ALT).expect("alt hash"),
                    Hash::from_base58(SAMPLE_P2PKH).expect("p2pkh hash"),
                ],
                amount: 20,
            },
            RecipientSpec::BridgeDeposit {
                root: default_bridge_lock_root().expect("default bridge root"),
                evm_address: EthAddress::from_hex_str(SAMPLE_EVM_ADDRESS)
                    .expect("sample evm address"),
                amount: 30,
            },
        ];

        let mut slab = NounSlab::<NockJammer>::new();
        for spec in specs {
            let noun = spec.to_noun(&mut slab);
            let space = slab.noun_space();
            let decoded = RecipientSpec::from_noun(&noun, &space)
                .expect("recipient spec should decode from noun");
            assert_eq!(decoded, spec);
        }
    }
}
