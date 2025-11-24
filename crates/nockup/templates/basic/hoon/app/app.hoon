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
    ~>  %slog.[0 'Peeks awaiting implementation']
    ~
  ::
  ++  poke
    |=  =ovum:moat
    ^-  [(list effect) _state]
    =/  cause  ((soft cause) cause.input.ovum)
    ?~  cause
      ~>  %slog.[3 (crip "invalid cause {<cause.input.ovum>}")]
      :_  state
      ^-  (list effect)
      ~[[%effect 'Invalid cause format']]
    ~>  %slog.[1 (cat 3 'poked: ' -.u.cause)]
    ~>  %slog.[0 'Pokes awaiting implementation']
    [~ state]
  --
--
((moat |) inner)
