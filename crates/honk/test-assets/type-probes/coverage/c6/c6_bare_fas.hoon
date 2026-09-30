::  c6: a bare `/` line where an import rune would go is Hoon, not an
::  import: it ends the import block (pipeline.rs parse_leading_imports ~434,
::  the `let Some(rune)` else branch) and parses as the root path, which
::  becomes the subject of the next line.
/
[42 %done]
