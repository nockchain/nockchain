use nockapp::driver::PokeResult;
use nockapp::noun::slab::NounSlab;
use nockapp::test::setup_nockapp;
use nockapp::wire::{SystemWire, Wire};
use nockapp::NockApp;
use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
use nockvm::noun::{Noun, NounAllocator, D};
use nockvm_macros::tas;
use tracing::info;

#[tracing::instrument(skip(nockapp))]
fn run_once(nockapp: &mut NockApp, i: u64) {
    info!("before poke construction");
    let poke = make_inc_poke();
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
    let space = res.noun_space();
    let root = unsafe { *res.root() };
    let val: Noun = root
        .in_space(&space)
        .slot(15)
        .map(|handle| handle.noun())
        .unwrap_or_else(|err| {
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
        let _test_arena = install_test_arena();
        for i in 1..4 {
            let poke = make_inc_poke();
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
            let space = res.noun_space();
            let root = unsafe { *res.root() };
            let val: Noun = root
                .in_space(&space)
                .slot(15)
                .map(|handle| handle.noun())
                .unwrap_or_else(|err| {
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

fn install_test_arena() -> NockStack {
    NockStack::new(NOCK_STACK_SIZE_TINY, 0)
}

fn make_inc_poke() -> NounSlab {
    let mut poke = NounSlab::new();
    poke.set_root(D(tas!(b"inc")));
    poke
}

fn state_peek_slab() -> NounSlab {
    [D(tas!(b"state")), D(0)].into()
}

fn timer_poke_slab() -> NounSlab {
    let mut slab = NounSlab::new();
    let timer_noun = nockvm::noun::T(&mut slab, &[D(tas!(b"command")), D(tas!(b"timer")), D(0)]);
    slab.set_root(timer_noun);
    slab
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(miri, ignore)]
async fn test_lax1_timeout_storm_does_not_starve_followup_peek_or_timer() {
    use std::time::Duration;

    use nockapp::drivers::timer::TimerWire;

    // Reproduces the LAX1 failure shape at the nockapp layer: once enough
    // timeout-driven poke tasks pile up, unrelated follow-up peek and timer
    // work should still make progress. On current code, both stall.
    let (_temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
    let burst = 4096usize;
    let timeout = Duration::from_nanos(1);
    let probe_deadline = Duration::from_secs(1);

    let mut burst_handles = Vec::with_capacity(burst);
    for _ in 0..burst {
        burst_handles.push(nockapp.get_handle());
    }
    let followup_handle = nockapp.get_handle();
    let shutdown_handle = nockapp.get_handle();

    let run_task = tokio::spawn(async move { nockapp.run().await });

    let mut burst_tasks = tokio::task::JoinSet::new();
    for handle in burst_handles {
        burst_tasks.spawn(async move {
            handle
                .poke_timeout(SystemWire.to_wire(), make_inc_poke(), timeout)
                .await
        });
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (followup_peek, followup_timer) = tokio::join!(
        tokio::time::timeout(probe_deadline, followup_handle.peek(state_peek_slab())),
        tokio::time::timeout(
            probe_deadline,
            followup_handle.poke(TimerWire::Tick.to_wire(), timer_poke_slab()),
        )
    );

    burst_tasks.abort_all();
    let _ = shutdown_handle.exit.exit(0).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;

    assert!(
        followup_peek.is_ok(),
        "follow-up peek timed out while timeout storm was active; timer_completed={}",
        followup_timer.is_ok(),
    );
    assert!(
        followup_timer.is_ok(),
        "follow-up timer poke timed out while timeout storm was active; peek_completed={}",
        followup_peek.is_ok(),
    );

    let followup_peek = followup_peek.expect("peek timeout already checked");
    let followup_timer = followup_timer.expect("timer timeout already checked");

    followup_peek.expect("follow-up peek should succeed while timeout storm is active");
    match followup_timer {
        Ok(PokeResult::Ack) | Ok(PokeResult::Nack) => {}
        Err(err) => {
            panic!("follow-up timer poke should complete while timeout storm is active: {err:?}")
        }
    }
}
