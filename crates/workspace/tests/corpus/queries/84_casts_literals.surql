-- Casts and literals: every cast the engine accepts, every prefixed literal,
-- ranges and record-id ranges, geometry, bytes and regex. Verified on
-- SurrealDB 3.2.3 (`<int> 1.9` is a runtime cast failure there, and `f""`
-- file literals sit behind an experimental flag, so neither appears).

RETURN <float> 1;

RETURN <float> '2.5';

RETURN <bool> 'true';

RETURN <bool> 'false';

RETURN <decimal> '1.5';

RETURN <decimal> 2;

RETURN <duration> '1h30m';

RETURN <uuid> '018e0f3a-1234-7abc-8def-0123456789ab';

RETURN <number> '42';

RETURN <number> 4.2;

RETURN <int> '42';

RETURN <int> 42.0;

RETURN <string> 42;

RETURN <string> true;

RETURN <string> d'2024-01-01T00:00:00Z';

RETURN <datetime> '2024-01-01T00:00:00Z';

RETURN <set> [1, 1, 2];

RETURN <set<int>> [1, 1, 2];

RETURN <set<string>> ['a', 'a'];

RETURN <array> [1, 2];

RETURN <array<int>> ['1', '2'];

RETURN <array<string>> [1, 2];

RETURN <option<int>> NONE;

RETURN <option<int>> 5;

RETURN <option<string>> 'x';

RETURN <record> 'article:hello';

RETURN <record<article>> 'article:hello';

RETURN <record<article | ticket>> 'ticket:1';

RETURN <object> { a: 1 };

RETURN <bytes> 'hello';

RETURN <geometry<point>> (1.0, 2.0);

RETURN <int | string> '1';

RETURN <float | int> 1;

-- Casts over fields and params inside a statement.
SELECT title, <string> views AS views_text, <float> views AS views_float FROM article;

SELECT subject, <string> priority AS priority_text FROM ticket;

SELECT title FROM article WHERE views > <int> '10';

LET $count_text = '5';

RETURN <int> $count_text + 1;

UPDATE ticket:1 SET priority = <int> '4';

-- Prefixed literals.
RETURN d"2024-01-01T00:00:00Z";

RETURN d'2024-06-15T12:30:00.123Z';

RETURN r"article:hello";

RETURN r'ticket:⟨1⟩';

RETURN u"018e0f3a-1234-7abc-8def-0123456789ab";

RETURN u'018e0f3a-1234-7abc-8def-0123456789ab';

RETURN b"48656c6c6f";

RETURN b'';

RETURN s"plain string";

RETURN s'also plain';

SELECT title FROM article WHERE published_at > d'2024-01-01T00:00:00Z';

-- `r'…'` is a record id on the engine (`type::of(r'a:b')` is `record`).
SELECT title FROM article WHERE author = r'account:ada';

SELECT title FROM article WHERE created_at > time::now() - 7d;

SELECT title FROM article WHERE created_at > d'2024-01-01T00:00:00Z' + 1w;

-- Numeric literal forms.
RETURN 1_000_000;

RETURN 1.5e3;

RETURN 0.5f;

RETURN 3dec;

RETURN 1h30m15s;

RETURN 250ms;

RETURN 1y2w3d;

RETURN NaN;

RETURN Infinity;

-- Ranges: bare, inclusive, half-open, and over record ids.
RETURN 1..5;

RETURN 1..=5;

RETURN ..5;

RETURN 1..;

RETURN 'a'..'z';

RETURN article:a..z;

RETURN article:a..=m;

RETURN ticket:1..100;

RETURN ticket:[1]..=[50];

-- The engine binds a cast looser than a range (`<array> 1..5` is `[1, 2, 3,
-- 4]`), so both spellings are the same value.
RETURN <array> 1..5;

RETURN <array> (1..5);

RETURN array::len(<array> (1..5));

RETURN [1, 2, 3, 4, 5][1..3];

RETURN [1, 2, 3, 4, 5][..2];

RETURN [1, 2, 3, 4, 5][3..];

-- Geometry literals in every shape.
RETURN (-0.118092, 51.509865);

RETURN { type: 'Point', coordinates: [-0.118092, 51.509865] };

RETURN { type: 'LineString', coordinates: [[0, 0], [1, 1], [2, 0]] };

RETURN { type: 'Polygon', coordinates: [[[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]] };

RETURN { type: 'MultiPoint', coordinates: [[0, 0], [1, 1]] };

RETURN { type: 'MultiLineString', coordinates: [[[0, 0], [1, 1]], [[2, 2], [3, 3]]] };

RETURN { type: 'MultiPolygon', coordinates: [[[[0, 0], [1, 0], [1, 1], [0, 0]]], [[[2, 2], [3, 2], [3, 3], [2, 2]]]] };

RETURN { type: 'GeometryCollection', geometries: [{ type: 'Point', coordinates: [0, 0] }, { type: 'LineString', coordinates: [[0, 0], [1, 1]] }] };

-- Regex literals.
RETURN /^h.*o$/;

RETURN /[a-z]+-[0-9]+/;

LET $slug_shape = /^[a-z0-9-]+$/;

RETURN 'hello-1' = $slug_shape;

-- Composite literals over all of the above.
RETURN {
	when: d'2024-01-01T00:00:00Z',
	who: r'account:ada',
	id: u'018e0f3a-1234-7abc-8def-0123456789ab',
	raw: b'00ff',
	span: 1..10,
	wait: 2h,
	where: (0.0, 0.0),
	tags: <set<string>> ['a', 'b', 'a']
};

RETURN [<float> 1, <decimal> 2, 3, 4.5, 5dec];
