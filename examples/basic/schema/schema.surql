-- The schema SurrealGuard analyzes. `surrealguard generate` reads these
-- DEFINEs to type every embedded `db.query(...)` against them.

DEFINE TABLE team SCHEMAFULL;
DEFINE FIELD name ON team TYPE string;

DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD team ON person TYPE record<team>;
