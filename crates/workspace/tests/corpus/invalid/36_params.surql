-- 6xxx — parameters. One intended violation per line. Only the codes a
-- `.surql` file can reach without a host file are here.

-- 6001: two constraints on one host param that cannot both hold.
SELECT title FROM article WHERE views > $x AND title = $x;

-- 6002: a LET that shadows a DEFINE PARAM with a different kind.
LET $max_priority = 'five';

-- 6004: a param read before the LET that binds it.
RETURN $later; LET $later = 1;

-- 6005: a document param outside the construct that binds it.
RETURN $before;

-- 6007: assignment to a protected parameter.
LET $auth = { id: 1 };
