-- Relation tables. Graph traversal precision (`->edge->target`, `<-edge<-in`)
-- is only as good as the IN/OUT sets recorded here, so the corpus keeps a
-- disjoint pair of edges: nothing can accidentally type-check via the wrong one.

DEFINE TABLE organization_permission SCHEMAFULL;

DEFINE FIELD name ON organization_permission TYPE string;

DEFINE TABLE organization_role SCHEMAFULL;

DEFINE FIELD name ON organization_role TYPE string;

DEFINE FIELD organization ON organization_role TYPE record<organization>;

DEFINE FIELD permissions ON organization_role TYPE array<record<organization_permission>> DEFAULT [];

DEFINE TABLE employee_of SCHEMAFULL TYPE RELATION FROM account TO organization;

DEFINE FIELD role ON employee_of TYPE record<organization_role>;

DEFINE FIELD unit ON employee_of TYPE record<organization_unit>;

DEFINE FIELD title ON employee_of TYPE option<string>;

DEFINE FIELD status ON employee_of TYPE 'invited' | 'active' | 'suspended' DEFAULT 'active';

DEFINE FIELD joined_at ON employee_of TYPE datetime DEFAULT time::now();

-- A second edge whose IN/OUT sets do NOT overlap `employee_of`: `assigned_to`
-- goes unit -> role, so it must never be offered while traversing off an
-- `account`.
DEFINE TABLE assigned_to SCHEMAFULL TYPE RELATION FROM organization_unit TO organization_role;

DEFINE FIELD weight ON assigned_to TYPE int DEFAULT 1;
