/=  *   /common/h-zoon
/=  zeke  /common/zeke
/=  w   /common/wrapper
/=  dt  /common/tx-engine
/=  sp  /common/stark/prover
/=  miner-kernel  /apps/dumbnet/miner
|%
+|  %state
+$  load-kernel-state
  $+  load-kernel-state
  $%  kernel-state-0
      kernel-state-1
      kernel-state-2
      kernel-state-3
      kernel-state-4
      kernel-state-5
      kernel-state-6
      kernel-state-7
      kernel-state-8
      kernel-state-9
      kernel-state-10
      kernel-state-11
  ==
::
::  ASERT anchor state stored with a puzzle's branch-local lineage.
+$  cached-asert-anchor  [min-ts=@ target-atom=@]
::
::  Branch-local per-puzzle ASERT state. Every post-activation block stores one
::  entry derived only from its parent entry and its own verified puzzle type.
::  Counts are post-activation blocks of each puzzle. Heads point to the latest
::  block of each puzzle on this ancestry. Anchors are branch-local so block
::  arrival order and side chains cannot affect target computation.
+$  puzzle-asert-state
  $:  zk-count=@
      ai-count=@
      zk-head=(unit block-id:dt)
      ai-head=(unit block-id:dt)
      zk-anchor=cached-asert-anchor
      ai-anchor=(unit cached-asert-anchor)
  ==
::
::
+$  kernel-state-0
  $:  %0
      c=consensus-state-0
      p=pending-state-0
      a=admin-state-0
      m=mining-state-0
    ::
      d=derived-state-0
      constants=blockchain-constants:v0:dt
  ==
::
+$  kernel-state-1
  $:  %1
      c=consensus-state-1
      p=pending-state-1
      a=admin-state-1
      m=mining-state-1
    ::
      d=derived-state-1
      constants=blockchain-constants:v0:dt
  ==
+$  kernel-state-2
  $:  %2
      c=consensus-state-2
      p=pending-state-2
      a=admin-state-2
      m=mining-state-2
    ::
      d=derived-state-2
      constants=blockchain-constants:v0:dt
  ==
+$  kernel-state-3
  $:  %3
      c=consensus-state-3
      p=pending-state-3
      a=admin-state-3
      m=mining-state-3
    ::
      d=derived-state-3
      constants=blockchain-constants:v0:dt
  ==
::
+$  kernel-state-4
  $:  %4
      c=consensus-state-4
      p=pending-state-4
      a=admin-state-4
      m=mining-state-4
    ::
      d=derived-state-4
      constants=blockchain-constants:v0:dt
  ==
::
+$  kernel-state-5
  $:  %5
      c=consensus-state-5
      a=admin-state-5
      m=mining-state-5
    ::
      d=derived-state-5
      constants=blockchain-constants:v0:dt
  ==
::
::  frozen pre-ASERT snapshot of blockchain-constants:v1 (without the five
::  asert-* fields appended in this PR). used to decode old %6 states that
::  were serialized before the schema change.
+$  blockchain-constants-v1-pre-asert
  $:  v1-phase=@
      bythos-phase=@
      data=[max-size=@ min-fee=@]
      base-fee=@
      input-fee-divisor=@
      blockchain-constants:v0:dt
  ==
::
+$  kernel-state-6
  $:  %6
      c=consensus-state-6
      a=admin-state-6
      m=mining-state-6
    ::
      d=derived-state-6
      constants=blockchain-constants-v1-pre-asert
  ==
::
::  frozen phase-1 snapshot of blockchain-constants:v1 (with the five
::  asert-* fields from kernel-state-7 but WITHOUT the phase-2
::  asert-anchor-min-timestamp field appended in this PR). used to decode
::  old %7 states that were serialized before the schema change.
+$  blockchain-constants-v1-phase-1
  $:  v1-phase=@
      bythos-phase=@
      data=[max-size=@ min-fee=@]
      base-fee=@
      input-fee-divisor=@
      blockchain-constants:v0:dt
      asert-phase=@
      asert-anchor-height=@
      asert-anchor-target-atom=@
      asert-ideal-block-time=@
      asert-half-life=@
  ==
::
::  kernel-state-7 originally had the same shape as kernel-state-6 but
::  tracked the schema change in blockchain-constants:v1: five ASERT fields
::  (asert-phase, anchor-height, anchor-target-atom, ideal-block-time,
::  half-life) were appended to the v1 wrapper in tx-engine-1.hoon. now
::  that a sixth field (asert-anchor-min-timestamp) has been added by
::  phase 2 of 014-aletheia, kernel-state-7 pins the frozen phase-1
::  snapshot so old %7 states still decode.
+$  kernel-state-7
  $:  %7
      c=consensus-state-7
      a=admin-state-7
      m=mining-state-7
    ::
      d=derived-state-7
      constants=blockchain-constants-v1-phase-1
  ==
::
::  kernel-state-8 carries the full post-phase-2 blockchain-constants:v1
::  (six asert-* fields, including the hardcoded anchor median-of-11).
::  the state-7-to-8 upgrade discards the old constants noun and lets
::  +update-constants reseed from *blockchain-constants:t on mainnet.
+$  kernel-state-8
  $:  %8
      c=consensus-state-8
      a=admin-state-8
      m=mining-state-8
    ::
      d=derived-state-8
      constants=blockchain-constants:v1:dt
  ==
::
::  kernel-state-9 moves consensus core maps and sets to h-zoon
::  containers while preserving the post-phase-2 constants shape.
+$  kernel-state-9
  $:  %9
      c=consensus-state-9
      a=admin-state-9
      m=mining-state-9
    ::
      d=derived-state-9
      constants=blockchain-constants:v1:dt
  ==
:::
::  kernel-state-10 records scheduled re-anchor median timestamps per accepted branch.
+$  kernel-state-10
  $:  %10
      c=consensus-state-10
      a=admin-state-9
      m=mining-state-9
    ::
      d=derived-state-9
      constants=blockchain-constants:v1:dt
  ==
::
::  Kernel state 11 stores the metadata needed to validate and build
::  both proof paths without reconstructing branch-local state.
+$  kernel-state-11
  $:  %11
      c=consensus-state-11
      a=admin-state-9
      m=mining-state-9
    ::
      d=derived-state-11
      constants=blockchain-constants:v1:dt
  ==
:::
+$  kernel-state  kernel-state-11
::
+$  consensus-state-0
  $+  consensus-state-0
  $:  balance=(z-mip block-id:v0:dt nname:v0:dt nnote:v0:dt)
      txs=(z-mip block-id:v0:dt tx-id:v0:dt tx:v0:dt) ::  fully validated transactions
      blocks=(z-map block-id:v0:dt local-page:v0:dt)  ::  fully validated blocks
    ::
      heaviest-block=(unit block-id:v0:dt) ::  most recent heaviest block
    ::
    ::  min timestamp of block that is a child of this block
      min-timestamps=(z-map block-id:v0:dt @)
    ::  this map is used to calculate epoch duration. it is a map of each
    ::  block-id to the first block-id in that epoch.
      epoch-start=(z-map block-id:v0:dt block-id:v0:dt)
    ::  this map contains the expected target for the child
    ::  of a given block-id.
      targets=(z-map block-id:v0:dt bignum:bignum:v0:dt)
    ::
    ::  Bitcoin block hash for genesis block
    ::>)  TODO: change face to btc-hash?
      btc-data=(unit (unit btc-hash:v0:dt))
      =genesis-seal:v0:dt  ::  desired seal for genesis block
  ==
::
+$  consensus-state-1  $+(consensus-state-1 consensus-state-0)
::
+$  consensus-state-2  $+(consensus-state-2 consensus-state-1)
::
+$  consensus-state-3
  $+  consensus-state-3
  $:  balance=(z-mip block-id:v0:dt nname:v0:dt nnote:v0:dt)
      txs=(z-mip block-id:v0:dt tx-id:v0:dt tx:v0:dt) ::  fully validated transactions
      raw-txs=(z-map tx-id:v0:dt raw-tx:v0:dt) :: raw versions of fully validated transactions
      blocks=(z-map block-id:v0:dt local-page:v0:dt)  ::  fully validated blocks
    ::
      heaviest-block=(unit block-id:v0:dt) ::  most recent heaviest block
    ::
    ::  min timestamp of block that is a child of this block
      min-timestamps=(z-map block-id:v0:dt @)
    ::  this map is used to calculate epoch duration. it is a map of each
    ::  block-id to the first block-id in that epoch.
      epoch-start=(z-map block-id:v0:dt block-id:v0:dt)
    ::  this map contains the expected target for the child
    ::  of a given block-id.
      targets=(z-map block-id:v0:dt bignum:bignum:v0:dt)
    ::
    ::  Bitcoin block hash for genesis block
    ::>)  TODO: change face to btc-hash?
      btc-data=(unit (unit btc-hash:v0:dt))
      =genesis-seal:v0:dt  ::  desired seal for genesis block
  ==
::
+$  consensus-state-4  $+(consensus-state-4 consensus-state-3)
::
+$  consensus-state-5
  $+  consensus-state-5
  ::
  ::  indexes and not-fully-validated state
  $:
    $:
    :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        blocks-needed-by=(z-jug tx-id:v0:dt block-id:v0:dt) :: dependencies
        excluded-txs=(z-set tx-id:v0:dt) :: transactions unneeded by any block
    ::
    ::  every tx-id in spent-by must be in raw-txs and vice-versa
        spent-by=(z-jug nname:v0:dt tx-id:v0:dt)
    ::
        pending-blocks=(z-map block-id:v0:dt [=page:v0:dt heard-at=@])  :: pending blocks
    ==
  ::
  ::  core consensus state
    $:  balance=(z-mip block-id:v0:dt nname:v0:dt nnote:v0:dt)
        txs=(z-mip block-id:v0:dt tx-id:v0:dt tx:v0:dt) ::  fully validated transactions
      ::
      :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        raw-txs=(z-map tx-id:v0:dt [=raw-tx:v0:dt heard-at=@]) :: raw transactions
      ::
        blocks=(z-map block-id:v0:dt local-page:v0:dt)  ::  fully validated blocks
      ::
        heaviest-block=(unit block-id:v0:dt) ::  most recent heaviest block
      ::
      ::  min timestamp of block that is a child of this block
        min-timestamps=(z-map block-id:v0:dt @)
      ::  this map is used to calculate epoch duration. it is a map of each
      ::  block-id to the first block-id in that epoch.
        epoch-start=(z-map block-id:v0:dt block-id:v0:dt)
      ::  this map contains the expected target for the child
      ::  of a given block-id.
        targets=(z-map block-id:v0:dt bignum:bignum:v0:dt)
      ::
      ::  Bitcoin block hash for genesis block
      ::>)  TODO: change face to btc-hash?
        btc-data=(unit (unit btc-hash:v0:dt))
        =genesis-seal:v0:dt  ::  desired seal for genesis block
    ==
  ==
::
+$  consensus-state-6
  $+  consensus-state-6
  ::
  ::  indexes and not-fully-validated state
  $:
    $:
    :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        blocks-needed-by=(z-jug tx-id:dt block-id:dt) :: dependencies
        excluded-txs=(z-set tx-id:dt) :: transactions unneeded by any block
    ::
    ::  every tx-id in spent-by must be in raw-txs and vice-versa
        spent-by=(z-jug nname:dt tx-id:dt)
    ::
        pending-blocks=(z-map block-id:dt [=page:dt heard-at=@])  :: pending blocks
    ==
  ::
  ::  core consensus state
    $:  balance=(z-mip block-id:dt nname:dt nnote:dt)
        txs=(z-mip block-id:dt tx-id:dt tx:dt) ::  fully validated transactions
      ::
      :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        raw-txs=(z-map tx-id:dt [=raw-tx:dt heard-at=@]) :: raw transactions
      ::
        blocks=(z-map block-id:dt local-page:dt)  ::  fully validated blocks
      ::
        heaviest-block=(unit block-id:dt) ::  most recent heaviest block
      ::
      ::  min timestamp of block that is a child of this block
        min-timestamps=(z-map block-id:dt @)
      ::  this map is used to calculate epoch duration. it is a map of each
      ::  block-id to the first block-id in that epoch.
        epoch-start=(z-map block-id:dt block-id:dt)
      ::  this map contains the expected target for the child
      ::  of a given block-id.
        targets=(z-map block-id:dt bignum:bignum:dt)
      ::
      ::  Bitcoin block hash for genesis block
      ::>)  TODO: change face to btc-hash?
        btc-data=(unit (unit btc-hash:dt))
        =genesis-seal:dt  ::  desired seal for genesis block
    ==
  ==
::
+$  consensus-state-7  $+(consensus-state-7 consensus-state-6)
::
+$  consensus-state-8  $+(consensus-state-8 consensus-state-7)
::
+$  consensus-state-9
  $+  consensus-state-9
  ::
  ::  indexes and not-fully-validated state
  $:
    $:
    :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        blocks-needed-by=(h-jug tx-id:dt block-id:dt) :: dependencies
        excluded-txs=(h-set tx-id:dt) :: transactions unneeded by any block
    ::
    ::  every tx-id in spent-by must be in raw-txs and vice-versa
        spent-by=(h-jug nname:dt tx-id:dt)
    ::
        pending-blocks=(h-map block-id:dt [=page:dt heard-at=@])  :: pending blocks
    ==
  ::
  ::  core consensus state
    $:  balance=(h-mip block-id:dt nname:dt nnote:dt)
        txs=(h-mip block-id:dt tx-id:dt tx:dt) ::  fully validated transactions
      ::
      :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        raw-txs=(h-map tx-id:dt [=raw-tx:dt heard-at=@]) :: raw transactions
      ::
        blocks=(h-map block-id:dt local-page:dt)  ::  fully validated blocks
      ::
        heaviest-block=(unit block-id:dt) ::  most recent heaviest block
      ::
      ::  min timestamp of block that is a child of this block
        min-timestamps=(h-map block-id:dt @)
      ::  this map is used to calculate epoch duration. it is a map of each
      ::  block-id to the first block-id in that epoch.
        epoch-start=(h-map block-id:dt block-id:dt)
      ::  this map contains the expected target for the child
      ::  of a given block-id.
        targets=(h-map block-id:dt bignum:bignum:dt)
      ::
      ::  Bitcoin block hash for genesis block
      ::>)  TODO: change face to btc-hash?
        btc-data=(unit (unit btc-hash:dt))
        =genesis-seal:dt  ::  desired seal for genesis block
    ==
  ==
:::
+$  consensus-state-10
  $+  consensus-state-10
  ::
  ::  indexes and not-fully-validated state
  $:
    $:
    :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        blocks-needed-by=(h-jug tx-id:dt block-id:dt) :: dependencies
        excluded-txs=(h-set tx-id:dt) :: transactions unneeded by any block
    ::
    ::  every tx-id in spent-by must be in raw-txs and vice-versa
        spent-by=(h-jug nname:dt tx-id:dt)
    ::
        pending-blocks=(h-map block-id:dt [=page:dt heard-at=@])  :: pending blocks
    ==
  ::
  ::  core consensus state
    $:  balance=(h-mip block-id:dt nname:dt nnote:dt)
        txs=(h-mip block-id:dt tx-id:dt tx:dt) ::  fully validated transactions
      ::
      :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        raw-txs=(h-map tx-id:dt [=raw-tx:dt heard-at=@]) :: raw transactions
      ::
        blocks=(h-map block-id:dt local-page:dt)  ::  fully validated blocks
      ::
        heaviest-block=(unit block-id:dt) ::  most recent heaviest block
      ::
      ::  min timestamp of block that is a child of this block
        min-timestamps=(h-map block-id:dt @)
      ::  dynamic scheduled ASERT anchor timestamps, isolated by puzzle type
        asert-anchor-min-timestamps=(map @tas (h-map block-id:dt @))
      ::  this map is used to calculate epoch duration. it is a map of each
      ::  block-id to the first block-id in that epoch.
        epoch-start=(h-map block-id:dt block-id:dt)
      ::  this map contains the expected target for the child
      ::  of a given block-id.
        targets=(h-map block-id:dt bignum:bignum:dt)
      ::
      ::  Bitcoin block hash for genesis block
      ::>)  TODO: change face to btc-hash?
        btc-data=(unit (unit btc-hash:dt))
        =genesis-seal:dt  ::  desired seal for genesis block
    ==
  ==

::
::
::  consensus-state-11 records proof versions for blocks whose puzzle type is
::  not derivable from height alone.
+$  consensus-state-11
  $+  consensus-state-11
  ::
  ::  indexes and not-fully-validated state
  $:
    $:
    :: keys in raw-txs must be in EXACTLY ONE OF blocks-needed-by or excluded-txs
        blocks-needed-by=(h-jug tx-id:dt block-id:dt) :: dependencies
        excluded-txs=(h-set tx-id:dt) :: transactions unneeded by any block
    ::
    ::  every tx-id in spent-by must be in raw-txs and vice-versa
        spent-by=(h-jug nname:dt tx-id:dt)
    ::
        pending-blocks=(h-map block-id:dt [=page:dt heard-at=@])  :: pending blocks
    ==
  ::
  ::  core consensus state
    $:  balance=(h-mip block-id:dt nname:dt nnote:dt)
        txs=(h-mip block-id:dt tx-id:dt tx:dt) ::  fully validated transactions
      ::
        raw-txs=(h-map tx-id:dt [=raw-tx:dt heard-at=@]) :: raw transactions
      ::
        blocks=(h-map block-id:dt local-page:dt)
        heaviest-block=(unit block-id:dt)
        min-timestamps=(h-map block-id:dt @)
        asert-anchor-min-timestamps=(map @tas (h-map block-id:dt @))
        epoch-start=(h-map block-id:dt block-id:dt)
        targets=(h-map block-id:dt bignum:bignum:dt)
        btc-data=(unit (unit btc-hash:dt))
        =genesis-seal:dt
        block-versions=(h-map block-id:dt proof-version:sp)
    ==
  ==
:::
+$  consensus-state  consensus-state-11
::
::  you will not have lost any chain state if you lost pending state, you'd just have to
::  request data again from peers and reset your mining state
+$  pending-state-0
  $+  pending-state
  $:  pending-blocks=(z-map block-id:v0:dt local-page:v0:dt)  ::  blocks for which we are waiting on txs
    ::  data we need
      block-tx=(z-jug block-id:v0:dt tx-id:v0:dt)  ::  tx-id's needed before pending block-id can be validated
      tx-block=(z-jug tx-id:v0:dt block-id:v0:dt)  ::  pending block-id's that include tx-id
    ::  data we have
      raw-txs=(z-map tx-id:v0:dt raw-tx:v0:dt)
      spent-by=(z-map nname:v0:dt tx-id:v0:dt)        ::  names of notes and the pending tx trying to spend it
      heard-at=(z-map tx-id:v0:dt page-number:v0:dt)  :: block height which a tx-id was first heard
  ==
::
+$  pending-state-1  $+(pending-state-1 pending-state-0)
::
+$  pending-state-2  $+(pending-state-2 pending-state-1)
::
+$  pending-state-3  $+(pending-state-3 pending-state-2)
::
+$  pending-state-4  $+(pending-state-4 pending-state-3)
::  for kernel version 5 and later there is no pending state
::
+$  admin-state-0
  $+  admin-state-0
  $:  desk-hash=(unit @uvI)               ::  hash of zkvm desk
      init=init-phase                     ::  boolean flag denoting whether kernel is in the init phase.
      retain=$~([~ 20] (unit @))          ::  finite age window for pending blocks and txs
                                          ::  value of ~ disables pending-block age GC and
                                          ::  gives txs a bounded safety lease
  ==
::
+$  admin-state-1  $+(admin-state-1 admin-state-0)
::
+$  admin-state-2  $+(admin-state-2 admin-state-1)
::
+$  admin-state-3  $+(admin-state-3 admin-state-2)
::
+$  admin-state-4  $+(admin-state-4 admin-state-3)
::
+$  admin-state-5  $+(admin-state-5 admin-state-4)
::
+$  admin-state-6  $+(admin-state-6 admin-state-5)
::
+$  admin-state-7  $+(admin-state-7 admin-state-6)
::
+$  admin-state-8  $+(admin-state-8 admin-state-7)
::
+$  admin-state-9  $+(admin-state-9 admin-state-8)
::
+$  admin-state  admin-state-9
::
+$  derived-state-0
  $+  derived-state-0
  $:  heaviest-chain=(z-map page-number:v0:dt block-id:v0:dt)
  ==
::
+$  derived-state-1
  $+  derived-state-1
  $:  highest-block-height=(unit page-number:v0:dt)
      heaviest-chain=(z-map page-number:v0:dt block-id:v0:dt)
  ==
::
+$  derived-state-2  $+(derived-state-2 derived-state-1)
::
+$  derived-state-3  $+(derived-state-3 derived-state-2)
::
+$  derived-state-4  $+(derived-state-4 derived-state-3)
::
+$  derived-state-5  $+(derived-state-5 derived-state-4)
::
+$  derived-state-6
  $+  derived-state-6
  $:  highest-block-height=(unit page-number:dt)
      heaviest-chain=(z-map page-number:dt block-id:dt)
  ==
::
+$  derived-state-7  $+(derived-state-7 derived-state-6)
::
+$  derived-state-8  $+(derived-state-8 derived-state-7)
::
+$  derived-state-9  $+(derived-state-9 derived-state-8)
::
::  Current derived state. `puzzle-asert-states` is complete for every accepted
::  post-activation block and follows forks independently.
+$  derived-state-11
  $+  derived-state-11
  $:  highest-block-height=(unit page-number:dt)
      heaviest-chain=(z-map page-number:dt block-id:dt)
      puzzle-asert-states=(h-map block-id:dt puzzle-asert-state)
  ==
::
+$  derived-state  derived-state-11
::
+$  mining-state-0
  $+  mining-state-0
  $:  mining=?                        ::  build candidate blocks?
      pubkeys=(z-set sig:v0:dt)          :: sigs for coinbase in mined blocks
      shares=(z-map sig:v0:dt @)         ::  shares of coinbase+fees among sigs
      candidate-block=page:v0:dt            ::  the next block we will attempt to mine.
      candidate-acc=tx-acc:v0:dt           ::  accumulator for txs in candidate block
      next-nonce=noun-digest:tip5:zeke  :: nonce being mined
  ==
::
+$  mining-state-1  $+(mining-state-1 mining-state-0)
::
+$  mining-state-2  $+(mining-state-2 mining-state-1)
::
+$  mining-state-3  $+(mining-state-3 mining-state-2)
::
+$  mining-state-4  $+(mining-state-4 mining-state-3)
::
+$  mining-state-5  $+(mining-state-4 mining-state-3)
::
+$  mining-state-6
  $+  mining-state-6
  $:  mining=?                        ::  build candidate blocks?
      shares=(z-map hash:dt @)              ::  shares of coinbase+fees among sighashes (v1)
      v0-shares=(z-map sig:v0:dt @)         ::  shares of coinbase+fees among sigs (v0)
      candidate-block=page:dt            ::  the next block we will attempt to mine.
      candidate-acc=*                   ::  old candidates are discarded by upgrades
      next-nonce=noun-digest:tip5:zeke  :: nonce being mined
  ==
::
+$  mining-state-7  $+(mining-state-7 mining-state-6)
::
+$  mining-state-8  $+(mining-state-8 mining-state-7)
::
+$  mining-state-9
  $+  mining-state-9
  $:  mining=?                        ::  build candidate blocks?
      shares=(z-map hash:dt @)              ::  shares of coinbase+fees among sighashes (v1)
      v0-shares=(z-map sig:v0:dt @)         ::  shares of coinbase+fees among sigs (v0)
      candidate-block=page:dt            ::  the next block we will attempt to mine.
      candidate-acc=tx-acc:dt           ::  accumulator for txs in candidate block
      next-nonce=noun-digest:tip5:zeke  :: nonce being mined
  ==
::
+$  mining-state  mining-state-9
::
+$  init-phase  $~(%.y ?)
::
+|  %io
+$  peer-id  @id  ::  libp2p PeerId in base58 format converted to a bytestring
+$  cause
  $+  cause
  $%  [%fact p=fact]  ::  wire format; message from king, kernel must validate these
      [%command p=command]  ::  originate locally
  ==
::
::  At activation, proof hashes bind one encoding of each Horner-evaluated polynomial.
++  canonical-pow-polynomial-height  112.500
++  canonical-pow-polynomials
  |=  proof=proof:sp
  ^-  ?
  %+  levy  objects.proof
  |=  pd=proof-data:sp
  ?+  -.pd  %.y
    %poly  =(p.pd (bpcan:zeke p.pd))
  ==
++  canonical-pow-proof
  |=  [height=@ proof=proof:sp]
  ^-  ?
  ?:  (lth height canonical-pow-polynomial-height)
    %.y
  (canonical-pow-polynomials proof)
:::
+$  ai-blake  ai-blake:dt
+$  ai-pow-nonce  ai-pow-nonce:dt
+$  ai-ext2  ai-ext2:dt
+$  ai-ext2s  ai-ext2s:dt
+$  ai-ext2-vec  ai-ext2-vec:dt
+$  ai-pow-commitments  ai-pow-commitments:dt
+$  ai-pow-public-inputs  ai-pow-public-inputs:dt
+$  ai-proof-node  ai-proof-node:dt
+$  ai-recursive-certificate  ai-recursive-certificate:dt
+$  ai-pow-certificate  ai-pow-certificate:dt
+$  ai-pow-artifact  ai-pow-artifact:dt
::
::  Tagged union of proof-of-work variants. The miner pokes the consensus
::  kernel with `[%command %pow pv=pow-variant]`; the consensus kernel
::  dispatches on `-.pv` so additional puzzle types (e.g. %ai-pow) can be
::  added without changing the outer `%command` shape.
+$  pow-variant
  $+  pow-variant
  $%  [%dumb-zkpow prf=proof:sp dig=tip5-hash-atom:zeke bc=noun-digest:tip5:zeke nonce=noun-digest:tip5:zeke]  ::  the existing puzzle-nock STARK PoW
      ::  AI matmul PoW wire shape. Carries a Rust-owned nonce plus the
      ::  recursive certificate only; raw Layer-0 proofs and the plain
      ::  MatmulProof are not persisted in blocks. This arm is verified via
      ::  the recursive-certificate jet (%ai-pow-verify).
      [%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]
  ==
::
+$  command
  $+  command
  $%  [%pow pv=pow-variant]  ::  check if a proof of work is good for the next block, issue a block if so
      [%set-mining-key v0=@t v1=@t]  ::  set $lock for coinbase in mined blocks
      [%set-mining-key-advanced v0=(list [share=@ m=@ keys=(list @t)]) v1=(list [share=@ phk=@t])]  :: multisig and/or split coinbases
      [%enable-mining p=?]  ::  switch for generating candidate blocks for mining
      [%timer p=~] :: ask for heaviest block, needed txs, and miner tx refresh
      [%born p=~]  ::  initial event the king sends on boot
      [%genesis p=[=btc-hash:dt block-height=@ message=cord]]  ::  emit genesis block with this template
      :: set expected btc height and msg hash of genesis block
      [%set-genesis-seal p=[height=page-number:dt msg-hash=@t]]
      [%btc-data p=(unit btc-hash:dt)]  ::  data from BTC RPC node
      test-command
  ==
::
::  commands only used during testing
+$  test-command
  $+  test-command
  $%  [%set-constants p=blockchain-constants:dt]
  ==
::
::  commands that can be performed if init-phase is %.y
+$  init-command
  $?  %born
      %set-mining-key
      %set-mining-key-advanced
      %enable-mining
      init-only-command
      %set-genesis-seal
  ==
::  commands that can *only* be performed if init-phase is %.y
+$  init-only-command
  $?  %genesis
      %set-constants
      %btc-data
  ==
::
+$  fact
  $+  fact
  $:  version=%0
    $=  data
    $%  [%heard-block p=page:dt]
        [%heard-tx p=raw-tx:dt]
        [%heard-elders p=[oldest=page-number:dt ids=(list block-id:dt)]]
    ==
  ==
::
+$  effect
  $+  effect
  $%  [%gossip p=fact]  :: broadcast tx or block to network
      [%request p=request]  :: request specific tx or block
      [%track p=track]  :: runtime tracking of blocks for %liar-block-id effect
      [%seen p=seen]    ::  seen so don't reprocess
      ::  Mining candidate emissions. `%mine-zk` is emitted when the
      ::  candidate block changes. `%mine-ai` is emitted for the AI puzzle
      ::  once at/after the AI-PoW activation height.
      [%mine-zk mine-start]
      [%mine-ai mine-start]
      lie
      span-effect
      [%exit code=@]
  ==
::
+$  mine-start
  $%  [%0 block-commitment=noun-digest:tip5:zeke target=bignum:bignum:dt pow-len=@]
      [%1 block-commitment=noun-digest:tip5:zeke target=bignum:bignum:dt pow-len=@]
      [%2 block-commitment=noun-digest:tip5:zeke target=bignum:bignum:dt pow-len=@]
      [%3 block-commitment=noun-digest:tip5:zeke target=bignum:bignum:dt pow-len=@]
      ::  %4 identifies the AI-PoW artifact emitted only through %mine-ai.
      [%4 block-commitment=noun-digest:tip5:zeke target=bignum:bignum:dt pow-len=@]
  ==
::
+$  seen
  $+  seen
  $%  [%block p=block-id:dt q=(unit page-number:dt)]  ::  block has been seen, don't reprocess
      [%tx p=tx-id:dt]                                ::  tx has been seen, don't reprocess
  ==
::
+$  span-field
  $%  [%n p=@ud]
      [%s p=@t]
  ==
+$  span-effect  [%span name=cord fields=(list (pair cord span-field))]
::
+$  request
  $+  request
  $%  [%block request-block]
      [%raw-tx request-tx]
  ==
::
++  request-block
  $%  [%by-height p=page-number:dt] ::  request block at height .p on each peer's heaviest chain
      [%elders p=block-id:dt q=peer-id] ::  request ancestor block IDs up to 24 deep from specific peer
  ==
::
++  request-tx
  $%  [%by-id p=tx-id:dt] ::  request raw-tx with id .p from peers
  ==
::
::  Records reason for failure if %.n
::  Returns `object` if %.y
::  Used to surface cause to liar effect.
++  reason
  |$  object
  (each object term)
::
::  the runtime tracks who sent us which blocks to determine which peers to
::  ban for a bad block. an %add effect is emitted when a block has a valid
::  digest. this tells the runtime to add that block-id and peer-id to
::  MessageTracker and means %liar-block-id is now
::  possible for that block-id (see the libp2p driver for further
::  information). %remove means that that block-id has valid txs as well, so
::  it is no longer necessary for the driver to track that block-id.
+$  track
  $+  track
  $%  [%add p=block-id:dt q=peer-id]  ::  everything but txs checked, add to tracking
      [%remove p=block-id:dt] ::  txs also valid, remove from tracking
  ==
::
+$  lie
  $%  ::  block has bad non-tx data, or raw-tx did not validate. this is only
      ::  ever returned as an effect in response to a particular tx or block
      ::  poke.
      [%liar-peer p=peer-id cause=term]  ::  block-id is wrong, or raw-tx did not validate
      ::
      ::  block-id is correct, block did not validate. this is only returned once
      ::  a block's fields are all checked as having been valid - so we know
      ::  the block-id and powork are valid in particular. so only bad tx data
      ::  can cause this to be emitted - and the libp2p driver will ban all nodes
      ::  that sent us this block-id as a result.
      [%liar-block-id p=block-id:dt cause=term]
  ==
::
::  $goof: kernel error type
::
+$  goof    [mote=term =tang]
+$  ovum    [[%poke ~] =pok]                                 ::  internal poke
::  $crud: kernel error wrapper
::
+$  crud    [=goof =pok]
::  $pok: kernel poke type
::
+$  pok     [eny=@ our=@ux now=@da =cause]
::
++  realnet-genesis-msg  (from-b58:hash:dt '2c8Ltbg44dPkEGcNPupcVAtDgD87753M9pG2fg8yC2mTEqg5qAFvvbT')
--
