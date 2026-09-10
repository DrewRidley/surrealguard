-- A condition reads a field path exactly as a projection of it does. Every
-- line here crosses a record link into a table that does not declare the
-- trailing segment — the same mistake as the `SELECT person.ghost` this corpus
-- has always caught — and every one of them was SILENT: a condition resolved
-- its paths with the non-crossing resolver, so it caught `WHERE ghost = 1` and
-- nothing whatsoever past a link.

-- The one-hop case, in a SELECT filter. `account.person` is a
-- `record<person>`, and `person` has no `ghost`.
SELECT username FROM account WHERE person.ghost = 1;

-- Multi-hop. `organization.owner` is a `record<account>` whose `person` is a
-- `record<person>`; the finding lands on `person`, the table actually missing
-- the field, not on the table the query names.
SELECT name FROM organization WHERE owner.person.ghost = 1;

-- Depth in the condition hides nothing: the walk descends both the boolean
-- operators and the parens.
SELECT name FROM organization WHERE tier = 'pro' AND (owner.ghost = 1 OR name = 'x');

-- ...including into a call argument.
SELECT name FROM organization WHERE string::len(owner.ghost) > 0;

-- A link naming SEVERAL tables is wrong only when NO arm declares the field.
-- That is the projection's rule, and now it is the condition's rule too:
-- `document.owner` is a `record<account | organization>` and neither declares
-- `ghost`.
SELECT title FROM document WHERE owner.ghost = 1;

-- A mutation's filter is the same condition in a different statement, and it
-- was silent for the same reason.
UPDATE organization SET tier = 'pro' WHERE owner.ghost = 1;

DELETE organization WHERE owner.ghost = 1;

-- A graph step's own filter runs against the edge table, and a link out of the
-- edge is crossed there too: `employee_of.role` is a `record<organization_role>`
-- which has no `ghost`.
SELECT ->employee_of[WHERE role.ghost = 1] AS jobs FROM account;
