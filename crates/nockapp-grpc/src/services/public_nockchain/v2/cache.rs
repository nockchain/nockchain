use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;
use std::time::Instant;

use nockapp_grpc_proto::pb::common::*;
use nockchain_types::tx_engine::{v0, v1};

use super::metrics::NockchainGrpcApiMetrics;
use crate::pb::common::v1::{ErrorCode, ErrorStatus};
use crate::pb::common::{v1 as pb_common_v1, v2 as pb_common_v2};
use crate::pb::public::v2::{wallet_get_balance_response, WalletGetBalanceResponse};
use crate::v2::pagination::{
    encode_cursor_address, encode_cursor_first_name, name_key, PageCursorAddress,
    PageCursorFirstName, PageKeyAddress, PageKeyFirstName,
};

pub const MAX_PAGE_BYTES: u64 = 3 * 1024 * 1024;
pub const MAX_PAGE_SIZE: usize = 1000;
pub const DEFAULT_PAGE_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_PAGE_SIZE: usize = 600;
const PER_ENTRY_OVERHEAD: usize = 8;

#[derive(Clone, Debug)]
pub struct AddressBalanceCache {
    map: dashmap::DashMap<PageKeyAddress, Arc<CachedBalanceEntryAddress>>,
}

impl AddressBalanceCache {
    pub fn new() -> Self {
        Self {
            map: dashmap::DashMap::new(),
        }
    }

    pub fn get(&self, key: &PageKeyAddress) -> Option<Arc<CachedBalanceEntryAddress>> {
        // Right now, we look for an exact match
        // In the future, we may want to return the latest entry if no exact match is found
        if let Some(entry) = self.map.get(key) {
            return Some(entry.clone());
        }
        None
    }

    pub fn insert(
        &self,
        address: &str,
        update: v0::BalanceUpdate,
    ) -> Arc<CachedBalanceEntryAddress> {
        let entry = Arc::new(CachedBalanceEntryAddress::from_update(
            address.to_string(),
            update,
        ));
        let key = PageKeyAddress::new(
            address.to_string(),
            entry.block_height_value,
            entry.block_id.clone(),
        );
        self.map.insert(key, entry.clone());
        entry
    }
}

#[derive(Debug)]
pub struct CachedBalanceEntryAddress {
    address: String,
    block_height: v0::BlockHeight,
    block_height_value: u64,
    block_id: v0::Hash,
    notes: BTreeMap<NameKey, (v0::Name, v0::NoteV0)>,
    // We can leave this field here for future use
    #[allow(dead_code)]
    inserted_at: Instant,
}

impl CachedBalanceEntryAddress {
    fn from_update(address: String, update: v0::BalanceUpdate) -> Self {
        let block_height_value = update.height.0 .0;
        let mut notes = BTreeMap::new();
        for (name, note) in update.notes.0.into_iter() {
            notes.insert(NameKey::from_name(&name), (name, note));
        }

        Self {
            address,
            block_height: update.height,
            block_height_value,
            block_id: update.block_id,
            notes,
            inserted_at: Instant::now(),
        }
    }

    pub fn build_paginated_response_address(
        &self,
        cursor: Option<PageCursorAddress>,
        client_page_items_limit: usize,
        max_bytes: u64,
        metrics: &Arc<NockchainGrpcApiMetrics>,
    ) -> std::result::Result<WalletGetBalanceResponse, ErrorStatus> {
        if client_page_items_limit > MAX_PAGE_SIZE || max_bytes > MAX_PAGE_BYTES {
            metrics
                .balance_request_error_invalid_request_limit_exceeded
                .increment();
            let err = ErrorStatus {
                code: ErrorCode::InvalidRequest as i32,
                message: "client_page_items_limit or max bytes exceeds maximum allowed".into(),
                details: None,
            };
            return Err(err);
        }

        let range_start = match cursor {
            Some(ref cur) => {
                let height_ok = cur.key.height == self.block_height_value;
                let block_ok = cur.key.block_id == self.block_id;
                if !height_ok || !block_ok {
                    metrics
                        .balance_request_error_invalid_request_snapshot_mismatch
                        .increment();
                    let err = ErrorStatus {
                        code: ErrorCode::InvalidRequest as i32,
                        message: "Page token does not match current snapshot; restart pagination"
                            .into(),
                        details: None,
                    };
                    return Err(err);
                }
                Bound::Excluded(NameKey::from_cursor_address(cur))
            }
            None => Bound::Unbounded,
        };

        let mut pb_notes: Vec<v2::BalanceEntry> =
            Vec::with_capacity(client_page_items_limit as usize);
        let mut total_bytes = 0usize;
        let mut last_name: Option<v1::Name> = None;
        let mut has_more = false;

        let mut iter = self.notes.range((range_start, Bound::Unbounded)).peekable();

        while let Some((_key, (name, note))) = iter.next() {
            let balance_entry = v2::BalanceEntry {
                name: Some(pb_common_v1::Name::from(name.clone())),
                note: Some(pb_common_v2::Note {
                    note_version: Some(pb_common_v2::note::NoteVersion::Legacy(
                        pb_common_v1::Note::from(note.clone()),
                    )),
                }),
            };

            let entry_len =
                <pb_common_v2::BalanceEntry as prost::Message>::encoded_len(&balance_entry)
                    + PER_ENTRY_OVERHEAD;
            if !pb_notes.is_empty() && total_bytes.saturating_add(entry_len) > max_bytes as usize {
                has_more = true;
                break;
            }

            total_bytes = total_bytes.saturating_add(entry_len);
            last_name = Some(name.clone());
            pb_notes.push(balance_entry);

            if pb_notes.len() >= client_page_items_limit {
                has_more = iter.peek().is_some();
                break;
            }
        }

        let next_page_token = if has_more {
            if let Some(ref last_name) = last_name {
                let cur = PageCursorAddress::new(
                    self.address.clone(),
                    &self.block_height,
                    &self.block_id,
                    last_name,
                );
                encode_cursor_address(&cur)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        Ok(WalletGetBalanceResponse {
            result: Some(wallet_get_balance_response::Result::Balance(
                pb_common_v2::Balance {
                    notes: pb_notes,
                    height: Some(pb_common_v1::BlockHeight::from(self.block_height.clone())),
                    block_id: Some(pb_common_v1::Hash::from(self.block_id.clone())),
                    page: Some(pb_common_v1::PageResponse { next_page_token }),
                },
            )),
        })
    }
}

#[derive(Clone, Debug)]
pub struct FirstNameBalanceCache {
    map: dashmap::DashMap<PageKeyFirstName, Arc<CachedBalanceEntryFirstName>>,
}

impl FirstNameBalanceCache {
    pub fn new() -> Self {
        Self {
            map: dashmap::DashMap::new(),
        }
    }

    pub fn get(&self, key: &PageKeyFirstName) -> Option<Arc<CachedBalanceEntryFirstName>> {
        // Right now, we look for an exact match
        // In the future, we may want to return the latest entry if no exact match is found
        if let Some(entry) = self.map.get(key) {
            return Some(entry.clone());
        }
        None
    }

    pub fn insert(
        &self,
        first_name: &v1::Hash,
        update: v1::BalanceUpdate,
    ) -> Arc<CachedBalanceEntryFirstName> {
        let entry = Arc::new(CachedBalanceEntryFirstName::from_update(first_name, update));
        let key = PageKeyFirstName::new(
            first_name.clone(),
            entry.block_height_value,
            entry.block_id.clone(),
        );
        self.map.insert(key, entry.clone());
        entry
    }
}

#[derive(Debug)]
pub struct CachedBalanceEntryFirstName {
    first_name: v1::Hash,
    block_height: v1::BlockHeight,
    block_height_value: u64,
    block_id: v1::Hash,
    notes: BTreeMap<NameKey, (v1::Name, v1::Note)>,
    // We can leave this field here for future use
    #[allow(dead_code)]
    inserted_at: Instant,
}

impl CachedBalanceEntryFirstName {
    fn from_update(first_name: &v1::Hash, update: v1::BalanceUpdate) -> Self {
        let block_height_value = update.height.0 .0;
        let mut notes = BTreeMap::new();
        for (name, note) in update.notes.0.into_iter() {
            notes.insert(NameKey::from_name(&name), (name, note));
        }

        Self {
            first_name: first_name.clone(),
            block_height: update.height,
            block_height_value,
            block_id: update.block_id,
            notes,
            inserted_at: Instant::now(),
        }
    }

    pub fn build_paginated_response_first_name(
        &self,
        cursor: Option<PageCursorFirstName>,
        client_page_items_limit: usize,
        max_bytes: u64,
        metrics: &Arc<NockchainGrpcApiMetrics>,
    ) -> std::result::Result<WalletGetBalanceResponse, ErrorStatus> {
        if client_page_items_limit > MAX_PAGE_SIZE || max_bytes > MAX_PAGE_BYTES {
            metrics
                .balance_request_error_invalid_request_limit_exceeded
                .increment();
            let err = ErrorStatus {
                code: ErrorCode::InvalidRequest as i32,
                message: "client_page_items_limit or max bytes exceeds maximum allowed".into(),
                details: None,
            };
            return Err(err);
        }

        let range_start = match cursor {
            Some(ref cur) => {
                let height_ok = cur.key.height == self.block_height_value;
                let block_ok = cur.key.block_id == self.block_id;
                if !height_ok || !block_ok {
                    metrics
                        .balance_request_error_invalid_request_snapshot_mismatch
                        .increment();
                    let err = ErrorStatus {
                        code: ErrorCode::InvalidRequest as i32,
                        message: "Page token does not match current snapshot; restart pagination"
                            .into(),
                        details: None,
                    };
                    return Err(err);
                }
                Bound::Excluded(NameKey::from_cursor_first_name(cur))
            }
            None => Bound::Unbounded,
        };

        let mut pb_notes: Vec<v2::BalanceEntry> =
            Vec::with_capacity(client_page_items_limit as usize);
        let mut total_bytes = 0usize;
        let mut last_name: Option<v1::Name> = None;
        let mut has_more = false;

        let mut iter = self.notes.range((range_start, Bound::Unbounded)).peekable();

        while let Some((_key, (name, note))) = iter.next() {
            let balance_entry = v2::BalanceEntry {
                name: Some(pb_common_v1::Name::from(name.clone())),
                note: Some(pb_common_v2::Note::from(note.clone())),
            };

            let entry_len =
                <pb_common_v2::BalanceEntry as prost::Message>::encoded_len(&balance_entry)
                    + PER_ENTRY_OVERHEAD;
            if !pb_notes.is_empty() && total_bytes.saturating_add(entry_len) > max_bytes as usize {
                has_more = true;
                break;
            }

            total_bytes = total_bytes.saturating_add(entry_len);
            last_name = Some(name.clone());
            pb_notes.push(balance_entry);

            if pb_notes.len() >= client_page_items_limit {
                has_more = iter.peek().is_some();
                break;
            }
        }

        let next_page_token = if has_more {
            if let Some(ref last_name) = last_name {
                let cur = PageCursorFirstName::new(
                    &self.first_name, &self.block_height, &self.block_id, last_name,
                );
                encode_cursor_first_name(&cur)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        Ok(WalletGetBalanceResponse {
            result: Some(wallet_get_balance_response::Result::Balance(
                pb_common_v2::Balance {
                    notes: pb_notes,
                    height: Some(pb_common_v1::BlockHeight::from(self.block_height.clone())),
                    block_id: Some(pb_common_v1::Hash::from(self.block_id.clone())),
                    page: Some(pb_common_v1::PageResponse { next_page_token }),
                },
            )),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct NameKey {
    first: [u64; 5],
    last: [u64; 5],
}

impl NameKey {
    fn from_name(name: &v1::Name) -> Self {
        let (first, last) = name_key(name);
        Self { first, last }
    }

    fn from_cursor_address(cursor: &PageCursorAddress) -> Self {
        Self {
            first: cursor.last_first.to_array(),
            last: cursor.last_last.to_array(),
        }
    }

    fn from_cursor_first_name(cursor: &PageCursorFirstName) -> Self {
        Self {
            first: cursor.last_first.to_array(),
            last: cursor.last_last.to_array(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::public_nockchain::v1::fixtures;
    use crate::services::public_nockchain::v2::metrics::init_metrics;
    use crate::v2::pagination::{cmp_name, decode_cursor_address};

    const PAGE_SIZE: usize = 2;

    #[tokio::test]
    async fn cache_paginates_without_duplicates() {
        let cache = AddressBalanceCache::new();
        let (update, mut names) = fixtures::make_balance_update(1000);

        let end = names[names.len() - 1].clone();

        let entry = cache.insert("test-address", update.clone());
        let cursor = PageCursorAddress {
            key: PageKeyAddress {
                address: "test-address".to_string(),
                height: update.height.0 .0,
                block_id: update.block_id,
            },
            last_first: end.first,
            last_last: end.last,
        };
        assert!(cache.get(cursor.key()).is_some());

        names.sort_by(cmp_name);
        let expected: Vec<pb_common_v1::Name> = names
            .iter()
            .map(|n| pb_common_v1::Name::from(n.clone()))
            .collect();

        let mut cursor: Option<PageCursorAddress> = None;
        let mut collected = Vec::new();

        let metrics = init_metrics();

        loop {
            let response = entry
                .build_paginated_response_address(cursor.clone(), PAGE_SIZE, 0, &metrics)
                .expect("pagination should succeed");

            let balance = match response.result {
                Some(wallet_get_balance_response::Result::Balance(balance)) => balance,
                _ => panic!("expected balance data"),
            };

            for note in balance.notes {
                collected.push(note.name.expect("balance entry missing name"));
            }

            let next_token = balance
                .page
                .and_then(|p| Some(p.next_page_token))
                .unwrap_or_default();

            if next_token.is_empty() {
                break;
            }

            cursor =
                Some(decode_cursor_address(&next_token).expect("cursor decode should succeed"));

            let cursor_key = cursor.as_ref().expect("expected cursor").key();
            assert!(cache.get(cursor_key).is_some());
        }

        assert_eq!(
            collected, expected,
            "paginated view should match expected order"
        );
    }

    #[tokio::test]
    async fn cache_respects_max_byte_budget() {
        let cache = AddressBalanceCache::new();
        let (update, names) = fixtures::make_balance_update(3);
        let entry = cache.insert("addr", update.clone());

        let (first_name, first_note) = update.notes.0.first().cloned().expect("at least one note");

        let first_entry = pb_common_v2::BalanceEntry {
            name: Some(pb_common_v1::Name::from(first_name)),
            note: Some(pb_common_v2::Note {
                note_version: Some(pb_common_v2::note::NoteVersion::Legacy(
                    pb_common_v1::Note::from(first_note),
                )),
            }),
        };

        let first_entry_len =
            <pb_common_v2::BalanceEntry as prost::Message>::encoded_len(&first_entry) + 8;

        // Allow one entry but not the second by choosing a byte budget that leaves room
        // for exactly the first encoded note
        let metrics = init_metrics();

        let response = entry
            .build_paginated_response_address(None, names.len(), first_entry_len as u64, &metrics)
            .expect("build paginated response");

        let balance = match response.result {
            Some(wallet_get_balance_response::Result::Balance(balance)) => balance,
            _ => panic!("expected balance result"),
        };

        assert_eq!(balance.notes.len(), 1, "byte budget should limit entries");
        assert!(
            !balance.page.expect("page info").next_page_token.is_empty(),
            "remaining entries should produce a continuation token"
        );
    }

    #[tokio::test]
    async fn cache_respects_client_page_items_limit() {
        let cache = AddressBalanceCache::new();
        let (update, names) = fixtures::make_balance_update(5);
        let entry = cache.insert("addr", update.clone());

        let client_page_items_limit = 2;

        let mut expected_names = names.clone();
        expected_names.sort_by(cmp_name);
        let expected_pb: Vec<pb_common_v1::Name> = expected_names
            .iter()
            .map(|n| pb_common_v1::Name::from(n.clone()))
            .collect();

        let mut cursor: Option<PageCursorAddress> = None;
        let mut offset = 0usize;
        let mut page_index = 0usize;

        let metrics = init_metrics();

        loop {
            let response = entry
                .build_paginated_response_address(
                    cursor.clone(),
                    client_page_items_limit,
                    MAX_PAGE_BYTES,
                    &metrics,
                )
                .expect("build paginated response");

            let balance = match response.result {
                Some(wallet_get_balance_response::Result::Balance(balance)) => balance,
                _ => panic!("expected balance result"),
            };

            let page_names: Vec<pb_common_v1::Name> = balance
                .notes
                .iter()
                .map(|entry| entry.name.clone().expect("balance entry missing name"))
                .collect();

            assert!(
                page_names.len() <= client_page_items_limit,
                "page {} should not exceed declared client_page_items_limit",
                page_index
            );
            if page_index == 0 {
                assert_eq!(
                    page_names.len(),
                    client_page_items_limit,
                    "first page should be capped by client_page_items_limit when enough entries exist"
                );
            }

            let expected_slice = &expected_pb[offset..offset + page_names.len()];
            assert_eq!(
                page_names, expected_slice,
                "page {} contents incorrect",
                page_index
            );
            offset += page_names.len();

            let next_token = balance.page.expect("page info present").next_page_token;

            if next_token.is_empty() {
                break;
            }

            cursor =
                Some(decode_cursor_address(&next_token).expect("cursor decode should succeed"));
            page_index += 1;
        }

        assert_eq!(offset, expected_pb.len(), "should traverse all entries");
    }
}
