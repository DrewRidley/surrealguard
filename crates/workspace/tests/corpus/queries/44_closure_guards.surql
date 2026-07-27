-- Equivalence class: CLOSURE GUARDS.
--
-- `array::filter(coll, |$e| p($e))` keeps exactly the elements for which `p`
-- held, so `p` is a positive guard over the surviving element — the same fact a
-- `WHERE` states about a SELECT's surviving rows, one abstraction down.
--
-- The two spellings must therefore agree. They did not: the row side had a
-- recognizer and the closure side had nothing, so filtering an `option<T>` field
-- to non-NONE left the element's field `option<T>` while the identical `WHERE`
-- produced `T`.
--
-- Every predicate below claims the same thing about `$r.person`, so every
-- `$kept_*` must have the same element kind.

LET $rows = (SELECT username, person FROM account);

-- ---- the reference: the same fact, written as a WHERE -------------------
LET $where_rows = (SELECT username, person FROM account WHERE person != NONE);

-- ---- the plain spelling -------------------------------------------------
LET $kept = array::filter($rows, |$r| $r.person != NONE);

-- ---- IS NOT NONE. Equivalent to `$kept`. -------------------------------
LET $kept_is_not = array::filter($rows, |$r| $r.person IS NOT NONE);

-- ---- a negated equality. Equivalent to `$kept`. ------------------------
LET $kept_negated = array::filter($rows, |$r| !($r.person = NONE));

-- ---- parenthesised. Equivalent to `$kept`. -----------------------------
LET $kept_paren = array::filter($rows, |$r| ($r.person != NONE));

-- ---- a conjunction. The other conjunct claims nothing, so still equivalent.
LET $kept_and = array::filter($rows, |$r| $r.person != NONE AND $r.username != '');

-- ---- the method spelling of the same call. ------------------------------
LET $kept_method = $rows.filter(|$r| $r.person != NONE);

-- ---- a predicate that proves nothing about the element. The element is
-- ---- unchanged: widening is sound, inventing a fact is not.
LET $kept_opaque = array::filter($rows, |$r| rand::bool());

-- ---- a filter whose subject is a scalar element, guarded on the element
-- ---- itself rather than on a field of it.
LET $reasons = (SELECT VALUE deactivated_reason FROM account);

LET $set_reasons = array::filter($reasons, |$reason| $reason != NONE);

-- ---- and the NULL sibling: `!= NONE` leaves the NULL in place, because
-- ---- `NULL = NONE` is FALSE on the engine.
LET $non_null_reasons = array::filter($reasons, |$reason| $reason != NULL);

RETURN {
	kept: $kept,
	kept_is_not: $kept_is_not,
	kept_negated: $kept_negated,
	kept_paren: $kept_paren,
	kept_and: $kept_and,
	kept_method: $kept_method,
	kept_opaque: $kept_opaque,
	set_reasons: $set_reasons,
	non_null_reasons: $non_null_reasons,
	where_rows: $where_rows
};
