::  p2 divergence: hoonc accepts a trailing :: comment after an opening '''
::  (++qut hed is (plus ace) vul); honk rejects it because
::  triple_quoted_open utils.rs:5381 accepts only vul or newline, with no
::  leading spaces
|%
++  main
  '''  :: comment
  abc
  '''
--
