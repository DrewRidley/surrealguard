-- Parentheses around a graph target are not a wall analysis stops at.
-- `->(X …)` means what `->X` means; everything the parenthesized form adds —
-- a filter, several targets, a `SELECT … FROM` projection — sits INSIDE the
-- traversal and had been read by nobody, so a query wrong in exactly the way
-- the unparenthesized spelling reports came back clean and `any`.
--
-- The schema half of the valid corpus is in scope here, so these are wrong
-- queries about real tables.

-- The landing half of a hop: `organization` is what the step produced, so its
-- filter is checked against `organization`. Both halves of this one are wrong
-- — an undeclared field, and an operator that cannot combine the operands.
SELECT ->employee_of->(organization WHERE nope = 1) AS o FROM account;
SELECT ->employee_of->(organization WHERE name > 1) AS o FROM account;

-- The edge half, for contrast: `joined_at` is `employee_of`'s, and the same
-- comparison is just as wrong there. This half already reported; it is here so
-- the two are pinned together and cannot drift apart again.
SELECT ->(employee_of WHERE joined_at > 1)->organization AS o FROM account;

-- A multi-target landing lists node tables, not edges. `organization` is on
-- `employee_of`'s far side; `sector` is not, and that is the contract.
SELECT ->employee_of->(organization, sector) AS o FROM account;

-- A `SELECT … FROM` inside the step projects the rows it reached, so its
-- fields belong to the table it landed on. `nope` is not one of them.
SELECT ->employee_of->(SELECT nope FROM organization) AS o FROM account;

-- Targets the vendored grammar admits in this position and SurrealDB's own
-- parser rejects (3.2.3): the destructure and the splat belong AFTER the
-- closing paren, not inside it. Each was silently `any`.
SELECT ->employee_of->(organization.{ name }) AS o FROM account;
SELECT ->employee_of->(organization.*) AS o FROM account;

-- A step's own LIMIT/START is the statement's clause written somewhere else,
-- and the parentheses are not a place where a negative row count becomes
-- acceptable — the engine rejects both outright.
SELECT ->employee_of->(organization LIMIT -1) AS o FROM account;
SELECT ->employee_of->(organization START -2) AS o FROM account;

-- A traversal resolves a type for its tail; nothing validated what the tail
-- NAMED. Every form below resolved cleanly and reported nothing.

-- A plain field on the landed row, and on the edge.
SELECT ->employee_of->organization.nope AS o FROM account;
SELECT ->employee_of.nope AS o FROM account;

-- A dotted chain that crosses the landed row's own link, and the edge's
-- implicit `out` link.
SELECT ->employee_of->organization.owner.nope AS o FROM account;
SELECT ->employee_of.out.nope AS o FROM account;

-- A field read off a splatted row: `.*` names no path segment, so `nope` is
-- still `organization`'s field to have.
SELECT ->employee_of->organization.*.nope AS o FROM account;

-- Behind an index, behind `[$]`, and behind a step's own filter — none of them
-- change which table the field belongs to.
SELECT ->employee_of->organization[0].nope AS o FROM account;
SELECT ->employee_of->organization[$].nope AS o FROM account;
SELECT ->employee_of[WHERE status = 'active']->organization.nope AS o FROM account;

-- In front of a method call: the receiver is wrong before the method is.
SELECT ->employee_of->organization.nope.len() AS o FROM account;

-- A method the receiver's kind does not have. The traversal is an array, and
-- SurrealDB says so: "no such method found for the array type" (3.2.3).
SELECT ->employee_of->organization.nosuchmethod() AS o FROM account;

-- In a WHERE, not a projection: the same tail, the same contract.
SELECT username FROM account WHERE ->employee_of->organization.nope = 1;

-- A traversal that resumes after a field tail whose field does not exist.
SELECT ->employee_of->organization.nope->employee_of->organization AS o FROM account;

-- FROM resolves its own idiom rather than walking it, so a field named in a
-- FROM traversal is checked there or nowhere. Both spellings were silent.
SELECT username FROM account->employee_of->organization.nope;
SELECT username FROM account->employee_of.nope;
