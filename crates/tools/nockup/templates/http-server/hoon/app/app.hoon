/+  *http
/=  *  /common/wrapper
=>
|%
+$  server-state  [%0 value=@]
++  page
  ^-  tape
  %-  trip
  '''
  <!doctype html>
  <html>
    <body>
      <h1>Hello NockApp!</h1>
      <div class="counter-display">
        Count: COUNT
      </div>
      
      <form method="POST" action="/increment" style="display: inline;">
        <button type="submit" class="increment-button">Increment Counter</button>
      </form>
      
      <form method="POST" action="/reset" style="display: inline;">
        <button type="submit" class="reset-button">Reset Counter</button>
      </form>
    </body>
  </html>
  '''
--
::
=>
|%
++  moat  (keep server-state)
::
++  inner
  |_  state=server-state
  ::
  ::  +load: upgrade from previous state
  ::
  ++  load
    |=  arg=server-state
    ^-  server-state
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
    ::  Parse request into components.
    =/  [id=@ uri=@t =method headers=(list header) body=(unit octs)]  +.u.sof-cau
    ::
    ?+    method  [~[[%res ~ %400 ~ ~]] state]
        %'GET'
      :_  state
      :_  ~
      ^-  effect
      :*  %res  id=id  %200
          ['content-type' 'text/html']~
          %-  to-octs
          %-  crip
          ^-  tape
          =/  index  (find "COUNT" page)
          ;:  weld
            (scag (need index) page)
            (scow %ud value.state)
            (slag (add (need index) ^~((lent "COUNT"))) page)
      ==  ==
      ::
        %'POST'
      ?:  =('/increment' uri)
        :_  state(value +(value.state))
        :_  ~
        ^-  effect
        :*  %res  id=id  %200
            ['content-type' 'text/html']~
            %-  to-octs
            %-  crip
            ^-  tape
            =/  index  (find "COUNT" page)
            ;:  weld
              (scag (need index) page)
              (scow %ud +(value.state))
              (slag (add (need index) ^~((lent "COUNT"))) page)
        ==  ==
      ::
      ?>  =('/reset' uri)
      :_  state(value 0)
      :_  ~
      ^-  effect
      :*  %res  id=id  %200
          ['content-type' 'text/html']~
          %-  to-octs
          %-  crip
          ^-  tape
          =/  index  (find "COUNT" page)
          ;:  weld
            (scag (need index) page)
            (scow %ud 0)
            (slag (add (need index) ^~((lent "COUNT"))) page)
      ==  ==
    ==
  --
--
((moat |) inner)
