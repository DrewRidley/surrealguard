-- 2xxx — types and nullability. One intended violation per line.

-- 2001: a value that cannot inhabit the field's declared type.
UPDATE article:hello SET views = 'many';

-- 2004: operands that make no sense together.
SELECT title FROM article WHERE title > 1;

-- 2005: a condition that is not a boolean.
IF 42 { RETURN 1 };

-- 2007: a cast to a type that does not exist.
RETURN <ghost> 1;

-- 2008: a conversion that can never succeed — value-proven, then kind-proven.
RETURN <int> 'abc';
RETURN <duration> true;

-- 2012: a body that does not return what it declares.
DEFINE FUNCTION fn::bad::returns_int() -> string { RETURN 1; };

-- 2015: an optional value in a position that needs one.
SELECT rating + 1 AS bumped FROM article WHERE id = article:hello;

-- 2017: an ORDER BY key the result rows do not have.
SELECT title FROM article WHERE views > 0 ORDER BY nope;

-- 2018: a negative LIMIT / START.
SELECT title FROM article WHERE views > 0 LIMIT -1;
SELECT title FROM article WHERE views > 0 START -5;

-- 2020: KILL with something that is not a live-query uuid.
LET $not_a_uuid = 42; KILL $not_a_uuid;

-- 2021: SHOW SINCE with neither a versionstamp nor a datetime.
SHOW CHANGES FOR TABLE ticket SINCE 'yesterday';

-- 2022: FOR over something that cannot be iterated.
FOR $x IN 42 { RETURN $x; };

-- 2025: a READONLY field written after creation.
UPDATE article:hello SET created_at = time::now();

-- 2026: a hand-written value to a VALUE-clause field that ignores it.
UPDATE article:hello SET updated_at = time::now();

-- 2030: an index / filter / splat on a scalar.
SELECT title[0] FROM article WHERE id = article:hello;
SELECT views[WHERE 1 = 1] FROM article WHERE id = article:hello;
SELECT views.* FROM article WHERE id = article:hello;

-- 2031: a regex literal that does not compile.
RETURN /unclosed(/;

-- 2032: literal content invalid for its kind.
RETURN d'2024-13-45T00:00:00Z';
RETURN u'not-a-uuid';

-- 2033: a malformed PATCH.
UPDATE article:hello PATCH [{ op: 'teleport', path: '/views', value: 1 }];
UPDATE article:hello PATCH [{ op: 'replace', path: 'views', value: 1 }];

-- 2034: a creation that omits required fields.
CREATE article SET title = 'only a title';

-- 2035: analyzer filter arguments out of order.
DEFINE ANALYZER bad_ngram TOKENIZERS blank FILTERS edgengram(5, 2);

-- 2036: a GeoJSON literal whose shape does not match its type.
RETURN { type: 'Pointt', coordinates: [1, 2] };

-- 2037: a DEFAULT that fails the field's own ASSERT.
DEFINE FIELD grade ON ticket TYPE int DEFAULT 9 ASSERT $value <= 5;

-- 2038: a constant write that fails the field's ASSERT.
UPDATE ticket:1 SET priority = 9;
