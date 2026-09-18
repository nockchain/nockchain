use std::time::Duration;

use tokio::time::timeout;

use crate::messages::{NockchainRequest, NockchainResponse};
use crate::transport::{TransportCommand, TransportEvent, TransportHandle};
use crate::types::{NodeId, RequestId};

pub(super) async fn next_matching(
    handle: &mut TransportHandle,
    label: &str,
    mut predicate: impl FnMut(&TransportEvent) -> bool,
) -> TransportEvent {
    let mut observed = Vec::new();
    match timeout(Duration::from_secs(10), async {
        loop {
            let event = handle.next_event().await.expect("transport event");
            if predicate(&event) {
                return event;
            }
            observed.push(format!("{event:?}"));
        }
    })
    .await
    {
        Ok(event) => event,
        Err(_) => panic!(
            "timed out waiting for {label}; observed events: {}",
            observed.join(", ")
        ),
    }
}

pub(super) async fn assert_request_response_contract(
    requester: &mut TransportHandle,
    responder: &mut TransportHandle,
    responder_id: NodeId,
) {
    let request = NockchainRequest::AuthenticatedGossip {
        pow: [0; 16],
        nonce: 7,
        message: serde_bytes::ByteBuf::from(b"transport-contract".to_vec()),
    };
    let request_id = RequestId::new(9);
    requester
        .commands
        .send(TransportCommand::SendRequest {
            id: request_id,
            peer: responder_id,
            request: request.clone(),
        })
        .await
        .expect("send request");

    let inbound_id = match next_matching(responder, "incoming request", |event| {
        matches!(event, TransportEvent::IncomingRequest { .. })
    })
    .await
    {
        TransportEvent::IncomingRequest {
            id,
            peer,
            request: received,
            ..
        } => {
            assert_eq!(peer, requester.local_node_id);
            assert_eq!(received, request);
            id
        }
        _ => unreachable!(),
    };
    responder
        .commands
        .send(TransportCommand::CompleteInbound {
            id: inbound_id,
            response: Some(NockchainResponse::Ack { acked: true }),
        })
        .await
        .expect("complete inbound request");

    match next_matching(
        requester,
        "request response",
        |event| matches!(event, TransportEvent::Response { id, .. } if *id == request_id),
    )
    .await
    {
        TransportEvent::Response { response, .. } => {
            assert_eq!(response, NockchainResponse::Ack { acked: true });
        }
        _ => unreachable!(),
    }
}
