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
