-- An event's WHEN runs against the row that triggered it, so a bare name in
-- it is a field of the target table. Nothing checked that.
--
-- The only resolution the event path performed was over `$event.<field>` —
-- and `$event` is the string `'CREATE' | 'UPDATE' | 'DELETE'`, so that check
-- could never see a row field at all. A WHEN naming a field the table does
-- not have defines cleanly and then simply never fires, which is the quietest
-- possible way for a trigger to be wrong.

-- The plain case: `account` has no `ghost`.
DEFINE EVENT account_ghost ON account WHEN ghost = 1 THEN {};

-- Beside the idiomatic `$event` guard, where the row read is easiest to miss.
DEFINE EVENT account_ghost_guarded ON account
	WHEN $event = 'UPDATE' AND ghost != NONE
	THEN {};

-- The walk descends the same shapes every other condition's does, so a read
-- buried in a call argument is caught too.
DEFINE EVENT account_ghost_call ON account
	WHEN string::len(ghost) > 0
	THEN {};

-- And it crosses a record link, like every other condition: `account.person`
-- is an `option<record<person>>` and `person` has no `ghost`.
DEFINE EVENT account_link_ghost ON account
	WHEN person.ghost = 1
	THEN {};
