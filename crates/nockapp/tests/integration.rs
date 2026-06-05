use nockapp::noun::slab::NounSlab;
use nockapp::test::setup_nockapp;
use nockapp::wire::{SystemWire, Wire};
use nockapp::NockApp;
use nockvm::noun::{Noun, Slots, D};
use nockvm_macros::tas;
use tracing::info;

#[tracing::instrument(skip(nockapp))]
fn run_once(nockapp: &mut NockApp, i: u64) {
    info!("before poke construction");
    let poke = D(tas!(b"inc")).into();
    info!("Poke constructed");
    let wire = SystemWire.to_wire();
    info!("Wire constructed");
    let _ = nockapp.poke_sync(wire, poke).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    info!("after poke_sync");
    let peek: NounSlab = [D(tas!(b"state")), D(0)].into();
    // res should be [~ ~ %0 val]
    let res = nockapp.peek_sync(peek);
    info!("after peek_sync");
    let res = res.unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    let root = unsafe { res.root() };
    let val: Noun = root.slot(15).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    unsafe {
        assert!(val.raw_equals(&D(i)), "Expected {} but got {:?}", i, val);
    }
    info!("after raw_equals");
}

// This is just an experimental test to exercise the tracing
// To run this test:
// OTEL_SERVICE_NAME="nockapp_test" RUST_LOG="debug" OTEL_EXPORTER_JAEGER_ENDPOINT=http://localhost:4317 cargo nextest run test_looped_sync_peek_and_poke --nocapture --run-ignored all
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn test_looped_sync_peek_and_poke() {
    use nockapp::observability::*;
    let subscriber = init_tracing().unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    eprintln!("Use docker compose up to start prometheus and jaeger");
    eprintln!("Prometheus dashboard: http://localhost:9090/");
    eprintln!("Jaeger dashboard: http://localhost:16686/");
    let (_temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("Starting run_forever");
        for i in 1.. {
            info!("before run_once");
            run_once(&mut nockapp, i);
            info!("after run_once");
        }
    });
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_sync_peek_and_poke() {
    let (_temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
    tokio::task::spawn_blocking(move || {
        for i in 1..4 {
            let poke = D(tas!(b"inc")).into();
            let wire = SystemWire.to_wire();
            let _ = nockapp.poke_sync(wire, poke).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let peek: NounSlab = [D(tas!(b"state")), D(0)].into();
            // res should be [~ ~ %0 val]
            let res = nockapp.peek_sync(peek);
            let res = res.unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let root = unsafe { res.root() };
            let val: Noun = root.slot(15).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            unsafe {
                assert!(val.raw_equals(&D(i)));
            }
        }
    })
    .await
    .expect("Synchronous test thread failed");
}
