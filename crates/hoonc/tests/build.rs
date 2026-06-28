// This test must be run in release mode or it will stack overflow.
mod test {

    use tempfile::TempDir;
    use tracing::{debug, info};

    #[ignore = "Skipping because test is too slow and CI should already self host hoonc from scratch"]
    #[tokio::test]
    async fn test_compile_test_app() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let temp_path = temp_dir.path();
        let out_path = format!(
            "{}/out.jam",
            hoonc::canonicalize_and_string(temp_path).to_lowercase()
        );

        // use std::path to get pwd() and then canonicalize
        let pwd = std::env::current_dir()?;
        let mut test_dir = pwd.clone();
        test_dir.pop();
        test_dir.push("test-app");

        let entry = test_dir.join("bootstrap/kernel.hoon");

        // TODO: Add -o flag to specify output file and then use the tmp-dir
        // TODO: instead of mutating the non-tmp filesystem in this test
        // Clean up any existing output file
        let _ = tokio::fs::remove_file(out_path).await;

        let mut deps_dir = pwd.clone();
        deps_dir.pop();
        deps_dir.push("hoon-deps");
        info!("Test directory: {:?}", test_dir);
        info!("Dependencies directory: {:?}", deps_dir);
        info!("Entry file: {:?}", entry);

        let (nockapp, out_path) =
            hoonc::initialize_with_default_cli(entry, deps_dir, None, false, false, false, true)
                .await?;

        let result = hoonc::run_build(nockapp, Some(out_path.clone())).await;
        assert!(result.is_ok());

        // Cleanup
        let _ = tokio::fs::remove_file(out_path.clone()).await;
        debug!("Removed file");

        // Second run to test consecutive execution
        // FIXME: This currently panics because of the one-shot.
        // let result = test_build(&mut nockapp).await;
        // // Cleanup
        // let _ = fs::remove_file("out.jam").await;
        Ok(())
    }
}
