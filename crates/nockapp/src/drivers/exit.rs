use tracing::{debug, error};

use crate::nockapp::driver::{make_driver, IODriverFn};
use crate::NounExt;

/// Creates an IO driver function for handling exit signals.
///
/// This function creates a driver that listens for exit signals and terminates
/// the process with the provided exit code when received.
///
/// # Returns
///
/// An `IODriverFn` that can be used with the NockApp to handle exit signals.
pub fn exit() -> IODriverFn {
    make_driver(|handle| async move {
        debug!("exit_driver: waiting for effect");
        loop {
            tokio::select! {
                eff = handle.next_effect() => {
                    match eff {
                        Ok(eff) => {
                            unsafe {
                                let noun = eff.root();
                                if let Ok(cell) = noun.as_cell() {
                                    if cell.head().eq_bytes(b"exit") && cell.tail().is_atom() {
                                        // Exit with the code provided in the tail
                                        if let Ok(exit_code) = cell.tail().as_atom().and_then(|atom| atom.as_u64()) {
                                            handle.exit.exit(exit_code as usize).await?;
                                        } else {
                                            // Default to error code 1 if we can't get a valid exit code
                                            handle.exit.exit(1).await?;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("Error receiving effect: {:?}", e);
                        }
                    }
                }
            }
        }
    })
}
