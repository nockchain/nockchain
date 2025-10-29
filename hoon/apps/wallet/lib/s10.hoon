/=  slip10  /common/slip10
/=  bip39  /common/bip39
/=  *   /common/zose
::  Convenience wrapper door for slip10 library
::  ** Never use slip10 directly in the wallet **
=>
|%
++  keyc  keyc:slip10
++  current-protocol  current-protocol:slip10
--
|_  bas=base:slip10
++  gen-master-key
  |=  [entropy=byts salt=byts]
  =/  argon-byts=byts
    :-  32
    %+  argon2-nockchain:argon2:crypto
      entropy
    salt
  =/  memo=tape  (from-entropy:bip39 argon-byts)
  ::  TODO: thread version through returned core rather than in userspace code
  :-  (crip memo)
  (from-seed:slip10 [64 (to-seed:bip39 memo "")] current-protocol:slip10)
++  from-seed
  |=  [=byts version=@]
  (from-seed:slip10 byts version)
::
++  from-private
  |=  =keyc
  (from-private:slip10 keyc)
::
++  from-public
  |=  =keyc
  (from-public:slip10 keyc)
::
::  derives the i-th child key(s) from a parent key.
::  index i can be any child index. returns the door
::  with the door sample modified with the values
::  corresponding to the key. the core sample can then
::  be harvested for keys.
::
++  derive
  |=  [parent=keyc typ=?(%pub %prv) i=@u]
  ?-    typ
      %pub
    =>  [cor=(from-public:slip10 parent) i=i]
    (derive:cor i)
  ::
      %prv
    =>  [cor=(from-private:slip10 parent) i=i]
    (derive:cor i)
  ==
::
++  from-extended-key
  |=  key=@t
  (from-extended-key:slip10 key)
--
