-- `LIVE SELECT` used to reach analysis as a table name and nothing else, so
-- no clause of it could be wrong. Every line here registers against a real
-- 3.2.3 without complaint, or fails only once the statement runs, which is
-- what makes each one worth a finding: none of them announces itself.

-- Refused while EXECUTING, not while parsing. `LIVE SELECT * FROM account:1`
-- answers with a subscription uuid and then never fires a single
-- notification — the worst shape a mistake can take.
LIVE SELECT * FROM account:1;

-- Refused while parsing: the engine stops at the comma. One subscription
-- watches one table.
LIVE SELECT * FROM account, organization;

-- Accepted, delivered, and quietly not honoured. Verified over ws:// against
-- 3.2.3: the same write that gives `LIVE SELECT * ... FETCH owner` a resolved
-- `{"id":"account:x","username":...}` gives the DIFF form
-- `[{"op":"replace","path":"","value":{"owner":"account:x",...}}]` — the link
-- is left exactly as it was stored, as if the FETCH were not written.
LIVE SELECT DIFF FROM organization FETCH owner;

-- `DIFF` is the diff form only in leading position. One projection later the
-- parser has already committed to a field list, so this reads as the field
-- path `DIFF` — and no table has that field, so every notification carries
-- `{"DIFF":null,"name":"..."}`, forever.
LIVE SELECT name, DIFF FROM organization;
