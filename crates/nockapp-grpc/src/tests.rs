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
    fn grpc_wire_source_has_owned_lifetime() {
        // Converted wire sources are owned by the WireRepr and released with
        // it; driver-created wires may still borrow static source literals.
        use nockapp::wire::WireRepr;

        use crate::pb::common::v1::wire_tag::Value;
        use crate::pb::common::v1::{Wire, WireTag};
        use crate::wire_conversion::grpc_wire_to_nockapp;

        let wire_with_source = |source: &str| Wire {
            source: source.to_string(),
            version: 1,
            tags: vec![WireTag {
                value: Some(Value::Text("poke".to_string())),
            }],
        };

        let big_source = "x".repeat(1 << 20);
        let wire = wire_with_source(&big_source);
        let repr = grpc_wire_to_nockapp(&wire).expect("converts");
        assert_eq!(repr.source.as_ref(), big_source);
        drop(repr);

        for _ in 0..100 {
            let wire = wire_with_source("zk-pow-miner");
            let repr = grpc_wire_to_nockapp(&wire).expect("converts");
            assert_eq!(repr.source.as_ref(), "zk-pow-miner");
        }

        // Driver-style construction still borrows 'static literals.
        let repr = WireRepr::new("sys", 1, vec![]);
        assert_eq!(repr.source.as_ref(), "sys");

        assert!(grpc_wire_to_nockapp(&wire_with_source("")).is_err());
    }

    #[test]
    fn test_error_codes() {
        use crate::pb::common::v1::ErrorCode;

        // Test that error codes are defined
        assert_eq!(ErrorCode::PeekFailed as i32, 2);
        assert_eq!(ErrorCode::PokeFailed as i32, 4);
        assert_eq!(ErrorCode::Timeout as i32, 6);
    }

    // ── WatchEffects streaming RPC integration test ──────────────────
    //
    // Constructs a `NockAppHandle` from raw channels (no real kernel —
    // the in-process effect bus and our manual control of it is the only
    // moving piece), spawns the private gRPC server bound to a free
    // ephemeral port, opens a `WatchEffects` client with a head-atom
    // filter, pushes effects into the broadcast bus, and asserts the
    // client receives only the filter-matching ones.
    #[tokio::test]
    async fn watch_effects_round_trip_with_head_filter() {
        use std::net::{SocketAddr, TcpListener};
        use std::sync::Arc;
        use std::time::Duration;

        use futures::StreamExt;
        use nockapp::driver::{IOAction, NockAppHandle};
        use nockapp::noun::slab::NounSlab;
        use nockapp::NockAppExit;
        use nockvm::noun::{IndirectAtom, NounAllocator, D, T};
        use nockvm_macros::tas;
        use once_cell::sync::Lazy;
        use tokio::sync::{broadcast, mpsc, Mutex};

        use crate::services::private_nockapp::client::PrivateNockAppGrpcClient;
        use crate::services::private_nockapp::server::PrivateNockAppGrpcServer;

        // Singleton metrics — gnort's global registry rejects double-registration.
        // Carry the metrics across tests via Lazy, shared by Arc clone.
        static METRICS: Lazy<Arc<nockapp::nockapp::metrics::NockAppMetrics>> = Lazy::new(|| {
            Arc::new(
                nockapp::nockapp::metrics::NockAppMetrics::register(
                    gnort::global_metrics_registry(),
                )
                .expect("register NockAppMetrics"),
            )
        });

        // ── 1. Bind a TcpListener to discover a free port; drop it; reuse the
        // port for tonic. Tiny race window between drop and rebind, fine for
        // a one-shot test.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let addr: SocketAddr = listener.local_addr().expect("local_addr");
        drop(listener);
        let server_url = format!("http://{addr}");

        // ── 2. Build a NockAppHandle from raw channels. The broadcast
        // sender we keep here is what we push test effects into;
        // `handle.effect_sender` is a clone, so the server's subscribe()
        // sees the same bus.
        let (action_tx, mut action_rx) = mpsc::channel::<IOAction>(64);
        let (effect_tx, _effect_seed_rx) = broadcast::channel::<NounSlab>(64);
        let effect_tx = Arc::new(effect_tx);
        let effect_rx_for_handle = effect_tx.subscribe();
        let (exit, _exit_rx) = NockAppExit::new();
        let handle = NockAppHandle {
            io_sender: action_tx,
            effect_sender: effect_tx.clone(),
            effect_receiver: Mutex::new(effect_rx_for_handle),
            metrics: METRICS.clone(),
            exit,
        };

        // Drain the action channel; we never poke/peek in this test.
        let _action_drainer = tokio::spawn(async move {
            while let Some(_action) = action_rx.recv().await {
                // intentionally ignored
            }
        });

        // ── 3. Spawn the server on the discovered port.
        // No bearer tokens in this test: opt out of private gRPC auth explicitly.
        let server = PrivateNockAppGrpcServer::new(handle).with_allow_unauthenticated(true);
        let server_handle = tokio::spawn(async move { server.serve(addr).await });

        // Give the server a tiny moment to come up.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // A miner enables mining immediately after this RPC returns. The first
        // candidate emitted by that poke must reach the new subscriber.
        let mut client = PrivateNockAppGrpcClient::connect(server_url.clone())
            .await
            .expect("client connect");
        for candidate in 0..32u64 {
            let mut stream = client
                .watch_effects(1, vec![b"mine".to_vec()])
                .await
                .expect("watch_effects subscribe");
            let mut slab = NounSlab::new();
            let head = D(tas!(b"mine"));
            let root = T(&mut slab, &[head, D(candidate)]);
            slab.set_root(root);
            effect_tx.send(slab).expect("publish first mine effect");

            let received = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("first stream.next within timeout")
                .expect("first stream not closed")
                .expect("first client received slab");
            let space = received.noun_space();
            let received_noun = unsafe { *received.root() };
            let cell = received_noun
                .in_space(&space)
                .as_cell()
                .expect("first effect is a cell");
            assert!(cell.head().eq_bytes("mine"));
            assert_eq!(
                cell.tail()
                    .as_atom()
                    .expect("first payload atom")
                    .as_u64()
                    .expect("first payload fits u64"),
                candidate
            );
        }

        let mut stream = client
            .watch_effects(1, vec![b"mine".to_vec()])
            .await
            .expect("watch_effects subscribe");

        // ── 5. Push a `[%mine 42]` effect → client should receive.
        {
            let mut slab = NounSlab::new();
            let head = D(tas!(b"mine"));
            let root = T(&mut slab, &[head, D(42)]);
            slab.set_root(root);
            effect_tx.send(slab).expect("publish mine effect");
        }
        let received = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("stream.next within timeout")
            .expect("stream not closed")
            .expect("client received slab");
        let space = received.noun_space();
        let received_noun = unsafe { *received.root() };
        let cell = received_noun
            .in_space(&space)
            .as_cell()
            .expect("effect is a cell");
        assert!(cell.head().eq_bytes("mine"));
        assert_eq!(
            cell.tail()
                .as_atom()
                .expect("payload atom")
                .as_u64()
                .expect("u64"),
            42
        );

        // ── 6. Push a `[%irrelvnt 99]` effect (8-char tas! limit) → client
        // must NOT receive.
        {
            let mut slab = NounSlab::new();
            let head = D(tas!(b"irrelvnt"));
            let root = T(&mut slab, &[head, D(99)]);
            slab.set_root(root);
            effect_tx.send(slab).expect("publish irrelevant effect");
        }
        let race = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
        assert!(
            race.is_err(),
            "expected no message for filtered-out effect, got one"
        );

        // ── 7. Push another `[%mine 7]` effect → client should receive again.
        {
            let mut slab = NounSlab::new();
            let head = D(tas!(b"mine"));
            let root = T(&mut slab, &[head, D(7)]);
            slab.set_root(root);
            effect_tx.send(slab).expect("publish mine effect 2");
        }
        let received = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("stream.next within timeout")
            .expect("stream not closed")
            .expect("client received slab 2");
        let space = received.noun_space();
        let received_noun = unsafe { *received.root() };
        let cell = received_noun
            .in_space(&space)
            .as_cell()
            .expect("effect is a cell");
        assert!(cell.head().eq_bytes("mine"));
        assert_eq!(
            cell.tail()
                .as_atom()
                .expect("payload atom")
                .as_u64()
                .expect("u64"),
            7
        );

        const LARGE_EFFECT_BYTES: usize = 5 * 1024 * 1024;
        let payload = vec![0xab; LARGE_EFFECT_BYTES];
        let mut slab = NounSlab::new();
        let head = D(tas!(b"mine"));
        let payload_atom = unsafe {
            let mut atom = IndirectAtom::new_raw_bytes(&mut slab, payload.len(), payload.as_ptr());
            let space = slab.noun_space();
            atom.normalize_as_atom(&space).as_noun()
        };
        let root = T(&mut slab, &[head, payload_atom]);
        slab.set_root(root);
        effect_tx.send(slab).expect("publish large mine effect");

        let received = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("large stream.next within timeout")
            .expect("large stream not closed")
            .expect("client received large slab");
        let space = received.noun_space();
        let received_noun = unsafe { *received.root() };
        let cell = received_noun
            .in_space(&space)
            .as_cell()
            .expect("large effect is a cell");
        assert!(cell.head().eq_bytes("mine"));
        assert_eq!(
            cell.tail()
                .as_atom()
                .expect("large payload atom")
                .bit_size(),
            LARGE_EFFECT_BYTES * 8
        );

        // ── 8. Tear down.
        drop(stream);
        drop(client);
        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn watch_effects_lag_ends_stream_with_error() {
        use std::net::{SocketAddr, TcpListener};
        use std::sync::{Arc, LazyLock};
        use std::time::Duration;

        use futures::StreamExt;
        use nockapp::driver::{IOAction, NockAppHandle};
        use nockapp::noun::slab::NounSlab;
        use nockapp::NockAppExit;
        use nockvm::noun::{D, T};
        use nockvm_macros::tas;
        use tokio::sync::{broadcast, mpsc, Mutex};

        use crate::services::private_nockapp::client::PrivateNockAppGrpcClient;
        use crate::services::private_nockapp::server::PrivateNockAppGrpcServer;

        static METRICS: LazyLock<Arc<nockapp::nockapp::metrics::NockAppMetrics>> =
            LazyLock::new(|| {
                Arc::new(
                    nockapp::nockapp::metrics::NockAppMetrics::register(
                        gnort::global_metrics_registry(),
                    )
                    .expect("register NockAppMetrics"),
                )
            });

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let addr: SocketAddr = listener.local_addr().expect("local_addr");
        drop(listener);
        let server_url = format!("http://{addr}");

        let (action_tx, mut action_rx) = mpsc::channel::<IOAction>(64);
        let (effect_tx, _effect_seed_rx) = broadcast::channel::<NounSlab>(8);
        let effect_tx = Arc::new(effect_tx);
        let effect_rx_for_handle = effect_tx.subscribe();
        let (exit, _exit_rx) = NockAppExit::new();
        let handle = NockAppHandle {
            io_sender: action_tx,
            effect_sender: effect_tx.clone(),
            effect_receiver: Mutex::new(effect_rx_for_handle),
            metrics: METRICS.clone(),
            exit,
        };
        let _action_drainer =
            tokio::spawn(async move { while action_rx.recv().await.is_some() {} });

        // No bearer tokens in this test: opt out of private gRPC auth explicitly.
        let server = PrivateNockAppGrpcServer::new(handle).with_allow_unauthenticated(true);
        let server_handle = tokio::spawn(async move { server.serve(addr).await });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = PrivateNockAppGrpcClient::connect(server_url)
            .await
            .expect("client connect");
        let mut stream = client
            .watch_effects(1, vec![b"mine".to_vec()])
            .await
            .expect("watch_effects subscribe");
        tokio::time::sleep(Duration::from_millis(50)).await;

        for i in 0..256u64 {
            let mut slab = NounSlab::new();
            let root = T(&mut slab, &[D(tas!(b"mine")), D(i)]);
            slab.set_root(root);
            effect_tx.send(slab).expect("publish mine effect");
        }

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "lagged WatchEffects stream did not report an error"
            );
            match tokio::time::timeout(Duration::from_secs(1), stream.next())
                .await
                .expect("stream.next within timeout")
            {
                Some(Ok(_)) => continue,
                Some(Err(e)) => {
                    let msg = e.to_string();
                    assert!(
                        msg.contains("WatchEffects stream error") && msg.contains("lagged"),
                        "unexpected stream error: {msg}"
                    );
                    break;
                }
                None => panic!("lagged WatchEffects stream closed without an error"),
            }
        }

        drop(stream);
        drop(client);
        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn poke_response_preserves_kernel_nack_error_status() {
        use std::net::{SocketAddr, TcpListener};
        use std::sync::{Arc, LazyLock};
        use std::time::Duration;

        use nockapp::driver::{IOAction, NockAppHandle, PokeResult};
        use nockapp::noun::slab::NounSlab;
        use nockapp::NockAppExit;
        use nockvm::noun::D;
        use tokio::sync::{broadcast, mpsc, Mutex};

        use crate::pb::common::v1::{ErrorCode, Wire};
        use crate::services::private_nockapp::client::{
            PokeResponseResult, PrivateNockAppGrpcClient,
        };
        use crate::services::private_nockapp::server::PrivateNockAppGrpcServer;

        static METRICS: LazyLock<Arc<nockapp::nockapp::metrics::NockAppMetrics>> =
            LazyLock::new(|| {
                Arc::new(
                    nockapp::nockapp::metrics::NockAppMetrics::register(
                        gnort::global_metrics_registry(),
                    )
                    .expect("register NockAppMetrics"),
                )
            });

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let addr: SocketAddr = listener.local_addr().expect("local_addr");
        drop(listener);
        let server_url = format!("http://{addr}");

        let (action_tx, mut action_rx) = mpsc::channel::<IOAction>(64);
        let (effect_tx, effect_rx_for_handle) = broadcast::channel::<NounSlab>(64);
        let (exit, _exit_rx) = NockAppExit::new();
        let handle = NockAppHandle {
            io_sender: action_tx,
            effect_sender: Arc::new(effect_tx),
            effect_receiver: Mutex::new(effect_rx_for_handle),
            metrics: METRICS.clone(),
            exit,
        };

        let action_handler = tokio::spawn(async move {
            match action_rx.recv().await.expect("poke action") {
                IOAction::Poke { ack_channel, .. } => {
                    ack_channel.send(PokeResult::Nack).expect("send nack");
                }
                IOAction::Peek { .. } => panic!("unexpected peek"),
            }
        });

        // No bearer tokens in this test: opt out of private gRPC auth explicitly.
        let server = PrivateNockAppGrpcServer::new(handle).with_allow_unauthenticated(true);
        let server_handle = tokio::spawn(async move { server.serve(addr).await });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut slab: NounSlab<nockapp::noun::slab::NockJammer> = NounSlab::new();
        slab.set_root(D(42));
        let payload = slab.jam().to_vec();
        let wire = Wire {
            source: "grpc".to_string(),
            version: 1,
            tags: vec![],
        };

        let mut client = PrivateNockAppGrpcClient::connect(server_url)
            .await
            .expect("client connect");
        let response = client
            .poke_response(1, wire, payload)
            .await
            .expect("poke response");

        match response {
            PokeResponseResult::Error(error) => {
                assert_eq!(error.code, ErrorCode::PokeFailed as i32);
                assert!(error.message.contains("Poke operation failed"));
            }
            other => panic!("expected preserved poke error status, got {other:?}"),
        }

        action_handler.await.expect("action handler");
        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn miner_credential_is_restricted_over_real_grpc() {
        use std::net::{SocketAddr, TcpListener};
        use std::sync::{Arc, LazyLock};
        use std::time::Duration;

        use nockapp::driver::{IOAction, NockAppHandle, PokeResult};
        use nockapp::noun::slab::NounSlab;
        use nockapp::noun::AtomExt;
        use nockapp::NockAppExit;
        use nockvm::noun::{Atom, D, T};
        use nockvm_macros::tas;
        use tokio::sync::{broadcast, mpsc, Mutex};
        use tonic::{Code, Request};

        use crate::pb::common::v1::{wire_tag, Wire, WireTag};
        use crate::pb::private::v1::nock_app_service_client::NockAppServiceClient;
        use crate::pb::private::v1::{
            poke_response, PeekRequest, PokeRequest, WatchEffectsRequest,
        };
        use crate::services::private_nockapp::server::{
            PrivateGrpcServerConfig, PrivateNockAppGrpcServer,
        };

        fn with_token<T>(body: T, token: &str) -> Request<T> {
            let mut request = Request::new(body);
            request.metadata_mut().insert(
                "x-nockchain-private-token",
                token.parse().expect("metadata token"),
            );
            request
        }

        static METRICS: LazyLock<Arc<nockapp::nockapp::metrics::NockAppMetrics>> =
            LazyLock::new(|| {
                Arc::new(
                    nockapp::nockapp::metrics::NockAppMetrics::register(
                        gnort::global_metrics_registry(),
                    )
                    .expect("register metrics"),
                )
            });

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let addr: SocketAddr = listener.local_addr().expect("local address");
        drop(listener);
        let (action_tx, mut action_rx) = mpsc::channel::<IOAction>(4);
        let (effect_tx, effect_rx) = broadcast::channel::<NounSlab>(4);
        let (exit, _exit_rx) = NockAppExit::new();
        let handle = NockAppHandle {
            io_sender: action_tx,
            effect_sender: Arc::new(effect_tx),
            effect_receiver: Mutex::new(effect_rx),
            metrics: METRICS.clone(),
            exit,
        };
        let operator = "0123456789abcdef0123456789abcdef";
        let miner = "abcdef0123456789abcdef0123456789";
        let config = PrivateGrpcServerConfig::new(
            addr,
            Some(operator.to_string()),
            Some(miner.to_string()),
            false,
        )
        .expect("validated role configuration");
        let server = PrivateNockAppGrpcServer::new(handle).with_config(config);
        let server_handle = tokio::spawn(async move { server.serve(addr).await });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = NockAppServiceClient::connect(format!("http://{addr}"))
            .await
            .expect("connect to private gRPC");
        let no_token = client
            .peek(Request::new(PeekRequest {
                pid: 1,
                path: vec![],
            }))
            .await
            .expect_err("missing token must be rejected");
        assert_eq!(no_token.code(), Code::Unauthenticated);

        let peek = client
            .peek(with_token(
                PeekRequest {
                    pid: 1,
                    path: vec![],
                },
                miner,
            ))
            .await
            .expect_err("miner cannot Peek");
        assert_eq!(peek.code(), Code::PermissionDenied);
        let watch = client
            .watch_effects(with_token(
                WatchEffectsRequest {
                    pid: 1,
                    head_filter: vec![],
                },
                miner,
            ))
            .await
            .expect_err("miner cannot watch unfiltered effects");
        assert_eq!(watch.code(), Code::PermissionDenied);
        let poke = client
            .poke(with_token(
                PokeRequest {
                    pid: 1,
                    wire: Some(Wire {
                        source: "grpc".to_string(),
                        version: 1,
                        tags: vec![],
                    }),
                    payload: vec![],
                },
                miner,
            ))
            .await
            .expect_err("miner cannot Poke an arbitrary wire");
        assert_eq!(poke.code(), Code::PermissionDenied);

        let allowed = client
            .watch_effects(with_token(
                WatchEffectsRequest {
                    pid: 1,
                    head_filter: vec![b"mine-ai".to_vec()],
                },
                miner,
            ))
            .await
            .expect("miner may watch AI mining candidates");
        drop(allowed);
        let allowed_zk = client
            .watch_effects(with_token(
                WatchEffectsRequest {
                    pid: 1,
                    head_filter: vec![b"mine-zk".to_vec()],
                },
                miner,
            ))
            .await
            .expect("miner may watch ZK mining candidates");
        drop(allowed_zk);
        assert!(
            action_rx.try_recv().is_err(),
            "denied calls reached the kernel"
        );

        let action_handler = tokio::spawn(async move {
            for expected_source in ["ai-pow-miner", "zk-pow-miner"] {
                match action_rx
                    .recv()
                    .await
                    .expect("allowed mined proof should reach node")
                {
                    IOAction::Poke {
                        wire, ack_channel, ..
                    } => {
                        assert_eq!(wire.source.as_ref(), expected_source);
                        ack_channel.send(PokeResult::Ack).expect("ack mined proof");
                    }
                    IOAction::Peek { .. } => panic!("miner must never Peek"),
                }
            }
        });
        for (source, kind) in [("ai-pow-miner", "ai-pow"), ("zk-pow-miner", "dumb-zkpow")] {
            let mut slab: NounSlab<nockapp::noun::slab::NockJammer> = NounSlab::new();
            let kind = <Atom as AtomExt>::from_value(&mut slab, kind)
                .expect("mined kind atom")
                .as_noun();
            let command = T(
                &mut slab,
                &[D(tas!(b"command")), D(tas!(b"pow")), kind, D(0)],
            );
            slab.set_root(command);
            let response = client
                .poke(with_token(
                    PokeRequest {
                        pid: 1,
                        wire: Some(Wire {
                            source: source.to_string(),
                            version: 1,
                            tags: vec![WireTag {
                                value: Some(wire_tag::Value::Text("mined".to_string())),
                            }],
                        }),
                        payload: slab.jam().to_vec(),
                    },
                    miner,
                ))
                .await
                .expect("restricted miner proof should reach node");
            assert!(matches!(
                response.into_inner().result,
                Some(poke_response::Result::Acknowledged(true))
            ));
        }
        action_handler.await.expect("proof action handler");
        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn private_server_without_operator_token_requires_explicit_opt_out() {
        use std::net::{SocketAddr, TcpListener};
        use std::sync::{Arc, LazyLock};
        use std::time::Duration;

        use nockapp::driver::{IOAction, NockAppHandle};
        use nockapp::noun::slab::NounSlab;
        use nockapp::NockAppExit;
        use tokio::sync::{broadcast, mpsc, Mutex};
        use tonic::{Code, Request};

        use crate::pb::private::v1::nock_app_service_client::NockAppServiceClient;
        use crate::pb::private::v1::WatchEffectsRequest;
        use crate::services::private_nockapp::server::{
            PrivateGrpcServerConfig, PrivateNockAppGrpcServer,
        };

        static METRICS: LazyLock<Arc<nockapp::nockapp::metrics::NockAppMetrics>> =
            LazyLock::new(|| {
                Arc::new(
                    nockapp::nockapp::metrics::NockAppMetrics::register(
                        gnort::global_metrics_registry(),
                    )
                    .expect("register metrics"),
                )
            });

        fn make_handle() -> (NockAppHandle, mpsc::Receiver<IOAction>) {
            let (action_tx, action_rx) = mpsc::channel::<IOAction>(4);
            let (effect_tx, effect_rx) = broadcast::channel::<NounSlab>(4);
            let (exit, _exit_rx) = NockAppExit::new();
            let handle = NockAppHandle {
                io_sender: action_tx,
                effect_sender: Arc::new(effect_tx),
                effect_receiver: Mutex::new(effect_rx),
                metrics: METRICS.clone(),
                exit,
            };
            (handle, action_rx)
        }

        fn free_loopback_addr() -> SocketAddr {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
            let addr = listener.local_addr().expect("local address");
            drop(listener);
            addr
        }

        // 1. No tokens and no opt-out: `serve` refuses before binding.
        let (handle, _action_rx) = make_handle();
        let addr = free_loopback_addr();
        let err = PrivateNockAppGrpcServer::new(handle)
            .serve(addr)
            .await
            .expect_err("tokenless private server must not start without the opt-out");
        let message = err.to_string();
        assert!(
            message.contains("NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN"),
            "{message}"
        );
        assert!(
            message.contains("--allow-unauthenticated-private-grpc"),
            "{message}"
        );
        assert!(
            TcpListener::bind(addr).is_ok(),
            "refused server must not hold the listener"
        );

        // 2. No tokens with the explicit opt-out: the server starts and a
        //    tokenless caller is served.
        let (handle, _action_rx_open) = make_handle();
        let addr = free_loopback_addr();
        let config = PrivateGrpcServerConfig::new(addr, None, None, true)
            .expect("explicit loopback development opt-out");
        let server = PrivateNockAppGrpcServer::new(handle).with_config(config);
        let open_server = tokio::spawn(async move { server.serve(addr).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut client = NockAppServiceClient::connect(format!("http://{addr}"))
            .await
            .expect("connect to opted-out private gRPC");
        let stream = client
            .watch_effects(Request::new(WatchEffectsRequest {
                pid: 1,
                head_filter: vec![],
            }))
            .await
            .expect("opted-out server serves a tokenless caller");
        drop(stream);
        open_server.abort();
        let _ = open_server.await;

        // 3. An operator token with the opt-out set: the token still applies.
        let (handle, _action_rx_token) = make_handle();
        let addr = free_loopback_addr();
        let config = PrivateGrpcServerConfig::new(
            addr,
            Some("0123456789abcdef0123456789abcdef".to_string()),
            None,
            true,
        )
        .expect("token configuration remains authenticated with opt-out");
        let server = PrivateNockAppGrpcServer::new(handle).with_config(config);
        let token_server = tokio::spawn(async move { server.serve(addr).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut client = NockAppServiceClient::connect(format!("http://{addr}"))
            .await
            .expect("connect to token-protected private gRPC");
        let denied = client
            .watch_effects(Request::new(WatchEffectsRequest {
                pid: 1,
                head_filter: vec![],
            }))
            .await
            .expect_err("opt-out must not bypass a configured operator token");
        assert_eq!(denied.code(), Code::Unauthenticated);
        token_server.abort();
        let _ = token_server.await;
    }
}
