::  regression: inline sail markdown (hoon-138 ++word, ++down): bold,
::  italic, smart quotes, code, links, images, escapes, a trailing `\`
::  line break, `#hoon` and hoon constants as code, `++arm` names, and
::  embedded hoon and sail.
|%
++  main  !>(..main)
++  i1
  !.
  ;>
    Some *bold* and _italic_ and "quoted" and `co\`de` text,
    *nested _em_ "q"* and \*escaped\* and a trailing \
    break with [a *link*](http://x.com/a\)b) and [spaced] (u)
    and ![alt text](pic.png) and ![](empty.png) here.
    Code: #foo #(add 1 2) ~zod %foo %.y %'cord' 0x1f -5 .1.2 ~
    and ++arm +$mold +*gate ++arm:core, 1.000 plain.
    Embeds {<5>} -{"tape"} ;{b "bold"} +{;i;} end.
    Bytes: é ü \é
++  i2
  !.
  ;div
    #hoon at the start of text
    0x1 first
  ==
--
