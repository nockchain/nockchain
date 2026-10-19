# `wallet-tx-builder`

`wallet-tx-builder` is the deterministic transaction-planning library used by the Nockchain wallet. It selects spendable notes, resolves locks, estimates word-count fees, and constructs recipient/refund plans for ordinary sends, bridge withdrawals, and legacy-v0 migration.

It does not hold keys, produce signatures, broadcast transactions, or decide whether the chain accepts a transaction.

## Place in the system

```text
wallet state + requested outputs + chain context
                         |
                wallet-tx-builder
                         |
 selected inputs + fee + outputs + signing requirements
                         |
          nockchain-wallet signs and submits
                         |
              node/kernel validates
```

## Maintained invariants

- Candidate ordering and selection are deterministic for identical inputs and `SelectionOrder`.
- Only notes whose version, lock, signer scope, and timelock satisfy the request are admitted.
- Historical protocol-fund recovery requires explicit authoritative identity and `FundMaturity` metadata through `LockRootLockMatcher::with_fund_notes`. Only the matching fund root can recover that first-name; direct notes to the same root remain ordinary notes.
- Fund maturity is evaluated at the supplied candidate height with checked `origin + minimum` arithmetic. An inactive minimum adds no gate; an active zero minimum still excludes future-origin notes. The historical first-name is independent of the configured minimum.
- Fund recovery reveals the unchanged bare multisig, without adding a timelock or changing its serialization, witness sizing, or fee. Ordinary timelocks and fees continue to use `ChainContext.height` (the snapshot tip), not the fund candidate height.
- Value is conserved: selected input value equals gifts, bridge amounts where applicable, fee, and refund. Checked arithmetic rejects overflow and underfunding.
- Requested gift amounts are never silently increased to consume change; positive remainder becomes an explicit refund.
- Fee calculation uses the chain's word-count model and is recomputed as selected inputs and witnesses change.
- Manual legacy-v0 and v1 input sets do not silently mix incompatible signing semantics.
- Bridge withdrawals account separately for the burned amount, bridge fee, recipient disbursement, transaction fee, and refund.
- Planner output remains unsigned. The wallet must resolve the indicated keys and the node must revalidate the final noun.

## Trust and consensus dependencies

The caller supplies chain height, constants, candidate notes, and ownership information. Stale chain state can produce a transaction that is no longer spendable, but deterministic planning must not turn stale data into key misuse or value creation.

For multisig fund recovery, the caller must obtain the fund identity and effective maturity policy for the explicit snapshot tip plus one, rejecting missing, malformed, or height-mismatched policy responses. Recompute that policy for each snapshot, including backward height movement; do not cache activation as a high-water mark.

Correctness relies on `nockchain-types` matching Hoon transaction molds, exact fee/word-count parity with consensus, correct lock and timelock interpretation, and checked integer arithmetic. Signature and cryptographic proof soundness are outside this crate.

## Validation

```sh
cargo test -p wallet-tx-builder
cargo check -p wallet-tx-builder
```
