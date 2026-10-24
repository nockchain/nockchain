^?
|%
::  $header: a single HTTP header key-value pair
::
+$  header  [k=@t v=@t]
::  +header-list: an ordered list of http headers
::
+$  header-list
(list [key=@t value=@t])
::  +method: exhaustive list of http verbs
::
+$  method
$?  %'CONNECT'
    %'DELETE'
    %'GET'
    %'HEAD'
    %'OPTIONS'
    %'PATCH'
    %'POST'
    %'PUT'
    %'TRACE'
==
::  $octs: length in bytes and payload
::
+$  octs  [p=@ q=@]
::  +to-octs: convert an atom to octs
::
++  to-octs
  |=  bod=@
  ^-  (unit octs)
  =/  len  (met 3 bod)
  ?:  =(len 0)  ~
  `[len bod]
::  $cause: the cause of an HTTP event
::
+$  cause
  $:  %req
      id=@
      uri=@t
      =method
      headers=(list header)
      body=(unit octs)
  ==
::  $effect: the result of an HTTP event
::
+$  effect
  $:  %res
      id=@
      status=@ud
      headers=(list header)
      body=(unit octs)
  ==
::  +request: a single http request
::
+$  request
$:  ::  method: http method
    ::
    method=method
    ::  url: the url requested
    ::
    ::    The url is not escaped. There is no escape.
    ::
    url=@t
    ::  header-list: headers to pass with this request
    ::
    =header-list
    ::  body: optionally, data to send with this request
    ::
    body=(unit octs)
==
::  +response-header: the status code and header list on an http request
::
::    We separate these away from the body data because we may not wait for
::    the entire body before we send a %progress to the caller.
::
+$  response-header
$:  ::  status: http status code
    ::
    status-code=@ud
    ::  headers: http headers
    ::
    headers=header-list
==
::  +http-event: packetized http
::
::    Urbit treats Earth's HTTP servers as pipes, where Urbit sends or
::    receives one or more %http-events. The first of these will always be a
::    %start or an %error, and the last will always be %cancel or will have
::    :complete set to %.y to finish the connection.
::
::    Calculation of control headers such as 'Content-Length' or
::    'Transfer-Encoding' should be performed at a higher level; this structure
::    is merely for what gets sent to or received from Earth.
::
+$  http-event
$%  ::  %start: the first packet in a response
    ::
    $:  %start
        ::  response-header: first event information
        ::
        =response-header
        ::  data: data to pass to the pipe
        ::
        data=(unit octs)
        ::  whether this completes the request
        ::
        complete=?
    ==
    ::  %continue: every subsequent packet
    ::
    $:  %continue
        ::  data: data to pass to the pipe
        ::
        data=(unit octs)
        ::  complete: whether this completes the request
        ::
        complete=?
    ==
    ::  %cancel: represents unsuccessful termination
    ::
    [%cancel ~]
==
::  +get-header: returns the value for :header, if it exists in :header-list
::
++  get-header
|=  [header=@t =header-list]
^-  (unit @t)
::
?~  header-list
    ~
::
?:  =(key.i.header-list header)
    `value.i.header-list
::
$(header-list t.header-list)
::  +set-header: sets the value of an item in the header list
::
::    This adds to the end if it doesn't exist.
::
++  set-header
|=  [header=@t value=@t =header-list]
^-  ^header-list
::
?~  header-list
    ::  we didn't encounter the value, add it to the end
    ::
    [[header value] ~]
::
?:  =(key.i.header-list header)
    [[header value] t.header-list]
::
[i.header-list $(header-list t.header-list)]
::  +delete-header: removes the first instance of a header from the list
::
++  delete-header
|=  [header=@t =header-list]
^-  ^header-list
::
?~  header-list
    ~
::  if we see it in the list, remove it
::
?:  =(key.i.header-list header)
    t.header-list
::
[i.header-list $(header-list t.header-list)]
::  +unpack-header: parse header field values
::
++  unpack-header
|^  |=  value=@t
    ^-  (unit (list (map @t @t)))
    (rust (cass (trip value)) values)
::
++  values
    %+  more
    (ifix [. .]:(star ;~(pose ace (just '\09'))) com)
    pairs
::
++  pairs
    %+  cook
    ~(gas by *(map @t @t))
    %+  most  (ifix [. .]:(star ace) mic)
    ;~(plug token ;~(pose ;~(pfix tis value) (easy '')))
::
++  value
    ;~(pose token quoted-string)
::
++  token                                         ::  7230 token
    %+  cook  crip
    ::NOTE  this is ptok:de-purl:html, but can't access that here
    %-  plus
    ;~  pose
    aln  zap  hax  buc  cen  pam  soq  tar  lus
    hep  dot  ket  cab  tic  bar  sig
    ==
::
++  quoted-string                                 ::  7230 quoted string
    %+  cook  crip
    %+  ifix  [. .]:;~(less (jest '\\"') doq)
    %-  star
    ;~  pose
    ;~(pfix bas ;~(pose (just '\09') ace prn))
    ;~(pose (just '\09') ;~(less (mask "\22\5c\7f") (shim 0x20 0xff)))
    ==
--
::  +simple-payload: a simple, one event response used for generators
::
+$  simple-payload
$:  ::  response-header: status code, etc
    ::
    =response-header
    ::  data: the data returned as the body
    ::
    data=(unit octs)
==
--