::  c3: the false branch of ?= crops a fork of cells by a fork of cells,
::  so crop_sint (mod.rs ~10946) crops by each option of the fork.
=/  x=?([%a @] [%b @] [%c @])  [%c 3]
?:  ?=(?([%a @] [%b @]) x)  ~  x
