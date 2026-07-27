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

-- A wildcard SEEDS the row; sibling projections are layered on top of it, and
-- a bare field renamed to a bare alias replaces the field it renames.
SELECT *, name AS org_name FROM organization;

SELECT *, ->employee_of AS memberships FROM account;

SELECT *, string::len(name) AS name_length FROM organization;

SELECT *, address.city AS city FROM organization;

SELECT *, address.{ city } FROM organization;

SELECT *, owner AS org_owner, tier AS plan FROM organization;

SELECT VALUE name FROM organization;

SELECT name FROM ONLY organization WHERE tier = 'pro' LIMIT 1;

SELECT id, name FROM organization ORDER BY name LIMIT 10 START 5;

-- `.*` on a record link is not an index — it names the row the link points at,
-- and yields ONE object, not an array (`SELECT author.* AS r FROM ONLY post:p1`
-- -> `{r: {id, name}}` on 3.0.5). The wrappers a link is reached through are
-- kept, so `array<record<sector>>` splats to `array<{…}>`.
SELECT owner.* AS owner_row FROM organization;

SELECT sectors.* AS sector_rows FROM organization;

SELECT owner.*.username AS owner_name FROM organization;

-- A link naming SEVERAL tables splats to the union of their rows: the engine
-- hands back whichever row the link happens to point at.
SELECT owner.* AS owner_row FROM document;

-- An EMPTY destructure is valid SurrealQL and evaluates to the empty object.
-- It used to be a parse error, which is fatal to a whole source rather than to
-- one expression. Engine-verified on SurrealDB 3.0.5:
--   SELECT id.{} FROM user:ada            -> [{id: {}}]
--   SELECT VALUE id.{} FROM ONLY user:ada -> {}
--   SELECT *, author.{} FROM post         -> [{author: {}, id, title}]
SELECT id.{} FROM organization;

SELECT owner.{} AS anonymous FROM organization;

SELECT *, address.{} FROM organization;

SELECT VALUE address.{} FROM organization;

-- `SELECT VALUE <path>.{…}` is the destructured OBJECT, not a row keyed by the
-- path it was read from (3.0.5: `SELECT VALUE author.{name} FROM post` ->
-- `[{name: 'Ada'}]`).
SELECT VALUE address.{ city } FROM organization;

-- A field read out of what a destructure built: the selected key resolves
-- through the object, exactly as it would through the row.
SELECT address.{ city }.city AS city FROM organization;

SELECT owner.{ username }.username AS owner_name FROM organization;
