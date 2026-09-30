::  An arm that fails to compile in a non-root chapter. By mug order the
::  chapter treap has root %as with children %eu and %gg, and %eu has only
::  the right child %av; inside %av the arm treap has root as with children
::  eu and gg, and eu has only the right child av, the failing arm.
|%
+|  %as
++  x  1
+|  %eu
++  y  1
+|  %gg
++  z  1
+|  %av
++  as  1
++  eu  1
++  gg  1
++  av  zzz
--
