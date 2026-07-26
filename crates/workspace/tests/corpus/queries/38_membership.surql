-- Equivalence class: MEMBERSHIP.
--
-- `x IN coll`, `coll CONTAINS x` and `x INSIDE coll` are three spellings of one
-- claim; `INSIDE` is an explicit alias of `IN` and `CONTAINS` is the same
-- operator with the operands swapped. So all three must reach the same verdict,
-- and the shape of the COLLECTION (a param, an array literal, a field, a
-- subquery) must not change the answer either.
--
-- The claim also proves the member is not NONE — nothing is a member of a
-- collection of `string` unless it is a `string`.

LET $names = (SELECT VALUE name FROM organization);

LET $maybe = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

-- ---- three spellings, one collection ------------------------------------
LET $in_param = IF $maybe IN $names THEN $maybe ELSE 'fallback' END;

-- Equivalent to `$in_param`.
LET $inside_param = IF $maybe INSIDE $names THEN $maybe ELSE 'fallback' END;

-- Equivalent to `$in_param`, operands swapped.
LET $contains_param = IF $names CONTAINS $maybe THEN $maybe ELSE 'fallback' END;

-- ---- the same claim, four collection shapes -----------------------------
-- An array LITERAL rather than a param. Equivalent to `$in_param`.
LET $in_literal = IF $maybe IN ['acme', 'globex'] THEN $maybe ELSE 'fallback' END;

-- A subquery. Equivalent to `$in_param`.
LET $in_subquery = IF $maybe IN (SELECT VALUE name FROM organization) THEN $maybe ELSE 'fallback' END;

-- A field path off an object param. Equivalent to `$in_param`.
LET $holder = { pool: $names };

LET $in_field = IF $maybe IN $holder.pool THEN $maybe ELSE 'fallback' END;

-- A parenthesised collection. Equivalent to `$in_param`.
LET $in_paren = IF $maybe IN ($names) THEN $maybe ELSE 'fallback' END;

-- ---- negation ------------------------------------------------------------
-- `NOT IN` is the negation of `IN`, so its ELSE branch proves exactly what
-- `$in_param`'s THEN branch does.
LET $not_in = IF $maybe NOT IN $names THEN 'fallback' ELSE $maybe END;

-- ---- a set-typed collection ---------------------------------------------
-- `set<string>` is a collection exactly as `array<string>` is; membership must
-- not care which.
LET $keywords = (SELECT VALUE keywords FROM ONLY document LIMIT 1);

LET $in_set = IF $maybe IN $keywords THEN $maybe ELSE 'fallback' END;

-- ---- the row side of the same three spellings ---------------------------
SELECT name, tier FROM organization WHERE tier IN ['pro', 'enterprise'];

SELECT name, tier FROM organization WHERE tier INSIDE ['pro', 'enterprise'];

SELECT name, tier FROM organization WHERE ['pro', 'enterprise'] CONTAINS tier;

-- Membership over an optional field: the claim proves the field is set.
SELECT name, logo FROM organization WHERE logo IN $names;

SELECT name, logo FROM organization WHERE $names CONTAINS logo;

RETURN {
	in_param: $in_param,
	inside_param: $inside_param,
	contains_param: $contains_param,
	in_literal: $in_literal,
	in_subquery: $in_subquery,
	in_field: $in_field,
	in_paren: $in_paren,
	not_in: $not_in,
	in_set: $in_set
};
