-- `LIVE SELECT`, in every form the engine accepts *and* honours. Each line
-- was registered against a real 3.2.3 over ws://, written to, and the raw
-- notification frames read back; each one carries what the statement says it
-- does. They are here so the live contract cannot be tightened into them.

LIVE SELECT * FROM organization;

LIVE SELECT name FROM organization;

LIVE SELECT name AS display FROM organization;

-- Computed and aliased projections are evaluated against the changed record,
-- which is the only row a notification ever has.
LIVE SELECT string::uppercase(name) AS shout FROM organization;

-- A dotted path through a link resolves per notification.
LIVE SELECT owner.username AS owner_name FROM organization;

-- `VALUE` delivers the bare value rather than a one-key object.
LIVE SELECT VALUE name FROM organization;

-- `DIFF` delivers a JSON-Patch array instead of a row. On its own it is
-- exactly what it says; only pairing it with FETCH is a mistake.
LIVE SELECT DIFF FROM organization;

-- WHERE is per-row, so a change stream can evaluate it. It applies on
-- CREATE, UPDATE and DELETE alike, including through a link path.
LIVE SELECT * FROM organization WHERE tier = 'pro';

LIVE SELECT * FROM organization WHERE owner.username = 'root';

-- FETCH is per-row too, and is honoured: the link comes back resolved in the
-- notification, on all three actions.
LIVE SELECT * FROM organization FETCH owner;

LIVE SELECT * FROM organization WHERE tier = 'pro' FETCH owner;

-- A graph traversal in the projection is re-evaluated per notification.
LIVE SELECT username, ->employee_of->organization AS employers FROM account;
