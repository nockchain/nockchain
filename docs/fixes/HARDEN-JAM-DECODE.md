# Harden Jam Decode Regression

## Summary

This regression covers two boot-time failure modes in checkpoint and state-jam handling. The first was a corrupt or truncated jam artifact whose encoded byte length could be decoded before any validation had a chance to reject the artifact, allowing the process to panic instead of treating the artifact as a bad checkpoint and falling back to another available checkpoint. The second was a Serf initialization failure where NockStack exhaustion while restoring checkpoint state could be reported to the caller as a generic dropped initialization channel rather than as the allocation problem that actually stopped boot.

## Background

Nockchain boot can restore persisted state from checkpoint jams, and operators can also import or export state jams. Checkpoints have multiple valid historical formats, so loading must continue to support V0, V1, and V2 checkpoint artifacts while rejecting corrupt variants deterministically. V2 checkpoints add an envelope around a payload, while older checkpoint formats encode the jammed noun directly. Exported state jams use a separate magic and version but have the same operational risk: they contain a jam byte field whose encoded length must never be trusted enough to allocate blindly.

The original checkpoint decode path delegated directly to owned bincode decoding for structures that contain `Bytes`. In the corrupt-length case, bincode attempted to allocate a buffer from the declared length before the checkpoint checksum, version, magic, or fallback behavior could run. This meant a single bad checkpoint file could abort the load path even when another valid checkpoint was present and could have been used.

Serf initialization had a separate error attribution problem. NockStack allocation failures are currently represented as panics with typed payloads inside the VM stack allocator. If such a panic occurred while copying or preserving checkpoint state during Serf thread startup, the Serf thread could exit before sending its initialization result, and the boot caller would observe a generic oneshot channel failure. That obscured the actionable fix for an operator, especially in cases where increasing `--stack-size` could resolve the boot failure.

## Fix

Checkpoint and exported state-jam decode now go through a small checked artifact reader before constructing owned values. The reader parses the bincode-standard primitive layout used by these artifacts, including varint lengths, fixed hashes, booleans, and byte slices. It verifies that declared byte lengths fit in memory address space and in the available input before copying bytes into owned `Bytes`, so malformed lengths become normal decode errors instead of allocation panics.

The checked decoder is used for V0, V1, and V2 checkpoint loading, including the V2 envelope and payload. Valid checkpoint variants still round-trip through the existing encoded format, and checksum validation still remains the integrity check for decoded checkpoint contents. The decoder is also used for exported state jams, with state-jam-specific magic, version, and recovery guidance.

Operator-facing messages now distinguish corrupt artifacts, unsupported versions, invalid magic, and checksum failures. The messages avoid exposing internal stack traces and instead tell operators whether to restore a checkpoint or state jam from a known-good peer, remove a bad checkpoint so another one can be tried, re-export a state jam, or use a compatible binary for the artifact version.

Serf initialization now wraps the phases that can trigger NockStack allocation panics during startup. The wrapper is intentionally narrow and only covers initialization phases such as stack allocation, copying checkpoint state, copying cold state, booting the kernel, loading checkpoint state through the kernel, and preserving the loaded state. If the panic payload is a NockStack allocation error, boot receives a direct Serf init allocation error with phase, configured stack size, checkpoint event number, checkpoint kernel hash, current kernel hash, and guidance to retry with a larger `--stack-size`. Unknown Serf init panics are still reported directly as Serf init panics with guidance to preserve the artifact and rerun with backtraces.

## Regression Coverage

- `corrupt_checkpoint_length_does_not_panic_and_falls_back_to_previous_checkpoint` covers a bad checkpoint with an absurd jam length alongside a valid older checkpoint. The expected behavior is no panic and successful fallback to the valid checkpoint.
- `serf_thread_init_stack_oom_is_not_collapsed_into_oneshot_error` covers checkpoint restore with insufficient NockStack space. The expected behavior is a direct stack allocation error that includes operator guidance, not a generic channel error.
- Additional focused coverage checks valid V2 checkpoint round-trip, corrupt V2 envelope payload length rejection, valid exported state-jam round-trip, and corrupt exported state-jam length rejection.

## Operational Expectations

If an operator sees a malformed checkpoint or state-jam error, the artifact should be treated as corrupt or incomplete. For checkpoints, the operator can remove the bad checkpoint and allow the peer to use another local checkpoint if one exists, or restore checkpoints from a known-good synced peer. For exported state jams, the operator should re-export from a known-good node or bootstrap from a valid checkpoint instead.

If an operator sees a Serf init allocation error, the first configuration fix is to retry with a larger `--stack-size`, typically `large` or `huge`. If the peer still cannot boot with the largest supported stack size, the operator should preserve the failing artifact for debugging and restore a checkpoint or state jam from a synced peer. The error includes checkpoint and kernel hash context so the operator can distinguish stack sizing from artifact or binary compatibility issues without needing raw panic output.

## Notes

This hardening does not add redundant validation of the noun produced by cueing a jam. The fix is deliberately at the artifact boundary: it prevents corrupt container metadata from triggering unsafe allocation behavior before existing checksum, version, magic, and cueing logic can run. PMA and future import/export paths should reuse the same checked artifact reader or an equivalent shared decoder rather than reintroducing direct owned bincode decoding for untrusted jam-bearing artifacts.
