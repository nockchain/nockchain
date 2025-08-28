use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use nockapp::driver::{make_driver, IODriverFn, NockAppHandle};
use nockapp::noun::slab::NounSlab;
use nockapp::wire::{WireRepr, WireTag as AppWireTag};
use nockapp::{Bytes, NockAppError, Noun};
use nockvm::noun::{D, T};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use tracing::{error, info};

use crate::client::NockAppGrpcClient;
use crate::wire_conversion::create_grpc_wire;
use crate::NockAppGrpcServer;

/// Create a gRPC server driver for NockApp
///
/// This function returns an IODriverFn that can be added to a NockApp instance
/// to start a gRPC server. Do NOT expose the server to an untrusted network, as
/// it is intended for local or controlled environments. `grpc_server_driver`
/// binds to `localhost:5555`. This driver is a core/admin gRPC driver for NockApp,
/// it includes operations like `Peek` and `Poke` so you should use an ssh tunnel
/// or VPN w/ firewalling to access it securely on a remote server. This is also
/// intended to be a demonstration of how to extend NockApp with custom I/O drivers.
///
/// # Example
/// ```rust,ignore
/// use nockapp_grpc::driver::grpc_server_driver;
/// // in an async context with a NockApp instance:
/// // app.add_io_driver(grpc_server_driver()).await;
/// ```
pub fn grpc_server_driver() -> IODriverFn {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5555);
    make_driver(move |handle: NockAppHandle| async move {
        info!("Starting gRPC server on {}", addr);

        let server = NockAppGrpcServer::new(handle);

        match server.serve(addr).await {
            Ok(_) => {
                info!("gRPC server shutting down gracefully");
                Ok(())
            }
            Err(e) => {
                error!("gRPC server error: {}", e);
                Err(nockapp::NockAppError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("gRPC server failed: {}", e),
                )))
            }
        }
    })
}

pub enum GrpcEffect {
    Peek {
        pid: u64,
        typ: String,
        path: Vec<String>,
    },

    Poke {
        pid: u64,
        payload: Vec<u8>,
    },
}

impl NounDecode for GrpcEffect {
    fn from_noun(effect: &Noun) -> Result<Self, NounDecodeError> {
        let Ok(effect_cell) = effect.as_cell() else {
            return Err(NounDecodeError::ExpectedCell);
        };
        if unsafe { effect_cell.head().raw_equals(&D(tas!(b"grpc"))) } {
            let effect_payload = effect_cell.tail().as_cell()?;

            match effect_payload.head().as_direct() {
                // [%grpc %poke pid payload]
                Ok(tag) if tag.data() == tas!(b"poke") => {
                    let eff = effect_payload.tail().as_cell()?;
                    let pid = u64::from_noun(&eff.head())?;

                    let mut slab: NounSlab = NounSlab::new();
                    slab.copy_into(eff.tail());
                    let payload = slab.jam().to_vec();
                    Ok(GrpcEffect::Poke { pid, payload })
                }
                // [%grpc %peek pid [%type path]]
                Ok(tag) if tag.data() == tas!(b"peek") => {
                    let peek_tail = effect_payload.tail().as_cell()?;
                    let pid: u64 = <u64>::from_noun(&peek_tail.head())?;

                    let meta = peek_tail.tail().as_cell()?; // [%type path]
                    let typ = String::from_noun(&meta.head())?;

                    let path_vec: Vec<String> = <Vec<String>>::from_noun(&meta.tail())?;
                    Ok(GrpcEffect::Peek {
                        pid,
                        typ,
                        path: path_vec,
                    })
                }
                _ => Err(NounDecodeError::InvalidTag),
            }
        } else {
            Err(NounDecodeError::InvalidTag)
        }
    }
}

pub fn grpc_listener_driver(addr: String) -> IODriverFn {
    make_driver(move |handle: NockAppHandle| async move {
        let mut client = NockAppGrpcClient::connect(addr.to_string())
            .await
            .map_err(|e| {
                NockAppError::OtherError(format!("gRPC client failed to connect: {}", e))
            })?;

        loop {
            match handle.next_effect().await {
                Ok(effect) => {
                    let effect_noun = unsafe { effect.root() };
                    let grpc_effect = GrpcEffect::from_noun(&effect_noun).map_err(|err| {
                        NockAppError::OtherError(format!(
                            "Failed to decode gRPC effect noun: {}",
                            err
                        ))
                    });
                    let grpc_effect = match grpc_effect {
                        Ok(effect) => effect,
                        Err(_) => continue,
                    };
                    match grpc_effect {
                        GrpcEffect::Poke { pid, payload } => {
                            let grpc_wire = create_grpc_wire();
                            let response = client
                                .poke(pid as i32, grpc_wire, payload)
                                .await
                                .map_err(|err| NockAppError::OtherError(err.to_string()))?;
                            if !response {
                                info!("Grpc poke not acked");
                            }
                        }
                        GrpcEffect::Peek { pid, typ, path } => {
                            let jam_bytes = client
                                .peek(pid as i32, path)
                                .await
                                .map_err(|_err| NockAppError::PeekFailed)?;
                            //  [%grpc-bind result=*]
                            //  on wire /grpc/1/pid/typ
                            let mut payload_slab: NounSlab = NounSlab::new();
                            let res_noun = payload_slab.cue_into(Bytes::from(jam_bytes))?;
                            let tag_noun = "grpc-bind".to_string().to_noun(&mut payload_slab);
                            let cause = T(&mut payload_slab, &[tag_noun, res_noun]);
                            payload_slab.set_root(cause);

                            let grpc_wire = WireRepr::new(
                                "grpc",
                                1,
                                vec![AppWireTag::Direct(pid), AppWireTag::String(typ.clone())],
                            );
                            let _ = handle.poke(grpc_wire, payload_slab).await?;
                        }
                    }
                }
                Err(_) => continue,
            }
        }
    })
}
