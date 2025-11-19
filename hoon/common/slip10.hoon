::  slip-10 implementation in hoon using the cheetah curve
::
::  to use, call one of the core initialization arms.
::  using the produced core, derive as needed and take out the data you want.
::
::  NOTE:  tested to be correct against the SLIP-10 spec
::   https://github.com/satoshilabs/slips/blob/master/slip-0010.md
::
/=  *  /common/zose
/=  *  /common/zeke
=,  hmac:crypto
=,  cheetah:zeke
=+  ecc=cheetah
::
::  prv:  private key
::  pub:  public key
::  cad:  chain code
::  dep:  depth in chain
::  ind:  index at depth
::  pif:  parent fingerprint (4 bytes)
::  ver:  protocol version (1 byte)
=>  |%
    +$  base  [prv=@ pub=a-pt:curve cad=@ dep=@ud ind=@ud pif=@ ver=@ud]
    --
|_  base
+$  base  ^base
::
+$  keyc  [key=@ cai=@ ver=@]  ::  prv/pub key + chain code + protocol version
::
::  elliptic curve operations and values
::
++  point  ch-scal:affine:curve
::
++  ser-p  ser-a-pt
::
++  n      g-order:curve
::
++  domain-separator  [14 'dees niahckcoN']
::
++  current-protocol  1
++  protocol-version  ver
::
::
::  rendering
::
++  private-key     ?.(=(0 prv) prv ~|(%know-no-private-key !!))
++  public-key      (ser-p pub)
++  chain-code      cad
::
++  identity        (hash160 public-key)
++  fingerprint     (cut 3 [16 4] identity)
::
++  hash160
  |=  d=@
  (ripemd-160:ripemd:crypto 32 (sha-256:sha d))
::
::  core initialization
::
++  from-seed
  |=  [byts version=@]
  ^+  +>
  =+  der=(hmac-sha512l domain-separator [wid dat])
  =/  [left=@ right=@]
    [(cut 3 [32 32] der) (cut 3 [0 32] der)]
  ::
  ::  In the case where the left is greater than or equal to the curve order,
  ::  We have an invalid key and will use the right digest to rehash until we
  ::  obtain a valid key. This prevents the distribution from being biased.
  |-
  ?:  (lth left n)
    +>.^$(prv left, pub (point left a-gen:curve), cad right, ver version)
  =/  der  (hmac-sha512l domain-separator 64^der)
  %=    $
    der  der
    left  (cut 3 [32 32] der)
    right  (cut 3 [0 32] der)
  ==
::
++  from-private
  |=  =keyc
  +>(prv key:keyc, pub (point key:keyc a-gen:curve), cad cai:keyc, ver ver:keyc)
::
++  from-public
  |=  =keyc
  +>(pub (de-a-pt key:keyc), cad cai:keyc, ver ver:keyc)
::
::  derivation arms: Only used for testing.
::
::    +derive-path
::
::  Given a bip32-style path, i.e "m/0'/25", derive the key associated
::  with that path.
::
++  derive-path
  |=  t=tape
  %-  derive-sequence
  (scan t derivation-path)
::
::    +derivation-path
::
::  Parses the bip32-style derivation path and return a list of indices
::
++  derivation-path
  ;~  pfix
    ;~(pose (jest 'm/') (easy ~))
  %+  most  fas
  ;~  pose
    %+  cook
      |=(i=@ (add i (bex 31)))
    ;~(sfix dem soq)
  ::
    dem
  ==  ==
::
::    +derive-sequence
::
::  Derives a key from a list of indices associated with a bip32-style path.
::
++  derive-sequence
  |=  j=(list @u)
  ?~  j  +>
  =.  +>  (derive i.j)
  $(j t.j)
::
::    +derive
::
::  Checks if prv has been set to 0, denoting a wallet which only
::  contains public keys. If prv=0, call derive-public otherwise
::  call derive-private.
::
++  derive
  ?:  =(0 prv)
    derive-public
  derive-private
::
::    +derive-private
::
::  derives the i-th child key from `prv`
::
++  derive-private
  |=  i=@u
  ^+  +>
  ::  we must have a private key to derive the next one
  ?:  =(0 prv)
    ~|  %know-no-private-key
    !!
  ::  derive child at i
  =/  [left=@ right=@]
    =-  [(cut 3 [32 32] -) (cut 3 [0 32] -)]
    %+  hmac-sha512l  [32 cad]
    ?:  (gte i (bex 31))
      ::  hardened child
      [37 (can 3 ~[4^i 32^prv 1^0])]
    ::  normal child
    [101 (can 3 ~[4^i 97^(ser-p (point prv a-gen:curve))])]
  =+  key=(mod (add left prv) n)
  ::
  ::  In the case where `left` is greater than or equal to the curve order,
  ::  or the key is the identity point, we have an invalid key and will
  ::  rehash `0x1 || right || i` to obtain a valid key. This prevents the
  ::  distribution from being biased.
  |-
  ?:  &(!=(0 key) (lth left n))
    %_  +>.^$
      prv   key
      pub   (point key a-gen:curve)
      cad   right
      dep   +(dep)
      ind   i
      pif   fingerprint
      ver   ver
    ==
  =/  [left=@ right=@]
    =-  [(cut 3 [32 32] -) (cut 3 [0 32] -)]
    %+  hmac-sha512l  [32 cad]
    [37 (can 3 ~[4^i 32^right 1^0x1])]
  %=    $
    left   left
    right  right
    key    (mod (add left prv) n)
  ==
::
::    +derive-public
::
::  derives the i-th child key from `pub`
++  derive-public
  |=  i=@u
  ^+  +>
  ::  public keys can't be hardened
  ?:  (gte i (bex 31))
    ~|  %cant-derive-hardened-public-key
    !!
  ::  derive child at i
  =/  [left=@ right=@]
    =-  [(cut 3 [32 32] -) (cut 3 [0 32] -)]
    %+  hmac-sha512l  [32 cad]
    101^(can 3 ~[4^i 97^(ser-p pub)])
  =+  key=(ch-add:affine:curve (point left a-gen:curve) pub)
  ::
  ::  In the case where `left` is greater than or equal to the curve order,
  ::  or the key is the identity point, we have an invalid key and will
  ::  rehash `0x1 || right || i` to obtain a valid key. This prevents the
  ::  distribution from being biased.
  |-
  ?:  &((lth left n) !=(a-id:curve key))
    %_  +>.^$
      pub   key
      cad   right
      dep   +(dep)
      ind   i
      pif   fingerprint
      ver   ver
    ==
  =/  [left=@ right=@]
    =-  [(cut 3 [32 32] -) (cut 3 [0 32] -)]
    %+  hmac-sha512l  [32 cad]
    [37 (can 3 ~[4^i 32^right 1^0x1])]
  %=    $
    left   left
    right  right
    key   (ch-add:affine:curve (point left a-gen:curve) pub)
  ==
::
::  extended key serialization
::
++  extended-private-key
  ^-  @t
  %-  crip
  %-  en:base58:wrap
  %-  add-checksum
  (serialize-extended %.y)
::
++  extended-public-key
  ^-  @t
  %-  crip
  %-  en:base58:wrap
  %-  add-checksum
  (serialize-extended %.n)
::
++  serialize-extended
  |=  include-private=?
  ^-  @
  =/  typ=@
    ?:  include-private
      0x110.6331  ::  produces "zprv" for 78-byte cheetah private keys
    0xc0e.bb09    ::  produces "zpub" for 142-byte cheetah public keys
  =/  key-data=@
    ?:  include-private
      ::  private key: 0x00 + 32-byte private key (33 bytes total)
      ::  this serves as a format indicator to distinguish between
      ::  private and public keys, and is inherited from the
      ::  bip32 spec to maintain format consistency
      (can 3 ~[[32 private-key] [1 0]])
    ::  public key: 97-byte cheetah curve point
    public-key
  =/  key-size=@  ?:(include-private 33 97)
  =/  total-size=@  (add key-size 46)  ::  key + 46 bytes metadata
  ::
  %+  can  3
  :~  [key-size key-data]
      [32 cad]
      [4 ind]
      [4 pif]
      [1 (mod dep 256)]
      [1 ver]
      [4 typ]
  ==
::
++  add-checksum
  |=  data=@
  ^-  @
  =/  data-size=@  (met 3 data)
  =/  hash1=@  (sha-256:sha data)
  =/  hash2=@  (sha-256:sha hash1)
  =/  checksum=@  (cut 3 [28 4] hash2)  ::  first 4 bytes
  (can 3 ~[[4 checksum] [data-size data]])
::
++  from-extended-key
  |=  key=@t
  ^+  +>
  =/  decoded=@
    (de:base58:wrap (trip key))
  ?>  (verify-checksum decoded)
  =/  total-size=@  (met 3 decoded)
  ::  remove the checksum
  =/  payload=@  (cut 3 [4 total-size] decoded)
  =/  typ=@  (cut 3 [0 4] key)
  =/  typ-text=@t  `@t`typ
  =/  is-private=?
    ?:  =(typ-text 'zprv')  %.y  ::  zprv
    ?:  =(typ-text 'zpub')  %.n  ::  zpub
    ~|("unsupported extended key type: {<typ>}" !!)
  =/  key-size=@  ?:(is-private 33 97)
  ::  check if protocol version byte exists (backward compatibility)
  =/  has-protocol-version=?
    ::  subtract the length of the checksum from total size
    (gte (sub total-size 4) (add key-size 46))
  =/  protocol-version=@ud
    ?:(has-protocol-version (cut 3 [(add key-size 41) 1] payload) 0)
  ::  metadata layout: [key-data][chain-code][index][parent-fp][depth][ver][typ]
  =/  depth=@ud  (cut 3 [(add key-size 40) 1] payload)
  =/  parent-fp=@  (cut 3 [(add key-size 36) 4] payload)
  =/  index=@ud  (cut 3 [(add key-size 32) 4] payload)
  =/  chain-code=@  (cut 3 [key-size 32] payload)
  =/  key-data=@  (cut 3 [0 key-size] payload)
  =/  private-key=@
    ?.  is-private  0
    (cut 3 [0 32] key-data)
  =/  public-key=a-pt:curve
    ?:  is-private
      (point private-key a-gen:curve)
    (de-a-pt key-data)
  %_  +>.$
      prv  private-key
      pub  public-key
      cad  chain-code
      dep  depth
      ind  index
      pif  parent-fp
      ver  protocol-version
  ==
::
++  verify-checksum
  |=  data=@
  ^-  ?
  =/  total-size=@  (met 3 data)
  =/  payload=@  (cut 3 [4 total-size] data)
  =/  provided-checksum=@  (cut 3 [0 4] data)
  =/  hash1=@  (sha-256:sha payload)
  =/  hash2=@  (sha-256:sha hash1)
  =/  computed-checksum=@  (cut 3 [28 4] hash2)
  =(provided-checksum computed-checksum)
--
::
|%
::
++  extended-from-keyc
  |=  [=keyc include-private=?]
  ^-  @t
  ?:  include-private
    extended-private-key:(from-private keyc)
  extended-public-key:(from-public keyc)
::
++  keyc-from-extended
  |=  key=@t
  ^-  keyc
  =/  core  (from-extended-key key)
  ?:  =(0 prv:core)
    [public-key:core chain-code:core ver:core]
  [private-key:core chain-code:core ver:core]
--
