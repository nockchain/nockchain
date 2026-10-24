//use futures::{stream, Stream};
use nockchain_types::tx_engine::v0;
use tonic::transport::Channel;

use crate::error::{NockAppGrpcError, Result};
use crate::pb::common::v1 as pb_common;
use crate::pb::common::v1::PageRequest;
use crate::pb::public::v1::nockchain_service_client::NockchainServiceClient as PublicNockchainClient;
use crate::pb::public::v1::*;

#[derive(Clone)]
pub struct PublicNockchainGrpcClient {
    client: PublicNockchainClient<Channel>,
}

impl PublicNockchainGrpcClient {
    pub async fn connect<T: AsRef<str>>(address: T) -> Result<Self> {
        let client = PublicNockchainClient::connect(address.as_ref().to_string()).await?;
        Ok(Self { client })
    }

    // Simple autopager: fetches all pages and aggregates notes client-side.
    // Returns the combined WalletBalanceData with an empty next_page_token.
    pub async fn wallet_get_balance(
        &mut self,
        address: String,
    ) -> Result<crate::pb::common::v1::WalletBalanceData> {
        let mut page_token = String::new();
        let mut all_notes: Vec<pb_common::BalanceEntry> = Vec::new();
        let mut height: Option<pb_common::BlockHeight> = None;
        let mut block_id: Option<pb_common::Hash> = None;

        loop {
            let req = WalletGetBalanceRequest {
                address: address.clone(),
                page: Some(PageRequest {
                    client_page_items_limit: 0, // let server choose default/cap
                    page_token: page_token.clone(),
                    max_bytes: 0,
                }),
            };
            let resp = self.client.wallet_get_balance(req).await?.into_inner();
            let balance = match resp.result {
                Some(wallet_get_balance_response::Result::Balance(b)) => b,
                Some(wallet_get_balance_response::Result::Error(e)) => {
                    return Err(NockAppGrpcError::Internal(e.message))
                }
                None => return Err(NockAppGrpcError::Internal("Empty response".into())),
            };

            if height.is_none() {
                height = balance.height.clone();
                block_id = balance.block_id.clone();
            }

            if balance.height != height || balance.block_id != block_id {
                return Err(NockAppGrpcError::Internal(
                    "Snapshot changed during pagination; retry".into(),
                ));
            }

            all_notes.extend(balance.notes.into_iter());
            page_token = balance
                .page
                .and_then(|p| {
                    if p.next_page_token.is_empty() {
                        None
                    } else {
                        Some(p.next_page_token)
                    }
                })
                .unwrap_or_default();

            if page_token.is_empty() {
                break;
            }
        }

        Ok(pb_common::WalletBalanceData {
            notes: all_notes,
            height,
            block_id,
            page: Some(pb_common::PageResponse {
                next_page_token: String::new(),
            }),
        })
    }

    pub async fn wallet_send_transaction(
        &mut self,
        raw_tx: v0::RawTx,
    ) -> Result<WalletSendTransactionResponse> {
        let pb_tx_id = pb_common::Hash::from(raw_tx.id.clone());
        let pb_raw_tx = pb_common::RawTransaction::from(raw_tx);

        let request = WalletSendTransactionRequest {
            tx_id: Some(pb_tx_id),
            raw_tx: Some(pb_raw_tx),
        };

        let response = self
            .client
            .wallet_send_transaction(request)
            .await?
            .into_inner();

        match response.result {
            Some(wallet_send_transaction_response::Result::Ack(_)) => Ok(response),
            Some(wallet_send_transaction_response::Result::Error(err)) => {
                Err(NockAppGrpcError::Internal(err.message))
            }
            None => Err(NockAppGrpcError::Internal("Empty response".into())),
        }
    }

    pub async fn transaction_accepted(
        &mut self,
        tx_id: pb_common::Base58Hash,
    ) -> Result<TransactionAcceptedResponse> {
        let request = TransactionAcceptedRequest { tx_id: Some(tx_id) };
        let response = self
            .client
            .transaction_accepted(request)
            .await?
            .into_inner();

        match response.result {
            Some(transaction_accepted_response::Result::Accepted(_)) => Ok(response),
            Some(transaction_accepted_response::Result::Error(err)) => {
                Err(NockAppGrpcError::Internal(err.message))
            }
            None => Err(NockAppGrpcError::Internal("Empty response".into())),
        }
    }

    // pub async fn transaction_confirmation(
    //     &mut self,
    //     tx_id: pb_common::Base58Hash,
    // ) -> Result<TransactionConfirmationResponse> {
    //     let request = TransactionConfirmationRequest { tx_id: Some(tx_id) };
    //     let response = self.client.transaction_confirmation(request).await?;
    //     Ok(response.into_inner())
    // }

    // Returns a stream of BalanceEntry across all pages.
    // The stream yields one entry at a time; it fetches the next page when needed.
    // pub fn wallet_get_balance_stream(
    //     &self,
    //     pid: i32,
    //     address: String,
    //     client_page_items_limit: Option<u32>,
    //     max_bytes: Option<u64>,
    // ) -> impl Stream<Item = Result<pb_common::BalanceEntry>> {
    //     // Clone the inner tonic client so the stream can own it independently.
    //     let client = self.client.clone();
    //     let client_page_items_limit = client_page_items_limit.unwrap_or(0);
    //     let max_bytes = max_bytes.unwrap_or(0);

    //     stream::unfold(
    //         Some((
    //             client,
    //             address,
    //             String::new(),
    //             Vec::<pb_common::BalanceEntry>::new(),
    //             0usize,
    //             pid,
    //         )),
    //         move |state| async move {
    //             let (mut client, address, mut next_page_token, mut buf, mut idx, pid) = state?;

    //             // If we have buffered entries, yield the next one.
    //             if idx < buf.len() {
    //                 let item = Ok(buf[idx].clone());
    //                 idx += 1;
    //                 return Some((
    //                     item,
    //                     Some((client, address, next_page_token, buf, idx, pid)),
    //                 ));
    //             }

    //             // Need to fetch another page. If token is empty and buffer was empty once, this is first page.
    //             let req = WalletGetBalanceRequest {
    //                 pid,
    //                 address: address.clone(),
    //                 page: Some(PageRequest {
    //                     client_page_items_limit,
    //                     page_token: next_page_token.clone(),
    //                     max_bytes,
    //                 }),
    //             };

    //             let resp = match client.wallet_get_balance(req).await {
    //                 Ok(r) => r.into_inner(),
    //                 Err(e) => return Some((Err(e.into()), None)),
    //             };

    //             let balance = match resp.result {
    //                 Some(wallet_get_balance_response::Result::Balance(b)) => b,
    //                 Some(wallet_get_balance_response::Result::Error(e)) => {
    //                     return Some((Err(NockAppGrpcError::Internal(e.message)), None))
    //                 }
    //                 None => {
    //                     return Some((
    //                         Err(NockAppGrpcError::Internal("Empty response".into())),
    //                         None,
    //                     ))
    //                 }
    //             };

    //             // Load buffer and update token
    //             buf = balance.notes;
    //             idx = 0;
    //             next_page_token = balance
    //                 .page
    //                 .and_then(|p| {
    //                     if p.next_page_token.is_empty() {
    //                         None
    //                     } else {
    //                         Some(p.next_page_token)
    //                     }
    //                 })
    //                 .unwrap_or_default();

    //             if buf.is_empty() {
    //                 // No items returned; if there is no next token either, end stream.
    //                 if next_page_token.is_empty() {
    //                     return None;
    //                 }
    //                 // Otherwise, loop to fetch next page.
    //                 return Some((
    //                     Err(NockAppGrpcError::Internal("Empty page returned".into())),
    //                     None,
    //                 ));
    //             }

    //             // Yield first entry from the freshly loaded buffer
    //             let item = Ok(buf[idx].clone());
    //             idx += 1;
    //             Some((
    //                 item,
    //                 Some((client, address, next_page_token, buf, idx, pid)),
    //             ))
    //         },
    //     )
    // }
}
