-- NONE / NULL sentinel narrowing. Verified against the engine on a table with
-- one NULL row, one absent row and one string row:
--
--   SELECT id, r FROM nt WHERE r != NONE  ->  the NULL row AND the string row
--   SELECT id, r FROM nt WHERE r != NULL  ->  the absent row AND the string row
--
-- so each guard removes only its own sentinel. A row-side and a param-side
-- recognizer that disagree about this ship a wrong type, not merely a wide one.

-- Row side: `!= NONE` keeps the `null`, `!= NULL` keeps the option.
SELECT deactivated_reason FROM account WHERE deactivated_reason != NONE;

SELECT deactivated_reason FROM account WHERE deactivated_reason != NULL;

-- Param side: the same two facts, reached through a flow guard rather than a
-- WHERE. These must agree with the rows above.
LET $reason = (SELECT VALUE deactivated_reason FROM ONLY account LIMIT 1);

LET $after_none_guard = IF $reason != NONE THEN $reason ELSE 'never set' END;

LET $after_null_guard = IF $reason != NULL THEN $reason ELSE 'cleared' END;

-- `??` is the one place both sentinels go: it falls through on NONE and NULL.
LET $coalesced = $reason ?? 'none given';

RETURN {
	after_none_guard: $after_none_guard,
	after_null_guard: $after_null_guard,
	coalesced: $coalesced
};
