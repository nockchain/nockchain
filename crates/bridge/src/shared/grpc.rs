#[cfg(feature = "bazel_build")]
pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!(env!("BRIDGE_DESCRIPTOR_BIN"));

#[cfg(not(feature = "bazel_build"))]
pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("bridge_descriptor");

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::FILE_DESCRIPTOR_SET;

    #[test]
    fn public_withdrawal_service_exposes_only_read_methods() {
        let descriptor = prost_types::FileDescriptorSet::decode(FILE_DESCRIPTOR_SET)
            .expect("decode bridge descriptor set");
        let service = descriptor
            .file
            .iter()
            .filter(|file| file.package.as_deref() == Some("bridge.ingress.v1"))
            .flat_map(|file| &file.service)
            .find(|service| service.name.as_deref() == Some("WithdrawalPublicQuery"))
            .expect("public withdrawal query service");
        let methods = service
            .method
            .iter()
            .map(|method| method.name.as_deref().expect("method name"))
            .collect::<Vec<_>>();

        assert_eq!(
            methods,
            vec![
                "ResolveBaseWithdrawal", "GetWithdrawal", "ListWithdrawalsByBurner",
                "GetWithdrawalReadiness", "GetWithdrawalQuote",
            ]
        );
        assert!(methods.iter().all(|method| {
            !["Register", "Advance", "Record", "Authorize", "Submit", "Retry", "Reserve"]
                .iter()
                .any(|mutation| method.contains(mutation))
        }));
    }
}
