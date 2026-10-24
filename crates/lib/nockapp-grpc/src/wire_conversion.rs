use nockapp::wire::{WireRepr, WireTag as NockAppWireTag};

use crate::error::{NockAppGrpcError, Result};
use crate::pb::common::v1::{wire_tag, Wire, WireTag};

/// Convert gRPC Wire to NockApp WireRepr
pub fn grpc_wire_to_nockapp(wire: &Wire) -> Result<WireRepr> {
    let source = match wire.source.as_str() {
        "" => {
            return Err(NockAppGrpcError::InvalidRequest(
                "Wire source cannot be empty".to_string(),
            ))
        }
        // The client-supplied source is OWNED by the wire and freed with it.
        // The previous per-request `Box::leak` let any client permanently
        // claim node memory (a 4 MiB request leaked ~4 MiB forever).
        s => s.to_string(),
    };

    let mut tags = Vec::new();
    for tag in &wire.tags {
        let nockapp_tag = match &tag.value {
            Some(wire_tag::Value::Text(s)) => NockAppWireTag::String(s.clone()),
            Some(wire_tag::Value::Number(n)) => NockAppWireTag::Direct(*n),
            None => {
                return Err(NockAppGrpcError::InvalidRequest(
                    "WireTag value is required".to_string(),
                ))
            }
        };
        tags.push(nockapp_tag);
    }

    Ok(WireRepr::new(source, wire.version, tags))
}

/// Convert NockApp WireRepr to gRPC Wire
pub fn nockapp_wire_to_grpc(wire: &WireRepr) -> Wire {
    let tags = wire
        .tags
        .iter()
        .map(|tag| {
            let value = match tag {
                NockAppWireTag::String(s) => wire_tag::Value::Text(s.clone()),
                NockAppWireTag::Direct(n) => wire_tag::Value::Number(*n),
            };
            WireTag { value: Some(value) }
        })
        .collect();

    Wire {
        source: wire.source.to_string(),
        version: wire.version,
        tags,
    }
}

/// Create a gRPC wire for the gRPC driver itself
pub fn create_grpc_wire() -> Wire {
    Wire {
        source: "grpc".to_string(),
        version: 1,
        tags: vec![],
    }
}

/// Create a system wire for system operations
pub fn create_system_wire() -> Wire {
    Wire {
        source: "sys".to_string(),
        version: 1,
        tags: vec![],
    }
}
