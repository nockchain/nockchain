::  c6: the last byte of the file is a `/` where an import clause could
::  start (pipeline.rs reject_leftover_import, no byte after the `/`). It
::  parses as the root path.
/