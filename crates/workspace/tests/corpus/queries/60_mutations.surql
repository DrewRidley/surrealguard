-- Mutations. Each responds with a shape derived from the target table, and
-- `ONLY` collapses the array — both are snapshot sites.

CREATE organization SET name = 'Acme', legal_name = 'Acme Inc', owner = account:root,
	address = { line1: '1 Main St', line2: NONE, city: 'Springfield', country: 'US' };

CREATE ONLY organization_unit SET name = 'Root', organization = organization:acme;

CREATE organization_role CONTENT { name: 'Owner', organization: organization:acme, permissions: [] };

UPDATE organization SET tier = 'pro' WHERE tier = 'free';

UPDATE ONLY organization:acme SET logo = 'logo.png' RETURN AFTER;

UPSERT organization_settings SET organization = organization:acme;

DELETE organization_unit WHERE headcount = 0;

DELETE organization_role WHERE name = 'Temp' RETURN BEFORE;

RELATE account:root->employee_of->organization:acme SET role = organization_role:owner, unit = organization_unit:root;

INSERT INTO sector { name: 'Retail' };

INSERT INTO sector [{ name: 'Tech' }, { name: 'Energy' }];

UPDATE organization SET name = 'Renamed' RETURN name, tier;
