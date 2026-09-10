-- SELECT clauses the narrowing corpus never reaches: OMIT, FETCH, TIMEOUT,
-- EXPLAIN, WITH INDEX / NOINDEX, VERSION-free paging with ORDER BY, and
-- record-id ranges in FROM. Every statement runs on SurrealDB 3.2.3.
-- (`PARALLEL` is gone in 3.x — a parse error — so it is deliberately absent.
-- `EXPLAIN` is written both ways the engine accepts: as the trailing clause
-- and as a prefix; `EXPLAIN FULL SELECT` is a parse error on 3.2.3.)

SELECT * OMIT body, embedding FROM article;

SELECT * OMIT embedding FROM article WHERE views > 10;

SELECT *, ->likes->article AS liked FROM account FETCH liked;

SELECT title, author FROM article FETCH author;

SELECT title, author.username FROM article FETCH author;

SELECT * FROM ticket FETCH reporter, article;

SELECT title FROM article WHERE views > 0 TIMEOUT 5s;

SELECT title FROM article WHERE slug = 'hello' TIMEOUT 500ms;

SELECT title FROM article WHERE slug = 'hello' EXPLAIN;

SELECT title FROM article WHERE slug = 'hello' EXPLAIN FULL;

EXPLAIN SELECT title FROM article WHERE slug = 'hello';

SELECT title FROM article WITH INDEX article_slug WHERE slug = 'hello';

SELECT title FROM article WITH INDEX article_author WHERE author = account:ada;

SELECT title FROM article WITH NOINDEX WHERE slug = 'hello';

SELECT title, views FROM article ORDER BY views DESC, title ASC LIMIT 5 START 10;

SELECT title FROM article ORDER BY RAND() LIMIT 3;

SELECT title, views FROM article ORDER BY views NUMERIC DESC;

SELECT title FROM article ORDER BY title COLLATE ASC;

-- Record-id ranges as a FROM target: the row shape is the table's.
SELECT title FROM article:a..z;

SELECT title FROM article:a..=m;

SELECT * FROM ticket:1..100;

SELECT * FROM ticket:[1]..=[50];

SELECT title FROM article:['a', 1]..['z', 9];

-- An explicit record id and a set of them.
SELECT title, views FROM article:hello;

SELECT title FROM article:hello, article:world;

SELECT title FROM [article:hello, article:world];

SELECT * FROM ONLY article:hello;

-- A subquery target with its own clauses.
SELECT title FROM (SELECT * FROM article WHERE views > 100 ORDER BY views DESC LIMIT 20) WHERE is_published = true;

-- Grouping over the new tables, with ORDER on the aggregate.
SELECT status, count() AS total FROM ticket GROUP BY status ORDER BY total DESC;

SELECT author, math::max(views) AS top, array::len(array::group(tags)) AS tag_count FROM article GROUP BY author;

SELECT * FROM ticket_stats;

SELECT status, total FROM ticket_stats WHERE total > 0;

-- Nested field of an optional link, and a link fetched in place.
SELECT article.title AS article_title FROM ticket;

SELECT reporter.username, article.title FROM ticket FETCH reporter;
