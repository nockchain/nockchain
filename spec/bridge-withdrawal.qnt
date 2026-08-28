module bridge_withdrawal {
  type Phase =
    | Absent
    | Pending
    | Assembling
    | Prepared
    | PeerCanonical
    | Authorized
    | Submitted
    | SequencerConfirmed
    | Terminal
    | Held
    | Compensated

  type PublicState =
    | PublicNone
    | PublicPending
    | PublicReady
    | PublicSubmitted
    | PublicSequencerConfirmed
    | PublicTerminal
    | PublicReorgHold

  type HoldKind =
    | NoHold
    | ShallowBaseFork
    | ShallowNockFork
    | DeepBaseFork
    | DeepNockFork
    | SimultaneousForks

  type WithdrawalState = {
    canonicalBurn: bool,
    burnId: str,
    phase: Phase,
    publicState: PublicState,
    proposalEpoch: int,
    proposalHash: str,
    rawTxIdentity: str,
    authorizedTxIdentity: str,
    authorizationRecorded: bool,
    secondBurnId: str,
    secondPhase: Phase,
    inclusionTxIdentity: str,
    inclusionHeight: int,
    inclusionBlock: str,
    settledNodes: Set[int],
    reservations: int -> str,
    payoutCount: int,
    compensationCount: int,
    journalGeneration: int,
    replayRequired: bool,
    hold: HoldKind,
    shallowForkPending: bool,
    baseAvailable: bool,
    nockAvailable: bool,
    quorumAvailable: bool,
    liquidityAvailable: bool,
    grossBurn: int,
    payoutAmount: int,
    bridgeFee: int,
    nockFee: int,
    selectedInputTotal: int,
    changeAmount: int,
  }

  pure val Nodes = Set(0, 1, 2)
  pure val Inputs = Set(0, 1)
  pure val MaxEpoch = 2
  pure val GrossBurn = 100
  pure val PayoutAmount = 95
  pure val BridgeFee = 3
  pure val NockFee = 2
  pure val SelectedInputTotal = 120
  pure val ChangeAmount = 23

  var state: WithdrawalState

  action init = state' = {
    canonicalBurn: false,
    burnId: "",
    phase: Absent,
    publicState: PublicNone,
    proposalEpoch: 0,
    proposalHash: "",
    rawTxIdentity: "",
    authorizedTxIdentity: "",
    authorizationRecorded: false,
    secondBurnId: "",
    secondPhase: Absent,
    inclusionTxIdentity: "",
    inclusionHeight: 0,
    inclusionBlock: "",
    settledNodes: Set(),
    reservations: Inputs.mapBy(_ => ""),
    payoutCount: 0,
    compensationCount: 0,
    journalGeneration: 0,
    replayRequired: false,
    hold: NoHold,
    shallowForkPending: false,
    baseAvailable: true,
    nockAvailable: true,
    quorumAvailable: true,
    liquidityAvailable: true,
    grossBurn: 0,
    payoutAmount: 0,
    bridgeFee: 0,
    nockFee: 0,
    selectedInputTotal: 0,
    changeAmount: 0,
  }

  action observeCanonicalBurn(burnId: str): bool = all {
    state.phase == Absent,
    burnId != "",
    state' = state
      .with("canonicalBurn", true)
      .with("burnId", burnId)
      .with("phase", Pending)
      .with("publicState", PublicPending)
      .with("grossBurn", GrossBurn)
      .with("payoutAmount", PayoutAmount)
      .with("bridgeFee", BridgeFee)
      .with("nockFee", NockFee)
      .with("selectedInputTotal", SelectedInputTotal)
      .with("changeAmount", ChangeAmount),
  }

  action observeDuplicateBurn(burnId: str): bool = all {
    state.canonicalBurn,
    state.burnId == burnId,
    state' = state,
  }
  action observeSecondCanonicalBurn(burnId: str): bool = all {
    state.phase != Absent,
    state.secondPhase == Absent,
    burnId != "",
    burnId != state.burnId,
    state' = state
      .with("secondBurnId", burnId)
      .with("secondPhase", Pending),
  }

  action reserveSecondWithdrawal = all {
    state.secondPhase == Pending,
    state.phase == Terminal,
    noReservations,
    state' = state
      .with("secondPhase", PeerCanonical)
      .with("reservations", Inputs.mapBy(_ => state.secondBurnId)),
  }


  action assemble(epoch: int, proposalHash: str, rawTxIdentity: str): bool = all {
    state.phase == Pending,
    state.canonicalBurn,
    state.quorumAvailable,
    state.liquidityAvailable,
    epoch > state.proposalEpoch,
    epoch <= MaxEpoch,
    proposalHash != "",
    rawTxIdentity != "",
    state' = state
      .with("phase", Assembling)
      .with("proposalEpoch", epoch)
      .with("proposalHash", proposalHash)
      .with("rawTxIdentity", rawTxIdentity),
  }

  action prepare = all {
    state.phase == Assembling,
    state' = state.with("phase", Prepared),
  }

  action replacePrepared(epoch: int, proposalHash: str, rawTxIdentity: str): bool = all {
    state.phase.in(Set(Assembling, Prepared)),
    epoch > state.proposalEpoch,
    epoch <= MaxEpoch,
    proposalHash != "",
    rawTxIdentity != "",
    state' = state
      .with("phase", Assembling)
      .with("proposalEpoch", epoch)
      .with("proposalHash", proposalHash)
      .with("rawTxIdentity", rawTxIdentity),
  }

  action canonicalize = all {
    state.phase == Prepared,
    state.quorumAvailable,
    state' = state
      .with("phase", PeerCanonical)
      .with("publicState", PublicReady)
      .with("reservations", Inputs.mapBy(_ => state.burnId)),
  }

  action authorize = all {
    state.phase == PeerCanonical,
    not(state.replayRequired),
    state' = state
      .with("phase", Authorized)
      .with("authorizedTxIdentity", state.rawTxIdentity)
      .with("authorizationRecorded", true),

  }

  action restartSequencer = all {
    state.phase.in(Set(PeerCanonical, Authorized, Submitted)),
    state.journalGeneration < 2,
    state' = state
      .with("journalGeneration", state.journalGeneration + 1)
      .with("replayRequired", true)
      .with("reservations", Inputs.mapBy(_ => ""))
      .with(
        "publicState",
        if (state.hold == NoHold) PublicPending else PublicReorgHold
      ),
  }

  action restoreReservations = all {
    state.replayRequired,
    state.phase.in(Set(PeerCanonical, Authorized, Submitted)),
    state' = state.with("reservations", Inputs.mapBy(_ => state.burnId)),
  }

  action replayJournal(generation: int): bool = all {
    state.replayRequired,
    generation == state.journalGeneration,
    allReserved,
    state' = state
      .with("replayRequired", false)
      .with(
        "publicState",
        if (state.hold != NoHold) {
          PublicReorgHold
        } else if (state.phase == Submitted) {
          PublicSubmitted
        } else {
          PublicReady
        }
      ),
  }

  action submit = all {
    state.phase == Authorized,
    not(state.replayRequired),
    allReserved,
    state.authorizedTxIdentity == state.rawTxIdentity,
    state' = state
      .with("phase", Submitted)
      .with("publicState", PublicSubmitted),
  }

  action include(height: int, blockId: str): bool = all {
    state.phase == Submitted,
    not(state.replayRequired),
    state.nockAvailable,
    height > 0,
    blockId != "",
    state' = state
      .with("inclusionTxIdentity", state.rawTxIdentity)
      .with("inclusionHeight", height)
      .with("inclusionBlock", blockId),
  }

  action reinclusion(height: int, blockId: str): bool = all {
    state.phase == Submitted,
    not(state.replayRequired),
    state.shallowForkPending,
    state.hold == ShallowNockFork,
    height > state.inclusionHeight,
    blockId != "",
    state' = state
      .with("inclusionTxIdentity", state.rawTxIdentity)
      .with("inclusionHeight", height)
      .with("inclusionBlock", blockId)
      .with("shallowForkPending", false)
      .with("hold", NoHold)
      .with("publicState", PublicSubmitted),
  }

  action confirm = all {
    state.phase == Submitted,
    not(state.replayRequired),
    not(state.shallowForkPending),
    state.inclusionTxIdentity == state.rawTxIdentity,
    state.inclusionBlock != "",
    state' = state
      .with("phase", SequencerConfirmed)
      .with("publicState", PublicSequencerConfirmed),
  }

  action settleKernel(node: int): bool = all {
    state.phase == SequencerConfirmed,
    Nodes.contains(node),
    not(state.settledNodes.contains(node)),
    state' = state.with("settledNodes", state.settledNodes.union(Set(node))),
  }

  action recordPayout = all {
    state.phase == SequencerConfirmed,
    allNodesSettled,
    conservation,
    state.payoutCount == 0,
    state.compensationCount == 0,
    state' = state.with("payoutCount", 1),
  }

  action releaseReservations = all {
    state.phase == SequencerConfirmed,
    state.payoutCount == 1,
    allReserved,
    state' = state.with("reservations", Inputs.mapBy(_ => "")),
  }

  action publishTerminal = all {
    state.phase == SequencerConfirmed,
    state.canonicalBurn,
    state.payoutCount == 1,
    noReservations,
    allNodesSettled,
    exactTxIdentity,
    conservation,
    state' = state
      .with("phase", Terminal)
      .with("publicState", PublicTerminal),
  }

  action shallowBaseFork = all {
    state.canonicalBurn,
    state.payoutCount == 0,
    not(state.phase.in(Set(Terminal, Compensated))),
    state' = if (state.authorizationRecorded) {
      state
        .with("phase", Held)
        .with("publicState", PublicReorgHold)
        .with("hold", ShallowBaseFork)
        .with("reservations", Inputs.mapBy(_ => state.burnId))
        .with("shallowForkPending", true)
    } else {
      state
        .with("canonicalBurn", false)
        .with("phase", Held)
        .with("publicState", PublicReorgHold)
        .with("hold", ShallowBaseFork)
        .with("shallowForkPending", true)
        .with("proposalHash", "")
        .with("rawTxIdentity", "")
        .with("authorizedTxIdentity", "")
        .with("reservations", Inputs.mapBy(_ => ""))
    },
  }

  action recoverShallowBaseFork = all {
    state.phase == Held,
    state.hold == ShallowBaseFork,
    state.shallowForkPending,
    not(state.canonicalBurn),
    state.baseAvailable,
    state' = state
      .with("canonicalBurn", true)
      .with("phase", Pending)
      .with("publicState", PublicPending)
      .with("hold", NoHold)
      .with("shallowForkPending", false),
  }

  action shallowNockFork = all {
    state.phase == Submitted,
    state.inclusionTxIdentity == state.rawTxIdentity,
    state.inclusionHeight == 10,
    state.payoutCount == 0,
    state' = state
      .with("inclusionTxIdentity", "")
      .with(
        "reservations",
        if (state.authorizationRecorded) Inputs.mapBy(_ => state.burnId)
        else state.reservations
      )
      .with("inclusionBlock", "")
      .with("hold", ShallowNockFork)
      .with("shallowForkPending", true)
      .with("publicState", PublicReorgHold),
  }

  action deepBaseFork = all {
    state.canonicalBurn,
    state.payoutCount == 0,
    state' = state
      .with("phase", Held)
      .with("publicState", PublicReorgHold)
      .with(
        "reservations",
        if (state.authorizationRecorded) Inputs.mapBy(_ => state.burnId)
        else state.reservations
      )
      .with("hold", DeepBaseFork)
      .with("shallowForkPending", false),
  }

  action deepNockFork = all {
    state.phase.in(Set(Submitted, SequencerConfirmed)),
    state.payoutCount == 0,
    state' = state
      .with("phase", Held)
      .with("publicState", PublicReorgHold)
      .with(
        "reservations",
        if (state.authorizationRecorded) Inputs.mapBy(_ => state.burnId)
        else state.reservations
      )
      .with("hold", DeepNockFork)
      .with("shallowForkPending", false),
  }

  action simultaneousForks = all {
    state.canonicalBurn,
    state.payoutCount == 0,
    state' = state
      .with("phase", Held)
      .with("publicState", PublicReorgHold)
      .with(
        "reservations",
        if (state.authorizationRecorded) Inputs.mapBy(_ => state.burnId)
        else state.reservations
      )
      .with("hold", SimultaneousForks)
      .with("shallowForkPending", false),
  }

  action compensate = all {
    state.phase == Held,
    not(state.canonicalBurn),
    state.payoutCount == 0,
    state.compensationCount == 0,
    state' = state
      .with("phase", Compensated)
      .with("compensationCount", 1)
      .with("shallowForkPending", false),
  }

  action healthyProgress = {
    if (state.phase == Absent) {
      observeCanonicalBurn("burn-1")
    } else if (state.phase == Pending) {
      assemble(1, "proposal-1", "raw-tx-1")
    } else if (state.phase == Assembling) {
      prepare
    } else if (state.phase == Prepared) {
      canonicalize
    } else if (state.phase == PeerCanonical and state.replayRequired and not(allReserved)) {
      restoreReservations
    } else if (state.phase == PeerCanonical and state.replayRequired) {
      replayJournal(state.journalGeneration)
    } else if (state.phase == PeerCanonical) {
      authorize
    } else if (state.phase == Authorized and state.replayRequired and not(allReserved)) {
      restoreReservations
    } else if (state.phase == Authorized and state.replayRequired) {
      replayJournal(state.journalGeneration)
    } else if (state.phase == Authorized) {
      submit
    } else if (state.phase == Submitted and state.replayRequired and not(allReserved)) {
      restoreReservations
    } else if (state.phase == Submitted and state.replayRequired) {
      replayJournal(state.journalGeneration)
    } else if (state.phase == Submitted and state.inclusionTxIdentity == "") {
      include(10, "block-10")
    } else if (state.phase == Submitted) {
      confirm
    } else if (state.phase == SequencerConfirmed and not(state.settledNodes.contains(0))) {
      settleKernel(0)
    } else if (state.phase == SequencerConfirmed and not(state.settledNodes.contains(1))) {
      settleKernel(1)
    } else if (state.phase == SequencerConfirmed and not(state.settledNodes.contains(2))) {
      settleKernel(2)
    } else if (state.phase == SequencerConfirmed and state.payoutCount == 0) {
      recordPayout
    } else if (state.phase == SequencerConfirmed and allReserved) {
      releaseReservations
    } else {
      publishTerminal
    }
  }

  action recoverShallowFork = any {
    recoverShallowBaseFork,
    reinclusion(11, "block-11"),
  }

  action step = any {
    healthyProgress,
    observeDuplicateBurn(state.burnId),
    observeSecondCanonicalBurn("burn-2"),
    reserveSecondWithdrawal,
    replacePrepared(2, "proposal-2", "raw-tx-2"),
    restartSequencer,
    recoverShallowFork,
    compensate,
    shallowBaseFork,
    shallowNockFork,
    deepBaseFork,
    deepNockFork,
    simultaneousForks,
  }

  def allReservedBy(owner: str): bool =
    Inputs.forall(input => state.reservations.get(input) == owner)
  val allReserved = allReservedBy(state.burnId)
  val allSecondReserved = allReservedBy(state.secondBurnId)
  val noReservations = Inputs.forall(input => state.reservations.get(input) == "")
  val allNodesSettled = state.settledNodes == Nodes
  val conservation = and {
    state.grossBurn == state.payoutAmount + state.bridgeFee + state.nockFee,
    state.selectedInputTotal == state.payoutAmount + state.nockFee + state.changeAmount,
  }
  val exactTxIdentity = and {
    state.rawTxIdentity != "",
    state.authorizedTxIdentity == state.rawTxIdentity,
    state.inclusionTxIdentity == state.rawTxIdentity,
  }

  val onePayoutPerBurnInv = state.payoutCount >= 0 and state.payoutCount <= 1
  val compensationExcludesPayoutInv =
    state.compensationCount == 0 or state.payoutCount == 0
  val oneReservationOwnerInv = and {
    Inputs.forall(input => {
      val owner = state.reservations.get(input)
      owner == "" or owner == state.burnId or owner == state.secondBurnId
    }),
    not(state.secondBurnId != "" and
      Inputs.exists(input => state.reservations.get(input) == state.burnId) and
      Inputs.exists(input => state.reservations.get(input) == state.secondBurnId)),
  }
  val retryIdentityInv = and {
    state.authorizedTxIdentity == "" or state.authorizedTxIdentity == state.rawTxIdentity,
    state.inclusionTxIdentity == "" or state.inclusionTxIdentity == state.rawTxIdentity,
  }
  val authorizedRawIdentityInv =
    not(state.authorizationRecorded) or and {
      state.rawTxIdentity != "",
      state.authorizedTxIdentity == state.rawTxIdentity,
    }
  val authorizedReservationHoldInv =
    not(state.authorizationRecorded and state.phase == Held) or allReserved
  val secondReservationOrderInv =
    state.secondPhase != PeerCanonical or state.phase == Terminal
  val terminalProofInv = state.phase != Terminal or and {
    state.canonicalBurn,
    state.payoutCount == 1,
    state.compensationCount == 0,
    exactTxIdentity,
    allNodesSettled,
    noReservations or allSecondReserved,
    conservation,
    state.publicState == PublicTerminal,
  }
  val unsafeForkHoldsInv = state.hold == NoHold or and {
    state.phase.in(Set(Held, Compensated, Submitted)),
    state.publicState == PublicReorgHold or state.phase == Compensated,
    state.payoutCount == 0,
  }
  val publicStateInv = and {
    state.phase != Absent or state.publicState == PublicNone,
    not(state.phase.in(Set(Pending, Assembling, Prepared))) or state.publicState == PublicPending,
    state.phase != PeerCanonical or
      state.publicState == PublicReady or
      (state.replayRequired and state.publicState == PublicPending),
    state.phase != Submitted or
      state.publicState.in(Set(PublicSubmitted, PublicReorgHold)) or
      (state.replayRequired and state.publicState == PublicPending),
    state.phase != SequencerConfirmed or state.publicState == PublicSequencerConfirmed,
    state.phase != Terminal or state.publicState == PublicTerminal,
    state.phase != Held or state.publicState == PublicReorgHold,
  }
  val activeReservationInv =
    not(state.phase.in(Set(PeerCanonical, Authorized, Submitted, SequencerConfirmed))) or
    state.replayRequired or allReserved or
    (state.payoutCount == 1 and noReservations)

  val allSafety = and {
    onePayoutPerBurnInv,
    compensationExcludesPayoutInv,
    oneReservationOwnerInv,
    retryIdentityInv,
    authorizedRawIdentityInv,
    authorizedReservationHoldInv,
    secondReservationOrderInv,
    terminalProofInv,
    unsafeForkHoldsInv,
    publicStateInv,
    activeReservationInv,
  }

  val healthyEnvironment = and {
    state.baseAvailable,
    state.nockAvailable,
    state.quorumAvailable,
    state.liquidityAvailable,
  }
  val healthyEligible = and {
    state.canonicalBurn,
    healthyEnvironment,
    state.hold == NoHold,
    state.compensationCount == 0,
    state.phase != Terminal,
  }

  temporal healthyAvailabilityAssumption =
    always(healthyEnvironment and state.hold == NoHold)

  temporal shallowRecoveryAvailabilityAssumption = always(and {
    state.hold != ShallowBaseFork or state.baseAvailable,
    state.hold != ShallowNockFork or state.nockAvailable,
  })

  // Fairness assumption: while a healthy lifecycle transition remains enabled,
  // the sequencer, observers, and kernels eventually schedule it.
  temporal healthyEventuallyTerminal =
    (healthyAvailabilityAssumption and weakFair(healthyProgress, state)) implies
      (healthyEligible leadsTo (state.phase == Terminal))

  // Fairness assumption: a continuously available shallow-fork recovery
  // observation is eventually consumed. Deep forks intentionally stay Held.
  temporal shallowForkEventuallyResolves =
    (shallowRecoveryAvailabilityAssumption and
      weakFair(healthyProgress, state) and
      weakFair(recoverShallowFork, state)) implies
      ((state.shallowForkPending and not(state.canonicalBurn)) leadsTo
        not(state.shallowForkPending))
}
