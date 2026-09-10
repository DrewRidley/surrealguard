-- A guard over a bare ROW field, narrowing the branch body. `$param` guards
-- have worked since narrowing existed; a row field's guard was inert, so every
-- query below reported the declared kind's contract violation inside a branch
-- that had ruled it out.
--
-- Nothing here is about `IF` in particular: the same facts reach an `AND`'s
-- right operand and an `ELSE`, and both are exercised.

-- `type::is_*` on a row field. `summary` is `option<string | null>`; the guard
-- leaves the `string`, which is the only arm with a `len`.
SELECT (IF type::is_string(summary) { summary.len() } ELSE { 0 }) AS summary_length FROM document;

-- The sentinel spelling of the same thing, on an `option<int>`.
SELECT (IF word_count != NONE { word_count + 1 } ELSE { 0 }) AS bumped FROM document;

-- The ELSE sees the negation of every branch condition, so the arithmetic there
-- is on the narrowed field too.
SELECT (IF word_count = NONE { 0 } ELSE { word_count + 1 }) AS bumped_else FROM document;

-- Short-circuit: `AND` evaluates its right operand only where the left held, so
-- the right operand reads the narrowed field.
SELECT title FROM document WHERE type::is_string(summary) AND summary.len() > 10;

-- A guard on a nested path narrows that path and not its siblings.
SELECT (IF address.line2 != NONE { address.line2.len() } ELSE { 0 }) AS line2_length FROM organization;
