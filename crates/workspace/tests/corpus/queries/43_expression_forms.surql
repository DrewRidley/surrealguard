-- Equivalence class: EXPRESSION FORMS THAT CARRY A VALUE.
--
-- `IF … THEN … ELSE … END` as an expression, `??`, and casts each produce a
-- value whose kind depends on facts learned elsewhere in the same expression.
-- Spellings that denote the same value must record the same kind.
--
-- (SurrealQL has no `CASE` statement — the multi-way form is an `IF … ELSE IF …
-- ELSE … END` chain, which is what `$if_chain` below spells.)

LET $p = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $n = (SELECT VALUE word_count FROM ONLY document LIMIT 1);

-- ---- IF as an expression -------------------------------------------------
LET $if_expr = IF $p != NONE THEN $p ELSE 'fallback' END;

-- The block-bodied spelling of the same expression. Equivalent to `$if_expr`.
LET $if_block = IF $p != NONE { RETURN $p } ELSE { RETURN 'fallback' };

-- A multi-way chain. The last arm is reached only when both guards failed, so
-- it sees `$p = NONE`.
LET $if_chain = IF $p != NONE THEN $p ELSE IF $n != NONE THEN 'counted' ELSE 'fallback' END;

-- An `IF` with no `ELSE` can fall off the end, so its value admits NONE.
LET $if_no_else = IF $p != NONE THEN $p END;

-- A parenthesised `IF` expression. Equivalent to `$if_expr`.
LET $if_paren = (IF $p != NONE THEN $p ELSE 'fallback' END);

-- ---- ?? ------------------------------------------------------------------
-- `??` falls through on BOTH sentinels, so it is total over `option<T>`.
LET $coalesce = $p ?? 'fallback';

-- The `IF`-expression spelling of the same thing. Equivalent to `$coalesce`.
LET $coalesce_as_if = IF $p != NONE THEN $p ELSE 'fallback' END;

-- Parenthesised operands. Equivalent to `$coalesce`.
LET $coalesce_paren = ($p) ?? ('fallback');

-- Chained: the first non-sentinel wins.
LET $coalesce_chain = $p ?? $p ?? 'fallback';

-- `??` after a guard has already removed the sentinel: the fallback is dead,
-- and the result is the guarded kind either way.
LET $coalesce_after_guard = IF $p != NONE THEN $p ?? 'fallback' ELSE 'fallback' END;

-- ---- casts ---------------------------------------------------------------
LET $cast_int = <int> '42';

-- Parenthesised operand. Equivalent to `$cast_int`.
LET $cast_paren = <int> ('42');

-- A cast is the value-side spelling of a kind claim, so casting a guarded
-- value must agree with casting a plain one.
LET $cast_guarded = IF $p != NONE THEN <string> $p ELSE 'fallback' END;

-- Casting the optional directly: the cast is the claim, so the result is not
-- optional.
LET $cast_option = <string> ($p ?? '');

-- A cast to the kind a value already has is the identity.
LET $cast_identity = <string> 'already a string';

-- ---- the same forms in a projection --------------------------------------
-- (An `IF … END` is not accepted as a projection expression by the grammar, so
-- only `??` and the cast are covered on the row side.)
SELECT summary ?? 'fallback' AS coalesced FROM document;

SELECT <string> title AS cast FROM document;

SELECT (summary) ?? ('fallback') AS coalesced_paren FROM document;

RETURN {
	if_expr: $if_expr,
	if_block: $if_block,
	if_chain: $if_chain,
	if_no_else: $if_no_else,
	if_paren: $if_paren,
	coalesce: $coalesce,
	coalesce_as_if: $coalesce_as_if,
	coalesce_paren: $coalesce_paren,
	coalesce_chain: $coalesce_chain,
	coalesce_after_guard: $coalesce_after_guard,
	cast_int: $cast_int,
	cast_paren: $cast_paren,
	cast_guarded: $cast_guarded,
	cast_option: $cast_option,
	cast_identity: $cast_identity
};
