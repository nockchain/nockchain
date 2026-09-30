::  regression: sail markdown blocks (hoon-138 ++cram): headings,
::  paragraphs, unordered, ordered and nested lists, block quotes, rules,
::  fenced code, verse, and `;` sail lines, in a `;>` block and as the
::  children of a tall element.
|%
++  main  !>(..main)
++  m1
  !.
  ;>
    # Title

    ## Sub title here

    A paragraph of text
    over two lines.

    - one
    - two
      continued
    - three
      - nested

    + first
    + second

    > a quote
    > more

    ---

    ```
    code block
      indented

    last
    ```

    before verse

            a line of verse
            and another

    after verse
    ;p: a sail line
    ;hr;
    the end
++  m2
  !.
  ;div
    some text
    ;p;
    more text
  ==
++  m3
  !.
  ;div
    ;p: first
    then text
    ###### deep

    last
  ==
--
