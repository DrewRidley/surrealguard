-- 7xxx — the warn-by-default lints. One intended violation per line.

-- 7004: control flow decided by a constant. (`WHERE 1 = 1` is the catalog's
-- other example and does not report yet, so it is not here.)
FOR $x IN [] { RETURN $x; };

-- 7005: a comparison against a closed literal set that can never match.
SELECT subject FROM ticket WHERE status = 'closd';

-- 7006: an empty IN list.
SELECT title FROM article WHERE views IN [];

-- 7007: a wildcard beside explicit fields.
SELECT *, title FROM article WHERE id = article:hello;

-- 7011: assignment to id.
UPDATE article:hello SET id = article:world;

-- 7012: a blocking call in a field clause that re-runs.
DEFINE FIELD fetched ON ticket VALUE http::get('https://example.com');

-- 7013: a suppression directive naming no catalog code.
-- surrealql-analyzer: allow(ghost)
SELECT title FROM article WHERE id = article:hello;
