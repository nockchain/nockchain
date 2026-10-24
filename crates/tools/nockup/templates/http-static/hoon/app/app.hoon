/=  *  /common/wrapper
=>
|%
+$  server-state  %stateless
+$  header  [k=@t v=@t]
+$  octs  [p=@ q=@]
+$  method
  $?  %'GET'
      %'HEAD'
      %'POST'
      %'PUT'
      %'DELETE'
      %'CONNECT'
      %'OPTIONS'
      %'TRACE'
      %'PATCH'
  ==
::
+$  cause
  $:  %req
      id=@
      uri=@t
      =method
      headers=(list header)
      body=(unit octs)
  ==
::
+$  effect
  $:  %res
      id=@
      status=@ud
      headers=(list header)
      body=(unit octs)
  ==
::
++  to-octs
  |=  bod=@
  ^-  (unit octs)
  =/  len  (met 3 bod)
  ?:  =(len 0)  ~
  `[len bod]
--
::
=>
|%
++  moat  (keep server-state)
::
++  inner
  |_  k=server-state
  ::
  ::  +load: upgrade from previous state
  ::    (but the server is stateless)
  ::
  ++  load
    |=  arg=server-state
    arg
  ::
  ::  +peek: external inspect
  ::
  ++  peek
    |=  =path
    ^-  (unit (unit *))
    ~>  %slog.[0 'Peeks awaiting implementation']
    ~
  ::
  ::  +poke: external apply
  ::
  ++  poke
    |=  =ovum:moat
    ^-  [(list effect) server-state]
    =/  sof-cau=(unit cause)  ((soft cause) cause.input.ovum)
    ?~  sof-cau
      ~&  "cause incorrectly formatted!"
      ~&  now.input.ovum
      !!
    =/  [id=@ uri=@t =method headers=(list header) body=(unit octs)]  +.u.sof-cau
    ~>  %slog.[0 [id+id uri+uri method+method headers+headers]]
    :_  k
    :_  ~
    ^-  effect
    =-  ~&  effect+-
        -
    ?+    method  [%res ~ %400 ~ ~]
        %'GET'
      :*  %res  id=id  %200
        ['content-type' 'text/html']~
      %-  to-octs
      '''
      <!doctype html>
      <html>
        <body>
          <h1>Hello NockApp!</h1>
        </body>
      </html>
      '''
    ==
    ::
        %'POST'
      !!
    ==
  --
--
((moat |) inner)
