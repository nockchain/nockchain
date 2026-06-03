use std::collections::BTreeSet;

use nockchain_types::common::Hash;
use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh, SpendCondition};
use nockchain_types::{EthAddress, EthAddressParseError};
use noun_serde::{NounDecode, NounEncode};
use serde::Deserialize;
use wallet_tx_builder::note_data::{PackedBlob, TypedNoteDataEntry};
use wallet_tx_builder::types::{PlannedOutput, RawNoteDataEntry};

use crate::{CrownError, NockAppError};

const MAX_BLOB_UTF8_BYTES: usize = 256 * 1024;

/// Trims and checks size; `None` if absent or empty/whitespace-only.
pub(crate) fn validate_blob_field(blob: Option<String>) -> Result<Option<Vec<u8>>, NockAppError> {
    let Some(raw) = blob else {
        return Ok(None);
    };
    let text = raw.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let bytes = text.as_bytes();
    if bytes.len() > MAX_BLOB_UTF8_BYTES {
        return Err(CrownError::Unknown(format!(
            "blob exceeds max size ({} UTF-8 bytes)",
            MAX_BLOB_UTF8_BYTES
        ))
        .into());
    }
    Ok(Some(bytes.to_vec()))
}

/// Validates optional UTF-8 memo text for per-recipient note-data (matches kernel limits).
pub(crate) fn validate_memo_utf8(memo: Option<&str>) -> Result<Option<Vec<u8>>, NockAppError> {
    let Some(memo) = memo else {
        return Ok(None);
    };
    let memo_bytes = memo.as_bytes();
    if memo_bytes.len() > wallet_tx_builder::note_data::MAX_MEMO_UTF8_BYTES {
        return Err(CrownError::Unknown(format!(
            "Memo too large: {} UTF-8 bytes (max {})",
            memo_bytes.len(),
            wallet_tx_builder::note_data::MAX_MEMO_UTF8_BYTES
        ))
        .into());
    }
    if memo_bytes.is_empty() {
        return Err(CrownError::Unknown(
            "Memo cannot be empty. Omit the memo field instead.".to_string(),
        )
        .into());
    }
    Ok(Some(memo_bytes.to_vec()))
}

pub const BRIDGE_LOCK_ROOT_DEFAULT_B58: &str =
    "AcsPkuhXQoGeEsF91yynpm1kcW17PQ2Z1MEozgx7YnDPkZwrtzLuuqd";

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RecipientSpecToken {
    P2pkh {
        address: String,
        amount: u64,
        #[serde(default)]
        memo: Option<String>,
        #[serde(default)]
        blob: Option<String>,
    },
    Multisig {
        threshold: u64,
        addresses: Vec<String>,
        amount: u64,
        #[serde(default)]
        memo: Option<String>,
        #[serde(default)]
        blob: Option<String>,
    },
    #[serde(rename = "bridge-deposit")]
    BridgeDeposit {
        #[serde(rename = "evm-address")]
        evm_address: String,
        amount: u64,
    },
}

#[derive(Debug, Clone, NounEncode, NounDecode, PartialEq)]
pub enum RecipientSpec {
    #[noun(tag = "pkh")]
    P2pkh {
        address: Hash,
        amount: u64,
        memo: Option<PackedBlob>,
        blob: Option<PackedBlob>,
    },
    #[noun(tag = "multisig")]
    Multisig {
        threshold: u64,
        addresses: Vec<Hash>,
        amount: u64,
        memo: Option<PackedBlob>,
        blob: Option<PackedBlob>,
    },
    #[noun(tag = "bridge-deposit")]
    BridgeDeposit {
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
            memo: None,
            blob: None,
        })
    }

    pub fn into_recipient_spec(self) -> Result<RecipientSpec, NockAppError> {
        match self {
            RecipientSpecToken::P2pkh {
                address,
                amount,
                memo,
                blob,
            } => {
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
                let memo = validate_memo_utf8(memo.as_deref())?;
                let blob = validate_blob_field(blob)?;
                Ok(RecipientSpec::P2pkh {
                    address: recipient,
                    amount,
                    memo: memo.map(PackedBlob),
                    blob: blob.map(PackedBlob),
                })
            }
            RecipientSpecToken::Multisig {
                threshold,
                addresses,
                amount,
                memo,
                blob,
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
                let memo = validate_memo_utf8(memo.as_deref())?;
                let blob = validate_blob_field(blob)?;
                Ok(RecipientSpec::Multisig {
                    threshold,
                    addresses: parsed,
                    amount,
                    memo: memo.map(PackedBlob),
                    blob: blob.map(PackedBlob),
                })
            }
            RecipientSpecToken::BridgeDeposit {
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
                Ok(RecipientSpec::BridgeDeposit {
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
        return Err(CrownError::Unknown("At least one --recipient must be provided".into()).into());
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
        RecipientSpec::P2pkh {
            address,
            amount,
            memo,
            blob,
        } => {
            let lock = pkh_lock(1, std::slice::from_ref(address));
            let mut note_data = if include_data {
                vec![RawNoteDataEntry::from_lock(lock.clone())]
            } else {
                Vec::new()
            };
            if let Some(packed) = memo {
                note_data.push(TypedNoteDataEntry::memo(packed.0.clone()).to_raw_entry());
            }
            if let Some(packed) = blob {
                note_data.push(TypedNoteDataEntry::blob(packed.0.clone()).to_raw_entry());
            }
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
            memo,
            blob,
        } => {
            let lock = pkh_lock(*threshold, addresses);
            let mut note_data = vec![RawNoteDataEntry::from_lock(lock.clone())];
            if let Some(packed) = memo {
                note_data.push(TypedNoteDataEntry::memo(packed.0.clone()).to_raw_entry());
            }
            if let Some(packed) = blob {
                note_data.push(TypedNoteDataEntry::blob(packed.0.clone()).to_raw_entry());
            }
            Ok(PlannedOutput {
                lock_root: lock_root(&lock)?,
                amount: *amount,
                // Hoon always includes lock note-data for multisig outputs.
                note_data: vec![RawNoteDataEntry::from_lock(lock.clone())],
            })
        }
        RecipientSpec::BridgeDeposit {
            evm_address,
            amount,
        } => Ok(PlannedOutput {
            lock_root: Hash::from_base58(BRIDGE_LOCK_ROOT_DEFAULT_B58).map_err(|err| {
                NockAppError::from(CrownError::Unknown(format!(
                    "Invalid bridge lock root constant '{}': {}",
                    BRIDGE_LOCK_ROOT_DEFAULT_B58, err
                )))
            })?,
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
    use noun_serde::{NounDecode, NounEncode};
    use wallet_tx_builder::note_data::{
        PackedBlob, NOTE_DATA_KEY_BLOB, NOTE_DATA_KEY_LOCK, NOTE_DATA_KEY_MEMO,
    };

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
                memo: None,
                blob: None,
            },
            RecipientSpecToken::Multisig {
                threshold: 1,
                addresses: vec![SAMPLE_P2PKH_ALT.to_string(), SAMPLE_P2PKH.to_string()],
                amount: 5,
                memo: None,
                blob: None,
            },
            RecipientSpecToken::BridgeDeposit {
                evm_address: SAMPLE_EVM_ADDRESS.to_string(),
                amount: 9,
            },
        ];
        let specs = recipient_tokens_to_specs(tokens).expect("tokens -> specs");
        assert_eq!(specs.len(), 3);
        match &specs[0] {
            RecipientSpec::P2pkh {
                address,
                amount,
                memo,
                blob,
            } => {
                assert_eq!(*amount, 1000);
                assert!(memo.is_none());
                assert!(blob.is_none());
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
                memo,
                blob,
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
                assert!(memo.is_none());
                assert!(blob.is_none());
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
        assert!(format!("{err}").contains("At least one --recipient"));
    }

    #[test]
    fn recipient_spec_roundtrips_via_noun() {
        let specs = vec![
            RecipientSpec::P2pkh {
                address: Hash::from_base58(SAMPLE_P2PKH).expect("p2pkh hash"),
                amount: 10,
                memo: Some(PackedBlob(b"memo utf8".to_vec())),
                blob: Some(PackedBlob(vec![0, 1, 2])),
            },
            RecipientSpec::Multisig {
                threshold: 1,
                addresses: vec![
                    Hash::from_base58(SAMPLE_P2PKH_ALT).expect("alt hash"),
                    Hash::from_base58(SAMPLE_P2PKH).expect("p2pkh hash"),
                ],
                amount: 20,
                memo: None,
                blob: None,
            },
            RecipientSpec::BridgeDeposit {
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

    #[test]
    fn parse_recipient_json_accepts_blob() {
        let raw = format!(
            r#"{{"kind":"p2pkh","address":"{}","amount":1,"blob":"nns.nock"}}"#,
            SAMPLE_P2PKH
        );
        let token = RecipientSpecToken::from_cli_arg(&raw).expect("json with blob");
        let spec = token.into_recipient_spec().expect("into spec");
        match spec {
            RecipientSpec::P2pkh { blob, .. } => {
                assert_eq!(blob.map(|b| b.0), Some(b"nns.nock".to_vec()));
            }
            _ => panic!("expected p2pkh"),
        }
    }

    #[test]
    fn parse_recipient_json_accepts_blob_kebab_case() {
        let raw = format!(
            r#"{{"kind":"p2pkh","address":"{}","amount":1,"blob":"nns.nock"}}"#,
            SAMPLE_P2PKH
        );
        let token = RecipientSpecToken::from_cli_arg(&raw).expect("json with blob data");
        let spec = token.into_recipient_spec().expect("into spec");
        match spec {
            RecipientSpec::P2pkh { blob, .. } => {
                assert_eq!(blob.map(|b| b.0), Some(b"nns.nock".to_vec()));
            }
            _ => panic!("expected p2pkh"),
        }
    }

    #[test]
    fn planner_p2pkh_inserts_blob_before_memo() {
        let address = Hash::from_base58(SAMPLE_P2PKH).expect("p2pkh hash");
        let recipients = vec![RecipientSpec::P2pkh {
            address,
            amount: 5,
            memo: Some(PackedBlob(b"hi".to_vec())),
            blob: Some(PackedBlob(vec![1])),
        }];
        let outputs = planner_recipient_outputs(&recipients, true).expect("planner outputs");
        assert_eq!(outputs.len(), 1);
        let keys: Vec<_> = outputs[0]
            .note_data
            .iter()
            .map(|e| e.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![NOTE_DATA_KEY_LOCK, NOTE_DATA_KEY_BLOB, NOTE_DATA_KEY_MEMO]
        );
    }
}
