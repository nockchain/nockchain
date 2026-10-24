#![allow(clippy::unwrap_used)]

use std::collections::{HashMap, VecDeque};

use bridge::core::observation::base::{BaseObserverCore, BaseObserverRunner, BasePlanAction};
use bridge::core::observation::nock::{NockObserverCore, NockObserverRunner, NockPlanAction};
use bridge::core::ports::NockTipInfo;
use bridge::shared::errors::BridgeError;
use bridge::shared::runtime::{BaseBlockBatch, ChainEvent, NockBlockEvent};
use bridge::shared::types::{zero_tip5_hash, AtomBytes, BaseBlockRef};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::{BigNum, CoinbaseSplit, Hash as NockHash, Page};
use noun_serde::NounEncode;

#[path = "harness/in_memory_ports.rs"]
mod in_memory_ports;

use in_memory_ports::{InMemoryBaseSource, InMemoryKernelState};

fn sample_batch(start: u64, end: u64) -> BaseBlockBatch {
    let mut blocks = Vec::new();
    for height in start..=end {
        blocks.push(BaseBlockRef {
            height,
            block_id: AtomBytes(vec![height as u8]),
            parent_block_id: AtomBytes(vec![height.saturating_sub(1) as u8]),
        });
    }

    BaseBlockBatch {
        version: 0,
        first_height: start,
        last_height: end,
        blocks,
        withdrawals: Vec::new(),
        deposit_settlements: Vec::new(),
        block_events: HashMap::new(),
        prev: zero_tip5_hash(),
    }
}

fn sample_nock_block_event(height: u64) -> NockBlockEvent {
    let page = Page {
        digest: NockHash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
        pow: None,
        parent: NockHash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        tx_ids: vec![],
        coinbase: CoinbaseSplit::V0(vec![]),
        timestamp: 0,
        epoch_counter: 0,
        target: BigNum::from_u64(0),
        accumulated_work: BigNum::from_u64(0),
        height,
        msg: vec![],
    };

    let mut page_slab: NounSlab<NockJammer> = NounSlab::new();
    let page_noun = page.to_noun(&mut page_slab);

    NockBlockEvent {
        block: page,
        page_slab,
        page_noun,
        txs: vec![],
    }
}

#[tokio::test]
async fn base_runner_fetches_and_emits_batch_event() {
    let source = InMemoryBaseSource::default();
    *source.chain_tip.lock().await = 500;
    source
        .batches
        .lock()
        .await
        .extend(VecDeque::from([sample_batch(101, 105)]));

    let kernel = InMemoryKernelState::default();
    *kernel.base_next_height.lock().await = Some(101);

    let runner = BaseObserverRunner {
        core: BaseObserverCore {
            batch_size: 5,
            confirmation_depth: 10,
        },
        source,
        kernel: kernel.clone(),
    };

    let action = runner.tick_once().await.unwrap();
    assert_eq!(
        action,
        BasePlanAction::FetchWindow {
            chain_tip: 500,
            confirmed_height: 490,
            start: 101,
            end: 105,
        }
    );

    let events = kernel.events.lock().await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        ChainEvent::Base(batch) => {
            assert_eq!(batch.first_height, 101);
            assert_eq!(batch.last_height, 105);
            assert_eq!(batch.blocks.len(), 5);
        }
        ChainEvent::Nock(_) => panic!("expected base event"),
    }
}

#[tokio::test]
async fn base_runner_no_pending_height_emits_nothing() {
    let source = InMemoryBaseSource::default();
    *source.chain_tip.lock().await = 500;

    let kernel = InMemoryKernelState::default();
    *kernel.base_next_height.lock().await = None;

    let runner = BaseObserverRunner {
        core: BaseObserverCore {
            batch_size: 5,
            confirmation_depth: 10,
        },
        source,
        kernel: kernel.clone(),
    };

    let action = runner.tick_once().await.unwrap();
    assert_eq!(action, BasePlanAction::NoPendingHeight { chain_tip: 500 });
    assert!(kernel.events.lock().await.is_empty());
}

#[tokio::test]
async fn base_runner_pending_commit_emits_nothing() {
    let source = InMemoryBaseSource::default();
    *source.chain_tip.lock().await = 500;

    let kernel = InMemoryKernelState::default();
    *kernel.base_pending_commit_active.lock().await = true;
    *kernel.base_next_height.lock().await = Some(101);

    let runner = BaseObserverRunner {
        core: BaseObserverCore {
            batch_size: 5,
            confirmation_depth: 10,
        },
        source,
        kernel: kernel.clone(),
    };

    let action = runner.tick_once().await.unwrap();
    assert_eq!(action, BasePlanAction::PendingBaseBlockCommitActive);
    assert!(kernel.events.lock().await.is_empty());
}

#[tokio::test]
async fn nock_runner_fetches_and_emits_block_event() {
    let mut source = in_memory_ports::InMemoryNockSource::default();
    source.tips.push_back(NockTipInfo {
        height: 250,
        tip_hash: "tip-b58".to_string(),
    });
    source.blocks.push_back(sample_nock_block_event(150));

    let kernel = InMemoryKernelState::default();
    *kernel.nock_next_height.lock().await = Some(150);

    let mut runner = NockObserverRunner {
        core: NockObserverCore {
            confirmation_depth: 100,
        },
        source,
        kernel: kernel.clone(),
    };

    let action = runner.tick_once().await.unwrap();
    assert_eq!(
        action,
        NockPlanAction::FetchHeight {
            tip_height: 250,
            confirmed_target: 150,
            height: 150,
        }
    );

    let events = kernel.events.lock().await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        ChainEvent::Nock(block) => {
            assert_eq!(block.height(), 150);
            assert_eq!(block.txs.len(), 0);
        }
        ChainEvent::Base(_) => panic!("expected nock event"),
    }
}

#[tokio::test]
async fn nock_runner_not_yet_confirmed_emits_nothing() {
    let mut source = in_memory_ports::InMemoryNockSource::default();
    source.tips.push_back(NockTipInfo {
        height: 250,
        tip_hash: "tip-b58".to_string(),
    });

    let kernel = InMemoryKernelState::default();
    *kernel.nock_next_height.lock().await = Some(175);

    let mut runner = NockObserverRunner {
        core: NockObserverCore {
            confirmation_depth: 100,
        },
        source,
        kernel: kernel.clone(),
    };

    let action = runner.tick_once().await.unwrap();
    assert_eq!(
        action,
        NockPlanAction::NotYetConfirmed {
            tip_height: 250,
            confirmed_target: 150,
            next_needed_height: 175,
        }
    );
    assert!(kernel.events.lock().await.is_empty());
}

#[tokio::test]
async fn base_runner_replay_transcript_handles_mixed_paths_deterministically() {
    let source = InMemoryBaseSource::default();
    source.chain_tip_responses.lock().await.extend([
        Ok(500),
        Ok(2800),
        Ok(2800),
        Err(BridgeError::Runtime(
            "simulated base tip source error".to_string(),
        )),
    ]);
    source
        .batch_responses
        .lock()
        .await
        .push_back(Ok(sample_batch(1001, 2000)));

    let kernel = InMemoryKernelState::default();
    kernel.base_next_height_results.lock().await.extend([
        Ok(None),
        Ok(Some(2001)),
        Ok(Some(1001)),
        Ok(Some(1001)),
    ]);

    let runner = BaseObserverRunner {
        core: BaseObserverCore {
            batch_size: 1000,
            confirmation_depth: 300,
        },
        source,
        kernel: kernel.clone(),
    };

    let action1 = runner.tick_once().await.expect("tick1");
    assert_eq!(action1, BasePlanAction::NoPendingHeight { chain_tip: 500 });

    let action2 = runner.tick_once().await.expect("tick2");
    assert_eq!(
        action2,
        BasePlanAction::NotYetConfirmed {
            chain_tip: 2800,
            confirmed_height: 2500,
            next_needed_height: 2001,
            needed_confirmed_height: 3000,
            blocks_until_ready: 500,
        }
    );

    let action3 = runner.tick_once().await.expect("tick3");
    assert_eq!(
        action3,
        BasePlanAction::FetchWindow {
            chain_tip: 2800,
            confirmed_height: 2500,
            start: 1001,
            end: 2000,
        }
    );

    let err = runner.tick_once().await.expect_err("tick4 should fail");
    assert!(
        err.to_string().contains("simulated base tip source error"),
        "unexpected error: {err}"
    );

    let events = kernel.events.lock().await;
    assert_eq!(events.len(), 1, "only the fetch tick should emit");
    match &events[0] {
        ChainEvent::Base(batch) => {
            assert_eq!(batch.first_height, 1001);
            assert_eq!(batch.last_height, 2000);
        }
        ChainEvent::Nock(_) => panic!("expected base event"),
    }
}

#[tokio::test]
async fn nock_runner_replay_transcript_handles_fetch_none_and_error_paths() {
    let mut source = in_memory_ports::InMemoryNockSource::default();
    source.tip_results.extend([
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-a".to_string(),
        })),
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-b".to_string(),
        })),
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-c".to_string(),
        })),
        Err(BridgeError::Runtime(
            "simulated nock tip source error".to_string(),
        )),
    ]);
    source
        .block_results
        .extend([Ok(Some(sample_nock_block_event(150))), Ok(None)]);

    let kernel = InMemoryKernelState::default();
    kernel.nock_next_height_results.lock().await.extend([
        Ok(Some(175)),
        Ok(Some(150)),
        Ok(Some(150)),
        Ok(Some(150)),
    ]);

    let mut runner = NockObserverRunner {
        core: NockObserverCore {
            confirmation_depth: 100,
        },
        source,
        kernel: kernel.clone(),
    };

    let action1 = runner.tick_once().await.expect("tick1");
    assert_eq!(
        action1,
        NockPlanAction::NotYetConfirmed {
            tip_height: 250,
            confirmed_target: 150,
            next_needed_height: 175,
        }
    );

    let action2 = runner.tick_once().await.expect("tick2");
    assert_eq!(
        action2,
        NockPlanAction::FetchHeight {
            tip_height: 250,
            confirmed_target: 150,
            height: 150,
        }
    );

    let action3 = runner.tick_once().await.expect("tick3");
    assert_eq!(
        action3,
        NockPlanAction::FetchHeight {
            tip_height: 250,
            confirmed_target: 150,
            height: 150,
        }
    );

    let err = runner.tick_once().await.expect_err("tick4 should fail");
    assert!(
        err.to_string().contains("simulated nock tip source error"),
        "unexpected error: {err}"
    );

    let events = kernel.events.lock().await;
    assert_eq!(events.len(), 1, "only one fetch should emit an event");
    match &events[0] {
        ChainEvent::Nock(block) => assert_eq!(block.height(), 150),
        ChainEvent::Base(_) => panic!("expected nock event"),
    }
}

#[tokio::test]
async fn base_runner_replay_peek_and_emit_errors_then_recovers() {
    let source = InMemoryBaseSource::default();
    source
        .chain_tip_responses
        .lock()
        .await
        .extend([Ok(2800), Ok(2800), Ok(2800)]);
    source
        .batch_responses
        .lock()
        .await
        .extend([Ok(sample_batch(1001, 2000)), Ok(sample_batch(1001, 2000))]);

    let kernel = InMemoryKernelState::default();
    kernel.base_next_height_results.lock().await.extend([
        Err(BridgeError::Runtime(
            "simulated base peek source error".to_string(),
        )),
        Ok(Some(1001)),
        Ok(Some(1001)),
    ]);
    kernel
        .emit_results
        .lock()
        .await
        .push_back(Err(BridgeError::Runtime(
            "simulated base emit error".to_string(),
        )));

    let runner = BaseObserverRunner {
        core: BaseObserverCore {
            batch_size: 1000,
            confirmation_depth: 300,
        },
        source,
        kernel: kernel.clone(),
    };

    let err1 = runner.tick_once().await.expect_err("tick1 should fail");
    assert!(
        err1.to_string()
            .contains("simulated base peek source error"),
        "unexpected error: {err1}"
    );

    let err2 = runner.tick_once().await.expect_err("tick2 should fail");
    assert!(
        err2.to_string().contains("simulated base emit error"),
        "unexpected error: {err2}"
    );

    let action3 = runner.tick_once().await.expect("tick3 should recover");
    assert_eq!(
        action3,
        BasePlanAction::FetchWindow {
            chain_tip: 2800,
            confirmed_height: 2500,
            start: 1001,
            end: 2000,
        }
    );

    let events = kernel.events.lock().await;
    assert_eq!(events.len(), 1, "only recovered emit should be recorded");
    match &events[0] {
        ChainEvent::Base(batch) => {
            assert_eq!(batch.first_height, 1001);
            assert_eq!(batch.last_height, 2000);
        }
        ChainEvent::Nock(_) => panic!("expected base event"),
    }
}

#[tokio::test]
async fn base_runner_replay_fetch_error_then_recovers() {
    let source = InMemoryBaseSource::default();
    source
        .chain_tip_responses
        .lock()
        .await
        .extend([Ok(2800), Ok(2800)]);
    source.batch_responses.lock().await.extend([
        Err(BridgeError::Runtime(
            "simulated base batch fetch error".to_string(),
        )),
        Ok(sample_batch(1001, 2000)),
    ]);

    let kernel = InMemoryKernelState::default();
    kernel
        .base_next_height_results
        .lock()
        .await
        .extend([Ok(Some(1001)), Ok(Some(1001))]);

    let runner = BaseObserverRunner {
        core: BaseObserverCore {
            batch_size: 1000,
            confirmation_depth: 300,
        },
        source,
        kernel: kernel.clone(),
    };

    let err1 = runner.tick_once().await.expect_err("tick1 should fail");
    assert!(
        err1.to_string()
            .contains("simulated base batch fetch error"),
        "unexpected error: {err1}"
    );

    let action2 = runner.tick_once().await.expect("tick2 should recover");
    assert_eq!(
        action2,
        BasePlanAction::FetchWindow {
            chain_tip: 2800,
            confirmed_height: 2500,
            start: 1001,
            end: 2000,
        }
    );
    assert_eq!(
        kernel.events.lock().await.len(),
        1,
        "only recovered fetch should emit"
    );
}

#[tokio::test]
async fn nock_runner_replay_peek_and_emit_errors_then_recovers() {
    let mut source = in_memory_ports::InMemoryNockSource::default();
    source.tip_results.extend([
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-a".to_string(),
        })),
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-b".to_string(),
        })),
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-c".to_string(),
        })),
    ]);
    source
        .block_results
        .extend([Ok(Some(sample_nock_block_event(150))), Ok(Some(sample_nock_block_event(150)))]);

    let kernel = InMemoryKernelState::default();
    kernel.nock_next_height_results.lock().await.extend([
        Err(BridgeError::Runtime(
            "simulated nock peek source error".to_string(),
        )),
        Ok(Some(150)),
        Ok(Some(150)),
    ]);
    kernel
        .emit_results
        .lock()
        .await
        .push_back(Err(BridgeError::Runtime(
            "simulated nock emit error".to_string(),
        )));

    let mut runner = NockObserverRunner {
        core: NockObserverCore {
            confirmation_depth: 100,
        },
        source,
        kernel: kernel.clone(),
    };

    let err1 = runner.tick_once().await.expect_err("tick1 should fail");
    assert!(
        err1.to_string()
            .contains("simulated nock peek source error"),
        "unexpected error: {err1}"
    );

    let err2 = runner.tick_once().await.expect_err("tick2 should fail");
    assert!(
        err2.to_string().contains("simulated nock emit error"),
        "unexpected error: {err2}"
    );

    let action3 = runner.tick_once().await.expect("tick3 should recover");
    assert_eq!(
        action3,
        NockPlanAction::FetchHeight {
            tip_height: 250,
            confirmed_target: 150,
            height: 150,
        }
    );
    assert_eq!(
        kernel.events.lock().await.len(),
        1,
        "only recovered nock emit should be recorded"
    );
}

#[tokio::test]
async fn nock_runner_replay_fetch_error_and_none_then_recovers() {
    let mut source = in_memory_ports::InMemoryNockSource::default();
    source.tip_results.extend([
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-a".to_string(),
        })),
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-b".to_string(),
        })),
        Ok(Some(NockTipInfo {
            height: 250,
            tip_hash: "tip-c".to_string(),
        })),
    ]);
    source.block_results.extend([
        Err(BridgeError::Runtime(
            "simulated nock block fetch error".to_string(),
        )),
        Ok(None),
        Ok(Some(sample_nock_block_event(150))),
    ]);

    let kernel = InMemoryKernelState::default();
    kernel.nock_next_height_results.lock().await.extend([
        Ok(Some(150)),
        Ok(Some(150)),
        Ok(Some(150)),
    ]);

    let mut runner = NockObserverRunner {
        core: NockObserverCore {
            confirmation_depth: 100,
        },
        source,
        kernel: kernel.clone(),
    };

    let err1 = runner.tick_once().await.expect_err("tick1 should fail");
    assert!(
        err1.to_string()
            .contains("simulated nock block fetch error"),
        "unexpected error: {err1}"
    );

    let action2 = runner.tick_once().await.expect("tick2 should continue");
    assert_eq!(
        action2,
        NockPlanAction::FetchHeight {
            tip_height: 250,
            confirmed_target: 150,
            height: 150,
        }
    );
    assert!(
        kernel.events.lock().await.is_empty(),
        "none fetch result should not emit"
    );

    let action3 = runner.tick_once().await.expect("tick3 should recover");
    assert_eq!(
        action3,
        NockPlanAction::FetchHeight {
            tip_height: 250,
            confirmed_target: 150,
            height: 150,
        }
    );

    let events = kernel.events.lock().await;
    assert_eq!(events.len(), 1, "recovered fetch should emit one event");
    match &events[0] {
        ChainEvent::Nock(block) => assert_eq!(block.height(), 150),
        ChainEvent::Base(_) => panic!("expected nock event"),
    }
}
