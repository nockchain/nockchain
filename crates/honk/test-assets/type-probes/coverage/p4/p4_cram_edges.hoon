::  p4: sail markdown edges (hoon-138 ++cram): a heading id that skips an
::  embed and reads a link and an image, a `0` that is not a constant, a
::  `++arm` name with a digit and `-`, trailing spaces, and a last
::  paragraph dropped for a tab inside a `*` or backtick span.
|%
++  main  !>(..main)
++  h
  ;>
    # One *b* {<5>} [l](u) ![](p.png)

    a 0 b ++arm-2:core
    text   
    kept
++  d1
  ;>
    kept

    *a	b*
++  d2
  ;>
    kept

    `a	b`
--
