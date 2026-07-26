-- Record-reference back-links, projected. The schema half lives in
-- `schema/06_references_early.surql` and `schema/07_references_late.surql`,
-- which declare the same pair of `COMPUTED <~t` fields in opposite file
-- orders. The point of projecting them here is the OTHER half of the bug they
-- pin: the schema index and the query path used to disagree — a `<~t` whose
-- target was declared later read `unknown` from the field kind while a SELECT
-- of the very same field typed correctly, because the query path sees every
-- other source and the run-wide catalog only saw what preceded it.
--
-- Both fields must project the same shape they record in `== schema: fields ==`.

-- Target declared LATER than the traversal (07 after 06).
SELECT tasks FROM project;

-- Target declared EARLIER than the traversal (06 before 07).
SELECT milestones FROM project;

-- Both at once, beside a plain field, so a widening shows up here too.
SELECT name, tasks, milestones FROM project;
