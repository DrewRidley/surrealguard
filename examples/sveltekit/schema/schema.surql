-- Schema for the SvelteKit example. `surrealguard generate` types every
-- embedded `db.query(...)` / `db.live(\`...\`)` against these DEFINEs.

DEFINE TABLE team SCHEMAFULL;
DEFINE FIELD name ON team TYPE string;

DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD team ON person TYPE record<team>;
