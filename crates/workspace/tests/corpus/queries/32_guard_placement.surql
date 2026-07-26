-- Equivalence class: GUARD PLACEMENT.
--
-- One fact — `IF <x> = NONE THEN <diverge>` followed by a read of `<x>` — in
-- every syntactic position the analyzer has a separate fall-through path for.
-- Placement cannot change what a guard proves, so every `*_after` binding here
-- must record the same kind. Two of the five placements are only reachable from
-- a schema definition and live in `schema/08_facts.surql`:
-- `fn::facts::guard_throw_then` (a `DEFINE FUNCTION` body) and `length_band`
-- (a `DEFINE FIELD` clause).

-- Placement 1: the top level of a source.
LET $top = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

IF $top = NONE THEN THROW 'no logo' END;

LET $top_after = $top;

-- Placement 2: inside a block. Equivalent to placement 1; the block's own
-- inferred kind records the verdict a second time.
LET $block_after = {
	LET $inner = (SELECT VALUE logo FROM ONLY organization LIMIT 1);
	IF $inner = NONE THEN THROW 'no logo' END;
	RETURN $inner;
};

-- Placement 3: inside a `FOR` body, guarded with `CONTINUE`. Equivalent to
-- placement 1 for everything after the guard.
LET $logos = (SELECT VALUE logo FROM organization);

FOR $logo IN $logos {
	IF $logo = NONE THEN CONTINUE END;
	LET $for_after = $logo;
};

-- Placement 4: a nested block inside a `FOR` body — two fall-through layers.
FOR $logo2 IN $logos {
	LET $nested_after = {
		IF $logo2 = NONE THEN THROW 'no logo' END;
		RETURN $logo2;
	};
};

-- Placement 5: the guard is the last statement of a block and the read is
-- OUTSIDE it. A block does not leak its narrowing, so this is expected to stay
-- wide — recorded so the refactor cannot silently start (or stop) leaking.
LET $outer = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $scoped = {
	IF $outer = NONE THEN THROW 'no logo' END;
	RETURN 1;
};

LET $outer_after = $outer;

RETURN { top: $top_after, block: $block_after, scoped: $scoped, outer: $outer_after };
