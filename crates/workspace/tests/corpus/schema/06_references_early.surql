-- Record-reference back-links (`COMPUTED <~t`) across files, in BOTH
-- declaration orders. A back-reference is mutual: `<~task` is only resolvable
-- because `task` carries a `record<project> REFERENCE` field, so one of the two
-- tables is ALWAYS declared after the field that traverses it. Resolution must
-- not depend on which file sorts first, so this pair splits the two directions
-- across two files deliberately:
--
--   * `project.tasks` traverses `<~task`, and `task` lives in the LATER file;
--   * `project.milestones` (in the later file) traverses `<~milestone`, whose
--     proof lives HERE, in the earlier one.
--
-- Both must record the same shape. If either regresses to `unknown`, the
-- resolution has become order-dependent again.

DEFINE TABLE project SCHEMAFULL;

DEFINE FIELD name ON project TYPE string;

-- Forward reference: `task` is defined in 07, after this statement.
DEFINE FIELD tasks ON project COMPUTED <~task;

DEFINE TABLE milestone SCHEMAFULL;

DEFINE FIELD label ON milestone TYPE string;

DEFINE FIELD project ON milestone TYPE record<project> REFERENCE;

-- The deliberate silence: `sector` exists but no `record<project> REFERENCE`
-- field points back at `project`, so the traversal is genuinely unprovable.
-- It stays untyped and raises NOTHING — only a target that exists nowhere is a
-- typo worth reporting.
DEFINE FIELD unlinked ON project COMPUTED <~sector;
