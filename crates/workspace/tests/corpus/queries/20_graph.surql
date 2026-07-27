-- Graph traversal. The receiver's table decides which edges are traversable;
-- a regression here shows up as a widened (or lost) response kind.

SELECT ->employee_of->organization AS orgs FROM account;

SELECT ->employee_of.title AS titles FROM account;

SELECT <-employee_of<-account AS members FROM organization;

SELECT ->employee_of->organization.name AS org_names FROM account;

SELECT ->employee_of[WHERE status = 'active']->organization AS active_orgs FROM account;

SELECT ->assigned_to->organization_role AS roles FROM organization_unit;

SELECT employees FROM organization;

LET $member = (SELECT VALUE ->employee_of->organization FROM ONLY account LIMIT 1);

LET $edge_titles = (SELECT VALUE ->employee_of.title FROM account);

-- `.*` after a traversal names the LANDED ROW, not a link to it. It expands to
-- exactly what `SELECT * FROM organization` projects — same builder, so the two
-- can never disagree. Engine-verified on SurrealDB 3.0.5:
--   SELECT ->follows->user.* AS r FROM ONLY user:ada -> {r: [{age, id, name}]}
SELECT ->employee_of->organization.* AS orgs FROM account;

-- The wildcard on the edge itself stands on the edge table, exactly as
-- `->employee_of.title` does — including the implicit `id`/`in`/`out`.
SELECT ->employee_of.* AS memberships FROM account;

-- Backward, and after a step-local filter: neither changes what `.*` means.
SELECT <-employee_of<-account.* AS members FROM organization;

SELECT ->employee_of[WHERE status = 'active']->organization.* AS active_orgs FROM account;

-- A field read off a splatted row. `.*` names no path segment, so this is
-- `->employee_of->organization.name`, and the unaliased spelling keys under
-- `->employee_of.->organization.name` with the wildcard dropped — the engine's
-- own key simplification.
SELECT ->employee_of->organization.*.name AS org_names FROM account;

SELECT ->employee_of->organization.* FROM account;

-- `.*` at the END of a field tail, on the edge's implicit `out` link.
SELECT ->employee_of.out.* AS orgs FROM account;

-- A `.{…}` destructure after a traversal selects off the LANDED ROW, and names
-- no output key of its own: the selected object lands whole under the traversal
-- key, one per traversed record. Engine-verified on SurrealDB 3.0.5:
--   SELECT ->follows->user.{name, age} FROM user
--     -> {"->follows": {"->user": [{age, name}]}}
SELECT ->employee_of->organization.{ name, tier } AS orgs FROM account;

SELECT ->employee_of->organization.{ name, tier } FROM account;

-- The empty destructure survives a traversal too (`[{}]` per traversed record).
SELECT ->employee_of->organization.{} AS orgs FROM account;

-- A field read out of the destructured object. The destructure contributes no
-- key, the field behind it does — the unaliased spelling keys under
-- `->employee_of.->organization.name`, verified on 3.0.5.
SELECT ->employee_of->organization.{ name }.name AS org_names FROM account;

SELECT ->employee_of->organization.{ name }.name FROM account;

-- A destructure on the EDGE (a single hop with a tail reads off the edge
-- table), and a field out of it.
SELECT ->employee_of.{ title, status } AS memberships FROM account;

SELECT ->employee_of.{ status }.status AS statuses FROM account;

-- Backward traversal and a step-local filter change neither.
SELECT <-employee_of<-account.{ username } AS members FROM organization;

SELECT ->employee_of[WHERE status = 'active']->organization.{ name }.name AS active FROM account;

-- Every part BEHIND a traversal is checked against the value it stands on —
-- these are the valid spellings of the forms that used to resolve a type and
-- validate nothing. A regression shows up here as a false finding.

-- A plain field, and a dotted chain that crosses the landed row's own link.
SELECT ->employee_of->organization.name AS org_names FROM account;

SELECT ->employee_of->organization.owner.username AS owners FROM account;

-- The edge's implicit `out` link, crossed into the node it names.
SELECT ->employee_of.out.name AS org_names FROM account;

-- A field read off a splatted row: `.*` names no path segment, so the field
-- behind it belongs to the row the splat expanded.
SELECT ->employee_of->organization.*.name AS org_names FROM account;

-- A traversal that RESUMES after a field tail: `owner` is a `record<account>`,
-- so the second traversal steps from `account`, not from `organization`.
-- Checking it against `organization` reported this valid query as two graph
-- violations (3001 + 3002).
SELECT ->employee_of->organization.owner->employee_of->organization AS peers FROM account;

-- A step-local filter does not change which table the tail reads from.
SELECT ->employee_of[WHERE status = 'active']->organization.name AS active_names FROM account;

-- The valid FROM spelling: the leading name is the SOURCE TABLE, not a field
-- of itself, and the traversal off it lands on a table.
SELECT name FROM account->employee_of->organization;
