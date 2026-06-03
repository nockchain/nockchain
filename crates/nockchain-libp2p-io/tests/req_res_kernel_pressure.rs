use std::process::Command;
use std::time::Duration;

use nockchain_libp2p_io::test_support::{
    base58_for_tip5_seed, collect_tip5_zset_strings_for_seeds,
    measure_realistic_heard_block_decode, measure_tip5_zset_traversal_for_seeds,
    observe_bounded_key_fair_queue_pressure, observe_mixed_peer_key_fair_queue_pressure,
    QueuePressurePeer, ReqResKernelPressureProbe, ReqResStateBoundsProbe,
};

#[test]
fn req_res_kernel_pressure_deep_zset_walk_is_iterative_and_ordered() {
    let seeds = (0..4096).collect::<Vec<_>>();

    let tx_ids =
        collect_tip5_zset_strings_for_seeds(&seeds).expect("deep zset should be traversed");

    assert_eq!(tx_ids.len(), seeds.len());
    assert_eq!(tx_ids[0], base58_for_tip5_seed(0));
    assert_eq!(tx_ids[4095], base58_for_tip5_seed(4095));
}

#[test]
fn req_res_kernel_pressure_speculative_tx_prefetch_claims_are_capped_and_deduped() {
    let max_claims = 256;
    let tx_ids = (0..(max_claims + 32))
        .map(|idx| format!("tx-{idx:04}"))
        .collect::<Vec<_>>();
    let mut probe = ReqResKernelPressureProbe::new();

    let first_claim = probe.claim_speculative_tx_prefetches(tx_ids.clone(), max_claims);
    assert_eq!(first_claim.len(), max_claims);
    assert_eq!(first_claim[0], "tx-0000");
    assert_eq!(first_claim[255], "tx-0255");

    let duplicate_claim = probe.claim_speculative_tx_prefetches(tx_ids.clone(), max_claims);
    assert_eq!(duplicate_claim.len(), 32);
    assert_eq!(duplicate_claim[0], "tx-0256");

    probe.clear_speculative_tx_prefetch("tx-0000");
    let reclaimed = probe.claim_speculative_tx_prefetches(tx_ids, 1);
    assert_eq!(reclaimed, vec![String::from("tx-0000")]);
}

#[tokio::test]
async fn req_res_kernel_pressure_key_fair_queue_rejects_per_key_and_global_overflow() {
    let observation = observe_bounded_key_fair_queue_pressure().await;

    assert!(observation.per_key_rejected);
    assert!(observation.total_rejected);
    assert!(observation.recovered_after_recv);
    assert_eq!(observation.received_before_recovery, Some((1, 10)));
    assert_eq!(observation.received_after_recovery, vec![(2, 20), (3, 30)]);
}

#[tokio::test]
async fn req_res_kernel_pressure_mixed_peer_queue_preserves_honest_turns() {
    let observation = observe_mixed_peer_key_fair_queue_pressure().await;

    assert!(observation.hostile_per_key_rejected);
    assert!(observation.total_rejected_after_mixed_fill);
    assert_eq!(observation.accepted_honest_items, 3);
    assert_eq!(
        observation.received,
        vec![
            (QueuePressurePeer::Hostile, 0),
            (QueuePressurePeer::Honest(1), 10),
            (QueuePressurePeer::Honest(2), 20),
            (QueuePressurePeer::Honest(3), 30),
            (QueuePressurePeer::Hostile, 1),
            (QueuePressurePeer::Hostile, 2),
            (QueuePressurePeer::Hostile, 3),
        ]
    );
}

#[test]
#[ignore = "measurement harness, run explicitly when gathering local timing data"]
fn req_res_kernel_pressure_gossip_decode_report() {
    let sizes = [64_usize, 256, 1024, 4096, 16_384];
    let runs = 7_usize;

    println!("zset_items,runs,p50_us,p95_us,max_us,output_count");
    for item_count in sizes {
        let seeds = (0..item_count as u64).collect::<Vec<_>>();
        let mut durations = Vec::with_capacity(runs);
        let mut output_count = 0;

        for _ in 0..runs {
            let measurement = measure_tip5_zset_traversal_for_seeds(&seeds)
                .expect("zset traversal measurement should complete");
            assert_eq!(measurement.item_count, item_count);
            assert_eq!(measurement.output_count, item_count);
            output_count = measurement.output_count;
            durations.push(measurement.elapsed);
        }

        durations.sort_unstable();
        let p50 = durations[durations.len() / 2];
        let p95 = percentile_duration(&durations, 95);
        let max = durations[durations.len() - 1];
        println!(
            "{item_count},{runs},{},{},{},{output_count}",
            duration_micros(p50),
            duration_micros(p95),
            duration_micros(max)
        );
    }
}

fn percentile_duration(sorted_durations: &[Duration], percentile: usize) -> Duration {
    let last_index = sorted_durations.len().saturating_sub(1);
    let index = last_index.saturating_mul(percentile).div_ceil(100);
    sorted_durations[index]
}

fn duration_micros(duration: Duration) -> u128 {
    duration.as_micros()
}

#[test]
#[ignore = "measurement harness, run explicitly when gathering local decode data"]
fn req_res_kernel_pressure_heard_block_decode_report() {
    let tx_counts = [0_usize, 64, 256, 1024, 4096];
    let runs = 7_usize;

    println!(
        "tx_ids_per_block,runs,jam_bytes,height_p50_us,height_p95_us,tx_ids_p50_us,tx_ids_p95_us,tx_ids_max_us,decoded_tx_ids"
    );
    for tx_ids_per_block in tx_counts {
        let mut height_durations = Vec::with_capacity(runs);
        let mut tx_ids_durations = Vec::with_capacity(runs);
        let mut jam_bytes = 0_usize;
        let mut decoded_tx_ids = 0_usize;

        for run_index in 0..runs {
            let height = 20_000_u64
                .saturating_add((tx_ids_per_block as u64).saturating_mul(100))
                .saturating_add(run_index as u64);
            let tx_seed_base = height.saturating_mul(1_000_000);
            let tx_seeds = (0..tx_ids_per_block as u64)
                .map(|tx_index| tx_seed_base.saturating_add(tx_index))
                .collect::<Vec<_>>();
            let measurement = measure_realistic_heard_block_decode(height, &tx_seeds)
                .expect("synthetic heard-block fact should decode");

            assert_eq!(measurement.tx_id_count, tx_ids_per_block);
            assert_eq!(measurement.decoded_tx_id_count, tx_ids_per_block);
            assert_eq!(measurement.height, height);
            jam_bytes = measurement.jam_bytes;
            decoded_tx_ids = measurement.decoded_tx_id_count;
            height_durations.push(measurement.height_elapsed);
            tx_ids_durations.push(measurement.tx_ids_elapsed);
        }

        height_durations.sort_unstable();
        tx_ids_durations.sort_unstable();
        let height_p50 = height_durations[height_durations.len() / 2];
        let height_p95 = percentile_duration(&height_durations, 95);
        let tx_ids_p50 = tx_ids_durations[tx_ids_durations.len() / 2];
        let tx_ids_p95 = percentile_duration(&tx_ids_durations, 95);
        let tx_ids_max = tx_ids_durations[tx_ids_durations.len() - 1];

        println!(
            "{tx_ids_per_block},{runs},{jam_bytes},{},{},{},{},{},{decoded_tx_ids}",
            duration_micros(height_p50),
            duration_micros(height_p95),
            duration_micros(tx_ids_p50),
            duration_micros(tx_ids_p95),
            duration_micros(tx_ids_max)
        );
    }
}

#[test]
#[ignore = "measurement harness, run explicitly when gathering local memory data"]
fn req_res_kernel_pressure_deferred_block_memory_report() {
    let workloads = [
        ("64-blocks-0-tx", 64_usize, 0_usize),
        ("64-blocks-64-tx", 64, 64),
        ("256-blocks-64-tx", 256, 64),
        ("256-blocks-256-tx", 256, 256),
    ];

    println!(
        "workload,blocks,tx_ids_per_block,total_jam_bytes,avg_jam_bytes,rss_before_kib,rss_after_kib,rss_delta_kib,deferred_total"
    );
    for (label, block_count, tx_ids_per_block) in workloads {
        let rss_before_kib = current_rss_kib();
        let mut probe = ReqResStateBoundsProbe::new(256);
        let mut total_jam_bytes = 0_usize;

        for block_index in 0..block_count {
            let height = 10_000_u64.saturating_add(block_index as u64);
            let tx_seed_base = height.saturating_mul(1_000_000);
            let tx_seeds = (0..tx_ids_per_block as u64)
                .map(|tx_index| tx_seed_base.saturating_add(tx_index))
                .collect::<Vec<_>>();
            let jam_bytes = probe
                .defer_realistic_heard_block(height, &tx_seeds)
                .expect("synthetic heard block should build")
                .expect("synthetic heard block should fit deferred storage");
            total_jam_bytes = total_jam_bytes.saturating_add(jam_bytes);
        }

        let rss_after_kib = current_rss_kib();
        let rss_delta_kib = rss_after_kib.saturating_sub(rss_before_kib);
        let deferred_total = probe.deferred_heard_block_total();
        assert_eq!(deferred_total, block_count);

        println!(
            "{label},{block_count},{tx_ids_per_block},{total_jam_bytes},{},{rss_before_kib},{rss_after_kib},{rss_delta_kib},{deferred_total}",
            total_jam_bytes / block_count
        );
    }
}

fn current_rss_kib() -> u64 {
    let pid = std::process::id().to_string();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .expect("ps should be available for rss sampling");
    assert!(
        output.status.success(),
        "ps rss sampling failed with status {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8(output.stdout).expect("ps rss output should be utf8");
    stdout
        .trim()
        .parse::<u64>()
        .expect("ps rss output should parse as kib")
}
