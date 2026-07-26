-- Equivalence class: DISCRIMINANTS and LITERAL-UNION EQUALITY.
--
-- Two facts that ought to narrow a union by comparing a projection of it
-- against a constant:
--
--   1. the table of a record link — `type::table(x)`, `record::tb(x)` and
--      `meta::tb(x)` are three names for ONE function, so all three must reach
--      the same verdict;
--   2. a scalar-literal union field compared to one of its members —
--      `stage = 'draft'` on `'draft' | 'review' | 'published'`.
--
-- Fact 2 is the §1.3 case: a plain `string` field DOES narrow to the compared
-- literal (`name = 'acme'` records `"acme"`) while a literal-UNION field does
-- not, because the recognizer is an equality test on base kinds rather than a
-- lattice meet. The two `SELECT`s at the bottom of this file record exactly
-- that disagreement.

LET $doc = (SELECT owner, reviewer, stage FROM ONLY document LIMIT 1);

IF $doc = NONE THEN THROW 'no document' END;

-- ---- table discriminant, three spellings of one function ----------------
LET $tt_positive = IF type::table($doc.owner) = 'account' THEN $doc.owner ELSE NONE END;

-- Equivalent to `$tt_positive`.
LET $rt_positive = IF record::tb($doc.owner) = 'account' THEN $doc.owner ELSE NONE END;

-- Equivalent to `$tt_positive`.
LET $mt_positive = IF meta::tb($doc.owner) = 'account' THEN $doc.owner ELSE NONE END;

-- Operand order is a no-op: `'account' = type::table(x)` is the same claim.
LET $tt_flipped = IF 'account' = type::table($doc.owner) THEN $doc.owner ELSE NONE END;

-- Negative polarity: excluding one table of a two-table union leaves the other.
-- Equivalent to `$tt_positive` with the arms swapped.
LET $tt_negative = IF type::table($doc.owner) != 'account' THEN NONE ELSE $doc.owner END;

-- Parenthesised subject. `(x)` is `x`; equivalent to `$tt_positive`.
LET $tt_paren = IF type::table(($doc.owner)) = 'account' THEN $doc.owner ELSE NONE END;

-- The `type::is_record` spelling of the same claim.
LET $is_record_positive = IF type::is_record($doc.owner, 'account') THEN $doc.owner ELSE NONE END;

-- ---- literal-union equality ---------------------------------------------
LET $stage = (SELECT VALUE stage FROM ONLY document LIMIT 1);

IF $stage = NONE THEN THROW 'no document' END;

LET $stage_eq = IF $stage = 'draft' THEN $stage ELSE 'draft' END;

-- `IN` over the union's members is the same claim as an `=` chain.
LET $stage_in = IF $stage IN ['draft', 'review'] THEN $stage ELSE 'draft' END;

-- ---- the row side of both facts -----------------------------------------
-- A plain `string` field narrows to the compared literal…
SELECT name FROM organization WHERE name = 'acme';

-- …but the same predicate on a scalar-literal UNION field does not. Same
-- recognizer, same predicate, different answer — the disagreement §1.3
-- describes.
SELECT stage FROM document WHERE stage = 'draft';

SELECT tier FROM organization WHERE tier = 'pro';

-- Operand order, and the `IN` spelling, on the same field.
SELECT tier FROM organization WHERE 'pro' = tier;

SELECT tier FROM organization WHERE tier IN ['pro', 'enterprise'];

-- The row-side table discriminant, all three spellings.
SELECT owner FROM document WHERE type::table(owner) = 'account';

SELECT owner FROM document WHERE record::tb(owner) = 'account';

SELECT owner FROM document WHERE meta::tb(owner) = 'account';

RETURN {
	tt: $tt_positive,
	rt: $rt_positive,
	mt: $mt_positive,
	tt_flipped: $tt_flipped,
	tt_negative: $tt_negative,
	tt_paren: $tt_paren,
	is_record: $is_record_positive,
	stage_eq: $stage_eq,
	stage_in: $stage_in
};
