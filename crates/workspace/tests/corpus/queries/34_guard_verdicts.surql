-- Equivalence class: DEAD-BRANCH VERDICT.
--
-- A guard whose outcome is already fixed by what an earlier guard proved makes
-- its branch dead. This is the one consumer of the expression-fact layer that
-- can produce a NEW diagnostic on valid code: greying a branch that really can
-- run is a false positive, and an unhelpful one, because a greyed branch is
-- also dropped from the exit-set union — so the verdict shows up here as a
-- MISSING variant in the recorded kind, which is what makes it pinnable.
--
-- Every `IF` below returns an `int` from its THEN and a `string` from its ELSE,
-- so the recorded kind of each binding names the arms the analyzer believes can
-- run: `int | string` means both were kept, `string` means the THEN was greyed,
-- `int` means the ELSE was.
--
-- Two gates keep this sound, and both have a case here:
--
--   * a verdict is drawn ONLY from a kind a prior guard actually narrowed,
--     never from a declared one (`$live_base`, `$live_declared`); and
--   * only atoms whose decision has a case in this file may decide at all —
--     `Truthy`, `Ord`, `Member` and `HasKind` return "unknown" for now, and
--     `$live_kind_predicate` is what would change if one were enabled without
--     a case to go with it.

-- ---------------------------------------------------------------------------
-- Dead, because an earlier guard already proved the opposite
-- ---------------------------------------------------------------------------

-- The subject is narrowed to a plain `record<…>` by the diverging guard, so the
-- second `= NONE` can never hold: the THEN is dead and only the ELSE survives.
LET $r = (SELECT VALUE reviewer FROM ONLY document LIMIT 1);

IF $r = NONE THEN THROW 'no reviewer' END;

LET $dead_then = IF $r = NONE THEN 1 ELSE 'set' END;

-- The mirror image: the guard narrows the subject to `none`, so the second
-- `= NONE` always holds and the ELSE is dead.
LET $r2 = (SELECT VALUE reviewer FROM ONLY document LIMIT 1);

IF $r2 != NONE THEN THROW 'has a reviewer' END;

LET $dead_else = IF $r2 = NONE THEN 1 ELSE 'set' END;

-- The NULL sentinel decides independently of the NONE one. `$s` is narrowed to
-- `string | null`, so `IS NULL` is still open — BOTH arms survive here, which is
-- the boundary that keeps the two sentinels from being confused.
LET $s = (SELECT VALUE summary FROM ONLY document LIMIT 1);

IF $s = NONE THEN THROW 'unset' END;

LET $live_after_none = IF $s IS NULL THEN 1 ELSE 'present' END;

-- A table discriminant. The first guard pins the record to one table, so the
-- second discriminant test on the other table can never hold.
LET $o = (SELECT VALUE owner FROM ONLY document LIMIT 1);

IF type::table($o) != 'account' THEN THROW 'not an account' END;

LET $dead_discriminant = IF type::table($o) = 'organization' THEN 1 ELSE 'account' END;

-- A literal-union field pinned to one member. `!= 'draft'` diverges, so `$g` is
-- exactly `'draft'` afterwards and `= 'draft'` can no longer fail: the ELSE is
-- dead.
--
-- The always-FALSE direction (`= 'published'` on the same pinned `$g`) is
-- deliberately not written here. It is a comparison that can never hold, which
-- 7005 reports on its own — so it belongs in a corpus of mistakes, not in this
-- one, which is valid input by construction.
LET $g = (SELECT VALUE stage FROM ONLY document LIMIT 1);

IF $g != 'draft' THEN THROW 'not a draft' END;

LET $dead_literal_else = IF $g = 'draft' THEN 1 ELSE 'other' END;

-- A COMPOUND guard. One conjunct proven false decides the conjunction, which
-- the recognizer this replaces declined to decompose at all: it settled only
-- single comparisons, so an `AND` kept both arms however impossible it was.
LET $c = (SELECT VALUE reviewer FROM ONLY document LIMIT 1);

IF $c = NONE THEN THROW 'no reviewer' END;

LET $dead_conjunction = IF $c = NONE AND type::table($c) = 'account' THEN 1 ELSE 'neither' END;

-- ---------------------------------------------------------------------------
-- Live, and the reason each one must stay live
-- ---------------------------------------------------------------------------

-- A BASE binding: nothing narrowed it, so its kind is the one it was declared
-- with and says nothing about this program point. Greying the defensive guard
-- everyone writes is the false positive this whole class exists to avoid.
LET $base = (SELECT VALUE reviewer FROM ONLY document LIMIT 1);

LET $live_base = IF $base = NONE THEN 1 ELSE 'set' END;

-- The same, on a field DECLARED non-optional. `title` can never be NONE, and
-- the guard is still kept: the declared kind is not evidence about flow.
LET $declared = (SELECT VALUE title FROM ONLY document LIMIT 1);

LET $live_declared = IF $declared = NONE THEN 1 ELSE 'set' END;

-- A kind predicate over a subject a guard DID narrow. `$w` is an `int` here, so
-- `type::is_string($w)` is provably false — and the branch is kept anyway,
-- because `HasKind` is not yet allowed to decide. When it is enabled, this
-- binding collapses to `'no'` and this comment is the record of why.
LET $w = (SELECT VALUE word_count FROM ONLY document LIMIT 1);

IF $w = NONE THEN THROW 'unset' END;

LET $live_kind_predicate = IF type::is_string($w) THEN 1 ELSE 'no' END;

-- An ORDERING guard over the same narrowed subject: `Ord` decides nothing
-- either, and `> 0` is genuinely open on an `int` regardless.
LET $live_ordering = IF $w > 0 THEN 1 ELSE 'other' END;

-- A DISJUNCTION with one impossible arm is not decided by that arm: the other
-- may still be the reason it holds.
LET $live_disjunction = IF $w = NONE OR $w > 0 THEN 1 ELSE 'no' END;

RETURN {
	dead_then: $dead_then,
	dead_else: $dead_else,
	live_after_none: $live_after_none,
	dead_discriminant: $dead_discriminant,
	dead_literal_else: $dead_literal_else,
	dead_conjunction: $dead_conjunction,
	live_base: $live_base,
	live_declared: $live_declared,
	live_kind_predicate: $live_kind_predicate,
	live_ordering: $live_ordering,
	live_disjunction: $live_disjunction
};
