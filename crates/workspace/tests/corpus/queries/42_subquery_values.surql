-- Equivalence class: SUBQUERIES AS VALUES.
--
-- A `(SELECT …)` in value position is an expression like any other, and
-- wrapping it in more parentheses, indexing it, or reaching it through a `LET`
-- must not change what it denotes. `FROM ONLY … LIMIT 1` is the cardinality
-- proof that turns the array into an `option<row>`; `count()` under `GROUP ALL`
-- is the aggregate whose promotion has to survive the same wrappings.

-- ---- the array form and its aliases -------------------------------------
LET $rows = (SELECT name, tier FROM organization);

-- Doubled parentheses. Equivalent to `$rows`.
LET $rows_paren = ((SELECT name, tier FROM organization));

-- Reached through an alias. Equivalent to `$rows`.
LET $rows_alias = $rows;

-- ---- the ONLY form -------------------------------------------------------
-- `FROM ONLY … LIMIT 1` proves at most one row, so the value is an option.
LET $one = (SELECT name, tier FROM ONLY organization LIMIT 1);

-- (`LIMIT (1)` and `LIMIT 1 + 0` — the const-foldable limits `literal_limit`
-- declines — are rejected by the grammar, which takes a bare literal there, so
-- they cannot be covered from the corpus.)

-- Indexing the array form is the other way to reach one row. Note it is NOT
-- equivalent to `$one`: `[0]` on a possibly-empty array is not a cardinality
-- proof, so a wider answer here is correct.
LET $one_indexed = (SELECT name, tier FROM organization LIMIT 1)[0];

-- A UNIQUE-indexed equality predicate is the second cardinality proof.
LET $one_unique = (SELECT title FROM ONLY document WHERE title = 'spec');

-- …and the parenthesised spelling of the same predicate. Equivalent to
-- `$one_unique`, including the absence of a W4026.
LET $one_unique_paren = (SELECT title FROM ONLY document WHERE (title = 'spec'));

-- ---- count() -------------------------------------------------------------
LET $count = (SELECT VALUE count() FROM organization GROUP ALL)[0];

-- Parenthesised aggregate. Equivalent to `$count`.
LET $count_paren = (SELECT VALUE (count()) FROM organization GROUP ALL)[0];

-- The `?? 0` spelling that makes the empty-result case total. Equivalent to
-- `$count` plus a fallback.
LET $count_or_zero = (SELECT VALUE count() FROM organization GROUP ALL)[0] ?? 0;

-- An aggregate inside an arithmetic expression, bare and parenthesised. These
-- must agree, and neither may raise E5002: `math::sum` under `GROUP ALL` takes
-- the whole column.
SELECT math::sum(headcount) * 2 AS doubled FROM organization_unit GROUP ALL;

SELECT (math::sum(headcount)) * 2 AS doubled FROM organization_unit GROUP ALL;

-- ---- a subquery as a FROM source ----------------------------------------
-- `(organization)` is `organization`, so the projection is schema-validated
-- and typed either way.
SELECT name, tier FROM organization;

SELECT name, tier FROM (organization);

-- ---- a subquery as a predicate operand ----------------------------------
SELECT name FROM organization WHERE name IN (SELECT VALUE name FROM organization);

RETURN {
	rows: $rows,
	rows_paren: $rows_paren,
	rows_alias: $rows_alias,
	one: $one,
	one_indexed: $one_indexed,
	one_unique: $one_unique,
	one_unique_paren: $one_unique_paren,
	count: $count,
	count_paren: $count_paren,
	count_or_zero: $count_or_zero
};
