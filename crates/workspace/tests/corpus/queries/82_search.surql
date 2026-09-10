-- Index-backed operators: full-text matching (`@@`, `@n@` with `search::*`)
-- against the FULLTEXT index on `article.body`, and KNN (`<|k, ef|>` on the
-- HNSW index, `<|k, DISTANCE|>` brute force) against `article.embedding`.
-- Verified on SurrealDB 3.2.3, where the bare `<|k|>` form is rejected.

SELECT title FROM article WHERE body @@ 'quick fox';

SELECT title, search::score(1) AS score FROM article WHERE body @1@ 'quick fox' ORDER BY score DESC;

SELECT title, search::highlight('<b>', '</b>', 1) AS highlighted FROM article WHERE body @1@ 'fox';

SELECT title, search::offsets(1) AS offsets FROM article WHERE body @1@ 'fox';

SELECT title, search::score(1) + search::score(2) AS score FROM article WHERE body @1@ 'fox' OR body @2@ 'dog';

SELECT title FROM article WHERE body @@ 'fox' AND views > 0 LIMIT 10;

SELECT title FROM article WHERE body @@ 'fox' LIMIT $search_limit;

SELECT VALUE title FROM article WHERE body @@ 'fox';

SELECT title FROM article WHERE embedding <|2, 40|> [0.1, 0.2, 0.3];

SELECT title, vector::distance::knn() AS distance FROM article WHERE embedding <|3, 100|> [0.1, 0.2, 0.3] ORDER BY distance;

SELECT title FROM article WHERE embedding <|2, COSINE|> [0.1, 0.2, 0.3];

SELECT title FROM article WHERE embedding <|2, EUCLIDEAN|> [0.1, 0.2, 0.3];

SELECT title, vector::similarity::cosine(embedding, [0.1, 0.2, 0.3]) AS similarity FROM article ORDER BY similarity DESC LIMIT 5;

SELECT title, vector::distance::euclidean(embedding, [0.0, 0.0, 0.0]) AS distance FROM article WHERE views > 0 ORDER BY distance LIMIT 5;

LET $query_vector = [0.1, 0.2, 0.3];

SELECT title FROM article WHERE embedding <|4, 40|> $query_vector;

LET $term = 'fox';

SELECT title FROM article WHERE body @@ $term;
