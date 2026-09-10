-- Operators the narrowing corpus never spells: `IS` / `IS NOT`, `?:`, `**`,
-- `%`, the CONTAINS* and INSIDE* families, `OUTSIDE`, `INTERSECTS`, `IN` /
-- `NOT IN`, regex matching, and the bidirectional `<->` traversal. Every
-- statement runs on SurrealDB 3.2.3 (the fuzzy `~` family does not, and is
-- absent on purpose).

RETURN 1 IS 1;

RETURN 1 IS NOT 2;

RETURN NONE IS NONE;

RETURN NULL IS NOT NONE;

SELECT title FROM article WHERE published_at IS NOT NONE;

SELECT title FROM article WHERE rating IS NONE;

SELECT subject FROM ticket WHERE closed_at IS NOT NONE AND status = 'closed';

RETURN true ?: 'fallback';

RETURN NONE ?: 'fallback';

RETURN 0 ?: 10;

SELECT title, rating ?: 0 AS rating_or_zero FROM article;

SELECT title, published_at ?? created_at AS shown_at FROM article;

RETURN 2 ** 10;

RETURN 2.5 ** 2;

RETURN 17 % 5;

RETURN -7 % 3;

SELECT title, views % 10 AS last_digit FROM article;

SELECT title, math::pow(views, 2) AS squared, views ** 2 AS squared_too FROM article;

RETURN 'hello world' CONTAINS 'world';

RETURN [1, 2, 3] CONTAINS 2;

RETURN [1, 2, 3] CONTAINSNOT 9;

RETURN [1, 2, 3] CONTAINSALL [1, 2];

RETURN [1, 2, 3] CONTAINSANY [3, 9];

RETURN [1, 2, 3] CONTAINSNONE [7, 9];

SELECT title FROM article WHERE tags CONTAINS 'rust';

SELECT title FROM article WHERE tags CONTAINSALL ['rust', 'db'];

SELECT title FROM article WHERE tags CONTAINSANY ['rust', 'go'];

SELECT title FROM article WHERE tags CONTAINSNONE ['spam'];

SELECT title FROM article WHERE tags CONTAINSNOT 'draft';

SELECT title FROM article WHERE body CONTAINS 'fox';

RETURN 2 INSIDE [1, 2, 3];

RETURN 9 NOTINSIDE [1, 2, 3];

RETURN [1, 2] ALLINSIDE [1, 2, 3];

RETURN [1, 9] ANYINSIDE [1, 2, 3];

RETURN [8, 9] NONEINSIDE [1, 2, 3];

SELECT title FROM article WHERE 'rust' INSIDE tags;

SELECT title FROM article WHERE ['rust', 'db'] ALLINSIDE tags;

SELECT subject FROM ONLY ticket WHERE status INSIDE ['open'] LIMIT 1;

RETURN 2 IN [1, 2, 3];

RETURN 9 NOT IN [1, 2, 3];

SELECT title FROM article WHERE views IN [0, 1, 2];

SELECT subject FROM ticket WHERE status IN ['open'];

SELECT subject FROM ticket WHERE priority NOT IN [1, 2];

SELECT subject FROM ticket WHERE priority IN [1, 2, 3] AND status = 'open';

-- Geometry: a point inside / outside a polygon, and two shapes intersecting.
LET $london = {
	type: 'Polygon',
	coordinates: [[[-0.38, 51.37], [0.1, 51.37], [0.1, 51.61], [-0.38, 51.61], [-0.38, 51.37]]]
};

RETURN (-0.118092, 51.509865) INSIDE $london;

RETURN (2.3522, 48.8566) OUTSIDE $london;

RETURN { type: 'Point', coordinates: [-0.118092, 51.509865] } INTERSECTS $london;

SELECT title FROM article WHERE location INSIDE $london;

SELECT title FROM article WHERE location OUTSIDE $london;

SELECT title FROM article WHERE location INTERSECTS $london;

-- Regex: a literal on either side of `=`, and `string::matches`.
RETURN 'hello' = /^hel+o$/;

RETURN /^h/ = 'hello';

-- `string::matches` takes its pattern as a string or a regex (3.2.3 returns
-- true here).
RETURN string::matches('hello', /^h.*o$/);

SELECT title FROM article WHERE title = /^[A-Z]/;

SELECT title FROM article WHERE string::matches(slug, /^[a-z0-9-]+$/);

SELECT title FROM article WHERE slug != /^draft-/;

-- The bidirectional traversal: edges in either direction, and the far node.
SELECT <->likes AS engagement FROM article;

SELECT <->likes<->account AS people FROM article;

SELECT <->likes<->article AS articles FROM account;

SELECT ->likes->article.title AS liked_titles FROM account;

SELECT <-likes<-account.username AS fans FROM article;

SELECT ->likes[WHERE weight > 1.0]->article AS strong_likes FROM account;

SELECT <-likes.weight AS weights FROM article;

SELECT count(<-likes) AS like_count FROM article;

-- Comparison chains and boolean algebra over typed fields.
-- `NOT (…)` is the `not()` builtin on the engine (`NOT true` without the
-- parentheses is a parse error on 3.2.3), and function names are
-- case-insensitive there.
SELECT title FROM article WHERE views >= 10 AND views <= 100 AND NOT (rating = NONE);

SELECT title FROM article WHERE (views > 10 OR rating > 4) AND is_published;

SELECT title FROM article WHERE !is_published;

SELECT subject FROM ticket WHERE priority <= $max_priority;
