::  p2: tape and cord literal forms: wide-tape escapes, tall """ tapes with
::  extra indentation, blank lines, escapes and {} interpolation, ''' cords
::  (indented, blank lines, no content line), cord \ / continuations (hatch utils.rs
::  soil ~5058-5215, cord ~5334-5470, LineMap::new_with_docs tall-tape offsets)
|%
++  main
  =/  n  7
  :*  "a\\b\"c\{d\41e\7e"
      'a\\b\'c\41\7e'
      'foo\
      /bar'
      """
      line one
        indented {<n>}
      \{brace} \\ \41 {<[n n]>}

      last
      """
      '''
      cord line
        more

      tail
      '''
      "~ascii !#$%&()*+,-./:;<=>?@[]^_`|~"
      '''

      '''
  ==
--
