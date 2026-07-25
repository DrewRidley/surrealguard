-- Projection precision: bare fields, aliases, nested destructure, `[*]` over a
-- refined `array<object>`, and traversal through an `option<record<…>>` link.

SELECT name, tier, created_at FROM organization;

SELECT name AS org_name, legal_name AS legal FROM organization;

SELECT address.city, address.line2 FROM organization;

SELECT address.{ city, country } FROM organization;

SELECT contacts[*].email FROM organization;

SELECT contacts[WHERE primary = true].label FROM organization;

SELECT owner.username, owner.settings.timezone FROM organization;

SELECT person.first_name, person.last_name FROM account;

SELECT * FROM organization_settings;

SELECT VALUE name FROM organization;

SELECT name FROM ONLY organization WHERE tier = 'pro' LIMIT 1;

SELECT id, name FROM organization ORDER BY name LIMIT 10 START 5;
