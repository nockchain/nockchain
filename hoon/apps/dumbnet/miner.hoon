/=  mine  /common/pow
/=  sp  /common/stark/prover
/=  *  /common/zoon
/=  *  /common/zeke
/=  *  /common/wrapper
=<  ((moat |) inner)  :: wrapped kernel
=>
  |%
  ::  Outer envelope of a successful mine: `[%command %pow %dumb-zkpow ...]`.
  ::  The `%dumb-zkpow` tag selects this variant of the consensus-kernel's
  ::  `pow-variant` tagged union (see hoon/apps/dumbnet/lib/types.hoon).
  +$  mine-success
    $:  %command
        %pow
        %dumb-zkpow
        =proof
        dig=tip5-hash-atom
        header=noun-digest:tip5
        nonce=noun-digest:tip5
    ==
  +$  effect  [%mine-result (each [hash=noun-digest:tip5 mine-success] dig=noun-digest:tip5)]
  ::  Version %5 proofs exclude the nonce from the proven statement and
  ::  transcript. Keep one immutable suffix per candidate in each worker.
  +$  proof-cache  [header=noun-digest:tip5 pow-len=@ prf=proof:sp]
  +$  kernel-state  [%state version=%2 cache=(unit proof-cache)]
  +$  cause
    $%  [%0 header=noun-digest:tip5 nonce=noun-digest:tip5 target=bignum:bignum pow-len=@]
        [%1 header=noun-digest:tip5 nonce=noun-digest:tip5 target=bignum:bignum pow-len=@]
        [%2 header=noun-digest:tip5 nonce=noun-digest:tip5 target=bignum:bignum pow-len=@]
        [%3 header=noun-digest:tip5 nonce=noun-digest:tip5 target=bignum:bignum pow-len=@]
        [%5 header=noun-digest:tip5 nonce=noun-digest:tip5 target=bignum:bignum pow-len=@]
    ==
  --
|%
++  moat  (keep kernel-state) :: no state
++  inner
  |_  k=kernel-state
  ::  do-nothing load
  ++  load
    |=  =kernel-state  kernel-state
  ::  crash-only peek
  ++  peek
    |=  arg=*
    =/  pax  ((soft path) arg)
    ?~  pax  ~|(not-a-path+arg !!)
    ~|(invalid-peek+pax !!)
  ::  poke: try to prove a block
  ++  poke
    |=  [wir=wire eny=@ our=@ux now=@da dat=*]
    ^-  [(list effect) k=kernel-state]
    =/  cause  ((soft cause) dat)
    ?~  cause
      ~>  %slog.[1 'poke: Bad cause']
      `k
    =/  cause  u.cause
    =/  input=prover-input:sp
      ?-  -.cause
        %0  [%0 header.cause nonce.cause pow-len.cause]
        %1  [%1 header.cause nonce.cause pow-len.cause]
        %2  [%2 header.cause nonce.cause pow-len.cause]
        %3  [%3 header.cause nonce.cause pow-len.cause]
        %5  [%5 header.cause nonce.cause pow-len.cause]
      ==
    =/  reuse=?
      ?.  =(%5 -.cause)  %.n
      ?~  cache.k  %.n
      ?&  =(header.cause header.u.cache.k)
          =(pow-len.cause pow-len.u.cache.k)
      ==
    =/  [prf=proof:sp dig=tip5-hash-atom]
      ?:  reuse
        ?~  cache.k  !!
        =/  reused=proof:sp
          (set-v5-proof-nonce:mine prf.u.cache.k nonce.cause)
        [reused (proof-to-pow:sp reused)]
      (prove-block-inner:mine input)
    =/  next-cache=(unit proof-cache)
      ?:  =(%5 -.cause)
        (some [header.cause pow-len.cause prf])
      ~
    :_  k(cache next-cache)
    ?:  (check-target:mine dig target.cause)
      [%mine-result %& (atom-to-digest:tip5 dig) %command %pow %dumb-zkpow prf dig header.cause nonce.cause]~
    [%mine-result %| (atom-to-digest:tip5 dig)]~
  --
--
