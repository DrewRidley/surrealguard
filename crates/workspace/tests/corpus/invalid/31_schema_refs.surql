-- 1xxx — schema references. One intended violation per line; the schema half
-- of the valid corpus (including `article`, `ticket`, `likes`) is in scope.

-- 1001: a table that does not exist.
SELECT title FROM ghost_table WHERE id = ghost_table:one;

-- 1002: a field the table does not declare.
SELECT nope FROM article WHERE id = article:hello;

-- 1012: a schema object of the named kind that does not exist.
REBUILD INDEX ghost_index ON article;

-- 1021: removing something that was never defined.
REMOVE TABLE ghost_table;

-- 1022: a second definition without OVERWRITE / IF NOT EXISTS.
DEFINE TABLE article SCHEMAFULL;

-- 1023: FETCH on a field that cannot hold records.
SELECT title FROM article WHERE id = article:hello FETCH views;

-- 1024: SPLIT on a field that is not a collection.
SELECT title FROM article WHERE id = article:hello SPLIT views;

-- 1025: a subfield declared under a parent that is not object-shaped.
DEFINE FIELD views.nested ON article TYPE int;

-- 1027: an index-backed operator whose field has no supporting index.
SELECT title FROM article WHERE title @@ 'hello';
SELECT title FROM article WHERE tags <|2, 40|> ['a'];

-- 1029: a second index over the same field set.
DEFINE INDEX article_author_again ON article FIELDS author;

-- 1032: an analyzer filter naming an unknown language.
DEFINE ANALYZER klingon TOKENIZERS blank FILTERS snowball(klingon);
