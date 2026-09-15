module bridge_withdrawal_tests {
  import bridge_withdrawal.* from "./bridge-withdrawal"

  run throughAuthorization = observeCanonicalBurn("burn-1")
    .then(assemble(1, "proposal-1", "raw-tx-1"))
    .then(prepare)
    .then(canonicalize)
    .then(authorize)

  run throughConfirmation = throughAuthorization
    .then(submit)
    .then(include(10, "block-10"))
    .then(confirm)

  run finishTerminal = settleKernel(0)
    .then(settleKernel(1))
    .then(settleKernel(2))
    .then(recordPayout)
    .then(releaseReservations)
    .then(publishTerminal)

  run happySimulationTest = init
    .then(throughConfirmation)
    .then(finishTerminal)
    .expect(and {
      allSafety,
      state.phase == Terminal,
      state.publicState == PublicTerminal,
      state.payoutCount == 1,
      noReservations,
    })

  run restartSimulationTest = init
    .then(throughAuthorization)
    .then(restartSequencer)
    .expect(and {
      state.phase == Authorized,
      state.replayRequired,
      state.journalGeneration == 1,
      noReservations,
      allSafety,
    })
    .then(restoreReservations)
    .then(replayJournal(1))
    .then(submit)
    .then(include(10, "block-10"))
    .then(confirm)
    .then(finishTerminal)
    .expect(and {
      allSafety,
      state.phase == Terminal,
      state.journalGeneration == 1,
      exactTxIdentity,
    })

  run baseReorgSimulationTest = init
    .then(observeCanonicalBurn("burn-1"))
    .then(shallowBaseFork)
    .expect(and {
      allSafety,
      state.phase == Held,
      state.hold == ShallowBaseFork,
      not(state.canonicalBurn),
      noReservations,
    })
    .then(recoverShallowBaseFork)
    .then(assemble(1, "proposal-1", "raw-tx-1"))
    .then(prepare)
    .then(canonicalize)
    .then(authorize)
    .then(submit)
    .then(include(10, "block-10"))
    .then(confirm)
    .then(finishTerminal)
    .expect(and { allSafety, state.phase == Terminal })

  run authorizedBaseForkPreservesIdentityTest = init
    .then(throughAuthorization)
    .then(shallowBaseFork)
    .expect(and {
      allSafety,
      state.phase == Held,
      state.hold == ShallowBaseFork,
      state.canonicalBurn,
      state.authorizationRecorded,
      state.rawTxIdentity == "raw-tx-1",
      state.authorizedTxIdentity == "raw-tx-1",
      allReserved,
    })

  run secondWithdrawalWaitsForFirstConfirmationTest = init
    .then(observeCanonicalBurn("burn-1"))
    .then(observeSecondCanonicalBurn("burn-2"))
    .then(assemble(1, "proposal-1", "raw-tx-1"))
    .then(prepare)
    .then(canonicalize)
    .then(authorize)
    .expect(and {
      allSafety,
      state.secondPhase == Pending,
      allReserved,
    })
    .then(submit)
    .then(include(10, "block-10"))
    .then(confirm)
    .then(finishTerminal)
    .then(reserveSecondWithdrawal)
    .expect(and {
      allSafety,
      state.phase == Terminal,
      state.secondPhase == PeerCanonical,
      allSecondReserved,
    })

  run nockReinclusionSimulationTest = init
    .then(throughAuthorization)
    .then(submit)
    .then(include(10, "block-10"))
    .then(shallowNockFork)
    .expect(and {
      allSafety,
      state.phase == Submitted,
      state.hold == ShallowNockFork,
      state.inclusionTxIdentity == "",
    })
    .then(reinclusion(11, "block-11"))
    .then(confirm)
    .then(finishTerminal)
    .expect(and {
      allSafety,
      state.phase == Terminal,
      state.inclusionTxIdentity == "raw-tx-1",
      state.inclusionBlock == "block-11",
    })

  run duplicateObservationSimulationTest = init
    .then(observeCanonicalBurn("burn-1"))
    .then(observeDuplicateBurn("burn-1"))
    .expect(and {
      allSafety,
      state.phase == Pending,
      state.burnId == "burn-1",
      state.payoutCount == 0,
    })

  run replacementEpochSimulationTest = init
    .then(observeCanonicalBurn("burn-1"))
    .then(assemble(1, "proposal-1", "raw-tx-1"))
    .then(prepare)
    .then(replacePrepared(2, "proposal-2", "raw-tx-2"))
    .then(prepare)
    .then(canonicalize)
    .then(authorize)
    .expect(and {
      allSafety,
      state.proposalEpoch == 2,
      state.proposalHash == "proposal-2",
      state.rawTxIdentity == "raw-tx-2",
      state.authorizedTxIdentity == "raw-tx-2",
    })

  run staleKernelCannotTerminalizeTest = init
    .then(throughConfirmation)
    .then(settleKernel(0))
    .then(settleKernel(1))
    .expect(and {
      allSafety,
      not(allNodesSettled),
      state.phase == SequencerConfirmed,
      state.payoutCount == 0,
    })

  run deepBaseForkHoldsBeforeMutationTest = init
    .then(observeCanonicalBurn("burn-1"))
    .then(assemble(1, "proposal-1", "raw-tx-1"))
    .then(prepare)
    .then(canonicalize)
    .then(deepBaseFork)
    .expect(and {
      allSafety,
      state.phase == Held,
      state.hold == DeepBaseFork,
      state.canonicalBurn,
      state.rawTxIdentity == "raw-tx-1",
      allReserved,
      state.payoutCount == 0,
    })

  run simultaneousForksHoldTest = init
    .then(throughAuthorization)
    .then(submit)
    .then(include(10, "block-10"))
    .then(simultaneousForks)
    .expect(and {
      allSafety,
      state.phase == Held,
      state.hold == SimultaneousForks,
      exactTxIdentity,
      state.payoutCount == 0,
    })

  run compensatedBurnCannotPayTest = init
    .then(observeCanonicalBurn("burn-1"))
    .then(shallowBaseFork)
    .then(compensate)
    .expect(and {
      allSafety,
      state.phase == Compensated,
      state.compensationCount == 1,
      state.payoutCount == 0,
    })
}
