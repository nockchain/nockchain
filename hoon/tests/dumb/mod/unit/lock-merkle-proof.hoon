/=  tx-engine  /common/tx-engine
/=  helpers  /tests/dumb/helpers
/=  zo  /common/zoon
/=  hz  /common/h-zoon
/=  *  /common/zeke
/=  *  /common/test
|_  constants=blockchain-constants:tx-engine
+*  t  ~(. tx-engine constants)
    h  ~(. helpers constants)
::
++  test-lock-merkle-proof-valid-2
  =/  pkh-left  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-right  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  sc-left=spend-condition:t  form.pkh-left
  =/  sc-right=spend-condition:t  form.pkh-right
  =/  lock=lock:t  [%2 [sc-left sc-right]]
  =/  proof-left=lock-merkle-proof:t
    (build-lock-merkle-proof:lock:t lock 1)
  =/  proof-right=lock-merkle-proof:t
    (build-lock-merkle-proof:lock:t lock 2)
  =/  lock-root=hash:t  (hash:lock:t lock)
  =/  parent-firstname=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root])
  =/  left-ok=?  (check:lock-merkle-proof:t proof-left parent-firstname)
  =/  right-ok=?  (check:lock-merkle-proof:t proof-right parent-firstname)
  %+  expect-eq
    !>  [%.y %.y]
  !>  [left-ok right-ok]
::
++  test-lock-merkle-proof-fake
  =/  pkh-left  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-right  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  sc-left=spend-condition:t  form.pkh-left
  =/  sc-right=spend-condition:t  form.pkh-right
  =/  lock=lock:t  [%2 [sc-left sc-right]]
  =/  proof-left=lock-merkle-proof-full:v1:t
    (build-lock-merkle-proof-full:lock:t lock 1)
  =/  lock-root=hash:t  (hash:lock:t lock)
  =/  parent-firstname=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root])
  =/  valid=?  (check:lock-merkle-proof:t proof-left parent-firstname)
  =/  wrong-leaf=lock-merkle-proof-full:v1:t
    proof-left(spend-condition sc-right)
  =/  wrong-leaf-res=?
    (check:lock-merkle-proof:t wrong-leaf parent-firstname)
  ::  twiddle the first element of lock-root hash to produce fake root
  =/  fake-root=hash:t  lock-root(- (mix +2:lock-root +6:lock-root))
  =/  forged-proof=lock-merkle-proof-full:v1:t
    proof-left(merk-proof merk-proof.proof-left(root fake-root))
  =/  forged-res=?
    (check:lock-merkle-proof:t forged-proof parent-firstname)
  %+  expect-eq
    !>  [%.y %.n %.n]
  !>  [valid wrong-leaf-res forged-res]
::
++  test-lock-merkle-proof-arity-four
  =/  pkh-one  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-two  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  pkh-three  (make-pkh-from-sig:spend-condition:t p:default-keys-3:h)
  =/  hax-four  (make-hax:spend-condition:t 42)
  =/  sc-one=spend-condition:t  form.pkh-one
  =/  sc-two=spend-condition:t  form.pkh-two
  =/  sc-three=spend-condition:t  form.pkh-three
  =/  sc-four=spend-condition:t  form.hax-four
  =/  lock=lock:t  (from-list:lock:t ~[sc-one sc-two sc-three sc-four])
  =/  lock-root=hash:t  (hash:lock:t lock)
  =/  parent-firstname=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root])
  =/  res-one=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 1) parent-firstname)
  =/  res-two=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 2) parent-firstname)
  =/  res-three=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 3) parent-firstname)
  =/  res-four=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 4) parent-firstname)
  %+  expect-eq
    !>  [%.y %.y %.y %.y]
  !>  [res-one res-two res-three res-four]
::
++  test-lock-merkle-proof-arity-eight
  =/  pkh-one  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-two  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  pkh-three  (make-pkh-from-sig:spend-condition:t p:default-keys-3:h)
  =/  hax-four  (make-hax:spend-condition:t 100)
  =/  hax-five  (make-hax:spend-condition:t 101)
  =/  hax-six  (make-hax:spend-condition:t 102)
  =/  hax-seven  (make-hax:spend-condition:t 103)
  =/  hax-eight  (make-hax:spend-condition:t 104)
  =/  sc-one=spend-condition:t  form.pkh-one
  =/  sc-two=spend-condition:t  form.pkh-two
  =/  sc-three=spend-condition:t  form.pkh-three
  =/  sc-four=spend-condition:t  form.hax-four
  =/  sc-five=spend-condition:t  form.hax-five
  =/  sc-six=spend-condition:t  form.hax-six
  =/  sc-seven=spend-condition:t  form.hax-seven
  =/  sc-eight=spend-condition:t  form.hax-eight
  =/  lock=lock:t
    %-  from-list:lock:t
    ~[sc-one sc-two sc-three sc-four sc-five sc-six sc-seven sc-eight]
  =/  lock-root=hash:t  (hash:lock:t lock)
  =/  parent-firstname=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root])
  =/  res-one=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 1) parent-firstname)
  =/  res-two=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 2) parent-firstname)
  =/  res-three=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 3) parent-firstname)
  =/  res-four=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 4) parent-firstname)
  =/  res-five=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 5) parent-firstname)
  =/  res-six=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 6) parent-firstname)
  =/  res-seven=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 7) parent-firstname)
  =/  res-eight=?
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock 8) parent-firstname)
  %+  expect-eq
    !>  [%.y %.y %.y %.y %.y %.y %.y %.y]
  !>  [res-one res-two res-three res-four res-five res-six res-seven res-eight]
::
++  test-lock-merkle-proof-arity-sixteen
  =/  pkh-one  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-two  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  pkh-three  (make-pkh-from-sig:spend-condition:t p:default-keys-3:h)
  =/  hax-four  (make-hax:spend-condition:t 200)
  =/  hax-five  (make-hax:spend-condition:t 201)
  =/  hax-six  (make-hax:spend-condition:t 202)
  =/  hax-seven  (make-hax:spend-condition:t 203)
  =/  hax-eight  (make-hax:spend-condition:t 204)
  =/  hax-nine  (make-hax:spend-condition:t 205)
  =/  hax-ten  (make-hax:spend-condition:t 206)
  =/  hax-eleven  (make-hax:spend-condition:t 207)
  =/  hax-twelve  (make-hax:spend-condition:t 208)
  =/  hax-thirteen  (make-hax:spend-condition:t 209)
  =/  hax-fourteen  (make-hax:spend-condition:t 210)
  =/  hax-fifteen  (make-hax:spend-condition:t 211)
  =/  hax-sixteen  (make-hax:spend-condition:t 212)
  =/  scs=(list spend-condition:t)
    :~  form.pkh-one
        form.pkh-two
        form.pkh-three
        form.hax-four
        form.hax-five
        form.hax-six
        form.hax-seven
        form.hax-eight
        form.hax-nine
        form.hax-ten
        form.hax-eleven
        form.hax-twelve
        form.hax-thirteen
        form.hax-fourteen
        form.hax-fifteen
        form.hax-sixteen
    ==
  =/  lock=lock:t  (from-list:lock:t scs)
  =/  lock-root=hash:t  (hash:lock:t lock)
  =/  parent-firstname=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root])
  =/  indices=(list @)  (gulf 1 16)
  =/  results=(list ?)
    %+  turn  indices
    |=  idx=@
    (check:lock-merkle-proof:t (build-lock-merkle-proof:lock:t lock idx) parent-firstname)
  %+  expect-eq
    !>  (reap 16 %.y)
  !>  results
::
++  test-lock-merkle-proof-six
  =/  pkh-one  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-two  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  pkh-three  (make-pkh-from-sig:spend-condition:t p:default-keys-3:h)
  =/  hax-four  (make-hax:spend-condition:t 300)
  =/  hax-five  (make-hax:spend-condition:t 301)
  =/  hax-six  (make-hax:spend-condition:t 302)
  =/  filler=spend-condition:t  ~[[%brn ~]]
  =/  scs=(list spend-condition:t)
    :~  form.pkh-one
        form.pkh-two
        form.pkh-three
        form.hax-four
        form.hax-five
        form.hax-six
    ==
  =/  lock=lock:t  (from-list:lock:t scs)
  =/  lock-root=hash:t  (hash:lock:t lock)
  =/  parent-firstname=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root])
  =/  proof-indices=(list @)  (gulf 1 8)
  =/  proofs=(list lock-merkle-proof:t)
    %+  turn  proof-indices
    |=  idx=@
    (build-lock-merkle-proof:lock:t lock idx)
  =/  results=(list ?)
    %+  turn  proofs
    |=  lmp=lock-merkle-proof:t
    (check:lock-merkle-proof:t lmp parent-firstname)
  =/  padded=(list ?)
    %+  turn  (slag 6 proofs)
    |=  lmp=lock-merkle-proof:t
    =/  sc=spend-condition:t
      ?:  ?=([%full * * *] lmp)
        =+  [ver sc ax mp]=lmp
        sc
      =+  [sc ax mp]=lmp
      sc
    =(sc filler)
  ;:  weld
    %+  expect-eq
      !>  (reap 8 %.y)
     !>  results
    %+  expect-eq
      !>  (reap 2 %.y)
     !>  padded
  ==
::
::  Version-specific tests for stub/full lock-merkle-proof
::
++  test-lock-merkle-proof-stub-axis-must-be-one
  ::  stub proofs must have axis=1, reject axis>1
  ::  For a 2-branch lock, even leaf-number=1 has axis>1 in the merkle tree
  =/  pkh-left  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-right  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  sc-left=spend-condition:t  form.pkh-left
  =/  sc-right=spend-condition:t  form.pkh-right
  =/  lock-multi=lock:t  [%2 [sc-left sc-right]]
  =/  lock-root-multi=hash:t  (hash:lock:t lock-multi)
  =/  parent-firstname-multi=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root-multi])
  ::  build a stub proof for a multi-branch lock - axis will be >1
  =/  proof-stub-multi=lock-merkle-proof-stub:v1:t
    (build-lock-merkle-proof-stub:lock:t lock-multi 1)
  ::  stub check should reject because axis>1 for multi-branch locks
  =/  stub-multi-rejected=?
    !(check:lock-merkle-proof-stub:v1:t proof-stub-multi parent-firstname-multi)
  ::  For a single spend-condition lock, axis IS 1
  =/  lock-single=lock:t  sc-left
  =/  lock-root-single=hash:t  (hash:lock:t lock-single)
  =/  parent-firstname-single=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root-single])
  =/  proof-stub-single=lock-merkle-proof-stub:v1:t
    (build-lock-merkle-proof-stub:lock:t lock-single 1)
  ::  stub check should accept axis=1 for single spend-condition locks
  =/  stub-single-accepted=?
    (check:lock-merkle-proof-stub:v1:t proof-stub-single parent-firstname-single)
  %+  expect-eq
    !>  [%.y %.y]
  !>  [stub-multi-rejected stub-single-accepted]
::
++  test-lock-merkle-proof-stub-full-hash-divergence
  ::  stub and full must produce DIFFERENT hashes for the same underlying data
  ::  This is critical: if they were the same, old proofs could be replayed
  =/  pkh  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  sc=spend-condition:t  form.pkh
  =/  lock=lock:t  sc  ::  single spend-condition lock
  ::  build both versions
  =/  proof-stub=lock-merkle-proof-stub:v1:t
    (build-lock-merkle-proof-stub:lock:t lock 1)
  =/  proof-full=lock-merkle-proof-full:v1:t
    (build-lock-merkle-proof-full:lock:t lock 1)
  ::  hash both
  =/  hash-stub=hash:t  (hash:lock-merkle-proof-stub:v1:t proof-stub)
  =/  hash-full=hash:t  (hash:lock-merkle-proof-full:v1:t proof-full)
  ::  they MUST be different
  =/  hashes-differ=?  !=(hash-stub hash-full)
  ::  sanity: same version hashes itself consistently
  =/  stub-deterministic=?  =(hash-stub (hash:lock-merkle-proof-stub:v1:t proof-stub))
  =/  full-deterministic=?  =(hash-full (hash:lock-merkle-proof-full:v1:t proof-full))
  %+  expect-eq
    !>  [%.y %.y %.y]
  !>  [hashes-differ stub-deterministic full-deterministic]
::
++  test-lock-merkle-proof-stub-backward-compat
  ::  raw stub data (3-tuple without %1 tag) must validate via wrapper
  =/  pkh  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  sc=spend-condition:t  form.pkh
  =/  lock=lock:t  sc
  =/  lock-root=hash:t  (hash:lock:t lock)
  =/  parent-firstname=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root])
  ::  build raw stub proof (3-tuple)
  =/  proof-stub=lock-merkle-proof-stub:v1:t
    (build-lock-merkle-proof-stub:lock:t lock 1)
  ::  cast to wrapper type and validate
  =/  proof-wrapper=lock-merkle-proof:t  proof-stub
  =/  wrapper-accepts-stub=?
    (check:lock-merkle-proof:t proof-wrapper parent-firstname)
  %+  expect-eq
    !>  %.y
  !>  wrapper-accepts-stub
::
++  test-lock-merkle-proof-wrapper-version-dispatch
  ::  verify wrapper correctly dispatches to stub vs full check logic
  =/  pkh-left  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  pkh-right  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  sc-left=spend-condition:t  form.pkh-left
  =/  sc-right=spend-condition:t  form.pkh-right
  ::  For stub tests, use single spend-condition lock (axis=1)
  =/  lock-single=lock:t  sc-left
  =/  lock-root-single=hash:t  (hash:lock:t lock-single)
  =/  parent-firstname-single=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root-single])
  ::  stub proof at axis=1 (single lock) should pass wrapper
  =/  proof-stub=lock-merkle-proof-stub:v1:t
    (build-lock-merkle-proof-stub:lock:t lock-single 1)
  =/  stub-via-wrapper=?
    (check:lock-merkle-proof:t proof-stub parent-firstname-single)
  ::  For full tests, use multi-branch lock to show axis>1 works
  =/  lock-multi=lock:t  [%2 [sc-left sc-right]]
  =/  lock-root-multi=hash:t  (hash:lock:t lock-multi)
  =/  parent-firstname-multi=hash:t
    (hash-hashable:tip5 [leaf+& hash+lock-root-multi])
  ::  full proof at a complete multi-branch condition should pass wrapper
  =/  proof-full=lock-merkle-proof-full:v1:t
    (build-lock-merkle-proof-full:lock:t lock-multi 2)
  =/  full-via-wrapper=?
    (check:lock-merkle-proof:t proof-full parent-firstname-multi)
  ::  stub proof from multi-branch lock should FAIL via wrapper (axis>1)
  =/  proof-stub-multi=lock-merkle-proof-stub:v1:t
    (build-lock-merkle-proof-stub:lock:t lock-multi 1)
  =/  stub-multi-via-wrapper=?
    (check:lock-merkle-proof:t proof-stub-multi parent-firstname-multi)
  %+  expect-eq
    !>  [%.y %.y %.n]
  !>  [stub-via-wrapper full-via-wrapper stub-multi-via-wrapper]
:::
::  Complete-condition admission uses the same rule at every height.
++  authorization-bc
  ^-  blockchain-constants:tx-engine
  %*  .  constants
    v1-phase  1
    bythos-phase  100
    base-fee  0
    input-fee-divisor  1
    data  [2.048 0]
  ==
++  tb  ~(. tx-engine authorization-bc)
++  hb  ~(. helpers authorization-bc)
++  open-condition
  ^-  spend-condition:tb
  ~[[%tim [[~ ~] [~ ~]]]]
:::
::  A real note, seed, and transaction, checked through the lock context,
::  aggregate spends and the same accumulator used by blocks and miners.
++  authorization-verdict
  |=  $:  lk=lock:tb
          lmp=lock-merkle-proof:tb
          height=@
          origin=@
          signed=?
          preimages=(z-map:zo hash:tb *)
      ==
  ^-  [static=? ctx=? spends=? acc=?]
  =/  k=blockchain-constants:tx-engine  authorization-bc
  =/  note=nnote-1:v1:tb
    %*  .  *nnote-1:v1:tb
      version      %1
      origin-page  origin
      name         (new-v1:nname:tb (hash:lock:tb lk) [*hash:tb %.n])
      note-data    *(z-map:zo @tas *)
      assets       1.000.000
    ==
  =/  sed=seed:v1:tb
    %^  simple:seed-v1:tb
      (hash:lock:tb (lock-from-sig:tb p:default-keys-2:hb))
      assets.note
    (hash:nnote:tb note)
  =/  wit=witness:tb  *witness:tb
  =/  sp=spend-1:v1:tb
    %*  .  *spend-1:v1:tb
      witness  wit(lmp lmp, hax preimages)
      seeds    (~(put z-in:zo *seeds:v1:tb) sed)
      fee      0
    ==
  =.  sp  ?:(signed (sign:spend-1:v1:tb sp s:default-keys-1:hb) sp)
  =/  sps=spends:tb  (~(put z-by:zo *spends:tb) name.note [%1 sp])
  =/  raw=raw-tx:v1:tb  (new:raw-tx:v1:tb sps)
  =/  ctx=check-context:tb
    [height origin (sig-hash:spend-1:v1:tb sp) witness.sp bythos-phase.k ~]
  =/  balance=(h-map:hz nname:tb nnote:tb)
    (~(put h-by:hz *(h-map:hz nname:tb nnote:tb)) name.note note)
  =/  valid=(reason:tb ~)
    %-  validate-with-context:spends:tb
    [balance sps height max-size.data.k bythos-phase.k]
  =/  acc=(reason:tb tx-acc:tb)
    (process:tx-acc:tb (new:tx-acc:tb `balance height) raw)
  :*  (validate:raw-tx:v1:tb raw)
      (check:check-context:tb ctx (lock-hash:nnote-1:v1:tb note))
      ?=(%.y -.valid)
      ?=(%.y -.acc)
  ==
:::
++  test-lock-merkle-proof-complete-leaf-shapes
  =/  results=(list ?)
    %+  turn  ~[1 2 4 8 16]
    |=  arity=@
    =/  lk=lock:tb  (from-list:lock:tb (reap arity open-condition))
    =/  first=hash:tb  (first:nname:tb (hash:lock:tb lk))
    %+  levy  (gulf 1 arity)
    |=  index=@
    =/  proof=lock-merkle-proof-full:v1:tb
      (build-lock-merkle-proof-full:lock:tb lk index)
    ?&  (check:lock-merkle-proof-full:v1:tb proof first)
        (check:lock-merkle-proof:tb proof first)
        =([%.y %.y %.y %.y] (authorization-verdict lk proof 100 0 %.n ~))
    ==
  %+  expect-eq  !>((reap 5 %.y))
  !>  results
:::
++  test-lock-merkle-proof-authorization-axis-and-tag
  ::  Sweep axes and arity tags independently of inclusion hashing.
  ::  An OR tag must agree with the complete-condition depth.
  =/  results=(list ?)
    %+  turn  ~[0 1 2 3 4 8 16 32]
    |=  tag=@
    =/  tag-hash=hash:tb  (hash-hashable:tip5 leaf+tag)
    %+  levy  (gulf 0 128)
    |=  axis=@
    =/  expected=?
      ?|  =(axis 1)
          ?&  ?|  =(tag 2)  =(tag 4)  =(tag 8)  =(tag 16)  ==
              (gte axis (mul 3 tag))
              (lth axis (mul 4 tag))
          ==
      ==
    =(expected (authorization-leaf:lock-merkle-proof-full:v1:tb axis ~[tag-hash]))
  ;:  weld
    (expect-eq !>((reap 8 %.y)) !>(results))
    (expect-eq !>(%.n) !>((authorization-leaf:lock-merkle-proof-full:v1:tb 6 ~)))
  ==
:::
++  test-lock-merkle-proof-tagged-tree-format
  =/  k=blockchain-constants:tx-engine  authorization-bc
  =/  leaf=hash:tb  (hash:spend-condition:tb open-condition)
  =/  results=(list ?)
    %+  turn  ~[2 4 8 16]
    |=  arity=@
    =/  lk=lock:tb  (from-list:lock:tb (reap arity open-condition))
    =/  h=hashable:tip5  (hashable:lock:tb lk)
    ?>  ?=(^ -.h)
    %+  levy  (homo ~[0 1 2 3 4 8 16 32])
    |=  tag=@
    =/  tree=hashable:tip5  [leaf+tag q.h]
    =/  sample=[h=hashable:tip5 idx=@]  [tree 2]
    =/  [axis=@ proof=merk-proof:merkle]
      %-  prove-hashable-by-index:merkle  sample
    =/  full=lock-merkle-proof-full:v1:tb
      [%full open-condition axis proof]
    =/  root=hash:tb  root.proof
    =/  first=hash:tb  (first:nname:tb root)
    =/  wit=witness:tb  *witness:tb
    =/  ctx=check-context:tb
      [100 0 *hash:tb wit(lmp full) bythos-phase.k ~]
    ?&  (verify-merk-proof:merkle leaf axis proof)
        =(=(tag arity) (check:lock-merkle-proof-full:v1:tb full first))
        =(=(tag arity) (check:lock-merkle-proof:tb full first))
        =(=(tag arity) (check:check-context:tb ctx first))
    ==
  %+  expect-eq  !>((reap 4 %.y))
  !>  results
:::
++  test-lock-merkle-proof-malformed-complete-proofs
  =/  lk=lock:tb  (from-list:lock:tb (reap 4 open-condition))
  =/  first=hash:tb  (first:nname:tb (hash:lock:tb lk))
  =/  proof=lock-merkle-proof-full:v1:tb
    (build-lock-merkle-proof-full:lock:tb lk 2)
  =/  path=(list hash:tb)  path.merk-proof.proof
  =/  wrong-tag=hash:tb  (hash-hashable:tip5 leaf+2)
  =/  bad=(list lock-merkle-proof-full:v1:tb)
    :~  proof(axis 0)
        proof(axis 1)
        proof(axis 6)
        proof(axis 16)
        proof(merk-proof merk-proof.proof(path (slag 1 path)))
        proof(merk-proof merk-proof.proof(path [wrong-tag path]))
        proof(merk-proof merk-proof.proof(path (weld (scag (dec (lent path)) path) ~[wrong-tag])))
        proof(merk-proof merk-proof.proof(root wrong-tag))
    ==
  =/  results=(list ?)
    %+  turn  bad
    |=  invalid=lock-merkle-proof-full:v1:tb
    ?&  !(check:lock-merkle-proof-full:v1:tb invalid first)
        !(check:lock-merkle-proof:tb invalid first)
        =([%.y %.n %.n %.n] (authorization-verdict lk invalid 100 0 %.n ~))
    ==
  %+  expect-eq  !>((reap 8 %.y))
  !>  results
:::
++  test-lock-merkle-proof-context-height-independence
  =/  lk=lock:tb  (from-list:lock:tb (reap 4 open-condition))
  =/  proof=lock-merkle-proof-full:v1:tb
    (build-lock-merkle-proof-full:lock:tb lk 2)
  =/  results=(list ?)
    %+  turn  ~[100 169.999 170.000 170.001 169.999]
    |=  height=@
    ?&  =([%.y %.y %.y %.y] (authorization-verdict lk proof height 0 %.n ~))
        =([%.y %.y %.y %.y] (authorization-verdict lk proof height 99 %.n ~))
        =([%.y %.n %.n %.n] (authorization-verdict lk proof(axis 6) height 0 %.n ~))
    ==
  %+  expect-eq  !>((reap 5 %.y))
  !>  results
:::
++  test-lock-merkle-proof-complete-primitive-enforcement
  =/  pkh  (make-pkh-from-sig:spend-condition:tb p:default-keys-1:hb)
  =/  hax  (make-hax:spend-condition:tb 42)
  =/  timing=lock-primitive:tb  [%tim [[`200 ~] [~ ~]]]
  =/  sc=spend-condition:tb  (weld form.pkh [timing form.hax])
  =/  lk=lock:tb  (from-list:lock:tb (reap 4 sc))
  =/  proof=lock-merkle-proof-full:v1:tb
    (build-lock-merkle-proof-full:lock:tb lk 1)
  =/  preimages=(z-map:zo hash:tb *)  (~(put z-by:zo *(z-map:zo hash:tb *)) h.hax 42)
  =/  wrong=(z-map:zo hash:tb *)  (~(put z-by:zo *(z-map:zo hash:tb *)) h.hax 43)
  ;:  weld
    (expect-eq !>([%.y %.y %.y %.y]) !>((authorization-verdict lk proof 200 0 %.y preimages)))
    (expect-eq !>([%.y %.n %.n %.n]) !>((authorization-verdict lk proof 199 0 %.y preimages)))
    (expect-eq !>([%.y %.n %.n %.n]) !>((authorization-verdict lk proof 200 0 %.n preimages)))
    (expect-eq !>([%.y %.n %.n %.n]) !>((authorization-verdict lk proof 200 0 %.y ~)))
    (expect-eq !>([%.y %.n %.n %.n]) !>((authorization-verdict lk proof 200 0 %.y wrong)))
  ==
:::
++  test-lock-merkle-proof-complete-empty-and-burn
  ::  Empty conditions never authorize, even as included leaves or whole locks.
  ::  Burn padding is a genuine leaf but never authorizes a spend.
  =/  burn=spend-condition:tb  ~[[%brn ~]]
  =/  lk=lock:tb  [%2 ~ burn]
  =/  root=hash:tb  (hash:lock:tb lk)
  =/  tag=hash:tb  (hash-hashable:tip5 leaf+2)
  =/  empty=lock-merkle-proof-full:v1:tb
    [%full ~ 6 [root ~[(hash:spend-condition:tb burn) tag]]]
  =/  padding=lock-merkle-proof-full:v1:tb
    [%full burn 7 [root ~[(hash:spend-condition:tb ~) tag]]]
  =/  first=hash:tb  (first:nname:tb root)
  =/  whole-root=hash:tb  (hash:spend-condition:tb ~)
  =/  whole-first=hash:tb  (first:nname:tb whole-root)
  =/  whole-full=lock-merkle-proof-full:v1:tb  [%full ~ 1 [whole-root ~]]
  =/  whole-stub=lock-merkle-proof-stub:v1:tb  [~ 1 [whole-root ~]]
  ;:  weld
    (expect-eq !>(%.y) !>((verify-merk-proof:merkle whole-root 1 merk-proof.whole-full)))
    (expect-eq !>(%.n) !>((check:lock-merkle-proof-full:v1:tb empty first)))
    (expect-eq !>(%.n) !>((check:lock-merkle-proof:tb empty first)))
    (expect-eq !>(%.y) !>((check:lock-merkle-proof-full:v1:tb padding first)))
    (expect-eq !>([%.y %.n %.n %.n]) !>((authorization-verdict lk empty 100 0 %.n ~)))
    (expect-eq !>([%.y %.n %.n %.n]) !>((authorization-verdict lk padding 100 0 %.n ~)))
    (expect-eq !>(%.n) !>((check:lock-merkle-proof-full:v1:tb whole-full whole-first)))
    (expect-eq !>(%.n) !>((check:lock-merkle-proof:tb whole-full whole-first)))
    (expect-eq !>(%.n) !>((check:lock-merkle-proof-stub:v1:tb whole-stub whole-first)))
    (expect-eq !>(%.n) !>((check:lock-merkle-proof:tb whole-stub whole-first)))
  ==
:::
++  test-lock-merkle-proof-single-form-context
  =/  lk=lock:tb  open-condition
  =/  full=lock-merkle-proof-full:v1:tb
    (build-lock-merkle-proof-full:lock:tb lk 1)
  =/  stub=lock-merkle-proof-stub:v1:tb
    (build-lock-merkle-proof-stub:lock:tb lk 1)
  ;:  weld
    (expect-eq !>([%.y %.n %.n %.n]) !>((authorization-verdict lk full 99 0 %.n ~)))
    (expect-eq !>([%.y %.y %.y %.y]) !>((authorization-verdict lk full 100 0 %.n ~)))
    (expect-eq !>([%.y %.y %.y %.y]) !>((authorization-verdict lk stub 99 0 %.n ~)))
    (expect-eq !>([%.y %.y %.y %.y]) !>((authorization-verdict lk stub 100 0 %.n ~)))
  ==
::::
++  test-lock-merkle-proof-ordinary-maturity-phase-boundary
  =/  h=@  (need consensus-batch-phase:page:tb)
  =/  pkh  (make-pkh-from-sig:spend-condition:tb p:default-keys-1:hb)
  =/  timing=lock-primitive:tb  [%tim [[`100 ~] [~ ~]]]
  =/  lk=lock:tb  (weld form.pkh ~[timing])
  =/  proof=lock-merkle-proof-stub:v1:tb
    (build-lock-merkle-proof-stub:lock:tb lk 1)
  =/  results=(list ?)
    %+  turn  ~[(dec h) h +(h) (dec h)]
    |=  height=@
    ?&  =([%.y %.n %.n %.n] (authorization-verdict lk proof height (sub height 99) %.y ~))
        =([%.y %.y %.y %.y] (authorization-verdict lk proof height (sub height 100) %.y ~))
        =([%.y %.n %.n %.n] (authorization-verdict lk proof height (sub height 100) %.n ~))
    ==
  %+  expect-eq  !>((reap 4 %.y))
  !>  results
::::
++  test-lock-merkle-proof-ordinary-pkh-phase-boundary
  =/  h=@  (need consensus-batch-phase:page:tb)
  =/  pkh  (make-pkh-from-sig:spend-condition:tb p:default-keys-1:hb)
  =/  lk=lock:tb  form.pkh
  =/  proof=lock-merkle-proof-stub:v1:tb
    (build-lock-merkle-proof-stub:lock:tb lk 1)
  =/  results=(list ?)
    %+  turn  ~[(dec h) h +(h) (dec h)]
    |=  height=@
    ?&  =([%.y %.y %.y %.y] (authorization-verdict lk proof height height %.y ~))
        =([%.y %.n %.n %.n] (authorization-verdict lk proof height height %.n ~))
    ==
  %+  expect-eq  !>((reap 4 %.y))
  !>  results
::::
::  These fixtures test admission shape, not transaction hashes or authorization.
++  stub-axis-shape-raw
  |=  sps=spends:v1:tb
  ^-  raw-tx:v1:tb
  %*  .  *raw-tx:v1:tb
    spends  sps
  ==
++  stub-axis-boundary-case
  |=  axis=@
  ^-  ?
  =/  stub=lock-merkle-proof-stub:v1:tb
    (build-lock-merkle-proof-stub:lock:tb open-condition 1)
  =/  proof=lock-merkle-proof-stub:v1:tb  stub(axis axis)
  ?>  ?=(^ spend-condition.proof)
  ?>  (based:lock-merkle-proof-stub:v1:tb proof)
  =/  ordinary=nname:tb  [*hash:tb *hash:tb ~]
  =/  fund=nname:tb  [fund-note-firstname:tb *hash:tb ~]
  ?>  !=(-.ordinary -.fund)
  =/  wit=witness:tb  *witness:tb
  =/  wit=witness:tb  wit(lmp proof)
  =/  sp=spend-1:v1:tb  *spend-1:v1:tb
  =/  sp=spend-1:v1:tb  sp(witness wit)
  =/  ordinary-raw=raw-tx:v1:tb
    (stub-axis-shape-raw (~(put z-by:zo *spends:v1:tb) ordinary [%1 sp]))
  =/  fund-raw=raw-tx:v1:tb
    (stub-axis-shape-raw (~(put z-by:zo *spends:v1:tb) fund [%1 sp]))
  ?&  =(=(axis 1) (valid-stub-axes:spends:v1:tb spends.ordinary-raw))
      =(=(axis 1) (valid-stub-axes:raw-tx:tb ordinary-raw))
      (valid-stub-axes:spends:v1:tb spends.fund-raw)
      (valid-stub-axes:raw-tx:tb fund-raw)
  ==
++  test-stub-axis-admission-fund-and-ordinary-boundaries
  =/  results=(list ?)
    :~  (stub-axis-boundary-case 0)
        (stub-axis-boundary-case 1)
        (stub-axis-boundary-case 7)
        (stub-axis-boundary-case 0xffff.ffff.0000.0000)
    ==
  (expect-eq !>((reap 4 %.y)) !>(results))
::::
++  test-stub-axis-admission-mixed-spends-and-versions
  =/  stub=lock-merkle-proof-stub:v1:tb
    (build-lock-merkle-proof-stub:lock:tb open-condition 1)
  ?>  ?=(^ spend-condition.stub)
  =/  wit=witness:tb  *witness:tb
  =/  sp=spend-1:v1:tb  *spend-1:v1:tb
  =/  good-wit=witness:tb  wit(lmp stub)
  =/  bad-wit=witness:tb  wit(lmp stub(axis 7))
  =/  good=spend-1:v1:tb  sp(witness good-wit)
  =/  bad=spend-1:v1:tb  sp(witness bad-wit)
  =/  ordinary=nname:tb  [*hash:tb *hash:tb ~]
  =/  other=nname:tb  [*hash:tb (hash:lock:tb open-condition) ~]
  =/  fund=nname:tb  [fund-note-firstname:tb *hash:tb ~]
  ?>  !=(ordinary other)
  =/  sps=spends:v1:tb
    (~(gas z-by:zo *spends:v1:tb) ~[[ordinary [%1 good]] [fund [%1 bad]]])
  ?>  (valid-stub-axes:raw-tx:tb (stub-axis-shape-raw sps))
  =/  mixed=spends:v1:tb  (~(put z-by:zo sps) other [%1 bad])
  ?>  !(valid-stub-axes:spends:v1:tb mixed)
  ?>  !(valid-stub-axes:raw-tx:tb (stub-axis-shape-raw mixed))
  =/  legacy=raw-tx:v1:tb
    (stub-axis-shape-raw (~(put z-by:zo *spends:v1:tb) ordinary [%0 *spend-0:v1:tb]))
  =/  full=lock-merkle-proof-full:v1:tb
    (build-lock-merkle-proof-full:lock:tb (from-list:lock:tb (reap 2 open-condition)) 2)
  ?>  (gth axis.full 1)
  =/  full-sp=spend-1:v1:tb  sp(witness wit(lmp full))
  ;:  weld
    (expect-eq !>(%.y) !>((valid-stub-axes:raw-tx:tb legacy)))
    (expect-eq !>(%.y) !>((valid-stub-axes:raw-tx:tb *raw-tx:v0:tb)))
    (expect-eq !>(%.y) !>((valid-stub-axes:raw-tx:tb (stub-axis-shape-raw (~(put z-by:zo *spends:v1:tb) ordinary [%1 full-sp])))))
    (expect-eq !>([%.y %.y %.y %.y]) !>((authorization-verdict (from-list:lock:tb (reap 2 open-condition)) full 100 0 %.n ~)))
  ==
--
