use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::NockApp;
use crate::kernel::form::Kernel;

pub async fn setup_nockapp(jam: &str) -> (TempDir, NockApp) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let temp_dir_path = temp_dir.path().to_path_buf();
    // Try multiple possible locations for the jam file
    let possible_paths = [
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-jams")
            .join(jam),
        Path::new("open/crates/nockapp/test-jams").join(jam),
        // Add other potential paths
    ];

    let jam_bytes = possible_paths
        .iter()
        .find_map(|path| fs::read(path).ok())
        .unwrap_or_else(|| panic!("Failed to read {} file from any known location", jam));

    let kernel_f =
        async |checkpoint| Kernel::load(&jam_bytes, checkpoint, vec![], Default::default()).await;
    (
        temp_dir,
        NockApp::new(
            kernel_f,
            &temp_dir_path,
            Some(std::time::Duration::from_secs(1)),
        )
        .await
        .expect("Could not create NockApp"),
    )
}

#[cfg(test)]
pub mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use bytes::Bytes;
    use nockvm::jets::util::slot;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Noun, D, T};
    use nockvm::serialization::{cue, jam};
    use nockvm::unifying_equality::unifying_equality;
    use nockvm_macros::tas;
    use tracing::info;
    use tracing_test::traced_test;

    use super::setup_nockapp;
    use crate::nockapp::wire::{SystemWire, Wire};
    use crate::noun::slab::{slab_equality, slab_noun_equality, NockJammer, NounSlab};
    use crate::save::{SaveableCheckpoint, Saver};
    use crate::utils::NOCK_STACK_SIZE;
    use crate::{NockApp, NounExt};

    async fn save_nockapp(nockapp: &mut NockApp) {
        nockapp.tasks.close();
        let permit = nockapp.save_mutex.clone().lock_owned().await;
        let _ = nockapp.save(permit).await;
        let _ = nockapp.tasks.wait().await;
        nockapp.tasks.reopen();
    }

    // Panics if checkpoint failed to load, only permissible because this is expressly for testing
    async fn spawn_save_t(nockapp: &mut NockApp, sleep_t: std::time::Duration) {
        let sleepy_time = tokio::time::sleep(sleep_t);
        let permit = nockapp.save_mutex.clone().lock_owned().await;
        let _join_handle = nockapp
            .save_f(sleepy_time, permit)
            .await
            .expect("Failed to spawn nockapp save task");
        // join_handle.await.expect("Failed to save nockapp").expect("Failed to save nockapp 2");
    }

    // Test nockapp save
    // TODO: bump the actual serf event number (can we do a poke to the test kernel?)
    #[test]
    #[traced_test]
    #[cfg_attr(miri, ignore)]
    fn test_nockapp_save_race_condition() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let (temp, mut nockapp) = runtime.block_on(setup_nockapp("test-ker.jam"));
        assert_eq!(nockapp.kernel.serf.event_number.load(Ordering::SeqCst), 0);
        // first run
        runtime.block_on(spawn_save_t(&mut nockapp, Duration::from_millis(1000)));
        // second run
        nockapp.kernel.serf.event_number.store(1, Ordering::SeqCst); // we need to set the actual serf event number
        runtime.block_on(spawn_save_t(&mut nockapp, Duration::from_millis(5000)));
        // Simulate what the event handlers would be doing and wait for the task tracker to be done
        nockapp.tasks.close();
        runtime.block_on(nockapp.tasks.wait());
        nockapp.tasks.reopen();
        // Shutdown the runtime immediately
        runtime.shutdown_timeout(std::time::Duration::from_secs(0));

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to build runtime");

        let (_, checkpoint_opt) = runtime
            .block_on(Saver::<NockJammer>::try_load(
                &temp.path().to_path_buf(),
                None,
            ))
            .expect("Failed trying to load checkpoint");
        let checkpoint: SaveableCheckpoint = checkpoint_opt.expect("No checkpoint found");
        info!("checkpoint: {:?}", checkpoint);
        assert_eq!(checkpoint.event_num, 1);
    }

    // Test nockapp save
    // TODO: need a way to grab arvo state from the serf. Probably a serf action
    // TODO: use slab equality, not unifying equality
    #[tokio::test]
    #[traced_test]
    #[cfg_attr(miri, ignore)]
    async fn test_nockapp_save() {
        // console_subscriber::init();
        let (temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
        let first_checkpoint = nockapp
            .kernel
            .checkpoint()
            .await
            .expect("Couldn't get kernel checkpoint");

        assert_eq!(nockapp.kernel.serf.event_number.load(Ordering::SeqCst), 0);
        // Save
        info!("Saving nockapp");
        save_nockapp(&mut nockapp).await;
        // Permit should be dropped

        // A valid checkpoint should exist in one of the jam files
        let (_, checkpoint_opt) = Saver::<NockJammer>::try_load(&temp.path().to_path_buf(), None)
            .await
            .expect("Could not load checkpoint");
        let checkpoint: SaveableCheckpoint = checkpoint_opt.expect("No checkpoint loaded");

        // Checkpoint event number should be 0
        assert_eq!(checkpoint.event_num, 0);

        info!("Asserting checkpoint and arvo equality");
        // Checkpoint kernel should be equal to the saved kernel
        assert!(slab_equality(&checkpoint.state, &first_checkpoint.state));
        assert!(slab_equality(&checkpoint.cold, &first_checkpoint.cold));
    }

    // Test nockapp poke
    #[tokio::test]
    #[traced_test]
    #[cfg_attr(miri, ignore)]
    async fn test_nockapp_poke_save() {
        let (temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
        assert_eq!(nockapp.kernel.serf.event_number.load(Ordering::SeqCst), 0);
        let state_before_poke = nockapp
            .kernel
            .checkpoint()
            .await
            .expect("Can't get kernel state before poke");

        let poke_noun = D(tas!(b"inc"));
        let poke = {
            let mut slab = NounSlab::new();
            slab.copy_into(poke_noun);
            slab
        };

        let wire = SystemWire.to_wire();
        let _ = nockapp.kernel.poke(wire, poke).await.unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });

        // Save
        save_nockapp(&mut nockapp).await;

        // A valid checkpoint should exist in one of the jam files
        let (_, checkpoint_opt) = Saver::<NockJammer>::try_load(&temp.path().to_path_buf(), None)
            .await
            .expect("Failed to load checkpoint");
        let checkpoint: SaveableCheckpoint = checkpoint_opt.expect("No checkpoint");

        // Checkpoint event number should be 1
        assert!(checkpoint.event_num == 1);
        let state_after_poke = nockapp
            .kernel
            .checkpoint()
            .await
            .expect("Failed to get checkpoint after poke");

        assert!(slab_equality(&checkpoint.state, &state_after_poke.state));
        assert!(slab_equality(&checkpoint.cold, &state_after_poke.cold));
        assert!(!slab_equality(&checkpoint.state, &state_before_poke.state));
    }

    #[tokio::test]
    #[traced_test]
    #[cfg_attr(miri, ignore)]
    async fn test_nockapp_save_multiple() {
        let (temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
        assert_eq!(nockapp.kernel.serf.event_number.load(Ordering::SeqCst), 0);
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);

        for i in 1..4 {
            // Poke to increment the state
            let poke_noun = D(tas!(b"inc"));
            let poke = {
                let mut slab = NounSlab::new();
                slab.copy_into(poke_noun);
                slab
            };
            let wire = SystemWire.to_wire();
            let _ = nockapp.kernel.poke(wire, poke).await.unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });

            // Save
            save_nockapp(&mut nockapp).await;

            // A valid checkpoint should exist in one of the jam files
            let (_, checkpoint_opt) =
                Saver::<NockJammer>::try_load(&temp.path().to_path_buf(), None)
                    .await
                    .expect("Failed to load checkpoint");
            let checkpoint: SaveableCheckpoint = checkpoint_opt.expect("No checkpoint found");

            // Checkpoint event number should be i
            assert!(checkpoint.event_num == i);

            // Checkpointed state should have been incremented
            let peek_noun = T(&mut stack, &[D(tas!(b"state")), D(0)]);
            let peek = {
                let mut slab = NounSlab::new();
                slab.copy_into(peek_noun);
                slab
            };

            // res should be [~ ~ [%0 val]]
            let mut res = nockapp.kernel.peek(peek).await.unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            res.modify_noun(|r| {
                slot(r, 7)
                    .unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    })
                    .as_cell()
                    .unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    })
                    .tail()
            });

            let comp = {
                let mut slab = NounSlab::new();
                slab.copy_into(D(i));
                slab
            };

            assert!(
                slab_equality(&res, &comp),
                "res: {:?} != comp: {:?}",
                res,
                comp
            );
        }
    }

    // Tests for fallback to previous checkpoint if checkpoint is corrupt
    // TODO: ask about this test and reframe it for 'Saver'
    /*
    #[tokio::test]
    #[traced_test]
    #[cfg_attr(miri, ignore)]
    async fn test_nockapp_corrupt_check() {
        let (temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
        assert_eq!(nockapp.kernel.serf.event_number.load(Ordering::SeqCst), 0);

        // Save a valid checkpoint
        save_nockapp(&mut nockapp).await;

        // Generate an invalid checkpoint by incrementing the event number
        let mut invalid = nockapp
            .kernel
            .checkpoint()
            .await
            .expect("Could not get kernel checkpoint");
        invalid.event_num += 1;
        assert!(!invalid.validate());

        // The invalid checkpoint has a higher event number than the valid checkpoint
        let mut checkpoint_stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let valid = jam_paths
            .load_checkpoint(&mut checkpoint_stack)
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        assert!(valid.event_num < invalid.event_num);

        // Save the corrupted checkpoint, because of the toggle buffer, we will write to jam file 1
        assert!(!jam_paths.1.exists());
        let jam_path = &jam_paths.1;
        let jam_bytes = invalid.encode().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        tokio::fs::write(jam_path, jam_bytes)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });

        // The loaded checkpoint will be the valid one
        let chk = jam_paths
            .load_checkpoint(&mut checkpoint_stack)
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        assert!(chk.event_num == valid.event_num);
    }
    */

    #[tokio::test]
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
        let c = slab2.cue_into(jammed_bytes).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        unsafe { assert!(slab_noun_equality(slab1.root(), &c)) }
    }

    #[tokio::test]
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
        unsafe { assert!(slab_noun_equality(state_slab.root(), &c)) }
    }

    #[tokio::test]
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
