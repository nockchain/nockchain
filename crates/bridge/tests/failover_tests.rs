#![allow(clippy::unwrap_used, clippy::unnecessary_cast)]
//! Integration tests for 3-of-5 degraded operation and failover.
//!
//! These tests validate the core architectural change: all nodes sign,
//! proposer selects and posts, failover works when proposer is offline.
//!
//! ## Test Scenarios
//!
//! 1. **Happy path** - All 5 nodes healthy, deposit flows through
//! 2. **Proposer offline** - Kill proposer, verify failover after backoff
//! 3. **Minimum viable (3 nodes)** - Kill 2 nodes, verify still operational
//! 4. **Below threshold (2 nodes)** - Kill 3 nodes, verify bridge halts
//! 5. **Node recovery** - Stalled deposit completes when node returns
//! 6. **Duplicate prevention** - Same deposit twice, verify single submission
//! 7. **Crash recovery** - Kill proposer mid-posting, restart, verify completion

#[cfg(feature = "bazel_build")]
use bridge_test_harness::{sample_deposit, sample_deposit_with_nonce, TestCluster, TestNode};
#[cfg(not(feature = "bazel_build"))]
mod test_harness;

#[cfg(not(feature = "bazel_build"))]
use self::test_harness::{sample_deposit, sample_deposit_with_nonce, TestCluster, TestNode};

/// Test 1: Happy path - all 5 nodes online, deposit goes through.
///
/// Expected behavior:
/// - All 5 nodes sign the deposit
/// - Proposer collects 5 signatures
/// - Proposer posts to Base successfully
/// - Exactly 1 submission recorded
#[tokio::test]
async fn test_happy_path_all_nodes_online() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(100, 1000);

    // Trigger deposit across all nodes
    cluster.trigger_deposit(deposit.clone()).await;

    // Wait for signatures to propagate (all 5 online nodes should sign)
    cluster.wait_for_signatures(&deposit, 5).await;

    // Verify all nodes have threshold
    for node in &cluster.nodes {
        assert!(
            node.has_threshold(&bridge::types::DepositId::from_effect_payload(&deposit)),
            "Node {} should have reached threshold",
            node.node_id
        );
    }

    // Proposer posts
    let proposer_id = cluster.initial_proposer(&deposit);
    let result = cluster.post_deposit_from_node(&deposit, proposer_id);
    assert!(result.is_ok(), "Proposer should successfully post deposit");

    // Verify exactly 1 submission
    let submissions = cluster.base_submissions();
    assert_eq!(submissions.len(), 1, "Should have exactly 1 submission");
    assert_eq!(
        submissions[0].submitter_node_id, proposer_id,
        "Submission should be from proposer"
    );
    assert_eq!(
        submissions[0].signature_count, 5,
        "Should have all 5 signatures"
    );
}

/// Test 2: Proposer offline - failover to next node in rotation.
///
/// Expected behavior:
/// - 4 nodes sign (proposer offline)
/// - Threshold still reached (3-of-5)
/// - Failover node waits for backoff period
/// - Failover node posts successfully
#[tokio::test]
async fn test_proposer_offline_failover() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(200, 2000);

    // Determine proposer and kill it BEFORE deposit triggers
    let proposer_id = (deposit.block_height as usize) % 5;
    cluster.kill_node(proposer_id).await;

    // Trigger deposit (proposer is offline, won't sign)
    cluster.trigger_deposit(deposit.clone()).await;

    // Wait for remaining 4 nodes to reach threshold
    cluster.wait_for_signatures(&deposit, 4).await;

    // Verify proposer cannot post (offline)
    let result = cluster.post_deposit_from_node(&deposit, proposer_id);
    assert!(result.is_err(), "Offline proposer should fail to post");

    // Next node in rotation should be able to post (after backoff in real system)
    let failover_id = (proposer_id + 1) % 5;
    let result = cluster.post_deposit_from_node(&deposit, failover_id);
    assert!(result.is_ok(), "Failover node should successfully post");

    // Verify submission
    let submissions = cluster.base_submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].submitter_node_id, failover_id);
    assert!(
        submissions[0].signature_count >= 3,
        "Should have at least 3 signatures"
    );
}

/// Test 3: Minimum viable configuration - 3 of 5 nodes online.
///
/// Expected behavior:
/// - Exactly 3 nodes online and signing
/// - Threshold reached (3-of-5)
/// - Deposit still processes successfully
#[tokio::test]
async fn test_minimum_viable_three_nodes() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(300, 3000);

    // Kill 2 nodes (leaving exactly 3 online)
    cluster.kill_node(1).await;
    cluster.kill_node(3).await;

    // Trigger deposit
    cluster.trigger_deposit(deposit.clone()).await;

    // Wait for 3 nodes to reach threshold
    cluster.wait_for_signatures(&deposit, 3).await;

    // Any online node with threshold can post
    let online_nodes: Vec<usize> = cluster
        .nodes
        .iter()
        .filter(|n| n.is_online())
        .map(|n| n.node_id)
        .collect();

    assert_eq!(online_nodes.len(), 3, "Should have exactly 3 online nodes");

    // First online node posts
    let poster_id = online_nodes[0];
    let result = cluster.post_deposit_from_node(&deposit, poster_id);
    assert!(result.is_ok(), "Online node should successfully post");

    // Verify submission
    let submissions = cluster.base_submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].signature_count, 3);
}

/// Test 4: Below threshold - only 2 of 5 nodes online.
///
/// Expected behavior:
/// - Only 2 signatures collected
/// - Threshold NOT reached (need 3-of-5)
/// - Bridge cannot post deposit
#[tokio::test]
async fn test_below_threshold_bridge_halts() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(400, 4000);

    // Kill 3 nodes (leaving only 2 online)
    cluster.kill_node(0).await;
    cluster.kill_node(2).await;
    cluster.kill_node(4).await;

    // Trigger deposit
    cluster.trigger_deposit(deposit.clone()).await;

    // Give time for signatures to propagate
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Verify NO node has reached threshold
    for node in &cluster.nodes {
        if node.is_online() {
            assert!(
                !node.has_threshold(&bridge::types::DepositId::from_effect_payload(&deposit)),
                "Node {} should NOT have threshold with only 2 signatures",
                node.node_id
            );
        }
    }

    // Attempt to post from online node should fail
    let online_id = cluster
        .nodes
        .iter()
        .find(|n| n.is_online())
        .map(|n| n.node_id)
        .unwrap();

    let result = cluster.post_deposit_from_node(&deposit, online_id);
    assert!(result.is_err(), "Should fail to post without threshold");

    // Verify no submissions
    assert_eq!(cluster.base_submissions().len(), 0);
}

/// Test 5: Node recovery - deposit completes when third node returns.
///
/// Expected behavior:
/// - Start with 2 nodes (below threshold)
/// - Bring third node online
/// - Threshold reached
/// - Deposit successfully posts
#[tokio::test]
async fn test_node_recovery_completes_stalled_deposit() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(500, 5000);

    // Kill 3 nodes initially (only 2 online)
    cluster.kill_node(0).await;
    cluster.kill_node(2).await;
    cluster.kill_node(4).await;

    // Trigger deposit (will stall)
    cluster.trigger_deposit(deposit.clone()).await;

    // Verify stalled (no threshold)
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    let online_nodes: Vec<&TestNode> = cluster.nodes.iter().filter(|n| n.is_online()).collect();
    assert_eq!(online_nodes.len(), 2);

    for node in &online_nodes {
        assert!(
            !node.has_threshold(&bridge::types::DepositId::from_effect_payload(&deposit)),
            "Should not have threshold with 2 nodes"
        );
    }

    // Restart one node (now 3 online)
    cluster.restart_node(2).await;

    // Rebroadcast deposit to newly online node
    cluster.trigger_deposit(deposit.clone()).await;

    // Wait for threshold
    cluster.wait_for_signatures(&deposit, 3).await;

    // Now deposit can post
    let poster_id = cluster
        .nodes
        .iter()
        .find(|n| {
            n.is_online()
                && n.has_threshold(&bridge::types::DepositId::from_effect_payload(&deposit))
        })
        .map(|n| n.node_id)
        .unwrap();

    let result = cluster.post_deposit_from_node(&deposit, poster_id);
    assert!(
        result.is_ok(),
        "Deposit should complete after node recovery"
    );

    // Verify submission
    assert_eq!(cluster.base_submissions().len(), 1);
}

/// Test 6: Duplicate prevention - same deposit submitted twice rejected.
///
/// Expected behavior:
/// - First submission succeeds
/// - Second submission from different node fails
/// - Only 1 on-chain record
#[tokio::test]
async fn test_duplicate_deposit_prevention() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(600, 6000);

    // Trigger deposit
    cluster.trigger_deposit(deposit.clone()).await;

    // Wait for signatures
    cluster.wait_for_signatures(&deposit, 5).await;

    // First node posts
    let result1 = cluster.post_deposit_from_node(&deposit, 0);
    assert!(result1.is_ok(), "First submission should succeed");

    // Second node tries to post (duplicate)
    let result2 = cluster.post_deposit_from_node(&deposit, 1);
    assert!(result2.is_err(), "Duplicate submission should fail");

    // Verify only 1 submission
    assert_eq!(cluster.base_submissions().len(), 1);
}

/// Test 7: Crash recovery - proposer crashes mid-posting, restart and complete.
///
/// Expected behavior:
/// - Proposer starts posting
/// - Proposer crashes (simulated by going offline)
/// - Proposer restarts
/// - Proposer successfully completes posting
#[tokio::test]
async fn test_crash_recovery() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(700, 7000);

    // Trigger deposit
    cluster.trigger_deposit(deposit.clone()).await;

    // Wait for signatures
    cluster.wait_for_signatures(&deposit, 5).await;

    // Determine proposer
    let proposer_id = cluster.initial_proposer(&deposit);

    // Simulate crash: kill proposer before posting
    cluster.kill_node(proposer_id).await;

    // Verify proposer cannot post while offline
    let result = cluster.post_deposit_from_node(&deposit, proposer_id);
    assert!(result.is_err(), "Offline proposer should fail");

    // Restart proposer
    cluster.restart_node(proposer_id).await;

    // After restart, proposer can complete posting
    // (In real system, would need to re-sync cache from peers)
    cluster.trigger_deposit(deposit.clone()).await;
    cluster.wait_for_signatures(&deposit, 5).await;

    let result = cluster.post_deposit_from_node(&deposit, proposer_id);
    assert!(
        result.is_ok(),
        "Restarted proposer should successfully post"
    );

    // Verify submission
    assert_eq!(cluster.base_submissions().len(), 1);
}

/// Test 8: Multiple concurrent deposits with mixed node availability.
///
/// Expected behavior:
/// - Process multiple unique deposits concurrently
/// - With one node offline, remaining 4 should still reach threshold
/// - All deposits successfully complete
///
/// NOTE: This test is currently disabled due to timing issues with concurrent signature
/// propagation in the mock harness. The actual production code handles this correctly
/// via gossip protocol and persistent storage.
#[tokio::test]
async fn test_concurrent_deposits_mixed_availability() {
    let mut cluster = TestCluster::new(5).await;

    // Create 3 unique deposits with DIFFERENT as_of values (ensures unique DepositIds)
    let deposits: Vec<_> = (0..3)
        .map(|i| sample_deposit_with_nonce(800 + i, 8000 + (i * 100) as u64, 1 + i as u64))
        .collect();

    // Kill node 2 (4 nodes remain - still above threshold)
    cluster.kill_node(2).await;

    // Trigger all deposits
    for deposit in &deposits {
        cluster.trigger_deposit(deposit.clone()).await;
    }

    // Wait for signatures (4 nodes online)
    for deposit in &deposits {
        cluster.wait_for_signatures(deposit, 4).await;
    }

    // Post each deposit from its proposer (or any online node with threshold)
    for (i, deposit) in deposits.iter().enumerate() {
        let proposer_id = cluster.initial_proposer(deposit);

        // Use proposer if online, otherwise use node 0
        let poster_id = if cluster.nodes[proposer_id].is_online() {
            proposer_id
        } else {
            0
        };

        let result = cluster.post_deposit_from_node(deposit, poster_id);

        assert!(
            result.is_ok(),
            "Deposit {} (proposer {}, poster {}) should complete with 4 nodes online",
            i,
            proposer_id,
            poster_id
        );
    }

    // Verify all 3 deposits posted
    assert_eq!(
        cluster.base_submissions().len(),
        3,
        "All 3 deposits should be posted"
    );
}

/// Test 9: Signature threshold edge case - exactly 3 signatures.
///
/// Expected behavior:
/// - Exactly 3 nodes sign
/// - Threshold reached (3 == threshold)
/// - Deposit posts successfully
#[tokio::test]
async fn test_exactly_threshold_signatures() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(900, 9000);

    // Kill 2 nodes
    cluster.kill_node(3).await;
    cluster.kill_node(4).await;

    // Trigger deposit (only 3 will sign)
    cluster.trigger_deposit(deposit.clone()).await;

    // Wait for 3 signatures
    cluster.wait_for_signatures(&deposit, 3).await;

    // Verify threshold reached
    let online_count = cluster
        .nodes
        .iter()
        .filter(|n| {
            n.is_online()
                && n.has_threshold(&bridge::types::DepositId::from_effect_payload(&deposit))
        })
        .count();

    assert_eq!(online_count, 3, "All 3 online nodes should have threshold");

    // Post successfully
    let result = cluster.post_deposit_from_node(&deposit, 0);
    assert!(
        result.is_ok(),
        "Should post with exactly threshold signatures"
    );

    // Verify submission
    let submissions = cluster.base_submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].signature_count, 3);
}

/// Test 10: All nodes offline except proposer - cannot reach threshold.
///
/// Expected behavior:
/// - Only proposer signs (1 signature)
/// - Threshold NOT reached
/// - Bridge halts
#[tokio::test]
async fn test_only_proposer_online_fails() {
    let mut cluster = TestCluster::new(5).await;
    let deposit = sample_deposit(1000, 10000);

    let proposer_id = cluster.initial_proposer(&deposit);

    // Kill all nodes except proposer
    for i in 0..5 {
        if i != proposer_id {
            cluster.kill_node(i).await;
        }
    }

    // Trigger deposit
    cluster.trigger_deposit(deposit.clone()).await;

    // Give time for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify proposer alone cannot reach threshold
    assert!(
        !cluster.nodes[proposer_id]
            .has_threshold(&bridge::types::DepositId::from_effect_payload(&deposit)),
        "Single node should not reach threshold"
    );

    // Attempt to post should fail
    let result = cluster.post_deposit_from_node(&deposit, proposer_id);
    assert!(result.is_err(), "Single signature should not be enough");

    // No submissions
    assert_eq!(cluster.base_submissions().len(), 0);
}
