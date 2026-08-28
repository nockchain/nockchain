module reservation_owner_mutation {
  import bridge_withdrawal.* from "../bridge-withdrawal"

  action allowTwoReservationOwners = all {
    state.phase == PeerCanonical,
    state' = state.with(
      "reservations",
      state.reservations.set(0, state.burnId).set(1, "other-withdrawal")
    ),
  }

  action mutationStep = any { step, allowTwoReservationOwners }
}

module authorized_identity_mutation {
  import bridge_withdrawal.* from "../bridge-withdrawal"

  action replaceRawTxAfterAuthorization = all {
    state.phase.in(Set(Authorized, Submitted)),
    state' = state.with("rawTxIdentity", "replacement-raw-tx"),
  }

  action mutationStep = any { step, replaceRawTxAfterAuthorization }
}

module authorized_base_fork_mutation {
  import bridge_withdrawal.* from "../bridge-withdrawal"

  action discardAuthorizedIdentityOnBaseFork = all {
    state.phase == Authorized,
    state.authorizationRecorded,
    state' = state
      .with("canonicalBurn", false)
      .with("phase", Held)
      .with("publicState", PublicReorgHold)
      .with("hold", ShallowBaseFork)
      .with("shallowForkPending", true)
      .with("rawTxIdentity", "")
      .with("authorizedTxIdentity", "")
      .with("reservations", Inputs.mapBy(_ => "")),
  }

  action mutationStep = any { step, discardAuthorizedIdentityOnBaseFork }
}

module premature_second_withdrawal_mutation {
  import bridge_withdrawal.* from "../bridge-withdrawal"

  action reserveSecondBeforeFirstConfirmation = all {
    state.secondPhase == Pending,
    state.phase != Terminal,
    state' = state
      .with("secondPhase", PeerCanonical)
      .with("reservations", Inputs.mapBy(_ => state.secondBurnId)),
  }

  action mutationStep = any { step, reserveSecondBeforeFirstConfirmation }
}

module compensation_payout_mutation {
  import bridge_withdrawal.* from "../bridge-withdrawal"

  action payCompensatedBurn = all {
    state.phase == Compensated,
    state' = state.with("payoutCount", 1),
  }

  action mutationStep = any { step, payCompensatedBurn }
}

module premature_terminal_mutation {
  import bridge_withdrawal.* from "../bridge-withdrawal"

  action terminalWithoutKernelSettlement = all {
    state.phase == Submitted,
    state.inclusionTxIdentity == state.rawTxIdentity,
    state.inclusionBlock != "",
    state' = state
      .with("phase", Terminal)
      .with("publicState", PublicTerminal)
      .with("payoutCount", 1)
      .with("reservations", Inputs.mapBy(_ => "")),
  }

  action mutationStep = any { step, terminalWithoutKernelSettlement }
}

module skipped_deep_hold_mutation {
  import bridge_withdrawal.* from "../bridge-withdrawal"

  action mutateBeforeEnteringDeepHold = all {
    state.canonicalBurn,
    state.payoutCount == 0,
    state.hold == NoHold,
    state' = state.with("hold", DeepBaseFork),
  }

  action mutationStep = any { step, mutateBeforeEnteringDeepHold }
}
