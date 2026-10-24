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
  $%  [%response str=@t]
      [%exit code=@]
  ==
::
+$  cause
  $%  [%call str=@t]
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
      ~[[%exit 1]]
    ?>  ?=(%call -.u.cause)
    ?:  (~(has in `(set term)`(sy ~[%exit %x %q %quit])) str.u.cause)
      :_  state
      ^-  (list effect)
      :~  [%exit 0]
      ==
    :_  state
    ^-  (list effect)
    :~  [%response str.u.cause]
    ==
  --
--
((moat |) inner)
