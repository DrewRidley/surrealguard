-- The far half of the back-reference pair in 06. See that file for why the
-- two directions are split across files.

DEFINE TABLE task SCHEMAFULL;

DEFINE FIELD title ON task TYPE string;

-- The proof that makes 06's `project.tasks` resolve, written after it.
DEFINE FIELD project ON task TYPE record<project> REFERENCE;

-- Backward reference: `milestone` and its `REFERENCE` field are both in 06,
-- before this statement — the order that always worked.
DEFINE FIELD milestones ON project COMPUTED <~milestone;
