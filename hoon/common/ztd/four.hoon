/=  ztd-three  /common/ztd/three
=>  ztd-three
~%  %proof-lib  ..merkle  ~
::    proof-library
|%
+|  %sur-proof-stream
+$  noun-digests  (list noun-digest:tip5)
+$  proof-path  [leaf=fpoly path=noun-digests]
+$  proof-path-bf  [leaf=bpoly path=noun-digests]
::
+$  proof-data
  $%  [%m-root p=noun-digest:tip5]  :: merk-root
      [%puzzle commitment=noun-digest:tip5 nonce=noun-digest:tip5 len=@ p=*]
      [%codeword p=fpoly]
      [%terms p=bpoly]  :: terminals
      [%m-path p=proof-path]   ::  merk-path
      [%m-pathbf p=proof-path-bf]  ::  merk-path-bf
      [%comp-m p=noun-digest:tip5 num=@]  ::  composition-merk
      [%evals p=fpoly]  ::  evaluations
      [%heights p=(list @)]  ::  n, where 2^n is the number of rows
      [%poly p=bpoly]
  ==
::
+$  proof-objects  (list proof-data)
::
::  proof-version: discriminates proof shapes.
::    %0/%1/%2: ZK STARK proof variants (ZK PoW puzzle).
::    %3: AI PoW puzzle marker. Blocks no longer persist the AI
::        recursive certificate through this old proof-stream body;
::        tx-engine-1 defines the structured `pow-artifact` /
::        `%ai-pow` noun. version-to-puzzle-type in consensus.hoon
::        still maps %0/%1/%2 -> %dumb-zkpow, %3 -> %ai-pow.
+$  proof-version  ?(%3 %2 %1 %0)
+$  proof
  $%  $:  version=%2
          objects=proof-objects
          hashes=(list noun-digest:tip5)
          read-index=@
      ==
    ::
      $:  version=%1
          objects=proof-objects
          hashes=(list noun-digest:tip5)
          read-index=@
      ==
    ::
      $:  version=%0
          objects=proof-objects
          hashes=(list noun-digest:tip5)
          read-index=@
      ==
    ::
      ::  %3 is retained only as the legacy proof-version
      ::  discriminator. The canonical AI block artifact is
      ::  `[%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]`
      ::  via tx-engine-1 / dumbnet types, not this proof-stream arm.
      ::  ZK helpers crash on %3 via explicit ?= guards.
      $:  version=%3
          objects=proof-objects
          hashes=(list noun-digest:tip5)
          read-index=@
      ==
  ==
::
+$  tip5-hash-atom  @ux
::
::  number of items in proof used for pow
++  pow-items  7
::  extract pow from proof
++  get-pow
  ~/  %get-pow
  |=  p=proof
  ^-  proof
  ::  %3 (AI) is not a ZK proof-stream artifact. The ZK pow extraction
  ::  helper is meaningless for AI; callers that reach here for a %3
  ::  proof are calling the wrong helper.
  ?:  ?=(%3 -.p)  ~|(%get-pow-not-defined-for-v3 !!)
  p(objects (scag pow-items objects.p))
::
++  proof-to-pow
  ~/  %proof-to-pow
  |=  =proof
  ^-  tip5-hash-atom
  (digest-to-atom:tip5 (hash-proof (get-pow proof)))
::
++  hashable-proof-objects
  ~/  %hashable-proof-objects
  |=  ps=proof-objects
  ^-  hashable:tip5
  [%list (turn ps hashable-proof-data)]
::
++  hash-proof
  ~/  %hash-proof
  |=  p=proof
  ^-  noun-digest:tip5
  ?:  ?=(%3 -.p)  ~|(%hash-proof-not-defined-for-v3 !!)
  =/  rng  (absorb-proof-objects objects.p ~)
  =^  lis=(list belt)  rng  (belts:rng 5)
  =-  ?>  ?=(noun-digest:tip5 -)  -
  (list-to-tuple:tip5 lis)
::
++  absorb-proof-objects
  ~/  %absorb-proof-objects
  |=  [objs=proof-objects hashes=(list noun-digest:tip5)]
  ^+  tog:tip5
  =.  objs  (slag (lent hashes) objs)
  =/  lis-digests=[%list (list hashable:tip5)]
    =/  h  (hashable-noun-digests:tip5 hashes)
    ?>  ?=(%list -.h)
    h
  =/  lis-objects=[%list (list hashable:tip5)]
    =/  h  (hashable-proof-objects objs)
    ?>  ?=(%list -.h)
    h
  =/  big-lis=(list noun-digest:tip5)
    (turn `(list hashable:tip5)`(weld +.lis-digests +.lis-objects) hash-hashable:tip5)
  =/  sponge  (new:sponge:tip5)
  |-
  ?~  big-lis
    (new:tog:tip5 sponge:sponge)
  =+  [a=@ b=@ c=@ d=@ e=@]=i.big-lis
  =/  lis=(list belt)  [a b c d e ~]
  $(big-lis t.big-lis, sponge (absorb:sponge lis))
::
::
++  hashable-proof-data
  ~/  %hashable-proof-data
  |=  pd=proof-data
  ^-  hashable:tip5
  ?-    -.pd
    %m-root    [leaf+%m-root hash+p.pd]
    %puzzle    [leaf+%puzzle hash+commitment.pd hash+nonce.pd leaf+len.pd leaf+p.pd]
    %comp-m    [leaf+%comp-m hash+p.pd leaf+num.pd]
    %heights   [leaf+%heights leaf+p.pd]
    %codeword  [leaf+%codeword (hashable-fpoly:tip5 p.pd)]
    %evals     [leaf+%evals (hashable-fpoly:tip5 p.pd)]
    %terms     [leaf+%terms (hashable-bpoly:tip5 p.pd)]
    %poly      [leaf+%poly (hashable-bpoly:tip5 p.pd)]
  ::
      %m-pathbf
    :-  leaf+%m-pathbf
    :-  (hashable-bpoly:tip5 leaf.p.pd)
    (hashable-noun-digests:tip5 path.p.pd)
  ::
      %m-path
    :-  leaf+%m-mpath
    :-  (hashable-fpoly:tip5 leaf.p.pd)
    (hashable-noun-digests:tip5 path.p.pd)
  ==
::
++  hash-proof-data
  ~/  %hash-proof-data
  |=  pd=proof-data
  ^-  noun-digest:tip5
  (hash-hashable:tip5 (hashable-proof-data pd))
::
+|  %sur-fock
::
::  $zero-map: see description
::
::    Nock 10 edits the noun so it has subject for the original noun and new-subject for the
::    new edited noun. Nock 0 is proved exactly like a nock 10 but with new-subject=subject.
::    So when recording a nock 0 you want to just pass subject in for new-subject.
::    Basically a nock 0 is a special case of nock 10 where the edited tree is the original tree.
+$  zero-map  (map subject=* (map [axis=* new-subject=*] count=@))
+$  decode-map  (map [formula=* head=* tail=*] count=@)
+$  fock-return
  $+  fock-return
  $:  queue=(list *)
      zeroes=zero-map
      decodes=decode-map
      [s=* f=*]
      ::jutes=(list [@tas sam=* prod=*])
  ==
::  $dyck-stack: horner accumulated stack of dyck path
::  $dyck-felt: felt representing dyck-stack
::  $leaf-stack: horner accumulated stack of leaves
::  $leaf-felt: felt representing leaf-stack
::  $ion-fprint: compressed $ion-triple: a*len + b*dyck-felt + c*leaf-felt
::  $ion-triple: dyck encoding of a noun. called the ION fingerprint in the EDEN paper.
::  $compute-stack: horner accumulated stack of packed-tree-felts
::  $compute-felt: felt representing compute-stack
::  $tree-data:
::
::    .len: length of the leaf stack
+$  tree-data
  $~  [pone pzero pzero 0]
  $:  size=pelt   :: alf^len
      dyck=pelt
      leaf=pelt
      n=*
  ==
::
+$  pelt  $~(pzero @ux)
::
++  pzero  `pelt`(lift 0)
++  pone   `pelt`(lift 1)
::
++  pelt-lift
  ~/  %pelt-lift
  |=  b=belt
  ^-  pelt
  (lift b)
::
++  padd
  ~/  %padd
  |=  [p=pelt q=pelt]
  ^-  pelt
  (fadd p q)
::
++  pneg
  ~/  %pneg
  |=  p=pelt
  ^-  pelt
  (fneg p)
::
++  psub
  ~/  %psub
  |=  [p=pelt q=pelt]
  ^-  pelt
  (fsub p q)
::
++  pmul
  ~/  %pmul
  |=  [p=pelt q=pelt]
  ^-  pelt
  (fmul p q)
::
++  pscal
  ~/  %pscal
  |=  [c=belt p=pelt]
  ^-  pelt
  dat:(bpscal c [3 p])
::
::
++  pinv
  ~/  %pinv
  |=  p=pelt
  ^-  pelt
  (finv p)
::
::  +ppow: field power; computes x^n
++  ppow
  ~/  %ppow
    |=  [x=pelt n=@]
    (fpow x n)
::
++  print-pelt
  ~/  %print-pelt
  |=  [=pelt t=(list belt)]
  ^-  (list belt)
  :^    (~(snag bop [3 pelt]) 0)
      (~(snag bop [3 pelt]) 1)
    (~(snag bop [3 pelt]) 2)
  t
::
++  got-pelt
  ~/  %got-pelt
  |=  [mp=(map term belt) t=term]
  ^-  pelt
  =<  dat
  %-  init-bpoly
  :~  (~(got by mp) (crip (weld (trip t) "-a")))
      (~(got by mp) (crip (weld (trip t) "-b")))
      (~(got by mp) (crip (weld (trip t) "-c")))
  ==
::
++  compress-pelt
  ~/  %compress-pelt
  |=  [cs=(list pelt) ps=(list pelt)]
  ^-  pelt
  %+  roll  (zip-up cs ps)
  |=  [[c=pelt p=pelt] acc=pelt]
  ^-  pelt
  (padd acc (pmul c p))
::
+$  compute-queue  (list tree-data)
::
+|  %constants
++  ext-degree  3  ::  goldilocks field degree
--
