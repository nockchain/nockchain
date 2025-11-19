use tonic::transport::Channel;

use crate::error::{NockAppGrpcError, Result};
use crate::pb::common::v1::Wire;
use crate::pb::private::v1::nock_app_service_client::NockAppServiceClient as PrivateNockAppClient;
use crate::pb::private::v1::*;

#[derive(Clone)]
pub struct PrivateNockAppGrpcClient {
    client: PrivateNockAppClient<Channel>,
}

impl PrivateNockAppGrpcClient {
    pub async fn connect<T: AsRef<str>>(address: T) -> Result<Self> {
        let client = PrivateNockAppClient::connect(address.as_ref().to_string()).await?;
        Ok(Self { client })
    }

    // Monitoring ping is handled in MonitoringService, not here.

    pub async fn peek(&mut self, pid: i32, path: Vec<u8>) -> Result<Vec<u8>> {
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
}
