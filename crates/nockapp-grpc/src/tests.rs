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
        use nockvm::noun::{NounAllocator, D, T};
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
        let server = PrivateNockAppGrpcServer::new(handle);
        let server_handle = tokio::spawn(async move { server.serve(addr).await });

        // Give the server a tiny moment to come up.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ── 4. Connect a client and open WatchEffects with head_filter = [b"mine"].
        let mut client = PrivateNockAppGrpcClient::connect(server_url.clone())
            .await
            .expect("client connect");
        let mut stream = client
            .watch_effects(1, vec![b"mine".to_vec()])
            .await
            .expect("watch_effects subscribe");

        // Pause briefly so the server-side subscriber registers before we publish.
        tokio::time::sleep(Duration::from_millis(50)).await;

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

        // ── 8. Tear down.
        drop(stream);
        drop(client);
        server_handle.abort();
        let _ = server_handle.await;
    }
}
