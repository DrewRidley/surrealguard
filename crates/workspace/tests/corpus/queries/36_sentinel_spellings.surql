-- Equivalence class: SENTINEL GUARD SPELLING.
--
-- `= NONE` / `!= NONE` / `IS NONE` / `IS NOT NONE` are four spellings of two
-- facts, and the `NULL` family is four more spellings of two DIFFERENT facts.
-- `35_sentinels.surql` pins the `!=` spelling and the NONE/NULL separation;
-- this file pins the remaining spellings of the same facts, over both
-- `option<T>` and `option<T | null>`.
--
-- Within each group every binding must record the same kind. Across the NONE
-- and NULL groups they must NOT: a NULL survives `!= NONE` and a NONE survives
-- `!= NULL`, so on `option<string | null>` the two guards leave different
-- residues. That is a soundness boundary, not a precision one.

LET $opt = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $optnull = (SELECT VALUE summary FROM ONLY document LIMIT 1);

-- ---- option<string>, the NONE fact, positive polarity -------------------
LET $opt_ne_none = IF $opt != NONE THEN $opt ELSE 'fallback' END;

LET $opt_is_not_none = IF $opt IS NOT NONE THEN $opt ELSE 'fallback' END;

-- ---- option<string>, the NONE fact, negative polarity -------------------
-- Same fact, read out of the ELSE branch. Equivalent to `$opt_ne_none`.
LET $opt_eq_none = IF $opt = NONE THEN 'fallback' ELSE $opt END;

LET $opt_is_none = IF $opt IS NONE THEN 'fallback' ELSE $opt END;

-- ---- option<string>, the NULL fact --------------------------------------
-- `option<string>` admits no NULL at all, so these guards prove nothing new;
-- they must not remove the NONE.
LET $opt_ne_null = IF $opt != NULL THEN $opt ELSE 'fallback' END;

LET $opt_is_not_null = IF $opt IS NOT NULL THEN $opt ELSE 'fallback' END;

-- ---- option<string | null>, the NONE fact -------------------------------
-- Each of these must keep the `null`: `NULL = NONE` is FALSE in the engine, so
-- a NULL row passes a `!= NONE` guard.
LET $on_ne_none = IF $optnull != NONE THEN $optnull ELSE 'fallback' END;

LET $on_is_not_none = IF $optnull IS NOT NONE THEN $optnull ELSE 'fallback' END;

LET $on_eq_none = IF $optnull = NONE THEN 'fallback' ELSE $optnull END;

LET $on_is_none = IF $optnull IS NONE THEN 'fallback' ELSE $optnull END;

-- ---- option<string | null>, the NULL fact -------------------------------
-- Each of these must keep the `none`, and must NOT agree with the NONE group
-- above.
LET $on_ne_null = IF $optnull != NULL THEN $optnull ELSE 'fallback' END;

LET $on_is_not_null = IF $optnull IS NOT NULL THEN $optnull ELSE 'fallback' END;

LET $on_eq_null = IF $optnull = NULL THEN 'fallback' ELSE $optnull END;

LET $on_is_null = IF $optnull IS NULL THEN 'fallback' ELSE $optnull END;

-- ---- both sentinels at once ---------------------------------------------
-- The conjunction removes both; `??` is the value-side spelling of the same
-- thing and must agree.
LET $on_both = IF $optnull != NONE AND $optnull != NULL THEN $optnull ELSE 'fallback' END;

LET $on_coalesced = $optnull ?? 'fallback';

-- ---- the row side of the same eight facts -------------------------------
-- A `WHERE` is the same predicate over rows. The projected field kind must
-- match the param-side binding for the identical spelling.
SELECT summary FROM document WHERE summary != NONE;

SELECT summary FROM document WHERE summary IS NOT NONE;

SELECT summary FROM document WHERE summary != NULL;

SELECT summary FROM document WHERE summary IS NOT NULL;

SELECT logo FROM organization WHERE logo != NONE;

SELECT logo FROM organization WHERE logo IS NOT NONE;

RETURN {
	opt_ne_none: $opt_ne_none,
	opt_is_not_none: $opt_is_not_none,
	opt_eq_none: $opt_eq_none,
	opt_is_none: $opt_is_none,
	opt_ne_null: $opt_ne_null,
	opt_is_not_null: $opt_is_not_null,
	on_ne_none: $on_ne_none,
	on_is_not_none: $on_is_not_none,
	on_eq_none: $on_eq_none,
	on_is_none: $on_is_none,
	on_ne_null: $on_ne_null,
	on_is_not_null: $on_is_not_null,
	on_eq_null: $on_eq_null,
	on_is_null: $on_is_null,
	on_both: $on_both,
	on_coalesced: $on_coalesced
};
