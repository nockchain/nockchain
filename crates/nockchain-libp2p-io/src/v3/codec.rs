//! One protobuf message per libp2p request/response half-stream.
//!
//! A four-byte, big-endian unsigned length precedes the protobuf body. The
//! configured byte limit covers this prefix plus the complete protobuf body. The
//! sender then closes its write side (the request-response handler does this).
//! The receiver requires EOF before decoding, so an extra frame or trailing
//! data can never be mistaken for part of an accepted operation.

use std::io;

use async_trait::async_trait;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{request_response, StreamProtocol};

use crate::config::LibP2PConfig;
use crate::messages::{NockchainRequest, NockchainResponse};

#[derive(Clone, Debug)]
pub(crate) struct ProtobufCodec {
    request_size_maximum: u64,
    response_size_maximum: u64,
}

impl ProtobufCodec {
    pub(crate) fn new(request_size_maximum: u64, response_size_maximum: u64) -> Self {
        Self {
            request_size_maximum,
            response_size_maximum,
        }
    }
}

fn check_protocol(protocol: &StreamProtocol) -> io::Result<()> {
    if protocol.as_ref() != LibP2PConfig::req_res_protocol_version() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported peer request-response protocol",
        ));
    }
    Ok(())
}

async fn read_frame<T>(io: &mut T, size_maximum: u64) -> io::Result<Vec<u8>>
where
    T: AsyncRead + Unpin + Send,
{
    let mut prefix = [0u8; 4];
    io.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix);
    if u64::from(length) + 4 > size_maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "peer protobuf frame exceeds configured byte limit",
        ));
    }
    let length = usize::try_from(length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "peer frame length cannot be represented",
        )
    })?;
    let mut body = Vec::new();
    body.try_reserve_exact(length).map_err(io::Error::other)?;
    body.resize(length, 0);
    io.read_exact(&mut body).await?;

    let mut trailing = [0u8; 1];
    if io.read(&mut trailing).await? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected exactly one peer protobuf frame",
        ));
    }
    Ok(body)
}

async fn write_frame<T>(io: &mut T, body: &[u8], size_maximum: u64) -> io::Result<()>
where
    T: AsyncWrite + Unpin + Send,
{
    let length = u32::try_from(body.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "peer protobuf frame exceeds u32 length",
        )
    })?;
    if u64::from(length) + 4 > size_maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "peer protobuf frame exceeds configured byte limit",
        ));
    }
    io.write_all(&length.to_be_bytes()).await?;
    io.write_all(body).await
}

#[async_trait]
impl request_response::Codec for ProtobufCodec {
    type Protocol = StreamProtocol;
    type Request = NockchainRequest;
    type Response = NockchainResponse;

    async fn read_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        check_protocol(protocol)?;
        let body = read_frame(io, self.request_size_maximum).await?;
        super::decode_request(&body)
    }

    async fn read_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        check_protocol(protocol)?;
        let body = read_frame(io, self.response_size_maximum).await?;
        super::decode_response(&body)
    }

    async fn write_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        request: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        check_protocol(protocol)?;
        let body = super::encode_request(&request)?;
        write_frame(io, &body, self.request_size_maximum).await
    }

    async fn write_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        response: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        check_protocol(protocol)?;
        let body = super::encode_response(&response)?;
        write_frame(io, &body, self.response_size_maximum).await
    }
}

#[cfg(test)]
mod tests {
    use futures::io::Cursor;
    use libp2p::request_response::Codec as _;

    use super::*;
    use crate::messages::{block_with_txs_by_height_request_message, BatchRequestItem};

    fn protocol() -> StreamProtocol {
        StreamProtocol::new(LibP2PConfig::req_res_protocol_version())
    }

    #[tokio::test]
    async fn request_and_response_round_trip() {
        let mut codec = ProtobufCodec::new(10_000_000, 10_000_000);
        let request = NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 42,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: block_with_txs_by_height_request_message(123).unwrap(),
            }],
        };
        let mut stream = Cursor::new(Vec::new());
        codec
            .write_request(&protocol(), &mut stream, request.clone())
            .await
            .unwrap();
        stream.set_position(0);
        assert_eq!(
            codec.read_request(&protocol(), &mut stream).await.unwrap(),
            request
        );

        let response = NockchainResponse::Ack { acked: true };
        let mut stream = Cursor::new(Vec::new());
        codec
            .write_response(&protocol(), &mut stream, response.clone())
            .await
            .unwrap();
        stream.set_position(0);
        assert_eq!(
            codec.read_response(&protocol(), &mut stream).await.unwrap(),
            response
        );
    }

    #[tokio::test]
    async fn framing_has_a_fixed_network_order_length() {
        let mut stream = Cursor::new(Vec::new());
        write_frame(&mut stream, &[10, 20, 30], 7).await.unwrap();
        assert_eq!(stream.get_ref(), &[0, 0, 0, 3, 10, 20, 30]);
        stream.set_position(0);
        assert_eq!(read_frame(&mut stream, 7).await.unwrap(), [10, 20, 30]);
    }

    #[tokio::test]
    async fn truncated_prefix_and_body_are_rejected() {
        for bytes in [vec![], vec![0, 0, 0], vec![0, 0, 0, 2, 10]] {
            let error = read_frame(&mut Cursor::new(bytes), 10).await.unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        }
    }

    #[tokio::test]
    async fn trailing_data_and_a_second_frame_are_rejected() {
        let mut codec = ProtobufCodec::new(10_000_000, 10_000_000);
        let mut stream = Cursor::new(Vec::new());
        codec
            .write_response(
                &protocol(),
                &mut stream,
                NockchainResponse::Ack { acked: true },
            )
            .await
            .unwrap();
        let frame = stream.into_inner();
        for suffix in [vec![0], frame.clone()] {
            let mut bytes = frame.clone();
            bytes.extend_from_slice(&suffix);
            let error = codec
                .read_response(&protocol(), &mut Cursor::new(bytes))
                .await
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[tokio::test]
    async fn byte_limit_is_checked_before_reading_or_writing_body() {
        // Only the prefix is present: a size failure must precede a body read.
        let error = read_frame(&mut Cursor::new(4u32.to_be_bytes()), 7)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let mut output = Cursor::new(Vec::new());
        let error = write_frame(&mut output, &[1, 2, 3, 4], 7)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(output.get_ref().is_empty());
    }

    #[tokio::test]
    async fn unsupported_protocol_does_not_read_or_write() {
        let mut codec = ProtobufCodec::new(10_000_000, 10_000_000);
        let old_protocol = StreamProtocol::new("/nockchain-2-req-res");
        let mut stream = Cursor::new(vec![0; 4]);
        let error = codec
            .read_request(&old_protocol, &mut stream)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(stream.position(), 0);
        let error = codec
            .read_response(&old_protocol, &mut stream)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(stream.position(), 0);

        let mut output = Cursor::new(Vec::new());
        let error = codec
            .write_request(
                &old_protocol,
                &mut output,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 0,
                    items: vec![],
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        let error = codec
            .write_response(
                &old_protocol,
                &mut output,
                NockchainResponse::Ack { acked: true },
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(output.get_ref().is_empty());
    }
}
