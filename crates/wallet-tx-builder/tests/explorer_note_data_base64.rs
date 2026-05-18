//! Web explorer JSON exposes `%memo` and `%blob` note-data entries as **standard Base64**
//! (RFC 4648) of the jammed noun: length-prefixed packed belts (`encode_blob_belts`), matching
//! [`TypedNoteDataEntry::memo`] / [`TypedNoteDataEntry::blob`].

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use bytes::Bytes;
use nockchain_types::common::Hash;
use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh, SpendCondition};
use wallet_tx_builder::note_data::{MemoDataPayload, TypedNoteDataEntry};

/// Explorer snapshot: jam bytes for `%memo` (UTF-8 body is the long NNS description from create-tx).
const EXPLORER_MEMO_B64: &str = "gbEb0KnQysCA7/Q2tg74Y2hh6YDvBuIUDvhtaW7ngA8ylTcP6HRlbWDAR3Fymgb8FJCwdMBvmwOhB/w3ELFywB97aysHfBA6tHLAB0Grawf8MLcWecAvCyMLB3QxtjIw4LuFtaUDvtsZSDjgu5GB2AN+mVyaOeCnjYXRA3ramxsY8NnC8soBXw7E7B3w5UCc3gF/bG0MHdDD0tzcgA9iJBcO6G1lZGDAD5sDCQd8ELg6cUBnSxsDA/7Z2xs54IOYvckDPohbWDvgl4HIlQP+3BtbPeDTpb25Az4LSFs94I+hgbADetpaGRjQi5ymwIBP9lY2b7jzBnyQIjp0wC+TK6sH/DYWkHTApwNpCwd8uDkQecAPuwMJB3wyMrlywJ+bK5sHdBC6NzDgt5W1vQO+XJgYO+CXgbi4A/rb2BoY8N3C2soBfw4EpRzQXc5c2IAP4vY2DvhrdG/zgI+W5uIO6G9ja2lAB5l7AwN+3VyZPODPsYDcA34YG1s5oNPNsYABPcyNDAz4YXBw84APMhbmDvggc2XugE/GAkIO+GlzcOyAHpbHAjak/gZ8uzK5dMA3ywNJB3yyMjd6QE+jywMD/l0aHTqgv9XRgQGfTq5uHvDp0tzOAR8kDGQe8NPcztgBvwzkDR7wy+TC6AH/Te5kHvBByMLoAT9MLGwe6DJX";

/// Explorer snapshot: jam bytes for `%blob` (`nns/v1/claim/nns.nock`).
const EXPLORER_BLOB_B64: &str = "wWpAd3ObewO+XczLOOCzhaW1A/6Lm9s84Lu4vY2DrwU=";

#[test]
fn explorer_memo_base64_round_trips_canonical_jam() {
    let jam = Bytes::from(STANDARD.decode(EXPLORER_MEMO_B64).expect("memo base64"));
    let parsed = MemoDataPayload::from_blob(&jam).expect("explorer memo jam should cue");
    let entry = TypedNoteDataEntry::memo(parsed.bytes.clone()).to_raw_entry();
    assert_eq!(entry.key, "memo");
    assert_eq!(entry.blob, jam, "re-jam must match explorer jam bytes");
    assert_eq!(
        STANDARD.encode(entry.blob),
        EXPLORER_MEMO_B64,
        "STANDARD base64 of jam must match explorer JSON"
    );
    let text = std::str::from_utf8(&parsed.bytes).expect("memo utf-8");
    assert!(
        text.contains("Nockchain Naming System")
            || text.contains("the human-readable naming and verification layer for Nockchain"),
        "sanity: expected NNS marketing memo body"
    );
}

#[test]
fn explorer_blob_base64_round_trips_canonical_jam() {
    let jam = Bytes::from(STANDARD.decode(EXPLORER_BLOB_B64).expect("blob base64"));
    let parsed = MemoDataPayload::from_blob(&jam).expect("explorer blob jam should cue");
    let entry = TypedNoteDataEntry::blob(parsed.bytes.clone()).to_raw_entry();
    assert_eq!(entry.key, "blob");
    assert_eq!(entry.blob, jam);
    assert_eq!(STANDARD.encode(entry.blob), EXPLORER_BLOB_B64);
    assert_eq!(parsed.bytes.as_slice(), b"nns/v1/claim/nns.nock");
}

/// Lock in explorer JSON is structured; on the wire it is still jammed `[%0 lock]` like the wallet.
#[test]
fn explorer_lock_pkh_jam_is_stable_for_sample_address() {
    let hash = Hash::from_base58("8s29XUK8Do7QWt2MHfPdd1gDSta6db4c3bQrxP1YdJNfXpL3WPzTT5")
        .expect("sample p2pkh");
    let lock = Lock::SpendCondition(SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        1,
        vec![hash],
    ))]));
    let entry = TypedNoteDataEntry::lock(lock).to_raw_entry();
    assert_eq!(entry.key, "lock");
    assert!(
        entry.blob.len() > 32,
        "lock jam should be non-trivial; explorer embeds structured JSON, not this base64"
    );
}
