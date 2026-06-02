pub fn tracked_git_watch_paths(
    head_path: &str,
    packed_refs_path: &str,
    head_ref_path: Option<&str>,
) -> Vec<String> {
    let mut paths = vec![head_path.to_string(), packed_refs_path.to_string()];
    if let Some(head_ref_path) = head_ref_path {
        let head_ref_path = head_ref_path.trim();
        if !head_ref_path.is_empty() {
            paths.push(head_ref_path.to_string());
        }
    }
    paths
}

pub fn release_binary_link_args(
    profile: &str,
    target_os: &str,
    target_env: &str,
) -> Vec<&'static str> {
    // Linux/GNU PIE builds made PMA fsync-on quick-read throughput layout-sensitive
    // after the trusted-orchestrate text growth. Non-PIE costs text ASLR for this
    // benchmark binary, but restored the validated 83-84 peeks/s release layout.
    if profile == "release" && target_os == "linux" && target_env == "gnu" {
        vec!["-no-pie"]
    } else {
        Vec::new()
    }
}
