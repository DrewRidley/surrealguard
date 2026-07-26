-- Grouping parentheses. `(expr)` is a semantic no-op in SurrealQL, so every
-- line here has an unparenthesized twin elsewhere in the corpus and must infer
-- the same type. While `(…)` lowered to an opaque subquery node it silenced
-- narrowing, field validation and every expression-level rule behind it — a
-- loss no diagnostics-only harness can see, because nothing becomes *wrong*:
-- the type just goes to `unknown` and the rules stop running.

-- Grouping inside a projection.
SELECT (string::len(name)) * 2 AS scaled_length FROM organization;

-- Grouping around an aggregate. The aggregate-promotion recognizer has to see
-- `math::sum(...)` through the parentheses; when it could not, this raised a
-- hard E5002 false positive on valid SurrealQL.
SELECT (math::sum(headcount)) AS total_headcount FROM organization_unit GROUP ALL;

-- Grouping in a WHERE predicate: row narrowing must survive it, so `parent`
-- projects as a plain `record<organization>` rather than an option.
SELECT name, parent FROM organization WHERE (parent != NONE);

-- Grouping around a FROM source: `(organization)` is `organization`, so the
-- rows are schema-typed and the projection is validated against the table.
SELECT name, tier FROM (organization);

-- Grouping in a LET value, and doubled parentheses around a subquery.
LET $grouped = (1 + 2) * 3;

LET $grouped_rows = ((SELECT name FROM organization));

-- Grouping around a flow guard: the `IF ($x = NONE) THEN THROW` narrowing must
-- reach the statements after it exactly as the bare spelling does.
LET $one = (SELECT name, tier FROM ONLY organization LIMIT 1);

IF ($one = NONE) THEN THROW 'no organization' END;

LET $one_name = $one.name;

RETURN { grouped: $grouped, rows: $grouped_rows, one_name: $one_name };
