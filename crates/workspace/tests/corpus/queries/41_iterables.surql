-- Equivalence class: ITERABLES.
--
-- `FOR $x IN <e>` binds `$x` to the ELEMENT of whatever `<e>` denotes. The
-- element kind is a property of the collection, not of how the collection was
-- spelled, so every loop over a `string` collection below must bind `$x` to the
-- same kind — whether the collection arrived as an `array<string>`, a
-- `set<string>`, an `option<array<string>>`, an array literal, a subquery, or a
-- parenthesised any of those.
--
-- `option<array<T>>` is the interesting one: iterating it is only well-defined
-- where it is set, so the element is `T` and the optionality is an obligation
-- on the loop, not on the element.

LET $names = (SELECT VALUE name FROM organization);

LET $keywords = (SELECT VALUE keywords FROM ONLY document LIMIT 1);

LET $tags = (SELECT VALUE tags FROM ONLY document LIMIT 1);

-- ---- array<string> -------------------------------------------------------
FOR $from_array IN $names {
	LET $array_elem = $from_array;
};

-- ---- an array LITERAL. Equivalent to `$from_array`. ----------------------
FOR $from_literal IN ['acme', 'globex'] {
	LET $literal_elem = $from_literal;
};

-- ---- a subquery. Equivalent to `$from_array`. ---------------------------
FOR $from_subquery IN (SELECT VALUE name FROM organization) {
	LET $subquery_elem = $from_subquery;
};

-- ---- a parenthesised param. Equivalent to `$from_array`. -----------------
FOR $from_paren IN ($names) {
	LET $paren_elem = $from_paren;
};

-- ---- set<string>. Equivalent to `$from_array`. --------------------------
FOR $from_set IN $keywords {
	LET $set_elem = $from_set;
};

-- ---- option<array<string>>. The element is still `string`. --------------
FOR $from_option IN $tags {
	LET $option_elem = $from_option;
};

-- ---- option<array<string>> after a guard. Equivalent to `$from_array`. --
IF $tags = NONE THEN THROW 'no tags' END;

FOR $from_guarded_option IN $tags {
	LET $guarded_option_elem = $from_guarded_option;
};

-- ---- a range. The element is an `int`. ----------------------------------
FOR $from_range IN 1..3 {
	LET $range_elem = $from_range;
};

-- ---- an inclusive range. Equivalent to `$from_range`. -------------------
FOR $from_range_inclusive IN 1..=3 {
	LET $range_inclusive_elem = $from_range_inclusive;
};

-- ---- a range bound by a const-foldable expression. Equivalent again. -----
FOR $from_range_expr IN 1..(2 + 1) {
	LET $range_expr_elem = $from_range_expr;
};

-- (A record range — `organization:'a'..organization:'z'` — is not accepted in
-- `FOR … IN` position by the grammar, so it is not covered here.)

-- ---- the closure spelling of the same traversal -------------------------
-- `array::map`'s closure parameter is the element of the same collection, so
-- these must agree with the `FOR` bindings above.
LET $mapped_array = array::map($names, |$n| $n);

LET $mapped_set = array::map($keywords, |$k| $k);

LET $mapped_option = array::map($tags, |$t| $t);

RETURN { mapped: $mapped_array, mapped_set: $mapped_set, mapped_option: $mapped_option };
