"""Bazel helpers for nockapp Rust test targets."""

load("@crates//:defs.bzl", "aliases", "all_crate_deps")
load("@rules_rust//rust:defs.bzl", "rust_test")

NOCKAPP_BOOT_PMA_TESTS = [
    "kernel::boot::tests::bootstraps_checkpoint_copy_into_empty_event_log",
    "kernel::boot::tests::bootstraps_pma_from_checkpoint_once",
    "kernel::boot::tests::checkpoint_ahead_of_event_log_recovers_to_sqlite_boundary",
    "kernel::boot::tests::checkpoint_behind_event_log_replays_to_sqlite_boundary",
    "kernel::boot::tests::export_state_jam_creates_parent_dir",
    "kernel::boot::tests::falls_back_from_corrupt_newest_rotating_snapshot",
    "kernel::boot::tests::falls_back_from_manifest_only_corruption",
    "kernel::boot::tests::fresh_boot_with_committed_event_log_replays_from_zero",
    "kernel::boot::tests::honors_active_snapshot_selection_before_ordering",
    "kernel::boot::tests::moves_orphan_snapshot_files_to_corrupted_pma",
    "kernel::boot::tests::pma_ahead_of_event_log_recovers_to_sqlite_boundary",
    "kernel::boot::tests::pma_gc_switches_slabs_and_rebuilds_runtime_state",
    "kernel::boot::tests::pma_lagging_event_log_replays_to_sqlite_boundary",
    "kernel::boot::tests::refuses_boot_on_event_log_gap_after_snapshot",
    "kernel::boot::tests::replay_rejected_logged_event_fails_instead_of_synthesizing_crud",
    "kernel::boot::tests::replays_logged_events_after_snapshot_restore",
    "kernel::boot::tests::restores_epoch_snapshot_when_pma_is_missing",
    "kernel::boot::tests::rotates_snapshots_and_retires_oldest",
    "kernel::boot::tests::setup_allows_new_for_empty_data_dir",
    "kernel::boot::tests::shutdown_flush_rewrites_missing_active_pma_metadata",
    "kernel::boot::tests::snapshot_kernel_hash_mismatch_loads_state_like_checkpoint",
    "kernel::boot::tests::valid_pma_skips_corrupt_checkpoint_files",
    "kernel::boot::tests::valid_pma_with_unopenable_event_log_fails_closed",
]

_NOCKAPP_TEST_COMPILE_DATA = [
    "//open/assets:dumb",
    "//open/crates/nockapp/test-jams:cue-test.jam",
    "//open/crates/nockapp/test-jams:test-ker.jam",
]

def nockapp_boot_pma_skip_args():
    """Returns libtest arguments that skip heavyweight PMA boot tests."""
    return _skip_args(NOCKAPP_BOOT_PMA_TESTS)

def nockapp_boot_pma_test_args():
    """Returns libtest arguments for the isolated PMA boot target."""
    return NOCKAPP_BOOT_PMA_TESTS + ["--test-threads=1"]

def nockapp_unit_rust_test(name, args = None, size = "medium", timeout = "moderate"):
    """Defines a nockapp unit-test target with the crate's shared test setup.

    Args:
      name: Bazel target name.
      args: Optional libtest arguments.
      size: Bazel test size.
      timeout: Bazel test timeout.
    """
    if args == None:
        args = []
    # Embedded Diesel migrations are read by a proc macro at compile time.
    migration_srcs = native.glob(["migrations/**/*.sql"])
    test_compile_data = migration_srcs + _NOCKAPP_TEST_COMPILE_DATA
    rust_test(
        name = name,
        size = size,
        timeout = timeout,
        srcs = native.glob([
            "src/**/*.rs",
            "src/*.rs",
        ]),
        aliases = aliases(),
        args = args,
        compile_data = test_compile_data,
        crate_features = [
            "bazel_build",
            "default",
            "slog-tracing",
        ],
        crate_root = "src/lib.rs",
        data = _NOCKAPP_TEST_COMPILE_DATA,
        edition = "2021",
        proc_macro_deps = [
            "//open/crates/nockvm/rust/nockvm_macros",
        ] + all_crate_deps(proc_macro = True),
        rustc_env = {
            # rust_test compiles from bazel-out; point Diesel's embed_migrations!
            # at the compile_data tree instead of the source-tree manifest dir.
            "CARGO_MANIFEST_DIR": "$(BINDIR)/open/crates/nockapp",
            "DUMB_JAM_PATH": "$(location //open/assets:dumb)",
        },
        deps = all_crate_deps(
            normal = True,
            normal_dev = True,
        ) + [
            "//open/crates/nockvm/rust/ibig",
            "//open/crates/nockvm/rust/nockvm",
            "//open/crates/noun-serde",
        ],
    )

def _skip_args(tests):
    """Builds repeated --skip arguments for libtest.

    Args:
      tests: Fully-qualified test names to skip.
    """
    args = []
    for test in tests:
        args.extend(["--skip", test])
    return args
