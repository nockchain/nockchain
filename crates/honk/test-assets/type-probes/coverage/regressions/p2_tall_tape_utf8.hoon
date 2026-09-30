::  p2 regression (DIVERGENCES row 21): non-ASCII characters in a tall """
::  tape are one element per UTF-8 byte; honk used to reject "€" and keep
::  "é" as one code point element
|%
++  main
  """
  é€
  """
--
