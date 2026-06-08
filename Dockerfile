FROM ubuntu:24.04

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY target/release/nockchain /usr/local/bin/nockchain

ENV RUST_LOG=info,nockchain=info,nockchain_libp2p_io=info,libp2p=info,libp2p_quic=info
ENV MINIMAL_LOG_FORMAT=true

ENTRYPOINT ["/usr/local/bin/nockchain"]
