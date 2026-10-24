use std::collections::TryReserveError;
use std::convert::Infallible;
use std::io;

use cbor4ii_transport::core::error::DecodeError;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{request_response, StreamProtocol};

use super::TransportResponse;
use crate::messages::{NockchainRequest, NockchainResponse};

/// Uses the pinned transport CBOR semantics, retaining the bounded receive buffer's length.
#[derive(Clone)]
pub(crate) struct NockchainCodec {
    request_size_maximum: u64,
    response_size_maximum: u64,
}

impl Default for NockchainCodec {
    fn default() -> Self {
        Self {
            request_size_maximum: 1024 * 1024,
            response_size_maximum: 10 * 1024 * 1024,
        }
    }
}

impl NockchainCodec {
    pub(crate) fn set_request_size_maximum(mut self, maximum: u64) -> Self {
        self.request_size_maximum = maximum;
        self
    }

    pub(crate) fn set_response_size_maximum(mut self, maximum: u64) -> Self {
        self.response_size_maximum = maximum;
        self
    }
}

impl request_response::Codec for NockchainCodec {
    type Protocol = StreamProtocol;
    type Request = NockchainRequest;
    type Response = TransportResponse;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut buffer = Vec::new();
        io.take(self.request_size_maximum)
            .read_to_end(&mut buffer)
            .await?;
        cbor4ii_transport::serde::from_slice(buffer.as_slice()).map_err(decode_into_io_error)
    }

    async fn read_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut buffer = Vec::new();
        // Preserve take(max): do not probe or drain bytes beyond the existing read cap.
        io.take(self.response_size_maximum)
            .read_to_end(&mut buffer)
            .await?;
        let message: NockchainResponse = cbor4ii_transport::serde::from_slice(buffer.as_slice())
            .map_err(decode_into_io_error)?;
        Ok(TransportResponse {
            message,
            received_bytes: Some(buffer.len()),
        })
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        request: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let data =
            cbor4ii_transport::serde::to_vec(Vec::new(), &request).map_err(encode_into_io_error)?;
        io.write_all(data.as_ref()).await?;
        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        response: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let data = cbor4ii_transport::serde::to_vec(Vec::new(), &response.message)
            .map_err(encode_into_io_error)?;
        io.write_all(data.as_ref()).await?;
        Ok(())
    }
}

fn decode_into_io_error(error: cbor4ii_transport::serde::DecodeError<Infallible>) -> io::Error {
    match error {
        cbor4ii_transport::serde::DecodeError::Core(DecodeError::Read(error)) => {
            io::Error::other(error)
        }
        cbor4ii_transport::serde::DecodeError::Core(error @ DecodeError::Unsupported { .. }) => {
            io::Error::new(io::ErrorKind::Unsupported, error)
        }
        cbor4ii_transport::serde::DecodeError::Core(error @ DecodeError::Eof { .. }) => {
            io::Error::new(io::ErrorKind::UnexpectedEof, error)
        }
        cbor4ii_transport::serde::DecodeError::Core(error) => {
            io::Error::new(io::ErrorKind::InvalidData, error)
        }
        cbor4ii_transport::serde::DecodeError::Custom(error) => io::Error::other(error.to_string()),
    }
}

fn encode_into_io_error(
    error: cbor4ii_transport::serde::EncodeError<TryReserveError>,
) -> io::Error {
    io::Error::other(error)
}

#[cfg(test)]
mod tests {
    use futures::io::Cursor;
    use libp2p::request_response::Codec;

    use super::*;

    fn protocol() -> StreamProtocol {
        StreamProtocol::new("/codec-test")
    }

    #[tokio::test]
    async fn response_read_counts_alternate_encoding_unknown_fields_and_padding() {
        // {"Ack": {"acked": true, "extra": 0}}, with an overlong inner map length.
        let mut bytes = b"\xa1\x63Ack\xb8\x02\x65acked\xf5\x65extra\x00".to_vec();
        bytes.extend_from_slice(&[0xff, 0x00, 0x00]);
        let expected_len = bytes.len();
        let stock_response = libp2p::request_response::cbor::codec::Codec::<
            NockchainRequest,
            NockchainResponse,
        >::default()
        .read_response(&protocol(), &mut Cursor::new(bytes.as_slice()))
        .await
        .expect("fixture must be accepted by the pinned stock codec");
        assert_eq!(stock_response, NockchainResponse::Ack { acked: true });
        let response = NockchainCodec::default()
            .read_response(&protocol(), &mut Cursor::new(bytes))
            .await
            .unwrap();
        assert_eq!(response.message, NockchainResponse::Ack { acked: true });
        assert_eq!(response.received_bytes, Some(expected_len));
    }

    #[tokio::test]
    async fn response_read_cap_accepts_complete_prefix_but_rejects_truncated_value() {
        let message = NockchainResponse::Ack { acked: true };
        let canonical = cbor4ii_transport::serde::to_vec(Vec::new(), &message).unwrap();
        let mut codec = NockchainCodec::default().set_response_size_maximum(canonical.len() as u64);
        let mut exact = Cursor::new(canonical.clone());
        assert_eq!(
            codec
                .read_response(&protocol(), &mut exact)
                .await
                .unwrap()
                .received_bytes,
            Some(canonical.len())
        );

        let cap = canonical.len() + 2;
        let mut padded = canonical.clone();
        padded.resize(cap + 8, 0xff);
        let mut stream = Cursor::new(padded);
        let response = NockchainCodec::default()
            .set_response_size_maximum(cap as u64)
            .read_response(&protocol(), &mut stream)
            .await
            .unwrap();
        assert_eq!(response.message, message);
        assert_eq!(response.received_bytes, Some(cap));
        assert_eq!(stream.position(), cap as u64);

        let error = NockchainCodec::default()
            .set_response_size_maximum((canonical.len() - 1) as u64)
            .read_response(&protocol(), &mut Cursor::new(canonical))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn response_write_matches_stock_codec_without_metadata() {
        let message = NockchainResponse::Ack { acked: true };
        let mut expected = Cursor::new(Vec::new());
        libp2p::request_response::cbor::codec::Codec::<NockchainRequest, NockchainResponse>::default()
            .write_response(&protocol(), &mut expected, message.clone())
            .await
            .unwrap();
        for received_bytes in [None, Some(12345)] {
            let mut actual = Cursor::new(Vec::new());
            NockchainCodec::default()
                .write_response(
                    &protocol(),
                    &mut actual,
                    TransportResponse {
                        message: message.clone(),
                        received_bytes,
                    },
                )
                .await
                .unwrap();
            assert_eq!(actual.into_inner(), *expected.get_ref());
        }
    }
}
