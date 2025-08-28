#[cfg(test)]
mod tests {

    #[test]
    fn test_wire_conversion() {
        use crate::wire_conversion::{create_grpc_wire, create_system_wire};

        let grpc_wire = create_grpc_wire();
        assert_eq!(grpc_wire.source, "grpc");
        assert_eq!(grpc_wire.version, 1);
        assert!(grpc_wire.tags.is_empty());

        let sys_wire = create_system_wire();
        assert_eq!(sys_wire.source, "sys");
        assert_eq!(sys_wire.version, 1);
        assert!(sys_wire.tags.is_empty());
    }

    #[test]
    fn test_error_codes() {
        use crate::pb::ErrorCode;

        // Test that error codes are defined
        assert_eq!(ErrorCode::PeekFailed as i32, 2);
        assert_eq!(ErrorCode::PokeFailed as i32, 3);
        assert_eq!(ErrorCode::Timeout as i32, 5);
    }
}
