use std::pin::Pin;
use std::task::{Context, Poll};

use libp2p::core::muxing::{StreamMuxer, StreamMuxerEvent};
use libp2p::core::ConnectedPoint;
use libp2p::{quic, Multiaddr};

use crate::p2p_util::MultiaddrExt;

/// Preserve the numeric endpoint produced inside DNS resolution. The DNS
/// transport retains the dial alias, so report the resolved endpoint through
/// the SDK's connection-scoped address-change event before range issuance.
pub(crate) struct ResolvedQuicMuxer {
    connection: quic::Connection,
    resolved_remote: Option<Multiaddr>,
}

impl ResolvedQuicMuxer {
    pub(crate) fn new(connection: quic::Connection, endpoint: ConnectedPoint) -> Self {
        let remote = match endpoint {
            ConnectedPoint::Dialer { address, .. } => address,
            ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
        };
        Self {
            connection,
            resolved_remote: Some(remote),
        }
    }

    pub(crate) fn for_original_endpoint(mut self, endpoint: &ConnectedPoint) -> Self {
        // Numeric dials and inbound sockets already report their real endpoint;
        // do not generate redundant address changes or clone their addresses.
        if endpoint.get_remote_address().canonical_ip_addr().is_some() {
            self.resolved_remote = None;
        }
        self
    }
}

impl StreamMuxer for ResolvedQuicMuxer {
    type Substream = <quic::Connection as StreamMuxer>::Substream;
    type Error = <quic::Connection as StreamMuxer>::Error;

    fn poll_inbound(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Substream, Self::Error>> {
        Pin::new(&mut self.get_mut().connection).poll_inbound(cx)
    }

    fn poll_outbound(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Substream, Self::Error>> {
        Pin::new(&mut self.get_mut().connection).poll_outbound(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().connection).poll_close(cx)
    }

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<StreamMuxerEvent, Self::Error>> {
        let this = self.get_mut();
        if let Some(remote) = this.resolved_remote.take() {
            return Poll::Ready(Ok(StreamMuxerEvent::AddressChange(remote)));
        }
        Pin::new(&mut this.connection).poll(cx)
    }
}
