-- `id` refuses four of DEFINE FIELD's clauses, and the engine refuses them by
-- failing the definition outright — "Cannot use the `VALUE` keyword on the
-- `id` field". A record's identity is fixed when the row is created and is
-- what every link and index addresses it by, so a clause that would recompute
-- it on write has nowhere to land. We accepted all four in silence.
--
-- `OVERWRITE` throughout, so each line pins 1033 and not the redefinition
-- warning that would otherwise sit on top of it. It is also what the probes
-- against 3.2.3 had to use: without it a second `DEFINE FIELD id` fails as a
-- duplicate, which masks the answer to the question actually being asked.
--
-- On a table of this file's own: every invalid fixture shares one workspace,
-- and a `DEFINE FIELD id … VALUE rand::uuid()` on `account` would retype the
-- `id` every other fixture compares record ids against.
DEFINE TABLE id_probe SCHEMAFULL;

DEFINE FIELD OVERWRITE id ON id_probe VALUE rand::uuid();

DEFINE FIELD OVERWRITE id ON id_probe READONLY;

DEFINE FIELD OVERWRITE id ON id_probe COMPUTED rand::uuid();

DEFINE FIELD OVERWRITE id ON id_probe DEFAULT ALWAYS rand::uuid();

-- One definition may carry several of them, and each is its own rejection.
DEFINE FIELD OVERWRITE id ON id_probe READONLY VALUE rand::uuid();
