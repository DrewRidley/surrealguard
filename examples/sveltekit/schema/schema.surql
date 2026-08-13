-- Schema for the SvelteKit example.
--
-- `surrealguard generate` reads these DEFINEs and types every query named with
-- `defineQuery` / `defineLive` (and every `db.query("…")` literal) against them.
-- `scripts/db.mjs` applies this same file to a running SurrealDB, so the schema
-- SurrealGuard analyses and the schema the demo runs on cannot drift.
--
-- Three tables, deliberately: a team, the people on it, and the tickets those
-- people work. An audience reads that in one glance.

-- ---------------------------------------------------------------- teams ----
DEFINE TABLE team SCHEMAFULL PERMISSIONS FULL;
DEFINE FIELD name ON team TYPE string;

-- --------------------------------------------------------------- people ----
-- Open on purpose: the roster is public in this demo, so beats 1-3 (parameter
-- tracking, live updates, nesting) behave identically whoever is signed in.
-- The access control beat lives on `ticket`, below, where it is the only thing
-- changing and therefore the only thing to look at.
DEFINE TABLE person SCHEMAFULL PERMISSIONS FULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD team ON person TYPE record<team>;
-- `option<…>`, because not everyone in the roster has a login — and because it
-- keeps "add a person" a three-parameter write. Make them required instead and
-- SurrealGuard says so immediately: E2034, "`email` must be set when creating a
-- `person`", on the CREATE in `src/lib/queries.ts`.
DEFINE FIELD email ON person TYPE option<string>;
-- The hash never leaves the database: no record user can select it, and the
-- SIGNIN query below runs with full access, so `crypto::argon2::compare` still
-- sees it.
DEFINE FIELD password ON person TYPE option<string> PERMISSIONS NONE;

-- -------------------------------------------------------------- tickets ----
-- The access-control beat. A record user sees only their own team's tickets;
-- a root/system user bypasses permissions and sees all of them. Signing in as
-- someone else visibly changes the rows on screen, and nothing in the query
-- text says so — `$auth` does.
DEFINE TABLE ticket SCHEMAFULL
  PERMISSIONS
    FOR select WHERE team = $auth.team
    FOR create, update, delete NONE;
DEFINE FIELD title ON ticket TYPE string;
DEFINE FIELD team ON ticket TYPE record<team>;
DEFINE FIELD done ON ticket TYPE bool DEFAULT false;

-- --------------------------------------------------------------- access ----
-- Record-level authentication, defined in SurrealQL rather than in the app.
-- `db.signin({ access: "staff", variables: { email, password } })` runs the
-- SIGNIN query; whatever record it returns becomes `$auth` for that session.
DEFINE ACCESS staff ON DATABASE TYPE RECORD
  SIGNUP (
    CREATE person SET
      name = $name,
      email = $email,
      age = $age,
      team = $team,
      password = crypto::argon2::generate($password)
  )
  SIGNIN (
    SELECT * FROM person
    WHERE email = $email AND crypto::argon2::compare(password, $password)
  )
  DURATION FOR TOKEN 15m, FOR SESSION 12h;
