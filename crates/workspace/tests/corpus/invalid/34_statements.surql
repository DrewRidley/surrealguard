-- 4xxx — statement and clause misuse. One intended violation per line.

-- 4003: ONLY over a whole table without LIMIT 1.
SELECT title FROM ONLY article;

-- 4004: INSERT tuple with mismatched column and value counts.
INSERT INTO ticket (subject, reporter) VALUES ('one value');

-- 4005: BREAK outside any loop.
BREAK;

-- 4006: a statement after the block already returned.
DEFINE FUNCTION fn::bad::unreachable() { RETURN 1; RETURN 2; };

-- 4007: COMMIT with no open transaction.
COMMIT;

-- 4010: the same SET target twice in one statement.
UPDATE article:hello SET views = 1, views = 2;

-- 4011: the same projection key twice.
SELECT title, title FROM article WHERE id = article:hello;

-- 4012: OMIT without a wildcard projection.
SELECT title OMIT body FROM article WHERE id = article:hello;

-- 4013: a GROUP BY key that is not projected.
SELECT count() AS n FROM article GROUP BY author;

-- 4017: a value block that ends with LET.
LET $ends_with_let = { LET $inner = 1; };

-- 4018: a side-effecting subquery in a read position.
SELECT (DELETE ticket_log) AS gone FROM article WHERE id = article:hello;

-- 4019: CREATE on a relation table without in / out.
CREATE likes SET weight = 1.0;

-- 4020: a RETURN mode that is meaningless for the statement.
CREATE ticket SET subject = 'x', reporter = account:ada RETURN BEFORE;

-- 4021: SHOW CHANGES on a table without a CHANGEFEED.
SHOW CHANGES FOR TABLE article SINCE 1;

-- 4022: SELECT from a DROP table (defined here, on the line before).
DEFINE TABLE dropped_log SCHEMALESS DROP;
SELECT * FROM dropped_log WHERE id = dropped_log:one;

-- 4023: count() without a GROUP clause.
SELECT count() FROM article WHERE views > 0;

-- 4024: an IF branch its constant guard never reaches.
IF false { RETURN 1 };

-- 4025: a wildcard under a GROUP clause.
SELECT * FROM article GROUP BY author;

-- 4026: a filtered ONLY with no single-row proof.
SELECT title FROM ONLY article WHERE views > 1;

-- 4027: a projection that is neither a group key nor an aggregate.
SELECT author, title FROM article GROUP BY author;

-- 4028: an aggregate over a column with no GROUP clause.
SELECT math::sum(views) AS total FROM article WHERE views > 0;
