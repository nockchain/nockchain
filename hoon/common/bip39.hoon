::  bip39 implementation in hoon
::
/=  bip39-english  /common/bip39-english
/=  *  /common/zose
::
|%
++  from-entropy
  |=  byts
  ^-  tape
  =.  wid  (mul wid 8)
  ~|  [%unsupported-entropy-bit-length wid]
  ?>  &((gte wid 128) (lte wid 256))
  ::
  =+  cs=(div wid 32)
  =/  check=@
    %+  rsh  [0 (sub 256 cs)]
    (sha-256l:sha (div wid 8) dat)
  =/  bits=byts
    :-  (add wid cs)
    %+  can  0
    :~  cs^check
        wid^dat
    ==
  ::
  =/  pieces
    |-  ^-  (list @)
    :-  (end [0 11] dat.bits)
    ?:  (lte wid.bits 11)  ~
    $(bits [(sub wid.bits 11) (rsh [0 11] dat.bits)])
  ::
  =/  words=(list tape)
    %+  turn  pieces
    |=  ind=@ud
    (snag ind `(list tape)`bip39-english)
  ::
  %+  roll  (flop words)
  |=  [nex=tape all=tape]
  ?~  all  nex
  :(weld all " " nex)
::
::  +split-words: split a tape on ASCII spaces, dropping empty tokens
++  split-words
  |=  t=tape
  ^-  (list tape)
  =|  res=(list tape)
  =|  cur=tape
  |-  ^-  (list tape)
  ?~  t
    %-  flop
    ?~(cur res [(flop cur) res])
  ?:  =(' ' i.t)
    $(t t.t, cur ~, res ?~(cur res [(flop cur) res]))
  $(t t.t, cur [i.t cur])
::
::  +from-mnemonic: validate a bip39 mnemonic against the english wordlist,
::  word count, and checksum (the inverse of +from-entropy). Produces the
::  recovered entropy as byts, or ~ if the mnemonic is not valid bip39.
++  word-index
  ::  index of a word in the wordlist, or ~ if absent
  |=  [w=tape lst=(list tape)]
  ^-  (unit @)
  =/  i=@  0
  |-  ^-  (unit @)
  ?~  lst  ~
  ?:  =(w i.lst)  `i
  $(lst t.lst, i +(i))
::
++  from-mnemonic
  |=  mnem=tape
  ^-  (unit byts)
  =/  words=(list tape)  (split-words mnem)
  =/  n=@  (lent words)
  ::  valid word counts are 12 15 18 21 24 (divisible by 3 in [12,24])
  ?.  ?&((gte n 12) (lte n 24) =(0 (mod n 3)))
    ~
  ::  map each word to its 11-bit wordlist index; reject any unknown word
  =/  maybe=(list (unit @))
    %+  turn  words
    |=  w=tape
    (word-index w bip39-english)
  ?:  (lien maybe |=(u=(unit @) ?=(~ u)))
    ~
  =/  indices=(list @)  (turn maybe need)
  ::  reassemble the bit string: the first word holds the highest 11 bits
  =/  full=@
    %+  roll  indices
    |=  [i=@ acc=@]
    (add (lsh [0 11] acc) i)
  =/  cs-bits=@   (div n 3)
  =/  ent-bits=@  (sub (mul n 11) cs-bits)
  =/  ent-byts=@  (div ent-bits 8)
  =/  entropy=@   (rsh [0 cs-bits] full)
  =/  check=@     (end [0 cs-bits] full)
  ::  checksum is the top cs-bits of sha-256 over the entropy bytes
  =/  computed=@  (rsh [0 (sub 256 cs-bits)] (sha-256l:sha ent-byts entropy))
  ?.  =(check computed)
    ~
  `[ent-byts entropy]
::
::NOTE  always produces a 512-bit result
++  to-seed
  |=  [mnem=tape pass=tape]
  ^-  @
  %-  hmac-sha512t:pbkdf:crypto
  [(crip mnem) (crip (weld "mnemonic" pass)) 2.048 64]
::
++  en-base58
  |=  dat=@
  =/  cha
    '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
  %-  flop
  |-  ^-  tape
  ?:  =(0 dat)  ~
  :-  (cut 3 [(mod dat 58) 1] cha)
  $(dat (div dat 58))
::
++  de-base58
      |=  t=tape
      =-  (scan t (bass 58 (plus -)))
      ;~  pose
        (cook |=(a=@ (sub a 56)) (shim 'A' 'H'))
        (cook |=(a=@ (sub a 57)) (shim 'J' 'N'))
        (cook |=(a=@ (sub a 58)) (shim 'P' 'Z'))
        (cook |=(a=@ (sub a 64)) (shim 'a' 'k'))
        (cook |=(a=@ (sub a 65)) (shim 'm' 'z'))
        (cook |=(a=@ (sub a 49)) (shim '1' '9'))
      ==
--
