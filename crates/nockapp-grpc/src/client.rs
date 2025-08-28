use tonic::transport::Channel;

use crate::error::{NockAppGrpcError, Result};
use crate::pb::nock_app_service_client::NockAppServiceClient;
use crate::pb::*;

#[derive(Clone)]
pub struct NockAppGrpcClient {
    client: NockAppServiceClient<Channel>,
}

impl NockAppGrpcClient {
    pub async fn connect<T: AsRef<str>>(address: T) -> Result<Self> {
        let client = NockAppServiceClient::connect(address.as_ref().to_string()).await?;
        Ok(Self { client })
    }

    pub async fn ping(&mut self) -> Result<bool> {
        let request = PingRequest { zero: 0 };

        let response = self.client.ping(request).await?;
        let response = response.into_inner();

        match response.result {
            Some(ping_response::Result::Succ(succ)) => Ok(succ > 0),
            Some(ping_response::Result::Error(error)) => {
                Err(NockAppGrpcError::Internal(error.message))
            }
            None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
        }
    }

    pub async fn peek(&mut self, pid: i32, path: Vec<String>) -> Result<Vec<u8>> {
        let request = PeekRequest { pid, path };

        let response = self.client.peek(request).await?;
        let response = response.into_inner();

        match response.result {
            Some(peek_response::Result::Data(data)) => Ok(data),
            Some(peek_response::Result::Error(error)) => {
                Err(NockAppGrpcError::Internal(error.message))
            }
            None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
        }
    }

    // pub async fn peek_vase(&mut self, pid: i32, path: Vec<String>) -> Result<Vec<u8>> {
    //     let request = PeekVaseRequest { pid, path };

    //     let response = self.client.peek_vase(request).await?;
    //     let response = response.into_inner();

    //     match response.result {
    //         Some(peek_vase_response::Result::Vase(vase)) => Ok(vase),
    //         Some(peek_vase_response::Result::Error(error)) => {
    //             Err(NockAppGrpcError::Internal(error.message))
    //         }
    //         None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
    //     }
    // }

    pub async fn poke(&mut self, pid: i32, wire: Wire, payload: Vec<u8>) -> Result<bool> {
        let request = PokeRequest {
            pid,
            wire: Some(wire),
            payload,
        };

        let response = self.client.poke(request).await?;
        let response = response.into_inner();

        match response.result {
            Some(poke_response::Result::Acknowledged(ack)) => Ok(ack),
            Some(poke_response::Result::Error(error)) => {
                Err(NockAppGrpcError::Internal(error.message))
            }
            None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
        }
    }

    // pub async fn wallet_get_balance(
    //     &mut self,
    //     pid: i32,
    //     address: Option<String>,
    //     block_id: Option<String>,
    // ) -> Result<WalletBalanceData> {
    //     let request = WalletGetBalanceRequest {
    //         pid,
    //         address,
    //         block_id,
    //     };

    //     let response = self.client.wallet_get_balance(request).await?;
    //     let response = response.into_inner();

    //     match response.result {
    //         Some(wallet_get_balance_response::Result::Balance(balance)) => Ok(balance),
    //         Some(wallet_get_balance_response::Result::Error(error)) => {
    //             Err(NockAppGrpcError::Internal(error.message))
    //         }
    //         None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
    //     }
    // }

    // pub async fn mining_get_header(&mut self, pid: i32) -> Result<MiningHeaderData> {
    //     let request = MiningGetHeaderRequest { pid };

    //     let response = self.client.mining_get_header(request).await?;
    //     let response = response.into_inner();

    //     match response.result {
    //         Some(mining_get_header_response::Result::Header(header)) => Ok(header),
    //         Some(mining_get_header_response::Result::Error(error)) => {
    //             Err(NockAppGrpcError::Internal(error.message))
    //         }
    //         None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
    //     }
    // }

    // pub async fn block_get_by_id(&mut self, pid: i32, block_id: String) -> Result<BlockData> {
    //     let request = BlockGetByIdRequest { pid, block_id };

    //     let response = self.client.block_get_by_id(request).await?;
    //     let response = response.into_inner();

    //     match response.result {
    //         Some(block_get_by_id_response::Result::Block(block)) => Ok(block),
    //         Some(block_get_by_id_response::Result::Error(error)) => {
    //             Err(NockAppGrpcError::Internal(error.message))
    //         }
    //         None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
    //     }
    // }

    // pub async fn system_health_check(&mut self, pid: i32) -> Result<SystemHealthData> {
    //     let request = SystemHealthCheckRequest {
    //         pid,
    //         detailed: Some(true),
    //     };

    //     let response = self.client.system_health_check(request).await?;
    //     let response = response.into_inner();

    //     match response.result {
    //         Some(system_health_check_response::Result::Health(health)) => Ok(health),
    //         Some(system_health_check_response::Result::Error(error)) => {
    //             Err(NockAppGrpcError::Internal(error.message))
    //         }
    //         None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
    //     }
    // }
}
