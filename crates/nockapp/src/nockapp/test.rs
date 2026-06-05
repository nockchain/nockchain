use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::NockApp;
use crate::kernel::form::Kernel;
use crate::save::SaveableCheckpoint;

fn load_jam_bytes(jam: &str) -> Vec<u8> {
    // Try multiple possible locations for the jam file
    let possible_paths = [
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-jams")
            .join(jam),
        Path::new("open/crates/nockapp/test-jams").join(jam),
        // Add other potential paths
    ];

    possible_paths
        .iter()
        .find_map(|path| fs::read(path).ok())
        .unwrap_or_else(|| panic!("Failed to read {} file from any known location", jam))
}

pub async fn setup_nockapp(jam: &str) -> (TempDir, NockApp) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let jam_bytes = load_jam_bytes(jam);

    let kernel_f = move |_| async move {
        let kernel = Kernel::load(&jam_bytes, None, vec![], Default::default(), None).await?;
        Ok::<Kernel<SaveableCheckpoint>, crate::CrownError>(kernel)
    };
    (
        temp_dir,
        NockApp::new(kernel_f)
            .await
            .expect("Could not create NockApp"),
    )
}

#[cfg(test)]
pub mod tests {
    use bytes::Bytes;
    use nockvm::ext::noun_equality;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Noun, NounAllocator};
    use nockvm::serialization::{cue, jam};
    use nockvm::unifying_equality::unifying_equality;

    use super::setup_nockapp;
    use crate::noun::slab::{slab_equality, NounSlab};
    use crate::utils::NOCK_STACK_SIZE;
    use crate::NounExt;

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn test_jam_equality_stack() {
        let (_temp, nockapp) = setup_nockapp("test-ker.jam").await;
        let kernel = nockapp.kernel;
        let mut jam_stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let arvo_slab = kernel
            .serf
            .get_kernel_state_slab()
            .await
            .expect("Could not get kernel state slab");
        let mut arvo = arvo_slab.copy_to_stack(&mut jam_stack);
        let j = jam(&mut jam_stack, arvo);
        let mut c = cue(&mut jam_stack, j).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        // new nockstack
        unsafe { assert!(unifying_equality(&mut jam_stack, &mut arvo, &mut c)) }
    }

    // This actually gets used to test with miri
    // but when it was successful it took too long.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_equality_slab_no_driver() {
        let bytes = include_bytes!("../../test-jams/test-ker.jam");
        let mut slab1: NounSlab = NounSlab::new();
        slab1
            .cue_into(Bytes::from(Vec::from(bytes)))
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let jammed_bytes = slab1.jam();
        let mut slab2: NounSlab = NounSlab::new();
        let _c = slab2.cue_into(jammed_bytes).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert!(slab_equality(&slab1, &slab2));
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn test_jam_equality_slab() {
        let (_temp, nockapp) = setup_nockapp("test-ker.jam").await;
        let kernel = nockapp.kernel;
        let mut state_slab = kernel
            .serf
            .get_kernel_state_slab()
            .await
            .expect("Could not get kernel state slab");
        let bytes = state_slab.jam();
        let c = state_slab.cue_into(bytes).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let space = state_slab.noun_space();
        let root = unsafe { state_slab.root() };
        assert!(noun_equality(root.in_space(&space), c.in_space(&space)));
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn test_jam_equality_slab_stack() {
        let (_temp, nockapp) = setup_nockapp("test-ker.jam").await;
        let kernel = nockapp.kernel;
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let state_slab = kernel
            .serf
            .get_kernel_state_slab()
            .await
            .expect("Failed to get kernel state slab");
        // Use slab to jam
        let bytes = state_slab.jam();
        // Use the stack to cue
        let mut c = Noun::cue_bytes(&mut stack, &bytes).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let mut state_stack = state_slab.copy_to_stack(&mut stack);
        unsafe {
            // check for equality
            assert!(unifying_equality(&mut stack, &mut state_stack, &mut c))
        }
    }
}
