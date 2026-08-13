-- Seed data for the demo.
--
-- Deliberately tiny: two teams, five people, six tickets. A large seed was
-- observed to drop live subscriptions during the initial flood, and there is
-- nothing an audience gains from row two hundred.
--
-- This file lives under `scripts/` rather than `schema/` on purpose:
-- `surrealguard.toml` scans `schema/**/*.surql` as SCHEMA and `queries/**` as
-- queries, so data lives where neither glob reaches it.

-- `scripts/db.mjs` drops the database, applies `schema/schema.surql` and then
-- this file — so the schema SurrealGuard analyses and the schema this data
-- lands in are the same bytes.

CREATE team:red   SET name = "Red";
CREATE team:blue  SET name = "Blue";

-- `ada` and `grace` have logins (password: "demo"). The rest do not, which is
-- what `option<string>` on `email` / `password` is saying.
CREATE person:ada SET
  name = "Ada", age = 36, team = team:red,
  email = "ada@example.com", password = crypto::argon2::generate("demo");
CREATE person:grace SET
  name = "Grace", age = 45, team = team:blue,
  email = "grace@example.com", password = crypto::argon2::generate("demo");
CREATE person:alan       SET name = "Alan",       age = 41, team = team:red;
CREATE person:katherine  SET name = "Katherine",  age = 29, team = team:blue;
CREATE person:barbara    SET name = "Barbara",    age = 33, team = team:red;

CREATE ticket:t1 SET title = "Ship the parser",      team = team:red,  done = true;
CREATE ticket:t2 SET title = "Fix the flaky test",   team = team:red,  done = false;
CREATE ticket:t3 SET title = "Write the runbook",    team = team:red,  done = false;
CREATE ticket:t4 SET title = "Rotate the certs",     team = team:blue, done = true;
CREATE ticket:t5 SET title = "Upgrade the runtime",  team = team:blue, done = false;
CREATE ticket:t6 SET title = "Draft the changelog",  team = team:blue, done = false;
