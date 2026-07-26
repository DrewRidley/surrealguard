-- Equivalence class: BOOLEAN COMPOSITION.
--
-- `AND`, `OR`, `!` and parentheses compose facts. The De Morgan pairs below are
-- literally the same predicate written two ways, so each pair must record the
-- same kind:
--
--   `!(a = NONE OR b = NONE)`  ==  `a != NONE AND b != NONE`
--   `!(a != NONE AND b != NONE)` == `a = NONE OR b = NONE`
--
-- and a parenthesised predicate is the unparenthesised one, always.
--
-- (SurrealQL has no prefix `NOT` keyword — the grammar's `NOT` appears only in
-- `NOT IN` and `IS NOT` — so `!` is the only negation spelling here.)

LET $p = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $q = (SELECT VALUE legal_name FROM ONLY organization LIMIT 1);

-- ---- conjunction ---------------------------------------------------------
LET $and_both = IF $p != NONE AND $q != NONE THEN $p ELSE 'fallback' END;

-- The `&&` spelling of `AND`. Equivalent to `$and_both`.
LET $andand_both = IF $p != NONE && $q != NONE THEN $p ELSE 'fallback' END;

-- Conjunct order is irrelevant. Equivalent to `$and_both`.
LET $and_flipped = IF $q != NONE AND $p != NONE THEN $p ELSE 'fallback' END;

-- Both conjuncts about the SAME place. Equivalent to `$p != NONE` alone.
LET $and_same_place = IF $p != NONE AND $p != NULL THEN $p ELSE 'fallback' END;

-- The right operand of `AND` is evaluated only where the left holds, so the
-- left narrows the right. This is the short-circuit consumer.
LET $and_short_circuit = IF $p != NONE AND string::len($p) > 0 THEN $p ELSE 'fallback' END;

-- ---- disjunction ---------------------------------------------------------
-- A disjunction proves a fact only when EVERY disjunct proves it. These two
-- disjuncts prove different facts, so the THEN branch learns nothing…
LET $or_mixed = IF $p != NONE OR $q != NONE THEN $p ELSE 'fallback' END;

-- …but the ELSE branch of the same predicate proves BOTH negations.
LET $or_else = IF $p = NONE OR $q = NONE THEN 'fallback' ELSE $p END;

-- Both disjuncts about the same place, proving the same fact: this one does
-- narrow. Equivalent to `$p != NONE`.
LET $or_same_fact = IF $p != NONE OR $p IS NOT NONE THEN $p ELSE 'fallback' END;

-- The `||` spelling. Equivalent to `$or_else`.
LET $oror_else = IF $p = NONE || $q = NONE THEN 'fallback' ELSE $p END;

-- ---- negation ------------------------------------------------------------
-- `!(p = NONE)` is `p != NONE`. Equivalent to a bare `!= NONE` guard.
LET $not_eq = IF !($p = NONE) THEN $p ELSE 'fallback' END;

-- `!(p != NONE)` is `p = NONE`; the ELSE branch is the narrowed one.
LET $not_ne = IF !($p != NONE) THEN 'fallback' ELSE $p END;

-- Double negation is the identity.
LET $not_not = IF !!($p != NONE) THEN $p ELSE 'fallback' END;

-- ---- De Morgan pairs -----------------------------------------------------
-- These two are the same predicate. They must record the same kind.
LET $demorgan_and = IF $p != NONE AND $q != NONE THEN $p ELSE 'fallback' END;

LET $demorgan_not_or = IF !($p = NONE OR $q = NONE) THEN $p ELSE 'fallback' END;

-- …and so are these two, read out of the ELSE branch.
LET $demorgan_or = IF $p = NONE OR $q = NONE THEN 'fallback' ELSE $p END;

LET $demorgan_not_and = IF !($p != NONE AND $q != NONE) THEN 'fallback' ELSE $p END;

-- ---- parenthesisation ----------------------------------------------------
-- Every one of these is `$p != NONE`.
LET $paren_leaf = IF ($p != NONE) THEN $p ELSE 'fallback' END;

LET $paren_twice = IF (($p != NONE)) THEN $p ELSE 'fallback' END;

LET $paren_conjunct = IF ($p != NONE) AND ($q != NONE) THEN $p ELSE 'fallback' END;

LET $paren_whole = IF ($p != NONE AND $q != NONE) THEN $p ELSE 'fallback' END;

-- ---- the row side of the same compositions -------------------------------
SELECT name, logo, parent FROM organization WHERE logo != NONE AND parent != NONE;

SELECT name, logo, parent FROM organization WHERE !(logo = NONE OR parent = NONE);

SELECT name, logo, parent FROM organization WHERE logo != NONE OR parent != NONE;

SELECT name, logo FROM organization WHERE !(logo = NONE);

SELECT name, logo FROM organization WHERE (logo != NONE);

SELECT name, logo FROM organization WHERE ((logo != NONE));

RETURN {
	and_both: $and_both,
	andand_both: $andand_both,
	and_flipped: $and_flipped,
	and_same_place: $and_same_place,
	and_short_circuit: $and_short_circuit,
	or_mixed: $or_mixed,
	or_else: $or_else,
	or_same_fact: $or_same_fact,
	oror_else: $oror_else,
	not_eq: $not_eq,
	not_ne: $not_ne,
	not_not: $not_not,
	demorgan_and: $demorgan_and,
	demorgan_not_or: $demorgan_not_or,
	demorgan_or: $demorgan_or,
	demorgan_not_and: $demorgan_not_and,
	paren_leaf: $paren_leaf,
	paren_twice: $paren_twice,
	paren_conjunct: $paren_conjunct,
	paren_whole: $paren_whole
};
