-- The other side of `queries/83_row_field_guards.surql`: a guard narrows a row
-- field, and the narrowed kind is what the body is held to. Every line here was
-- silent, or reported the wrong thing about the wrong kind, while row-field
-- facts were being discarded.

-- Narrowed to the arm that does NOT answer the call. `word_count` is
-- `option<int>`, the guard leaves `int`, and `int` has no `uppercase` — a
-- sharper finding than the `none | int` one the declared kind would give, and a
-- finding at all only because the guard was read.
SELECT (IF type::is_int(word_count) { word_count.uppercase() } ELSE { '' }) AS shouted FROM document;

-- The negative branch is narrowed too: past `type::is_string`, `summary` is
-- what is left of `option<string | null>`, and neither `none` nor `null` has a
-- `len`.
SELECT (IF type::is_string(summary) { 0 } ELSE { summary.len() }) AS backwards FROM document;

-- The sentinel guard's dead side. `word_count = NONE` pins the field to `none`,
-- so the arithmetic in that branch is on a `none`.
SELECT (IF word_count = NONE { word_count + 1 } ELSE { 0 }) AS bumped_wrong FROM document;

-- Short-circuit, negatively: `OR` evaluates its right operand only where the
-- left did NOT hold, so `summary` is a `none | null` there.
SELECT title FROM document WHERE type::is_string(summary) OR summary.len() > 10;
