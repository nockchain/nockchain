use nockvm::noun::NounAllocator;
use tracing::{debug, error};

use crate::nockapp::driver::{make_driver, IODriverFn};

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
                                let exit_code = {
                                    let noun = eff.root();
                                    if let Ok(cell) = noun.as_cell() {
                                        let space = eff.noun_space();
                                        let cell = cell.in_space(&space);
                                        if cell.head().eq_bytes(b"exit")
                                            && cell.tail().is_atom()
                                        {
                                            Some(
                                                cell.tail()
                                                    .as_atom()
                                                    .and_then(|atom| atom.as_u64())
                                                    .ok(),
                                            )
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                };

                                if let Some(exit_code) = exit_code {
                                    let exit_code = exit_code.unwrap_or(1);
                                    handle.exit.exit(exit_code as usize).await?;
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
