::  regression: nested sail markdown (hoon-138 ++cram): list items with
::  several paragraphs, lists and code in list items and quotes, verse
::  stanzas, headings of every level, and markdown ending at `==` or an
::  outdent right after a rule, where hoon-138's column runs ahead of the
::  text for the rest of the line.
|%
++  main  !>(..main)
++  n1
  !.
  ;>
    - item one

      second paragraph of item one
    - item two
      + ordered inside
      + and again

    > quoted
    > - list in quote
    > - more

    - code in item
      ```
      fenced
        deeper
      ```
    - last
++  n2
  !.
  ;>
    stanza intro

            first stanza line
            first stanza two

            second stanza

    # One: *b* & c!

    ## Two

    ### Three

    #### Four

    ##### Five

    -----

    ###### Six
++  n3
  !.
  ;div
    text
    ---
  ==
++  n5
  =/  x
    ;>
      text

      ---
    x
++  n4
  !.
  ;div
    ;p;

    text after a blank line

    ;p;
  ==
--
