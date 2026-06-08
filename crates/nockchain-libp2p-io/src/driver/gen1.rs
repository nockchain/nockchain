use super::*;

pub(crate) fn build_unsupported_protocol_fallback_contexts(
    request_context: &OutboundRequestContext,
    local_peer_id: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
) -> Result<Vec<OutboundRequestContext>, NockAppError> {
    if request_context.fallback_attempted || request_context.generation != ReqResGeneration::Gen2 {
        return Ok(Vec::new());
    }

    match &request_context.request {
        NockchainRequest::BatchRequest { items, .. } => items
            .iter()
            .map(|item| {
                let request_slab = request_slab_from_message(&item.message)?;
                let request = NockchainRequest::new_request(
                    equix_builder, local_peer_id, &request_context.peer_id, &request_slab,
                );
                Ok(OutboundRequestContext::with_attempt(
                    request_context.peer_id,
                    ReqResGeneration::Gen1,
                    request,
                    request_context.retry_count.saturating_add(1),
                    true,
                ))
            })
            .collect(),
        NockchainRequest::Request { message, .. } => {
            let request_slab = request_slab_from_message(message)?;
            let request = NockchainRequest::new_request(
                equix_builder, local_peer_id, &request_context.peer_id, &request_slab,
            );
            Ok(vec![OutboundRequestContext::with_attempt(
                request_context.peer_id,
                ReqResGeneration::Gen1,
                request,
                request_context.retry_count.saturating_add(1),
                true,
            )])
        }
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => {
            Ok(Vec::new())
        }
    }
}
