pub mod proto {
    #[cfg(feature = "bazel_build")]
    include!(env!("BRIDGE_STATUS_PROTO_RS"));

    #[cfg(not(feature = "bazel_build"))]
    tonic::include_proto!("bridge.status.v1");
}
