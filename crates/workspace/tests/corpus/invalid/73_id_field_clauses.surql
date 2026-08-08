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

DEFINE FIELD OVERWRITE id ON account VALUE rand::uuid();

DEFINE FIELD OVERWRITE id ON account READONLY;

DEFINE FIELD OVERWRITE id ON account COMPUTED rand::uuid();

DEFINE FIELD OVERWRITE id ON account DEFAULT ALWAYS rand::uuid();

-- One definition may carry several of them, and each is its own rejection.
DEFINE FIELD OVERWRITE id ON organization READONLY VALUE rand::uuid();
