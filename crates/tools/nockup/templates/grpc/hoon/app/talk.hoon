/+  *lib
/=  *  /common/wrapper
::
=>
|%
+$  versioned-state
  $:  %v1
      ~
  ==
::
+$  cause
  $%  [%cause ~]
      [%command val=@t]
      ^cause
  ==
::
+$  effect
  $%  [%effect msg=@]
      ^effect
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
    :_  state
    ^-  (list effect)
    =/  pid  42  :: implementation-specific meaning
    =/  val  -.u.cause
    :~  [%grpc %peek pid %talk /path]
        [%grpc %poke pid val]
        [%exit ~]
    ==
  --
--
((moat |) inner)
