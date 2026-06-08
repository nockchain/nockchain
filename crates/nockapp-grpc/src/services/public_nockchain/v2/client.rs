use nockapp_grpc_proto::pb::common::v1::{Base58Hash, Base58Pubkey};
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Hash;
use nockchain_types::tx_engine::v1;
use tonic::transport::Channel;
use tonic::Code;

use crate::error::{NockAppGrpcError, Result};
use crate::pb::common::v1::PageRequest;
use crate::pb::common::{v1 as pb_common_v1, v2 as pb_common_v2};
use crate::pb::public::v2::nockchain_block_service_client::NockchainBlockServiceClient;
use crate::pb::public::v2::nockchain_metrics_service_client::NockchainMetricsServiceClient;
use crate::pb::public::v2::nockchain_service_client::NockchainServiceClient as PublicNockchainClient;
use crate::pb::public::v2::*;

#[derive(Clone)]
pub struct PublicNockchainGrpcClient {
    client: PublicNockchainClient<Channel>,
    block_client: NockchainBlockServiceClient<Channel>,
    metrics_client: NockchainMetricsServiceClient<Channel>,
}

pub enum BalanceRequest {
    Address(String),
    FirstName(String),
}

impl PublicNockchainGrpcClient {
    /// Connects a public Nockchain client that can issue both wallet RPCs and
    /// transaction-to-block lookup RPCs over the same channel.
    pub async fn connect<T: AsRef<str>>(address: T) -> Result<Self> {
        let channel = tonic::transport::Endpoint::new(address.as_ref().to_string())?
            .connect()
            .await?;
        let client = PublicNockchainClient::new(channel.clone());
        let block_client = NockchainBlockServiceClient::new(channel.clone());
        let metrics_client = NockchainMetricsServiceClient::new(channel);
        Ok(Self {
            client,
            block_client,
            metrics_client,
        })
    }

    // Simple autopager: fetches all pages and aggregates notes client-side.
    // Returns the combined WalletBalanceData with an empty next_page_token.
    pub async fn wallet_get_balance(
        &mut self,
        request: &BalanceRequest,
    ) -> Result<crate::pb::common::v2::Balance> {
        let mut page_token = String::new();
        let mut all_notes: Vec<pb_common_v2::BalanceEntry> = Vec::new();
        let mut height: Option<pb_common_v1::BlockHeight> = None;
        let mut block_id: Option<pb_common_v1::Hash> = None;

        loop {
            let sel = match request {
                BalanceRequest::Address(addr) => {
                    wallet_get_balance_request::Selector::Address(Base58Pubkey {
                        key: addr.clone(),
                    })
                }
                BalanceRequest::FirstName(fname) => {
                    wallet_get_balance_request::Selector::FirstName(Base58Hash {
                        hash: fname.clone(),
                    })
                }
            };
            let req = WalletGetBalanceRequest {
                selector: Some(sel),
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

        Ok(pb_common_v2::Balance {
            notes: all_notes,
            height,
            block_id,
            page: Some(pb_common_v1::PageResponse {
                next_page_token: String::new(),
            }),
        })
    }

    /// Submits a raw transaction through the public wallet API.
    pub async fn wallet_send_transaction(
        &mut self,
        raw_tx: v1::RawTx,
    ) -> Result<WalletSendTransactionResponse> {
        let pb_tx_id = pb_common_v1::Hash::from(raw_tx.id.clone());
        let pb_raw_tx = pb_common_v2::RawTransaction::from(raw_tx);

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

    /// Queries whether the public API currently considers the transaction
    /// accepted.
    pub async fn transaction_accepted(
        &mut self,
        tx_id: pb_common_v1::Base58Hash,
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

    /// Looks up the block currently containing a submitted transaction.
    ///
    /// The method returns `Ok(None)` while the tx is still pending or unknown,
    /// and returns the current inclusion `(height, block_id)` once the block
    /// index can resolve it.
    pub async fn get_transaction_block(
        &mut self,
        tx_id: pb_common_v1::Base58Hash,
    ) -> Result<Option<(u64, Hash)>> {
        let request = GetTransactionBlockRequest { tx_id: Some(tx_id) };
        let response = match self.block_client.get_transaction_block(request).await {
            Ok(response) => response.into_inner(),
            Err(err) if err.code() == Code::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        match response.result {
            Some(get_transaction_block_response::Result::Block(block)) => {
                let block_id = block
                    .block_id
                    .ok_or_else(|| NockAppGrpcError::Internal("missing block_id".into()))
                    .and_then(pb_hash_to_domain_hash)?;
                Ok(Some((block.height, block_id)))
            }
            Some(get_transaction_block_response::Result::Pending(_)) => Ok(None),
            Some(get_transaction_block_response::Result::Error(err)) => {
                Err(NockAppGrpcError::Internal(err.message))
            }
            None => Err(NockAppGrpcError::Internal("Empty response".into())),
        }
    }

    /// Reads the public explorer cache's current heaviest height.
    pub async fn explorer_heaviest_height(&mut self) -> Result<u64> {
        let response = self
            .metrics_client
            .get_explorer_metrics(GetExplorerMetricsRequest {})
            .await?
            .into_inner();

        match response.result {
            Some(get_explorer_metrics_response::Result::Metrics(metrics)) => {
                Ok(metrics.heaviest_height)
            }
            Some(get_explorer_metrics_response::Result::Error(err)) => {
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

/// Converts a protobuf hash payload into the tx-engine hash type used by the
/// bridge runtime.
fn pb_hash_to_domain_hash(hash: pb_common_v1::Hash) -> Result<Hash> {
    fn belt(value: Option<pb_common_v1::Belt>, limb: &str) -> Result<Belt> {
        Ok(Belt(
            value
                .ok_or_else(|| NockAppGrpcError::Internal(format!("missing {limb} in block hash")))?
                .value,
        ))
    }

    Ok(Hash([
        belt(hash.belt_1, "belt_1")?,
        belt(hash.belt_2, "belt_2")?,
        belt(hash.belt_3, "belt_3")?,
        belt(hash.belt_4, "belt_4")?,
        belt(hash.belt_5, "belt_5")?,
    ]))
}
