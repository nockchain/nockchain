/+  lib
/=  *  /common/wrapper
::
=>
|%
+$  versioned-state
  $:  %v1
      ~
  ==
::
+$  effect
  $%  [%effect @t]
  ==
::
+$  cause
  $%  [%cause ~]
      [%command val=@t]
  ==
--
|%
++  moat  (keep versioned-state)
::
++  inner
  |_  state=versioned-state
  ::
  ++  load
    |=  old-state=versioned-state
    ^-  _state
    ?:  =(-.old-state %v1)
      old-state
    old-state
  ::
  ++  peek
    |=  =path
    ^-  (unit (unit *))
    ~>  %slog.[0 'Received peek']
    ~
  ::
  ++  poke
    |=  =ovum:moat
    ^-  [(list effect) _state]
    ~>  %slog.[0 'Received poke']
    ~&  cause.input.ovum
    =/  cause  ((soft cause) cause.input.ovum)
    ?~  cause
      ~>  %slog.[3 (crip "invalid cause {<cause.input.ovum>}")]
      :_  state
      ^-  (list effect)
      ~[[%effect 'Invalid cause format']]
    ~>  %slog.[0 'Received poke']
    `state
  --
--
((moat |) inner)
