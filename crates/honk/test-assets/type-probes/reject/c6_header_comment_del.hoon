::  c6 rejection (both compilers): a comment is `vul`, printable characters
::  only, and DEL (127) is not printable (`prn`). The header rule stops before
::  the comment below (pipeline.rs HeaderParser::vul), and the body parser
::  rejects it.
::  a DEL:
42
