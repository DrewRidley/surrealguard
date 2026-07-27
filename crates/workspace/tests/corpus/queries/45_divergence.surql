-- Equivalence class: DIVERGENCE.
--
-- A guard that exits narrows everything after it. Whether it *exits* is the
-- question `block_diverges` answers, and it used to answer it by reading the
-- branch body's LAST statement only — so the same exit, written one line from
-- the end or wrapped in a form that carries statements, stopped counting.
--
-- Every guard below exits, so every `$*_name` after one must be `string`
-- rather than an option.

LET $plain = (SELECT name, tier FROM ONLY organization LIMIT 1);

IF $plain = NONE { THROW 'no organization'; };

LET $plain_name = $plain.name;

-- ---- the exit is not the last statement in the branch --------------------
-- Control cannot get past the THROW, so the trailing statement never runs and
-- the branch diverges exactly as the plain one does.
LET $trailing = (SELECT name, tier FROM ONLY organization LIMIT 1);

IF $trailing = NONE { THROW 'no organization'; LET $unreached = 1; };

LET $trailing_name = $trailing.name;

-- ---- the exit is inside a constant guard --------------------------------
-- `IF true { THROW … }` has no reachable fall-through, which is exactly what
-- `block_flow` already stopped on — and what nobody asking from outside could
-- see.
LET $constant = (SELECT name, tier FROM ONLY organization LIMIT 1);

IF $constant = NONE { IF true { THROW 'no organization'; }; };

LET $constant_name = $constant.name;

-- ---- the exit is a parenthesised statement -------------------------------
-- `(THROW …)` lowers to an expression statement wrapping the THROW. A pair of
-- parentheses is a semantic no-op, so it must exit too.
LET $parenthesised = (SELECT name, tier FROM ONLY organization LIMIT 1);

IF $parenthesised = NONE { (THROW 'no organization'); };

LET $parenthesised_name = $parenthesised.name;

-- ---- the exit is a nested block -----------------------------------------
LET $nested = (SELECT name, tier FROM ONLY organization LIMIT 1);

IF $nested = NONE { { THROW 'no organization'; }; };

LET $nested_name = $nested.name;

RETURN {
	plain: $plain_name,
	trailing: $trailing_name,
	constant: $constant_name,
	parenthesised: $parenthesised_name,
	nested: $nested_name
};
