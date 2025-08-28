use std::net::SocketAddr;

use nockapp::driver::{NockAppHandle, PokeResult};
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{D, T};
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

use crate::error::{NockAppGrpcError, Result};
use crate::pb::nock_app_service_server::{NockAppService, NockAppServiceServer};
use crate::pb::*;
use crate::wire_conversion::grpc_wire_to_nockapp;

pub struct NockAppGrpcServer {
    handle: NockAppHandle,
}

impl NockAppGrpcServer {
    pub fn new(handle: NockAppHandle) -> Self {
        Self { handle }
    }

    pub async fn serve(self, addr: SocketAddr) -> Result<()> {
        info!("Starting gRPC server on {}", addr);

        let service = NockAppServiceServer::new(self);

        Server::builder()
            .add_service(service)
            .serve(addr)
            .await
            .map_err(NockAppGrpcError::Transport)?;

        Ok(())
    }

    /// Convert path strings to a NounSlab for peek operations
    fn path_to_noun_slab(&self, path: &[String]) -> Result<NounSlab> {
        if path.is_empty() {
            return Err(NockAppGrpcError::InvalidRequest(
                "Path cannot be empty".to_string(),
            ));
        }

        let mut slab = NounSlab::new();
        let mut path_nouns = Vec::new();

        for segment in path {
            // Convert path segment to a tag atom
            let atom = nockapp::utils::make_tas(&mut slab, segment);
            path_nouns.push(atom.as_noun());
        }

        // Add terminating D(0)
        path_nouns.push(D(0));

        let path_noun = T(&mut slab, &path_nouns);
        slab.set_root(path_noun);

        Ok(slab)
    }

    /// Build error response with proper error status
    fn build_error_response<T>(&self, error: NockAppGrpcError) -> T
    where
        T: From<ErrorStatus>,
    {
        let error_status = ErrorStatus {
            code: match &error {
                NockAppGrpcError::PeekFailed => ErrorCode::PeekFailed as i32,
                NockAppGrpcError::PokeFailed => ErrorCode::PokeFailed as i32,
                NockAppGrpcError::Timeout => ErrorCode::Timeout as i32,
                NockAppGrpcError::InvalidRequest(_) => ErrorCode::InvalidRequest as i32,
                _ => ErrorCode::InternalError as i32,
            },
            message: error.to_string(),
            details: None,
        };
        T::from(error_status)
    }
}

#[tonic::async_trait]
impl NockAppService for NockAppGrpcServer {
    async fn ping(
        &self,
        request: Request<PingRequest>,
    ) -> std::result::Result<Response<PingResponse>, Status> {
        debug!("Ping request received");
        let req = request.into_inner();
        let succ = req.zero + 1;
        let response = PingResponse {
            result: Some(ping_response::Result::Succ(succ)),
        };
        Ok(Response::new(response))
    }

    async fn peek(
        &self,
        request: Request<PeekRequest>,
    ) -> std::result::Result<Response<PeekResponse>, Status> {
        let req = request.into_inner();
        debug!("CorePeek request: pid={}, path={:?}", req.pid, req.path);

        let path_slab = match self.path_to_noun_slab(&req.path) {
            Ok(slab) => slab,
            Err(e) => {
                warn!("Invalid path in Peek: {}", e);
                let response = PeekResponse {
                    result: Some(peek_response::Result::Error(self.build_error_response(e))),
                };
                return Ok(Response::new(response));
            }
        };

        match self.handle.peek(path_slab).await {
            Ok(Some(result_slab)) => {
                // Convert result to JAM-encoded bytes
                let jam_bytes = result_slab.jam();

                let response = PeekResponse {
                    result: Some(peek_response::Result::Data(jam_bytes.to_vec())),
                };
                Ok(Response::new(response))
            }
            Ok(None) => {
                debug!("Peek returned no result");
                let response = PeekResponse {
                    result: Some(peek_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::PeekFailed),
                    )),
                };
                Ok(Response::new(response))
            }
            Err(e) => {
                error!("Peek operation failed: {}", e);
                let response = PeekResponse {
                    result: Some(peek_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::NockApp(e)),
                    )),
                };
                Ok(Response::new(response))
            }
        }
    }

    // async fn peek_vase(
    //     &self,
    //     request: Request<PeekVaseRequest>,
    // ) -> std::result::Result<Response<PeekVaseResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!("CorePeekVase request: pid={}, path={:?}", req.pid, req.path);

    //     // Fallback implementation: use regular peek with a `vase` prefix in the path
    //     // This avoids requiring wrapper/kernel support for a dedicated peek_vase axis
    //     let mut vase_path = Vec::with_capacity(req.path.len() + 1);
    //     vase_path.extend(req.path);
    //     vase_path.push("vase".to_string());

    //     info!("Vase path: {:?}", vase_path);
    //     let path_slab = match self.path_to_noun_slab(&vase_path) {
    //         Ok(slab) => slab,
    //         Err(e) => {
    //             warn!("Invalid path in PeekVase: {}", e);
    //             let response = PeekVaseResponse {
    //                 result: Some(peek_vase_response::Result::Error(
    //                     self.build_error_response(e),
    //                 )),
    //             };
    //             return Ok(Response::new(response));
    //         }
    //     };
    //     info!("Path slab: {:?}", path_slab);
    //     match self.handle.peek(path_slab).await {
    //         Ok(Some(result_slab)) => {
    //             // Convert result to JAM-encoded bytes (this is a (unit (unit vase)))
    //             let jam_bytes = result_slab.jam();
    //             let response = PeekVaseResponse {
    //                 result: Some(peek_vase_response::Result::Vase(jam_bytes.to_vec())),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Ok(None) => {
    //             debug!("PeekVase returned no result");
    //             let response = PeekVaseResponse {
    //                 result: Some(peek_vase_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::PeekFailed),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Err(e) => {
    //             error!("PeekVase operation failed (fallback): {}", e);
    //             let response = PeekVaseResponse {
    //                 result: Some(peek_vase_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::NockApp(e)),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //     }
    // }

    async fn poke(
        &self,
        request: Request<PokeRequest>,
    ) -> std::result::Result<Response<PokeResponse>, Status> {
        let req = request.into_inner();
        debug!("Poke request: pid={}", req.pid);

        let wire = match req.wire {
            Some(wire) => match grpc_wire_to_nockapp(&wire) {
                Ok(w) => w,
                Err(e) => {
                    warn!("Invalid wire in Poke: {}", e);
                    let response = PokeResponse {
                        result: Some(poke_response::Result::Error(self.build_error_response(e))),
                    };
                    return Ok(Response::new(response));
                }
            },
            None => {
                warn!("Missing wire in Poke request");
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(self.build_error_response(
                        NockAppGrpcError::InvalidRequest("Wire is required".to_string()),
                    ))),
                };
                return Ok(Response::new(response));
            }
        };

        // Decode JAM payload
        let mut payload_slab = NounSlab::new();
        let _payload_noun = match payload_slab.cue_into(bytes::Bytes::from(req.payload)) {
            Ok(noun) => noun,
            Err(e) => {
                warn!("Failed to decode JAM payload: {:?}", e);
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(self.build_error_response(
                        NockAppGrpcError::Serialization(format!("JAM decoding failed: {:?}", e)),
                    ))),
                };
                return Ok(Response::new(response));
            }
        };

        match self.handle.poke(wire, payload_slab).await {
            Ok(PokeResult::Ack) => {
                debug!("Poke operation acknowledged");
                let response = PokeResponse {
                    result: Some(poke_response::Result::Acknowledged(true)),
                };
                Ok(Response::new(response))
            }
            Ok(PokeResult::Nack) => {
                debug!("Poke operation nacked");
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::PokeFailed),
                    )),
                };
                Ok(Response::new(response))
            }
            Err(e) => {
                error!("Poke operation failed: {}", e);
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::NockApp(e)),
                    )),
                };
                Ok(Response::new(response))
            }
        }
    }

    // async fn wallet_get_balance(
    //     &self,
    //     request: Request<WalletGetBalanceRequest>,
    // ) -> std::result::Result<Response<WalletGetBalanceResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!(
    //         "WalletGetBalance request: pid={}, address={:?}",
    //         req.pid, req.address
    //     );

    //     // Build path for balance peek
    //     let mut path = vec!["balance".to_string()];
    //     if let Some(addr) = req.address {
    //         path.push(addr);
    //     }

    //     let path_slab = match self.path_to_noun_slab(&path) {
    //         Ok(slab) => slab,
    //         Err(e) => {
    //             let response = WalletGetBalanceResponse {
    //                 result: Some(wallet_get_balance_response::Result::Error(
    //                     self.build_error_response(e),
    //                 )),
    //             };
    //             return Ok(Response::new(response));
    //         }
    //     };

    //     match self.handle.peek(path_slab).await {
    //         Ok(Some(_result_slab)) => {
    //             // TODO: Parse the result slab to extract balance information
    //             // For now, return a placeholder response
    //             let balance_data = WalletBalanceData {
    //                 notes: std::collections::HashMap::new(),
    //                 locked_balances: vec![],
    //                 block_id: req.block_id,
    //                 total_balance: Some(0),
    //             };

    //             let response = WalletGetBalanceResponse {
    //                 result: Some(wallet_get_balance_response::Result::Balance(balance_data)),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Ok(None) => {
    //             let response = WalletGetBalanceResponse {
    //                 result: Some(wallet_get_balance_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::PeekFailed),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Err(e) => {
    //             let response = WalletGetBalanceResponse {
    //                 result: Some(wallet_get_balance_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::NockApp(e)),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //     }
    // }

    // async fn wallet_send_transaction(
    //     &self,
    //     request: Request<WalletSendTransactionRequest>,
    // ) -> std::result::Result<Response<WalletSendTransactionResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!("WalletSendTransaction request: pid={}", req.pid);

    //     // TODO: Implement transaction building and submission
    //     // This would involve:
    //     // 1. Building a transaction noun from the request
    //     // 2. Poking it to the wallet system
    //     // 3. Returning the transaction hash and status

    //     let response = WalletSendTransactionResponse {
    //         result: Some(wallet_send_transaction_response::Result::Error(
    //             ErrorStatus {
    //                 code: ErrorCode::InternalError as i32,
    //                 message: "Not yet implemented".to_string(),
    //                 details: None,
    //             },
    //         )),
    //     };
    //     Ok(Response::new(response))
    // }

    // async fn wallet_get_transaction_status(
    //     &self,
    //     request: Request<WalletGetTransactionStatusRequest>,
    // ) -> std::result::Result<Response<WalletGetTransactionStatusResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!("WalletGetTransactionStatus request: pid={}", req.pid);

    //     // TODO: Implement transaction status lookup
    //     let response = WalletGetTransactionStatusResponse {
    //         result: Some(wallet_get_transaction_status_response::Result::Error(
    //             ErrorStatus {
    //                 code: ErrorCode::InternalError as i32,
    //                 message: "Not yet implemented".to_string(),
    //                 details: None,
    //             },
    //         )),
    //     };
    //     Ok(Response::new(response))
    // }

    // async fn mining_get_header(
    //     &self,
    //     request: Request<MiningGetHeaderRequest>,
    // ) -> std::result::Result<Response<MiningGetHeaderResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!("MiningGetHeader request: pid={}", req.pid);

    //     // Build path for mining header peek: ["mining", "header"]
    //     let path_slab = match self.path_to_noun_slab(&["mining".to_string(), "header".to_string()])
    //     {
    //         Ok(slab) => slab,
    //         Err(e) => {
    //             let response = MiningGetHeaderResponse {
    //                 result: Some(mining_get_header_response::Result::Error(
    //                     self.build_error_response(e),
    //                 )),
    //             };
    //             return Ok(Response::new(response));
    //         }
    //     };

    //     match self.handle.peek(path_slab).await {
    //         Ok(Some(_result_slab)) => {
    //             // TODO: Parse the mining header from the result slab
    //             // For now, return a placeholder response
    //             let header_data = MiningHeaderData {
    //                 commitment: vec![],
    //                 target: 0,
    //                 height: None,
    //                 parent_hash: None,
    //             };

    //             let response = MiningGetHeaderResponse {
    //                 result: Some(mining_get_header_response::Result::Header(header_data)),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Ok(None) => {
    //             let response = MiningGetHeaderResponse {
    //                 result: Some(mining_get_header_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::PeekFailed),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Err(e) => {
    //             let response = MiningGetHeaderResponse {
    //                 result: Some(mining_get_header_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::NockApp(e)),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //     }
    // }

    // async fn mining_submit_solution(
    //     &self,
    //     request: Request<MiningSubmitSolutionRequest>,
    // ) -> std::result::Result<Response<MiningSubmitSolutionResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!("MiningSubmitSolution request: pid={}", req.pid);

    //     // TODO: Implement mining solution submission
    //     let response = MiningSubmitSolutionResponse {
    //         result: Some(mining_submit_solution_response::Result::Error(
    //             ErrorStatus {
    //                 code: ErrorCode::InternalError as i32,
    //                 message: "Not yet implemented".to_string(),
    //                 details: None,
    //             },
    //         )),
    //     };
    //     Ok(Response::new(response))
    // }

    // async fn block_get_by_id(
    //     &self,
    //     request: Request<BlockGetByIdRequest>,
    // ) -> std::result::Result<Response<BlockGetByIdResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!(
    //         "BlockGetById request: pid={}, block_id={}",
    //         req.pid, req.block_id
    //     );

    //     // Build path for block peek: ["block", block_id]
    //     let path_slab = match self.path_to_noun_slab(&["block".to_string(), req.block_id.clone()]) {
    //         Ok(slab) => slab,
    //         Err(e) => {
    //             let response = BlockGetByIdResponse {
    //                 result: Some(block_get_by_id_response::Result::Error(
    //                     self.build_error_response(e),
    //                 )),
    //             };
    //             return Ok(Response::new(response));
    //         }
    //     };

    //     match self.handle.peek(path_slab).await {
    //         Ok(Some(_result_slab)) => {
    //             // TODO: Parse the block data from the result slab
    //             let block_data = BlockData {
    //                 block_id: req.block_id,
    //                 height: 0,
    //                 header: vec![],
    //                 transactions: vec![],
    //                 parent_hash: None,
    //                 timestamp: None,
    //             };

    //             let response = BlockGetByIdResponse {
    //                 result: Some(block_get_by_id_response::Result::Block(block_data)),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Ok(None) => {
    //             let response = BlockGetByIdResponse {
    //                 result: Some(block_get_by_id_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::PeekFailed),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Err(e) => {
    //             let response = BlockGetByIdResponse {
    //                 result: Some(block_get_by_id_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::NockApp(e)),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //     }
    // }

    // async fn block_get_heaviest(
    //     &self,
    //     request: Request<BlockGetHeaviestRequest>,
    // ) -> std::result::Result<Response<BlockGetHeaviestResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!("BlockGetHeaviest request: pid={}", req.pid);

    //     // Build path for heaviest block peek: ["heaviest-block"]
    //     let path_slab = match self.path_to_noun_slab(&["heaviest-block".to_string()]) {
    //         Ok(slab) => slab,
    //         Err(e) => {
    //             let response = BlockGetHeaviestResponse {
    //                 result: Some(block_get_heaviest_response::Result::Error(
    //                     self.build_error_response(e),
    //                 )),
    //             };
    //             return Ok(Response::new(response));
    //         }
    //     };

    //     match self.handle.peek(path_slab).await {
    //         Ok(Some(_result_slab)) => {
    //             // TODO: Parse the heaviest block data from the result slab
    //             let block_data = BlockData {
    //                 block_id: "".to_string(),
    //                 height: 0,
    //                 header: vec![],
    //                 transactions: vec![],
    //                 parent_hash: None,
    //                 timestamp: None,
    //             };

    //             let response = BlockGetHeaviestResponse {
    //                 result: Some(block_get_heaviest_response::Result::Block(block_data)),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Ok(None) => {
    //             let response = BlockGetHeaviestResponse {
    //                 result: Some(block_get_heaviest_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::PeekFailed),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //         Err(e) => {
    //             let response = BlockGetHeaviestResponse {
    //                 result: Some(block_get_heaviest_response::Result::Error(
    //                     self.build_error_response(NockAppGrpcError::NockApp(e)),
    //                 )),
    //             };
    //             Ok(Response::new(response))
    //         }
    //     }
    // }

    // async fn system_health_check(
    //     &self,
    //     request: Request<SystemHealthCheckRequest>,
    // ) -> std::result::Result<Response<SystemHealthCheckResponse>, Status> {
    //     let req = request.into_inner();
    //     debug!("SystemHealthCheck request: pid={}", req.pid);

    //     // For now, always return healthy
    //     let health_data = SystemHealthData {
    //         healthy: true,
    //         message: "NockApp gRPC server is running".to_string(),
    //         subsystems: std::collections::HashMap::new(),
    //         uptime_seconds: None,
    //         event_count: None,
    //     };

    //     let response = SystemHealthCheckResponse {
    //         result: Some(system_health_check_response::Result::Health(health_data)),
    //     };
    //     Ok(Response::new(response))
    // }
}
