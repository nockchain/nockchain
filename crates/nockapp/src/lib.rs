#![feature(negative_impls)]
#![feature(slice_pattern)]
// Allow unwrap in test code - standard practice for test assertions
#![cfg_attr(test, allow(clippy::unwrap_used))]

//! # Crown
//!
//! The Crown library provides a set of modules and utilities for working with
//! the Sword runtime. It includes functionality for handling jammed nouns, kernels (as jammed nouns),
//! and various types and utilities that make nockvm easier to use.
//!
//! ## Modules
//!
//! - `kernel`: Sword runtime interface.
//! - `noun`: Extensions and utilities for working with Urbit nouns.
//! - `utils`: Errors, misc functions and extensions.
//!
pub mod drivers;
pub(crate) mod event_log;
pub mod kernel;
pub mod nockapp;
pub mod noun;
pub mod observability;
pub(crate) mod snapshot;
pub mod utils;

use std::path::PathBuf;

pub use bytes::*;
pub use drivers::*;
pub use nockapp::*;
pub use nockvm::noun::Noun;
pub use noun::{AtomExt, IndirectAtomExt, JammedNoun, NounExt};
pub use utils::bytes::{ToBytes, ToBytesExt};
pub use utils::error::{CrownError, Result};

/// Returns the default directory where kernel data is stored.
///
/// # Arguments
///
/// * `dir` - A string slice that holds the kernel identifier.
///
/// # Example
///
/// ```
///
/// use std::path::PathBuf;
/// use nockapp::default_data_dir;
/// let dir = default_data_dir("nockapp");
/// assert_eq!(dir, PathBuf::from("./.data.nockapp"));
/// ```
pub fn default_data_dir(dir_name: &str) -> PathBuf {
    PathBuf::from(format!("./.data.{}", dir_name))
}

pub fn system_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NOCKAPP_HOME") {
        if !dir.trim().is_empty() {
            let path = PathBuf::from(&dir);
            if path.is_absolute() {
                return path;
            }
            if let Ok(current) = std::env::current_dir() {
                return current.join(path);
            }
            return PathBuf::from(dir);
        }
    }

    let home_dir = dirs::home_dir().expect("Failed to get home directory");
    home_dir.join(".nockapp")
}

/// Default size for the Nock stack.
pub const DEFAULT_NOCK_STACK_SIZE: usize = nockvm::mem::NOCK_STACK_SIZE_TINY;

#[cfg(test)]
pub mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use nockvm::pma::Pma;
    use uuid::Uuid;

    /// Per-test PMA filesystem sandbox.
    ///
    /// PMA tests should allocate their persistence files under a unique directory
    /// instead of serializing the whole test crate behind a global lock. This keeps
    /// tests parallel-safe while making the persistence boundary explicit.
    pub struct TestPmaSandbox {
        path: PathBuf,
    }

    impl TestPmaSandbox {
        pub fn new() -> Self {
            let root = PathBuf::from("/tmp/.test-pma");
            fs::create_dir_all(&root).expect("create PMA test root");

            for _ in 0..16 {
                let path = root.join(Uuid::new_v4().to_string());
                match fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(err) => panic!("create PMA test sandbox {}: {err}", path.display()),
                }
            }

            panic!(
                "failed to allocate unique PMA test sandbox under {}",
                root.display()
            );
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Default for TestPmaSandbox {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for TestPmaSandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Reified PMA-equipped stack for tests that need stack + PMA noun resolution.
    pub struct TestPmaStack {
        stack: NockStack,
        pma: Pma,
        // Keep the sandbox last so file-backed mappings are dropped before the
        // directory cleanup runs.
        sandbox: TestPmaSandbox,
    }

    impl TestPmaStack {
        pub fn with_words(stack_words: usize, pma_words: usize) -> Self {
            let sandbox = TestPmaSandbox::new();
            let pma_path = sandbox.path().join("test.pma");
            let pma = Pma::new(pma_words, pma_path).expect("create test PMA");
            let mut stack = NockStack::new(stack_words, 0);
            stack.install_pma_arena(Arc::clone(pma.arena()));
            Self {
                stack,
                pma,
                sandbox,
            }
        }

        pub fn stack(&self) -> &NockStack {
            &self.stack
        }

        pub fn stack_mut(&mut self) -> &mut NockStack {
            &mut self.stack
        }

        pub fn pma(&self) -> &Pma {
            &self.pma
        }

        pub fn pma_mut(&mut self) -> &mut Pma {
            &mut self.pma
        }

        pub fn stack_pma_mut(&mut self) -> (&mut NockStack, &mut Pma) {
            (&mut self.stack, &mut self.pma)
        }

        pub fn pma_path(&self) -> &Path {
            self.pma.path().as_path()
        }

        pub fn sandbox_path(&self) -> &Path {
            self.sandbox.path()
        }

        pub fn into_sandbox(self) -> TestPmaSandbox {
            let Self {
                stack,
                pma,
                sandbox,
            } = self;
            drop(stack);
            drop(pma);
            sandbox
        }
    }

    impl Default for TestPmaStack {
        fn default() -> Self {
            Self::with_words(NOCK_STACK_SIZE_TINY, 4096)
        }
    }
}
