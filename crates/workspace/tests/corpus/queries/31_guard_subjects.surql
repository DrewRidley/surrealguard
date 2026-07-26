-- Equivalence class: GUARD SUBJECT.
--
-- One fact — "this value is not NONE" — written over five different subjects.
-- The fact is the same in every case, so every `*_narrowed` binding below must
-- record the same kind as its neighbours. Where one records `option<string>`
-- and another `string`, the recognizer accepted one spelling of the subject and
-- declined an equivalent one; that difference is the refactor's target.
--
-- (`$this` and `$value` subjects are not writable at a query's top level — they
-- are bound only inside a `DEFINE FIELD` clause — so they live in
-- `schema/08_facts.surql` as `length_band` / `word_count_or_zero` instead.)

-- Subject 1: a bare param.
LET $bare = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $bare_narrowed = IF $bare != NONE THEN $bare ELSE 'fallback' END;

-- Subject 2: a field path off a param (`$x.y`). Equivalent to `$bare_narrowed`
-- — the subject is a different place, but the FACT and the narrowed kind are
-- the same shape.
LET $row = (SELECT name, logo FROM ONLY organization LIMIT 1);

IF $row = NONE THEN THROW 'no organization' END;

LET $path_narrowed = IF $row.logo != NONE THEN $row.logo ELSE 'fallback' END;

-- Subject 3: a two-segment field path (`$x.y.z`). Same fact, one segment
-- deeper; a per-segment cliff shows up here and not in `$path_narrowed`.
LET $settings = (SELECT VALUE settings FROM ONLY account LIMIT 1);

IF $settings = NONE THEN THROW 'no account' END;

LET $deep_narrowed = IF $settings.locale != NONE THEN $settings.locale ELSE 'fallback' END;

-- Subject 4: a `LET`-bound intermediate that ALIASES another binding. `$alias`
-- and `$alias_src` denote the same value, so this must record the same kind as
-- `$bare_narrowed`.
LET $alias_src = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $alias = $alias_src;

LET $alias_narrowed = IF $alias != NONE THEN $alias ELSE 'fallback' END;

-- Subject 5: guard the alias, read the ORIGINAL. Aliasing is not tracked
-- today, so this is expected to be wider than `$alias_narrowed` — the same
-- fact, learned about a place the reader spells differently.
LET $alias_cross = IF $alias != NONE THEN $alias_src ELSE 'fallback' END;

-- Subject 6: guard the original, read the alias. The mirror image of
-- `$alias_cross`.
LET $alias_cross_back = IF $alias_src != NONE THEN $alias ELSE 'fallback' END;

-- Subject 7: a parenthesised subject. `($bare)` is `$bare`; equivalent to
-- `$bare_narrowed`.
LET $paren_narrowed = IF ($bare) != NONE THEN $bare ELSE 'fallback' END;

RETURN {
	bare: $bare_narrowed,
	path: $path_narrowed,
	deep: $deep_narrowed,
	alias: $alias_narrowed,
	alias_cross: $alias_cross,
	alias_cross_back: $alias_cross_back,
	paren: $paren_narrowed
};
