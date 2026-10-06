::  wet gate entered with `=>  foo  $`, so the entry call's subject is the
::  formal gate itself and the redone core equals it; the body's $(x [x x])
::  fires from that same subject. hoon-138 ++fire finds [sut dox arm] in rib
::  and skips the re-mull; keying on the redone core mulls x=[@ @].
|%
++  main
  |=  [a=@ b=?]
  =/  foo  |*(x=@ [.+(x) $(x [x x])])
  =>  foo
  $
--
