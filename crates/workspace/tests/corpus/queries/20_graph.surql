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
