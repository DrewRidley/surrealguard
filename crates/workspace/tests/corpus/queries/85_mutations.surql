-- Mutations the narrowing corpus barely touches: INSERT with ON DUPLICATE KEY
-- UPDATE and IGNORE, UPDATE via MERGE / PATCH / REPLACE, every RETURN mode,
-- RELATE against a real edge table with SET and CONTENT, UPSERT, and DELETE
-- through a traversal. Verified on SurrealDB 3.2.3 with the seeded rows.

-- Seed rows the rest of the file writes against.
CREATE account:ada SET username = 'ada', email = 'ada@example.com';

CREATE account:bob SET username = 'bob', email = 'bob@example.com';

CREATE account:cyd SET username = 'cyd', email = 'cyd@example.com';

CREATE article:hello SET title = 'Hello', slug = 'Hello World', body = 'the quick brown fox', author = account:ada, embedding = [0.1, 0.2, 0.3];

CREATE article:world SET title = 'World', slug = 'world', body = 'the lazy dog', author = account:bob, embedding = [0.4, 0.5, 0.6], tags = ['rust', 'db'];

CREATE article:third CONTENT { title: 'Third', slug: 'third', body: 'a third body', author: account:ada, embedding: [0.7, 0.8, 0.9], rating: 4.5 };

CREATE ONLY article:fourth SET title = 'Fourth', slug = 'fourth', body = 'the fourth', author = account:bob, embedding = [0.0, 0.0, 1.0], location = (-0.1, 51.5);

CREATE article:located SET title = 'Located', slug = 'located', body = 'somewhere', author = account:ada, embedding = [0.5, 0.5, 0.5], location = { type: 'Point', coordinates: [-0.118092, 51.509865] };

CREATE ticket:1 SET subject = 'Login broken', reporter = account:ada;

CREATE ticket:2 SET subject = 'Typo', reporter = account:bob, priority = 5, article = article:hello;

CREATE ticket SET subject = 'Slow page', reporter = account:ada, priority = 2 RETURN label, number;

-- INSERT: objects, tuples, ON DUPLICATE KEY UPDATE, and IGNORE.
INSERT INTO ticket { subject: 'Inserted', reporter: account:bob };

INSERT INTO ticket [{ subject: 'Batch one', reporter: account:ada }, { subject: 'Batch two', reporter: account:bob, priority: 1 }];

INSERT INTO ticket (subject, reporter, priority) VALUES ('Tuple', account:ada, 4);

INSERT INTO ticket (subject, reporter) VALUES ('Tuple a', account:ada), ('Tuple b', account:bob);

INSERT INTO article (id, title, slug, body, author, embedding) VALUES (article:hello, 'Hello', 'hello', 'the quick brown fox', account:ada, [0.1, 0.2, 0.3]) ON DUPLICATE KEY UPDATE views += 1;

INSERT INTO article (id, title, slug, body, author, embedding) VALUES (article:hello, 'Hello', 'hello', 'the quick brown fox', account:ada, [0.1, 0.2, 0.3]) ON DUPLICATE KEY UPDATE views += 1, tags += 'seen';

INSERT INTO article { id: article:hello, title: 'Hello again', slug: 'hello', body: 'again', author: account:ada, embedding: [0.1, 0.2, 0.3] } ON DUPLICATE KEY UPDATE title = $input.title;

INSERT IGNORE INTO article { id: article:hello, title: 'Ignored', slug: 'ignored', body: 'x', author: account:ada, embedding: [0.0, 0.0, 0.0] };

INSERT IGNORE INTO ticket (id, subject, reporter) VALUES (ticket:1, 'Ignored', account:ada);

INSERT INTO ticket { subject: 'Returned', reporter: account:ada } RETURN label;

INSERT INTO ticket { subject: 'Returned none', reporter: account:ada } RETURN NONE;

-- UPDATE: SET (with compound assignments), MERGE, PATCH, REPLACE, UNSET.
UPDATE article:hello SET views += 1;

UPDATE article:hello SET views -= 1, tags += 'fox';

UPDATE article:hello SET tags -= 'fox';

UPDATE article:hello SET rating = 4.0, published_at = time::now();

UPDATE article:hello SET published_at = NONE;

UPDATE article:hello UNSET rating;

UPDATE article:hello MERGE { views: 10, tags: ['merged'] };

UPDATE article:hello MERGE { rating: 3.5 };

UPDATE article:hello PATCH [{ op: 'replace', path: '/views', value: 11 }];

UPDATE article:hello PATCH [{ op: 'add', path: '/tags/-', value: 'patched' }, { op: 'remove', path: '/rating' }];

UPDATE article:hello PATCH [{ op: 'copy', from: '/title', path: '/body' }];

-- REPLACE drops every field it does not name, so it names every field the
-- table requires (a DEFAULT only fills a field at creation, and the engine
-- rejects a REPLACE that would clear a READONLY one).
CREATE sector:retail SET name = 'Retail';

UPDATE sector:retail REPLACE { name: 'Retail & Wholesale' };

UPDATE article SET views += 1 WHERE author = account:ada;

UPDATE article SET tags = array::distinct(tags) WHERE array::len(tags) > 0;

UPDATE ticket:1 SET priority = 1;

UPDATE ticket:1 SET status = 'closed', closed_at = time::now();

UPDATE ticket SET priority = math::min([priority + 1, 5]) WHERE status = 'open';

UPDATE ONLY ticket:2 SET priority = 4;

-- Every RETURN mode.
UPDATE article:hello SET views += 1 RETURN NONE;

UPDATE article:hello SET views += 1 RETURN BEFORE;

UPDATE article:hello SET views += 1 RETURN AFTER;

UPDATE article:hello SET views += 1 RETURN DIFF;

UPDATE article:hello SET views += 1 RETURN views, title;

UPDATE article:hello SET views += 1 RETURN views AS hits, word_count;

UPDATE article:hello SET views += 1 RETURN *, views AS hits;

UPDATE ONLY article:hello SET views += 1 RETURN VALUE views;

CREATE ticket SET subject = 'Created returning', reporter = account:ada RETURN AFTER;

CREATE ticket SET subject = 'Created returning fields', reporter = account:ada RETURN number, label;

CREATE ticket SET subject = 'Created returning diff', reporter = account:ada RETURN DIFF;

DELETE ticket WHERE subject = 'Created returning diff' RETURN BEFORE;

DELETE ticket WHERE subject = 'Created returning' RETURN NONE;

-- UPSERT in each payload form.
UPSERT ticket:9 SET subject = 'Upserted', reporter = account:ada;

UPSERT ticket:9 MERGE { priority: 2 };

UPSERT ticket:8 CONTENT { subject: 'Upserted content', reporter: account:bob };

UPSERT ticket:9 PATCH [{ op: 'replace', path: '/priority', value: 3 }];

UPSERT sector:energy REPLACE { name: 'Energy' };

UPSERT ticket:9 SET priority += 1 RETURN AFTER;

UPSERT ONLY ticket:9 SET subject = 'Upserted only' RETURN subject;

-- RELATE against the `likes` edge: SET, CONTENT, ONLY, RETURN, and arrays of
-- endpoints.
RELATE account:ada->likes->article:hello SET weight = 2.0;

RELATE account:bob->likes->article:hello CONTENT { weight: 3.0, note: 'great' };

RELATE ONLY account:ada->likes->article:world SET weight = 1.5, note = 'ok';

RELATE account:bob->likes->article:world SET note = 'again' RETURN AFTER;

RELATE account:bob->likes->article:located RETURN weight, note;

RELATE account:cyd->likes->article:hello SET weight = 0.5 RETURN NONE;

RELATE [account:ada, account:bob]->likes->article:fourth;

RELATE account:ada->likes->[article:third, article:located] SET weight = 4.0;

INSERT RELATION INTO likes { in: account:cyd, out: article:world, weight: 2.5 };

-- Reads and writes through the edge.
UPDATE likes SET weight += 0.5 WHERE weight < 2.0;

UPDATE account:ada->likes SET note = 'touched';

DELETE account:ada->likes WHERE out = article:world;

-- The pair just deleted is free again, so the param-driven RELATE lands.
LET $liker = account:ada;

LET $liked = article:world;

RELATE $liker->likes->$liked SET weight = 9.0;

DELETE likes WHERE weight > 8.0 RETURN BEFORE;

DELETE account:bob->likes WHERE out = article:located;

SELECT ->likes->article.title AS titles FROM account:ada;

SELECT ->likes.weight AS weights FROM account:ada;

SELECT in.username AS who, out.title AS what, weight FROM likes;

SELECT in, out, weight FROM likes WHERE note != NONE ORDER BY weight DESC;

-- Records as targets: a param, a subquery, and a computed id.
LET $target = ticket:1;

UPDATE $target SET priority = 2;

UPDATE (SELECT VALUE id FROM ticket WHERE status = 'open') SET priority = 3;

UPDATE type::record('ticket', 2) SET subject = 'Typed record';

DELETE type::record('ticket:9');

CREATE type::record('ticket', 10) SET subject = 'Typed create', reporter = account:ada;

DELETE ticket:10;
