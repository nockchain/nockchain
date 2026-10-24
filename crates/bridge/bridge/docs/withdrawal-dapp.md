# Withdrawal DApp Spec

Status: Draft

Source of truth:
- `open/crates/bridge/bridge/docs/bridge-withdrawals.md`

## Why This Doc Exists

`bridge-withdrawals.md` is the canonical protocol and systems spec. It is
written for people implementing the bridge kernel, runtime, sequencer, and
storage layers.

This document is the DApp/UI translation of that spec. It is meant to answer:

1. What is the withdrawal DApp actually doing for the user?
2. What backend states matter to the UI?
3. What should the UI show, and what should it never pretend to control?
4. What assumptions should you make before wiring screens and
   API calls?

## One-Sentence Product Definition

The withdrawal DApp lets a user burn wrapped NOCK on Base to request a
withdrawal to Nockchain, then shows a small, honest set of public statuses
until that withdrawal is confirmed on Nockchain.

## Core Mental Model

The most important thing to understand is:

1. The DApp does not build, authorize, or submit withdrawal transactions by
   itself.
2. The user starts a withdrawal by burning wrapped NOCK on Base.
3. That burn creates a withdrawal inside the bridge system if and only if the
   amount is strictly above the configured minimum.
4. From that point on, the bridge backend owns the process.
5. The UI is mainly responsible for:
   - collecting the withdrawal destination and amount
   - helping the user send the Base burn transaction
   - displaying accurate coarse-grained withdrawal state
   - surfacing delays or support-worthy problems honestly

For the frontend, a withdrawal is a single object keyed by:
- `withdrawal_id = (as_of, base_event_id)`

Where:

1. `as_of` is the Base block hash the bridge uses as the counterpart reference
   point for the withdrawal
2. `base_event_id` is the unique identifier of the burn event within that
   Base-side history

This is mostly a backend/internal identifier. You may not want to show it
directly in the public UI, but the current product direction is to show both
the internal `withdrawal_id` and the more human-friendly references like the
Base burn tx hash and timestamps.

Do not think of the UI as operating on "attempts" or "tx proposals." Those
exist in backend coordination, but the user-facing object is the withdrawal.

## What Counts As A Withdrawal

A Base burn becomes a withdrawal only if:

1. the burn event is observed by the bridge
2. the kernel admits it into withdrawal state
3. the burned amount is strictly above the withdrawal minimum

Important:

1. Burns at or below the minimum should not be represented in the UI as active
   withdrawals.
2. The current target minimum in the canonical spec is `10,000 NOCKS`.
3. For now, the DApp should hardcode that minimum rather than fetch it from a
   backend API.
4. Do not assume the amount burned on Base is the same as the amount received
   on Nockchain. The withdrawal fee is deducted, so the received amount is
   lower than the burned amount.

## What The User Does

At a high level:

1. User connects Base wallet.
2. User enters:
   - withdrawal amount
   - Nockchain destination lock root
3. DApp validates the request client-side as much as it can.
4. User signs and submits the Base burn transaction.
5. DApp tracks the burn transaction on Base.
6. After the burn is confirmed enough on Base, the DApp transitions to a coarse
   "withdrawal pending" state.
7. Keep the DApp in that pending state until the withdrawal is confirmed on
   Nockchain or is delayed long enough that you should point the user to
   support/status guidance.

## What The UI Is Not Responsible For

The DApp must not pretend to own these steps:

1. proposal construction
2. peer canonicalization
3. sequencer authorization
4. withdrawal tx submission to Nockchain
5. withdrawal confirmation reconciliation
6. note reservation logic
7. replay/equivocation detection

Those belong to the bridge runtime, sequencer gRPC service, and kernel.

Do not expose those internal stages in the public DApp unless there is a real,
reliable product requirement for them.

## Public User-Facing Lifecycle

The public withdrawal DApp should use a very small number of states. The user
probably will not have reliable visibility into bridge-internal progress
between the Base burn and the final Nockchain confirmation.

Recommended user-facing stages:

1. `draft`
   - user is filling out the form
2. `awaiting_wallet_confirmation`
   - wallet popup is open for the Base burn tx
3. `awaiting_base_confirmation`
   - Base burn transaction has been sent but is not yet confirmed enough from
     the user’s point of view
4. `withdrawal_pending`
   - the burn is done and the user is now waiting for the bridge withdrawal to
     complete on Nockchain
5. `confirmed`
   - confirmed settlement observed and reconciled
6. `delayed`
   - the withdrawal is taking longer than expected and you should point the
     user to support or system status guidance

The public UI should not expose backend-internal states like:

1. proposal built
2. peer-canonical
3. sequencer authorized
4. sequencer submitted
5. hold
6. stop

Those may exist in admin/operator tooling, but keep the public DApp coarse
unless the backend exposes a deliberate user-safe product surface for them.

## Internal Backend States

The canonical spec implies these important backend facts:

1. A withdrawal is either live/unconfirmed or confirmed at the kernel level.
2. The sequencer gRPC service owns:
   - the authoritative authorized withdrawal record
   - the submitted / in-flight withdrawal record
   - the durable confirmation record for authorized withdrawals
3. Only one withdrawal may be sequencer-authorized / submitted / unconfirmed
   at a time.
4. If the sequencer is unavailable, withdrawals pause.
5. Unknown counterpart chain state can produce a hold.
6. Irreconcilable mismatch can produce a stop.

These are useful for backend and operator tooling, but not necessarily for the
public DApp.

If an internal/admin view exists, it may show:

1. sequencing
2. authorized
3. submitted
4. held
5. stopped
6. sequencer unavailable

## Recommended DApp Screens

### 1. Withdrawal Form

Purpose:
- create a new withdrawal request by helping the user submit the Base burn tx

Fields:

1. amount
2. destination
3. wallet/network status

Behavior:

1. Validate amount is present and positive.
2. Treat the destination as a lock root, not as a generic address field.
3. Provide a helper that can derive the simple-PKH lock root from a v1 address
   for the common case.
4. Still allow direct lock-root entry, since lock root is the canonical input.
5. Validate destination shape before wallet submission.
6. Warn if amount is below or near the hardcoded minimum.
7. Show the exact Base network the transaction will be submitted to.
8. Make it clear that the user is burning wrapped NOCK on Base to receive
   NOCK on Nockchain.
9. Make it clear that the user receives the post-fee amount on Nockchain, not
   the full burned amount.

### 2. Burn Submission Status

Purpose:
- track the user’s Base burn transaction before bridge-side processing starts

Show:

1. wallet-submitted tx hash
2. pending / confirmed status on Base
3. failure/revert if the burn tx fails

Important:

1. A successful wallet submission is not the same thing as a successful
   withdrawal.
2. After Base confirmation, the DApp can move into a coarse
   `withdrawal_pending` state even if it does not have bridge-internal
   visibility.

### 3. Withdrawal Status View

Purpose:
- show the backend-owned lifecycle once the burn has become a bridge withdrawal

Show:

1. withdrawal id
2. burned amount
3. expected received amount if the backend exposes it
4. destination
5. Base burn tx reference
6. current stage
7. timestamps for major stage changes when available
8. delay/support guidance if the withdrawal is taking longer than expected
9. human-friendly references like burn tx hash and timestamps alongside the
   internal `withdrawal_id`

### 4. Withdrawal History

Purpose:
- let the user see prior withdrawals and their final status

Show:

1. recent withdrawals
2. withdrawal id
3. burned amount
4. received amount if available
5. destination
6. created time
7. confirmed time if complete
8. current/final state

## UX Rules

### Rule 1: Never imply that backend-internal progress is public truth

Do not show:
- "confirmed" because peers agreed
- "complete" because a proposal was built
- "done" because the sequencer submitted a tx
- "authorized" or "submitted" unless the product deliberately exposes internal
  operator-level states

Show confirmed only after confirmed settlement has been observed and reconciled.

### Rule 2: Separate Base burn progress from Nockchain withdrawal progress

The user performs one Base transaction, but the actual withdrawal completes
later on the bridge/Nockchain side.

The UI should visually separate:

1. Base burn submitted/confirmed
2. waiting for bridge completion
3. Nockchain confirmed settlement

### Rule 3: Public UI should prefer delay messaging over internal hold/stop jargon

The words `hold` and `stop` are useful for operators, but may not be good
public-user concepts.

For most users, the better public states are:

1. pending
2. delayed
3. confirmed

### Rule 4: Stop is a real operator-visible problem

If the product exposes an actual user-visible terminal problem:

1. show the stop reason clearly
2. say bridge processing halted for this withdrawal
3. point the user to support/operator guidance if available

### Rule 5: Sequencer pause is usually internal/backend state

Because there is one sequencer-owned in-flight withdrawal at a time, the UI
may need internal/operator visibility for things like:

1. waiting for sequencer
2. sequencer unavailable
3. withdrawal paused pending sequencer recovery

But you should usually translate this into a coarse pending/delayed state
rather than expose sequencer internals directly.

### Rule 6: Delayed withdrawals need explicit support guidance

For public users:

1. say that a small delay is expected
2. if no withdrawal has shown up after 24 hours, direct the user to reach out
   on Telegram
3. do not imply the user should retry the burn or create a duplicate
   withdrawal request on their own

## Suggested Data Model For The Frontend

This is not the canonical backend schema. It is a frontend view model.

```ts
type WithdrawalUiStatus =
  | "draft"
  | "awaiting_wallet_confirmation"
  | "awaiting_base_confirmation"
  | "withdrawal_pending"
  | "confirmed"
  | "delayed";

type WithdrawalUiRecord = {
  withdrawalId: string;
  baseTxHash?: string;
  amount: string;
  destination: string;
  createdAt?: string;
  updatedAt?: string;
  status: WithdrawalUiStatus;
  nockTxName?: string;
  nockConfirmationRef?: string;
};
```

The backend may expose more detail, but you should normalize it into a model
like this.

## API Expectations

This doc does not define the final backend API, but the DApp probably needs
something close to:

1. `create burn transaction` or contract-write integration
2. `get withdrawal by id`
3. `list withdrawals for account`
4. `stream or poll withdrawal status updates`
5. `get bridge/sequencer status` for internal/admin tools if needed

Minimum useful payload for a withdrawal status API:

1. `withdrawal_id`
2. burned amount
3. expected or actual received amount if available
4. destination
5. base burn tx hash
6. current user-facing status
7. optional machine state
8. optional delay/support hint
9. timestamps
10. optional Nockchain tx name/reference

One unresolved product/API question is where public Nockchain-side visibility
should come from. The current expectation is that another operator may already
have an API for viewing incoming Nockchain transactions that the DApp can lean
on, because the team is reluctant to expose bridge-operated services publicly.

## Internal/Admin Sequencer UI Needs

Because the sequencer is now a separate gRPC service in the canonical design,
an internal/admin surface should assume backend status may come from more than
one subsystem:

1. bridge runtime
2. kernel-derived withdrawal state
3. sequencer service status

At minimum, an internal/admin surface should be able to show:

1. whether the sequencer is healthy
2. whether the withdrawal is waiting on sequencer authorization
3. whether the withdrawal was submitted by the sequencer
4. whether the sequencer has recorded confirmation for the withdrawal

The DApp does not need direct gRPC access. It just needs a backend/API layer
that exposes sequencer status in a frontend-safe way.

## Recommended Engineering Order For The DApp

If you are getting started, this is the right order:

1. Build a static withdrawal form and status screen.
2. Add wallet/network connection state.
3. Add Base burn submission flow.
4. Add polling/streaming for backend withdrawal status.
5. Implement the UI state machine for:
   - awaiting base confirmation
   - withdrawal pending
   - confirmed
   - delayed
6. Add withdrawal history.
7. If needed, add a separate internal/operator status view.
8. Add better delay/support messaging and empty/error states.

## Non-Goals For The First UI Iteration

Do not block initial UI work on:

1. exposing every backend attempt/epoch detail
2. displaying full proposal-envelope internals
3. exposing raw note-selection or reservation data to the user
4. implementing recovery controls in the UI
5. multi-withdrawal batch UX

The first useful UI should focus on:

1. initiating withdrawals
2. showing accurate status
3. explaining delays honestly

## Remaining Open Question

This still needs a backend/product decision:

1. What exact status API shape will expose coarse public states vs internal
   operator states?
   - There may already be an operator-owned API for viewing incoming
     Nockchain transactions to be able to flag withdrawals as complete that the DApp can reuse.
   - The bridge team is reluctant to expose bridge-operated services publicly,
     so this likely should not depend on making the bridge or sequencer
     directly public.

## Bottom Line

If you remember only a few things, remember these:

1. The user starts a withdrawal by burning on Base.
2. The burn only becomes a withdrawal if it clears the withdrawal minimum and
   is admitted by the bridge.
3. The UI is not the sequencer and not the bridge runtime.
4. The public UI should only show coarse states it can actually know.
5. Burned amount and received amount are not the same thing; the fee is
   deducted before the user receives NOCK on Nockchain.
6. Confirmed means confirmed settlement on Nockchain, not just "submitted" or
   "peer agreed."
7. The destination should be a lock root, with a helper for deriving the
   simple-PKH lock root from a v1 address.
8. Show both the internal `withdrawal_id` and the human-friendly references.
9. A small delay is expected; after 24 hours without a withdrawal showing up,
   tell the user to reach out on Telegram.
