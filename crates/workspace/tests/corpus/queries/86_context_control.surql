-- Context params, transactions, loops, closures, live queries and the
-- catalog statements: `$session` / `$token` / `$access` / `$parent`, BEGIN /
-- COMMIT / CANCEL, FOR over ranges and collections with BREAK / CONTINUE,
-- THROW, `array::map(|$x| …)` closures, LIVE SELECT / KILL, SHOW CHANGES,
-- REBUILD INDEX, and the `DEFINE PARAM` values. Verified on SurrealDB 3.2.3
-- except LIVE SELECT, which the HTTP transport refuses at runtime ("Unable to
-- perform the realtime query") while accepting the statement.

RETURN $session;

RETURN $session.ns;

RETURN $session.db;

RETURN $session.ip;

RETURN $token;

RETURN $access;

RETURN $auth;

RETURN { ns: $session.ns, db: $session.db, token: $token, access: $access };

SELECT title, (SELECT VALUE username FROM account WHERE id = $parent.author) AS author_name FROM article;

SELECT subject, (SELECT VALUE title FROM article WHERE id = $parent.article) AS about FROM ticket;

SELECT title, (SELECT count() FROM likes WHERE out = $parent.id GROUP ALL)[0].count AS like_count FROM article;

-- DEFINE PARAM values in every position a value can go.
RETURN $max_priority;

RETURN $search_limit * 2;

SELECT subject FROM ticket WHERE priority <= $max_priority LIMIT $search_limit;

UPDATE ticket SET priority = $max_priority WHERE subject = 'Typo';

LET $capped = math::min([$max_priority, 3]);

RETURN $capped;

-- Transactions: a committed one, a cancelled one, and a THROW that unwinds.
BEGIN;

CREATE ticket:tx SET subject = 'In a transaction', reporter = account:ada;

UPDATE ticket:tx SET priority = 1;

COMMIT;

BEGIN TRANSACTION;

CREATE ticket:cancelled SET subject = 'Never lands', reporter = account:ada;

CANCEL TRANSACTION;

BEGIN;

LET $open = (SELECT VALUE count() FROM ticket WHERE status = 'open' GROUP ALL)[0] ?? 0;

IF $open > 1000 { THROW 'too many open tickets' };

UPDATE ticket:tx SET status = 'closed', closed_at = time::now();

COMMIT;

-- FOR over ranges, arrays, and query results, with BREAK and CONTINUE.
FOR $i IN 1..3 {
	CREATE ticket SET subject = string::concat('Loop ', <string> $i), reporter = account:ada;
};

FOR $i IN 1..=2 {
	UPDATE ticket SET priority = $i WHERE subject = string::concat('Loop ', <string> $i);
};

FOR $tag IN ['rust', 'db', 'skip', 'go'] {
	IF $tag = 'skip' { CONTINUE };
	IF $tag = 'go' { BREAK };
	UPDATE article SET tags += $tag WHERE slug = 'hello';
};

FOR $t IN (SELECT VALUE id FROM ticket WHERE status = 'open') {
	UPDATE $t SET priority = math::max([priority - 1, 1]);
};

FOR $row IN (SELECT id, priority FROM ticket WHERE status = 'open') {
	IF $row.priority = 1 { CONTINUE };
	UPDATE $row.id SET priority = 1;
};

FOR $n IN [1, 2, 3] {
	LET $doubled = $n * 2;
	IF $doubled > 4 { BREAK };
};

-- THROW in each position it is reachable from.
IF 1 > 2 { THROW 'never thrown' };

IF (SELECT VALUE count() FROM ticket GROUP ALL)[0] = -1 THEN THROW 'impossible count' END;

LET $guarded = IF $max_priority > 10 { THROW 'priority ceiling too high' } ELSE { $max_priority };

RETURN $guarded;

-- Closures: array::map / filter / reduce / any / all / sort_by, and method
-- chaining on a typed receiver.
LET $views = (SELECT VALUE views FROM article);

RETURN array::map($views, |$v| $v * 2);

RETURN array::map($views, |$v: int| $v + 1);

RETURN array::filter($views, |$v| $v > 10);

RETURN array::fold($views, 0, |$acc, $v| $acc + $v);

RETURN array::any($views, |$v| $v > 100);

RETURN array::all($views, |$v| $v >= 0);

RETURN $views.map(|$v| $v * 10).filter(|$v| $v > 0).len();

RETURN [1, 2, 3].map(|$v| $v * $v);

RETURN ['b', 'a'].sort_natural();

SELECT title, tags.map(|$t| string::uppercase($t)) AS shouting FROM article;

SELECT title, array::filter(tags, |$t| $t != 'seen') AS clean_tags FROM article;

SELECT title, array::map(tags, |$t, $i| string::concat(<string> $i, ':', $t)) AS numbered FROM article;

LET $adder = |$a: int, $b: int| $a + $b;

RETURN $adder(2, 3);

LET $increment = |$x| $x + 1;

RETURN $increment(41);

RETURN (|$x| $x + 1)(41);

-- Live queries and their termination. `KILL` takes the uuid a LIVE SELECT
-- returned.
LIVE SELECT * FROM article;

LIVE SELECT DIFF FROM article;

LIVE SELECT title, views FROM article WHERE views > 10;

LIVE SELECT * FROM ticket WHERE status = 'open' FETCH reporter;

LET $live = u'018e0f3a-1234-7abc-8def-0123456789ab';

KILL $live;

KILL u'018e0f3a-1234-7abc-8def-0123456789ab';

-- Change feeds and index maintenance.
SHOW CHANGES FOR TABLE ticket SINCE d'2020-01-01T00:00:00Z';

SHOW CHANGES FOR TABLE ticket SINCE d'2020-01-01T00:00:00Z' LIMIT 10;

SHOW CHANGES FOR TABLE ticket SINCE 1;

REBUILD INDEX article_body_search ON article;

REBUILD INDEX IF EXISTS article_embedding ON article;

-- Sequences and the fields they drive.
RETURN sequence::nextval('ticket_number');

SELECT number, label FROM ticket ORDER BY number DESC LIMIT 5;

-- Info and value statements round out the statement kinds.
INFO FOR TABLE ticket;

INFO FOR DB;

LET $shape = { open: 0, closed: 0 };

RETURN $shape.open + $shape.closed;

RETURN (SELECT VALUE status FROM ticket GROUP BY status);

RETURN [
	(SELECT VALUE count() FROM article GROUP ALL)[0],
	(SELECT VALUE count() FROM ticket GROUP ALL)[0]
];
