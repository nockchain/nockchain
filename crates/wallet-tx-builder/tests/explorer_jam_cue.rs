//! Round-trip for canonical blob note-data (`%memo` and `%blob` keys): encode → jam → cue → same bytes.

use bytes::Bytes;
use wallet_tx_builder::note_data::{MemoDataPayload, TypedNoteDataEntry};

const BLOB: &str = "nns/v1/claim/nns.nock";
const MEMO: &str =
    "NYT 4/27/2026 Trump Is Dissatisfied With Iran\u{2019}s Plan to Reopen Strait of Hormuz";

#[test]
fn canonical_memo_and_blob_jams_roundtrip() {
    let blob_entry = TypedNoteDataEntry::blob(BLOB.as_bytes().to_vec()).to_raw_entry();
    let memo_entry = TypedNoteDataEntry::memo(MEMO.as_bytes().to_vec()).to_raw_entry();

    let blob = MemoDataPayload::from_blob(&blob_entry.blob).expect("blob");
    let memo = MemoDataPayload::from_blob(&memo_entry.blob).expect("memo");

    assert_eq!(blob.bytes.as_slice(), BLOB.as_bytes());
    assert_eq!(memo.bytes.as_slice(), MEMO.as_bytes());
}

#[test]
fn from_blob_rejects_per_byte_memo_jam() {
    // Per-byte `(list @ux)` jam (noun-serde `Vec<u8>`), not length-prefixed belts.
    let legacy_jam: &[u8] = &[
        193, 157, 193, 179, 193, 169, 65, 193, 160, 116, 208, 55, 168, 28, 244, 14, 250, 6, 149,
        131, 194, 65, 229, 160, 118, 80, 176, 81, 49, 184, 60, 120, 61, 120, 59, 56, 60, 40, 24,
        60, 25, 124, 30, 20, 12, 78, 12, 158, 14, 62, 15, 62, 15, 30, 14, 78, 15, 158, 14, 62, 15,
        110, 14, 158, 14, 94, 14, 78, 14, 10, 6, 191, 6, 79, 7, 167, 7, 71, 7, 5, 131, 39, 131,
        203, 131, 135, 131, 187, 3, 33, 30, 8, 192, 64, 100, 6, 159, 7, 5, 131, 67, 131, 179, 131,
        135, 131, 187, 131, 130, 193, 233, 193, 223, 65, 193, 224, 210, 224, 229, 224, 239, 224,
        240, 224, 229, 224, 238, 160, 96, 240, 105, 112, 122, 112, 121, 240, 112, 240, 116, 112,
        122, 80, 48, 248, 59, 184, 57, 40, 24, 28, 25, 252, 29, 92, 30, 188, 29, 188, 30, 92, 47,
    ];
    let err = MemoDataPayload::from_blob(&Bytes::copy_from_slice(legacy_jam)).unwrap_err();
    assert!(
        err.to_string().contains("length-prefixed"),
        "unexpected err: {err}"
    );
}
