use std::time::Duration;

use nockvm::noun::NounAllocator;
use tracing::{debug, error};

use crate::nockapp::driver::{make_driver, IODriverFn};
use crate::nockapp::outbound;

/// Effect head whose driver performs a network round-trip that shutdown must
/// not cut short. Registered here rather than by the performing driver because
/// only this driver's ordering relative to `%exit` is guaranteed.
const OUTBOUND_EFFECT: &[u8] = b"nockchain-grpc";

/// Bounds the wait when a registered effect has no driver attached to resolve
/// it -- the exit still happens, loudly.
const OUTBOUND_DRAIN_TIMEOUT: Duration = Duration::from_secs(120);

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
                                        // Effects arrive in emission order, so an outbound-work
                                        // effect emitted before %exit is always registered before
                                        // the exit below waits on it.
                                        if cell.head().eq_bytes(OUTBOUND_EFFECT) {
                                            outbound::register();
                                            None
                                        } else if cell.head().eq_bytes(b"exit")
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
                                    if !outbound::drain(OUTBOUND_DRAIN_TIMEOUT).await {
                                        error!(
                                            "exit_driver: outbound work unresolved after {:?}; \
                                             exiting anyway",
                                            OUTBOUND_DRAIN_TIMEOUT
                                        );
                                    }
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
