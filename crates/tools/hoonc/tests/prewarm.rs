use blake3::Hasher;

#[test]
fn prewarm_kernel_hash_matches_kernel_jam() {
    // The prewarm exported state must correspond to the bundled kernel jam; otherwise boot will
    // attempt to `+load` an incompatible state, which can be extremely expensive or fail.
    let exported =
        nockapp::export::ExportedState::decode(hoonc::PREWARM_STATE_JAM).expect("decode prewarm");

    let mut hasher = Hasher::new();
    hasher.update(hoonc::KERNEL_JAM);
    let kernel_hash = hasher.finalize();

    assert_eq!(
        exported.ker_hash, kernel_hash,
        "prewarm state ker_hash does not match bundled kernel jam hash"
    );
}
