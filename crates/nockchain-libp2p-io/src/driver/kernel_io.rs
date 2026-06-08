use either::{Either, Left, Right};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::error::{CrownError, ExternalError};
use nockapp::utils::make_tas;
use nockapp::utils::scry::ScryResult;
use nockapp::{AtomExt, NockAppError};
use nockvm::noun::{Atom, Noun, NounHandle, NounSpace, D, T};
use nockvm_macros::tas;
use tracing::{debug, trace, warn};

use crate::messages::{NockchainDataRequest, NockchainResponse};
use crate::metrics::NockchainP2PMetrics;

fn block_by_height_scry_slab(height: u64) -> Result<NounSlab, NockAppError> {
    let mut slab = NounSlab::new();
    let height_atom = Atom::new(&mut slab, height).as_noun();
    let noun = T(&mut slab, &[D(tas!(b"heavy-n")), height_atom, D(0)]);
    slab.set_root(noun);
    Ok(slab)
}

pub(crate) fn heavy_txs_scry_slab(height: u64) -> Result<NounSlab, NockAppError> {
    let mut slab = NounSlab::new();
    let height_atom = Atom::new(&mut slab, height).as_noun();
    let heavy_txs_tag = make_tas(&mut slab, "heavy-txs").as_noun();
    let noun = T(&mut slab, &[heavy_txs_tag, height_atom, D(0)]);
    slab.set_root(noun);
    Ok(slab)
}

/// Build a scry slab for the existing `[%heaviest-chain-blocks-range
/// start=@ end=@ ~]` peek arm at `open/hoon/apps/dumbnet/inner.hoon:389`.
/// Returns `(unit (unit (list [page-number block-id page (z-map tx-id tx)])))`.
/// Both endpoints are inclusive. `len = 0` is rejected by the caller; `len = 1`
/// reduces to the single-height path. Phase 3 of catch-up prefetch.
pub(crate) fn block_range_with_txs_scry_slab(
    start_height: u64,
    len: u8,
) -> Result<NounSlab, NockAppError> {
    if len == 0 {
        return Err(NockAppError::OtherError(String::from(
            "block-range scry len must be > 0",
        )));
    }
    let end_height = start_height
        .checked_add(u64::from(len) - 1)
        .ok_or_else(|| {
            NockAppError::OtherError(format!(
                "block-range scry start={start_height} len={len} overflowed u64"
            ))
        })?;
    let mut slab = NounSlab::new();
    let start_atom = Atom::new(&mut slab, start_height).as_noun();
    let end_atom = Atom::new(&mut slab, end_height).as_noun();
    let range_tag = make_tas(&mut slab, "heaviest-chain-blocks-range").as_noun();
    let noun = T(&mut slab, &[range_tag, start_atom, end_atom, D(0)]);
    slab.set_root(noun);
    Ok(slab)
}

pub(crate) fn raw_tx_by_id_scry_slab(tx_id: &str) -> Result<NounSlab, NockAppError> {
    let mut slab = NounSlab::new();
    let raw_tx_tag = make_tas(&mut slab, "raw-transaction").as_noun();
    let id_atom = Atom::from_value(&mut slab, tx_id.to_string())?;
    let noun = T(&mut slab, &[raw_tx_tag, id_atom.as_noun(), D(0)]);
    slab.set_root(noun);
    Ok(slab)
}

pub(crate) fn request_to_scry_slab(
    request: NockchainDataRequest,
) -> Result<NounSlab, NockAppError> {
    match request {
        NockchainDataRequest::BlockByHeight(height) => {
            debug!("Requesting block by height: {}", height);
            block_by_height_scry_slab(height)
        }
        NockchainDataRequest::BlockWithTxsByHeight(height) => {
            debug!("Requesting heaviest-chain tx bundle at height: {}", height);
            heavy_txs_scry_slab(height)
        }
        NockchainDataRequest::EldersById(str, _, _) => {
            debug!("Requesting elders by ID: {}", str);
            let mut slab = NounSlab::new();
            let id_atom = Atom::from_value(&mut slab, str)?;
            let noun = T(&mut slab, &[D(tas!(b"elders")), id_atom.as_noun(), D(0)]);
            slab.set_root(noun);
            Ok(slab)
        }
        NockchainDataRequest::RawTransactionById(str, _) => {
            debug!("Requesting raw transaction by ID: {}", str);
            raw_tx_by_id_scry_slab(&str)
        }
        NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            debug!(
                "Requesting heaviest-chain block range start={} len={}",
                start_height, len
            );
            block_range_with_txs_scry_slab(start_height, len)
        }
    }
}

pub(crate) fn create_scry_response(
    scry_res: &Noun,
    space: &NounSpace,
    heard_type: &str,
    res_slab: &mut NounSlab,
) -> Either<(), Result<NockchainResponse, NockAppError>> {
    match ScryResult::from_noun(scry_res, space) {
        ScryResult::BadPath => {
            warn!("Bad scry path");
            Left(())
        }
        ScryResult::Nothing => {
            trace!("Nothing found at scry path");
            Left(())
        }
        ScryResult::Some(payload) => Right(create_response_result_from_payload(
            payload, heard_type, res_slab,
        )),
        ScryResult::Invalid => Right(Err(NockAppError::OtherError(String::from(
            "Invalid scry result",
        )))),
    }
}

pub(crate) fn create_response_result_from_payload(
    payload: NounHandle<'_>,
    heard_type: &str,
    res_slab: &mut NounSlab,
) -> Result<NockchainResponse, NockAppError> {
    let payload = res_slab.copy_into(payload.noun(), payload.space());
    let response_noun = prepend_tas(res_slab, heard_type, vec![payload]).map_err(|_| {
        NockAppError::OtherError(String::from("Failed to prepend tas to response noun"))
    })?;
    res_slab.set_root(response_noun);
    Ok(NockchainResponse::new_response_result(res_slab.jam()))
}

fn prepend_tas(slab: &mut NounSlab, tas_str: &str, nouns: Vec<Noun>) -> Result<Noun, NockAppError> {
    let tas_atom = Atom::from_value(slab, tas_str)?;

    let mut cell_elements = Vec::with_capacity(nouns.len() + 1);
    cell_elements.push(tas_atom.as_noun());
    cell_elements.extend(nouns);

    Ok(T(slab, &cell_elements))
}

pub(crate) fn record_crown_error_metric(
    error: &CrownError<ExternalError>,
    metrics: &NockchainP2PMetrics,
) {
    match error {
        CrownError::External(_) => {
            metrics.requests_crown_error_external.increment();
        }
        CrownError::MutexError => {
            metrics.requests_crown_error_mutex.increment();
        }
        CrownError::InvalidKernelInput => {
            metrics
                .requests_crown_error_invalid_kernel_input
                .increment();
        }
        CrownError::UnknownEffect => {
            metrics.requests_crown_error_unknown_effect.increment();
        }
        CrownError::IOError(_) => {
            metrics.requests_crown_error_io_error.increment();
        }
        CrownError::Noun(_) => {
            metrics.requests_crown_error_noun_error.increment();
        }
        CrownError::InterpreterError(_) => {
            metrics.requests_crown_error_interpreter_error.increment();
        }
        CrownError::KernelError(_) => {
            metrics.requests_crown_error_kernel_error.increment();
        }
        CrownError::Utf8FromError(_) => {
            metrics.requests_crown_error_utf8_from_error.increment();
        }
        CrownError::Utf8Error(_) => {
            metrics.requests_crown_error_utf8_error.increment();
        }
        CrownError::NewtError | CrownError::Newt(_) => {
            metrics.requests_crown_error_newt_error.increment();
        }
        CrownError::BootError => {
            metrics.requests_crown_error_boot_error.increment();
        }
        CrownError::SerfLoadError => {
            metrics.requests_crown_error_serf_load_error.increment();
        }
        CrownError::SerfInitAllocationError(_) => {
            metrics
                .requests_crown_error_serf_init_allocation_error
                .increment();
        }
        CrownError::SerfInitPanic(_) => {
            metrics.requests_crown_error_serf_init_panic.increment();
        }
        CrownError::CheckpointKernelHashMismatch { .. } => {
            metrics.requests_crown_error_serf_load_error.increment();
        }
        CrownError::WorkBail => {
            metrics.requests_crown_error_work_bail.increment();
        }
        CrownError::PeekBail => {
            metrics.requests_crown_error_peek_bail.increment();
        }
        CrownError::WorkSwap => {
            metrics.requests_crown_error_work_swap.increment();
        }
        CrownError::TankError => {
            metrics.requests_crown_error_tank_error.increment();
        }
        CrownError::PlayBail => {
            metrics.requests_crown_error_play_bail.increment();
        }
        CrownError::QueueRecv(_) => {
            metrics.requests_crown_error_queue_recv.increment();
        }
        CrownError::SaveError(_) => {
            metrics.requests_crown_error_save_error.increment();
        }
        CrownError::IntError(_) => {
            metrics.requests_crown_error_int_error.increment();
        }
        CrownError::JoinError(_) => {
            metrics.requests_crown_error_join_error.increment();
        }
        CrownError::DecodeError(_) => {
            metrics.requests_crown_error_decode_error.increment();
        }
        CrownError::EncodeError(_) => {
            metrics.requests_crown_error_encode_error.increment();
        }
        CrownError::StateJamFormatError => {
            metrics
                .requests_crown_error_state_jam_format_error
                .increment();
        }
        CrownError::Timeout => {
            metrics.requests_crown_error_unknown.increment();
        }
        CrownError::Unknown(_) => {
            metrics.requests_crown_error_unknown.increment();
        }
        CrownError::ConversionError(_) => {
            metrics.requests_crown_error_conversion_error.increment();
        }
        CrownError::UnknownError(_) => {
            metrics.requests_crown_error_unknown_error.increment();
        }
        CrownError::QueueError(_) => {
            metrics.requests_crown_error_queue_error.increment();
        }
        CrownError::SerfMPSCError() => {
            metrics.requests_crown_error_serf_mpsc_error.increment();
        }
        CrownError::OneshotChannelError(_) => {
            metrics
                .requests_crown_error_oneshot_channel_error
                .increment();
        }
    }
}
