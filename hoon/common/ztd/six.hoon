/=  ztd-five  /common/ztd/five
=>  ztd-five
~%  %fri  ..proof-stream  ~
::    fri
|%
+$  fri-input
  $:  offset=belt
      omega=belt
      init-domain-len=@
      expansion-fac=@
      num-spot-checks=@
      folding-deg=@
  ==
::
++  compute-eval-domain
  ~/  %compute-eval-domain
  |=  [domain-len=@ omega=belt offset=@]
  ^-  (list belt)
  ~+
  =-  (flop acc)
  %+  roll  (range domain-len)
  |=  [i=@ acc=(list felt) omega-pow=_1]
  :_  (bmul omega-pow omega)
  [(bmul offset omega-pow) acc]
::
++  fri-door
  =,  merkle
  =,  proof-stream
  ~/  %fri-door
  =|  fri-input
  |%
  ++  log-folding-deg
    ~+
    ^-  @
    (dec (xeb folding-deg))
  ::
  ++  last-codeword-len
    ~+
    ^-  @
    (div init-domain-len (pow folding-deg num-rounds))
  ::
  ++  eval-domain
    ~+
    ^-  (list belt)
    (compute-eval-domain init-domain-len omega offset)
  ::
  ++  num-rounds
    ~+
    ^-  @
    =/  len  init-domain-len
    =/  num  0
    |-
    ?:  &((gth len expansion-fac) (lth (mul 4 num-spot-checks) len))
      $(num +(num), len (div len folding-deg))
    (max 1 (dec num))
  ::
  ::
  ++  prove
    ~/  %prove
    |=  [codeword=fpoly stream=proof]
    ^-  [fri-indices=(list @) stream=proof]
    |^
    ::  commit phase
    =^  codewords=(list codeword-data)  stream
      (commit codeword stream)
    ::
    ::  query phase
    (query codewords stream)
    ::
    ::
    +$  codeword-data  [codeword=mary merk=(unit [depth=@ heap=merk-heap])]
    ::
    ++  query
      |=  [codewords=(list codeword-data) stream=proof]
      ^-  [fri-indices=(list @) stream=proof]
      ::
      ::  Get random indices from the verifier to spot-check the folding
      =/  rng  ~(prover-fiat-shamir proof-stream stream)
      =^  fri-indices=(list @)  rng
        %-  indices:rng
        :+  num-spot-checks
          init-domain-len
        last-codeword-len
      ::
      =-  [fri-indices stream]
      %^  zip-roll  (range num-rounds)  codewords
      |=  [[round=@ data=codeword-data] indices=_fri-indices stream=_stream]
      =/  len  len.array:(~(change-step ave codeword.data) 3)
      =-  [(flop new-indices) stream]
      %+  roll  indices
      |=  [idx=@ new-indices=(list @) stream=_stream]
      ::
      =/  coset-idx  (mod idx (div len folding-deg))
      =/  merk  (need merk.data)
      =/  axis  (index-to-axis depth.merk coset-idx)
      ::  Compute merkle opening to idx in codeword and send to the verifier
      =/  leaf=fpoly
        (~(snag-as-fpoly ave codeword.data) coset-idx)
      =/  opening=merk-proof:merkle
        (build-merk-proof:merkle heap.merk axis)
      :-  [coset-idx new-indices]
      (~(push proof-stream stream) [%m-path leaf path.opening])
    ::
    ++  commit
      |=  [codeword=fpoly stream=proof]
      ^-  [codewords=(list codeword-data) stream=proof]
      =-  [(flop codewords) stream]
      %+  roll  (range +(num-rounds))
      |=  [round=@ codeword=_codeword codewords=(list codeword-data) omega=_(lift omega) round-offset=_(lift offset) stream=_stream]
      ?:  =(round num-rounds)
        ::  If it's the last round, send the raw codeword to the verifier instead
        ::  of a merkle tree
          :*   zero-fpoly
               codewords
               (fpow omega folding-deg)
               (fpow round-offset folding-deg)
               (~(push proof-stream stream) [%codeword codeword])
          ==
      ::
      =/  num  (div len.codeword folding-deg)
      ::
      ::  sort codeword into cosets
      =/  cosets=mary
        %-  zing-fpolys
        %+  turn  (range num)
        |=  k=@
        %-  init-fpoly
        %+  turn  (range folding-deg)
        |=  i=@
        =/  idx  (add (mul i num) k)
        (~(snag fop codeword) idx)
      ::
      ::  send codeword (as cosets) to verifier
      =/  merk=(pair @ merk-heap:merkle)
        (build-merk-heap:merkle cosets)
      =.  stream
        (~(push proof-stream stream) [%m-root h.q.merk])
      ::
      ::  get challenge from verifier
      =/  rng  ~(prover-fiat-shamir proof-stream stream)
      =^  alpha=felt  rng  $:felt:rng
      ::
      ::  compute new codeword
      =/  new-codeword=fpoly
        %-  init-fpoly
        %+  turn  (range len.array.cosets)
        |=  i=@
        =/  coset=fpoly  (~(snag-as-fpoly ave cosets) i)
        =/  eval-point=felt  (fdiv alpha (fmul round-offset (fpow omega i)))
        ::=/  eval-point=felt  (fdiv alpha (fpow omega i))
        (fpeval (fp-ifft coset) eval-point)
      ::
      :*  new-codeword
          [[cosets (some merk)] codewords]
          (fpow omega folding-deg)
          (fpow round-offset folding-deg)
          stream
      ==
    --
  ::
  ++  verify
    ~/  %verify
    |=  [stream=proof root=noun-digest:tip5]
    ^-  [[top-level-indices=(list @) merks=(list merk-data) deep-cosets=(map @ fpoly) res=?] proof]
    =/  original-version=proof-version  version.stream
    ::
    ::  extract roots and alphas values from commit phase
    =^  [roots=(list noun-digest:tip5) alphas=(list felt)]  stream
      =/  roots=(list noun-digest:tip5)  [root]~
      =-  [[(flop roots) (flop alphas)] str]
      %+  roll  (range num-rounds)
      |=  [i=@ str=_stream roots=_roots alphas=(list felt)]
      ?:  =(i 0)
        :+  str  roots
        ^-  (list felt)
        :_  alphas
        =/  rng  ~(verifier-fiat-shamir proof-stream str)
        =^  felt  rng  $:felt:rng
        felt
      =^  root  str
        =^(r str ~(pull proof-stream str) ?>(?=(%m-root -.r) p.r^str))
      :+  str
        [root roots]
      ^-  (list felt)
      :_  alphas
      =/  rng  ~(verifier-fiat-shamir proof-stream str)
      =^  felt  rng  $:felt:rng
      felt
    ?>  =((lent roots) (lent alphas))
    ::
    ::  extract last codeword
    =^  last-codeword=fpoly  stream
      =^(c stream ~(pull proof-stream stream) ?>(?=(%codeword -.c) p.c^stream))
    ::
    ::  Verify that the last codeword is low degree
    ~|  "last codeword len is not correct"
    ?>  =(len.last-codeword last-codeword-len)
    ?>  ?|  !=(%3 original-version)
            ~(cank fop last-codeword)
        ==
    =/  poly  (fp-ifft last-codeword)
    =/  deg  (fdegree ~(to-poly fop poly))
    =/  degree-bound
      %+  div
        (div init-domain-len expansion-fac)
      (pow folding-deg num-rounds)
    ~|  "last codeword is not low degree"
    ?>  (lth deg degree-bound)
    ::
    ::  get indices
    =/  [top-level-indices=(list @) *]
      =/  rng  ~(verifier-fiat-shamir proof-stream stream)
      %-  indices:rng
      :+  num-spot-checks
        init-domain-len
      last-codeword-len
    ::
    ::  Read cosets out of first codeword
    =^  [indices=(list @) deep-cosets=(map @ fpoly) merks=(list merk-data)]  stream
      =-  [[(flop new-indices) cosets merks] stream]
      %+  roll  top-level-indices
      |=  [idx=@ [new-indices=(list @) cosets=(map @ fpoly) merks=(list merk-data)] stream=_stream]
      =/  coset-idx  (mod idx (div init-domain-len folding-deg))
      =/  depth  (xeb (div init-domain-len folding-deg))
      =/  axis  (index-to-axis depth coset-idx)
      ::  read opening for index from proof
      =^  opening=proof-path  stream
        =^(o stream ~(pull proof-stream stream) ?>(?=(%m-path -.o) p.o^stream))
      ?>  ?|  !=(%3 original-version)
              ?&  =(len.leaf.opening folding-deg)
                  ~(cank fop leaf.opening)
              ==
          ==
      ::
      :_  stream
      :+  [idx new-indices]
        (~(put by cosets) coset-idx leaf.opening)
      :-  [(hash-hashable:tip5 (hashable-fpoly:tip5 leaf.opening)) axis root path.opening]
      merks
    ::
    ::
    =-  [[top-level-indices merks deep-cosets %.y] stream]
    %+  roll  (range num-rounds)
    |=  $:  round=@
            roots=_(tail roots)
            alphas=_alphas
            prev-indices=_indices
            prev-cosets=_deep-cosets
            prev-len=_init-domain-len
            omega=_(lift omega)
            round-offset=_(lift offset)
            merks=_merks
            stream=_stream
        ==
    ::
    =/  new-len  (div prev-len folding-deg)
    ::  Read all cosets out of the next codeword
    =^  [indices=(list @) cosets=(map @ fpoly) merks=(list merk-data)]  stream
      ?:  =(+(round) num-rounds)
        :: if it's the last round, we use the codeword which was written into
        :: the proof in the clear
        [[~ ~ merks] stream]
      =/  root  (head roots)
      =-  [[(flop new-indices) cosets merks] stream]
      %+  roll  prev-indices
      |=  [prev-idx=@ new-indices=(list @) cosets=(map @ fpoly) merks=_merks stream=_stream]
      =/  new-idx  (mod prev-idx (div prev-len folding-deg))
      =/  coset-idx  (mod new-idx (div new-len folding-deg))
      =/  depth  (xeb (div new-len folding-deg))
      =/  axis  (index-to-axis depth coset-idx)
      ::  read opening for index from proof
      =^  opening=proof-path  stream
        =^(o stream ~(pull proof-stream stream) ?>(?=(%m-path -.o) p.o^stream))
      ?>  ?|  !=(%3 original-version)
              ?&  =(len.leaf.opening folding-deg)
                  ~(cank fop leaf.opening)
              ==
          ==
      ::
      :^    [new-idx new-indices]
          (~(put by cosets) coset-idx leaf.opening)
        :-  [(hash-hashable:tip5 (hashable-fpoly:tip5 leaf.opening)) axis root path.opening]
        merks
      stream
    ::
    ::
    ::  Check folds
    =/  alpha  (head alphas)
    =/  res=?
      %+  levy  prev-indices
      |=  prev-idx=@
      =/  prev-coset-idx  (mod prev-idx (div prev-len folding-deg))
      =/  folded-val=felt
        =/  coeffs=fpoly
          (fp-ifft (~(got by prev-cosets) prev-coset-idx))
        =/  eval-point=felt
          (fdiv alpha (fmul round-offset (fpow omega prev-coset-idx)))
        (fpeval coeffs eval-point)
      =/  new-coset-idx  (mod prev-coset-idx (div new-len folding-deg))
      =/  new-codeword-val=felt
        ?:  =(+(round) num-rounds)
          :: for the last round, just read the value directly out of the last codeword
          (~(snag fop last-codeword) prev-coset-idx)
        =/  entry  (div prev-coset-idx (div new-len folding-deg))
        =/  coset  (~(got by cosets) new-coset-idx)
        (~(snag fop coset) entry)
      =(folded-val new-codeword-val)
    ::
    ::  crash if folds were incorrect
    ?>  =(res %.y)
    ::
    :*  ?~(roots roots (tail roots))
        ?~(alphas alphas (tail alphas))
        indices
        cosets
        new-len
        (fpow omega folding-deg)
        (fpow round-offset folding-deg)
        merks
        stream
    ==
  --  ::fri-door
--  ::fri
