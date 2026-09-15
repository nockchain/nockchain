::  Proof tests for post-confirmation bridge history divergence.
::
::  These fixtures bypass the Rust confirmation buffers and assert the kernel's
::  fail-stop boundary after accepted history changes. They do not model an
::  ordinary shallow fork or require automatic rewind as a launch condition.
::
/=  *  /common/test
/=  base-lib  /apps/bridge/base
/=  nock-lib  /apps/bridge/nock
/=  hel  /tests/bridge/helpers
/=  *  /apps/bridge/types
|%
++  has-stop-effect
  |=  effects=(list effect)
  ^-  ?
  ?~  effects  %.n
  ?:  ?=([%0 %stop * *] i.effects)
    %.y
  $(effects t.effects)
::
::  This fixture directly injects a replacement Base branch after the original
::  burn was accepted, bypassing the driver-side confirmation depth. With no
::  stop flag present, the kernel keeps the old hashchain cursor and orphaned
::  withdrawal, so the replacement reaches the same stop on every retry.
++  test-reorg-recovery-post-confirmation-base-retry-preserves-orphaned-withdrawal
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 10 0)
  =.  base-hashchain-next-height.hash-state.state  10
  =/  event-id=beid  (from-atom:blist 0x44)
  =/  recipient=nock-lock-root  [0x1 0x2 0x3 0x4 0x5]
  =/  burn-event=base-event
    :*  (to-atom:blist event-id)
        [%burn-for-withdrawal 0x1111 10.000.000.000 recipient]
    ==
  =/  canonical-raw=raw-base-blocks:cause
    :~  [10 0xa10 0x0 ~[burn-event]]
    ==
  =/  base  ~(. base-lib state)
  =/  [stage-effects=(list effect) staged=bridge-state]
    (incoming-base-blocks:base [canonical-raw [~ 0 0x0 *@da]])
  ?~  stage-effects
    ~|('expected canonical Base batch to stage' !!)
  ?>  ?=([%0 %base-block-withdrawals-pending *] i.stage-effects)
  =/  pending=pending-base-block-withdrawals  pending.i.stage-effects
  =/  ack=base-block-commit-ack
    [blocks-hash.pending first-height.pending last-height.pending]
  =/  base-staged  ~(. base-lib staged)
  =/  [ack-effects=(list effect) canonical=bridge-state]
    (commit-base-block-withdrawals:base-staged ack)
  =/  canonical-hash-state=hash-state  hash-state.canonical
  =/  orphan-present-before=?
    (~(has z-bi unsettled-withdrawals.canonical-hash-state) blocks-hash.pending event-id)
  =/  replacement-raw=raw-base-blocks:cause
    :~  [11 0xb11 0xb10 ~]
    ==
  =/  canonical-base  ~(. base-lib canonical)
  =/  [fork-effects=(list effect) after-fork=bridge-state]
    (incoming-base-blocks:canonical-base [replacement-raw [~ 0 0x0 *@da]])
  =/  after-fork-base  ~(. base-lib after-fork)
  =/  [retry-effects=(list effect) after-retry=bridge-state]
    (incoming-base-blocks:after-fork-base [replacement-raw [~ 0 0x0 *@da]])
  =/  orphan-present-after=?
    (~(has z-bi unsettled-withdrawals.hash-state.after-retry) blocks-hash.pending event-id)
  ;:  weld
    (expect-eq !>(~) !>(ack-effects))
    (expect !>(orphan-present-before))
    (expect !>((has-stop-effect fork-effects)))
    (expect-eq !>(canonical-hash-state) !>(hash-state.after-fork))
    (expect !>(?=(~ stop.after-fork)))
    (expect !>((has-stop-effect retry-effects)))
    (expect-eq !>(canonical-hash-state) !>(hash-state.after-retry))
    (expect !>(orphan-present-after))
  ==
::
::  This fixture likewise bypasses Nockchain's confirmation depth. With no stop
::  flag present, the old page hashchain and next height still reject a
::  post-confirmation replacement parent on every retry.
++  test-reorg-recovery-post-confirmation-nock-retry-preserves-hashchain
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 0 20)
  =.  nock-hashchain-next-height.hash-state.state  20
  =/  canonical-id=block-id:t  [0xa20 0x0 0x0 0x0 0x0]
  =/  canonical-page=page:v1:t  *page:v1:t
  =.  height.canonical-page  20
  =.  parent.canonical-page  *block-id:t
  =.  digest.canonical-page  canonical-id
  =.  tx-ids.canonical-page  *(z-set tx-id:t)
  =/  empty-txs=(z-map tx-id:t tx:t)  *(z-map tx-id:t tx:t)
  =/  nock  ~(. nock-lib state)
  =/  [canonical-effects=(list effect) canonical=bridge-state]
    (incoming-nockchain-block:nock [[canonical-page empty-txs] [~ 0 0x0 *@da]])
  =/  canonical-hash-state=hash-state  hash-state.canonical
  =/  replacement-page=page:v1:t  *page:v1:t
  =.  height.replacement-page  21
  =.  parent.replacement-page  [0xb20 0x0 0x0 0x0 0x0]
  =.  digest.replacement-page  [0xb21 0x0 0x0 0x0 0x0]
  =.  tx-ids.replacement-page  *(z-set tx-id:t)
  =/  canonical-nock  ~(. nock-lib canonical)
  =/  [fork-effects=(list effect) after-fork=bridge-state]
    (incoming-nockchain-block:canonical-nock [[replacement-page empty-txs] [~ 0 0x0 *@da]])
  =/  after-fork-nock  ~(. nock-lib after-fork)
  =/  [retry-effects=(list effect) after-retry=bridge-state]
    (incoming-nockchain-block:after-fork-nock [[replacement-page empty-txs] [~ 0 0x0 *@da]])
  ;:  weld
    (expect-eq !>(~) !>(canonical-effects))
    (expect-eq !>(21) !>(nock-hashchain-next-height.canonical-hash-state))
    (expect !>((has-stop-effect fork-effects)))
    (expect-eq !>(canonical-hash-state) !>(hash-state.after-fork))
    (expect !>(?=(~ stop.after-fork)))
    (expect !>((has-stop-effect retry-effects)))
    (expect-eq !>(canonical-hash-state) !>(hash-state.after-retry))
  ==
--
