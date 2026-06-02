#[path = "../build_support.rs"]
mod build_support;

#[test]
fn tracked_git_watch_paths_use_common_dir_branch_ref_for_worktrees() {
    let paths = build_support::tracked_git_watch_paths(
        "/shared/nockchain/.git/worktrees/bench-harness-phase2-closeout/HEAD",
        "/shared/nockchain/.git/packed-refs",
        Some("/shared/nockchain/.git/refs/heads/bench-harness-phase2-closeout"),
    );

    assert_eq!(
        paths,
        vec![
            "/shared/nockchain/.git/worktrees/bench-harness-phase2-closeout/HEAD",
            "/shared/nockchain/.git/packed-refs",
            "/shared/nockchain/.git/refs/heads/bench-harness-phase2-closeout",
        ]
    );
}

#[test]
fn release_binary_link_args_disable_pie_only_for_linux_gnu_release() {
    assert_eq!(
        build_support::release_binary_link_args("release", "linux", "gnu"),
        vec!["-no-pie"]
    );
    assert!(build_support::release_binary_link_args("debug", "linux", "gnu").is_empty());
    assert!(build_support::release_binary_link_args("release", "macos", "").is_empty());
    assert!(build_support::release_binary_link_args("release", "linux", "musl").is_empty());
}
