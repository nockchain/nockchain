::  c6 (was divergence 28): single-file form of the import-header spot bug.
::  hoonc parses the header (here only the /? Ford pin) apart from the body,
::  so the body %spot starts after it; honk used to start it at the /? line
::  (pipeline.rs parse_import_header, hatch runes/fas.rs import_header).
/?  310
`*`[1 2]
