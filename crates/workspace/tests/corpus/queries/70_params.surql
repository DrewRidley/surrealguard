-- Host-supplied parameters. Their inferred kind is the contract a host adapter
-- enforces at the call site, so it is a first-class snapshot site.

SELECT name, tier FROM organization WHERE name = $org_name;

SELECT * FROM organization WHERE tier = $tier LIMIT $page_size;

SELECT name FROM ONLY organization WHERE id = $org_id LIMIT 1;

-- A `$param` in an object-literal position is constrained by the target field's
-- declared subfield, so the nested params carry real kinds into the host
-- contract (they used to stay `any` and trip the write check).
CREATE organization SET name = $new_name, legal_name = $new_legal, owner = $owner_id,
	address = { line1: $line1, line2: NONE, city: $city, country: 'US' };

LET $tier_default = $default_tier;

UPDATE organization_unit SET headcount = $headcount WHERE organization = $org_id;

SELECT * FROM employee_of WHERE joined_at > $since;
