-- Shapes the expression-fact equivalence classes need, and the two guard
-- placements that are only reachable from a schema definition (`$this` in a
-- `COMPUTED`/`VALUE` clause, and a `DEFINE FUNCTION` body).
--
-- Every field here exists to make ONE fact observable in the snapshot:
--
--   * `owner` / `reviewer` — a MULTI-table `record<a | b>`, the only shape a
--     table discriminant (`type::table` / `record::tb` / `meta::tb`) can narrow;
--   * `stage` — a scalar literal union, the §1.3 `status = 'active'` shape;
--   * `word_count` — `option<int>`, for ordering guards (`> 0`);
--   * `tags` — `option<array<string>>`, an iterable behind an option;
--   * `keywords` — `set<string>`, the array/set pair;
--   * `summary` — `option<string | null>`, where the NONE and NULL guards must
--     stay apart.
--
-- A `COMPUTED` field's kind is INFERRED, which is what makes a guard written in
-- a `DEFINE FIELD` clause visible to the precision snapshot at all: the clause's
-- narrowing shows up as the field's recorded kind.

DEFINE TABLE document SCHEMAFULL;

DEFINE FIELD title ON document TYPE string;

DEFINE FIELD owner ON document TYPE record<account | organization>;

DEFINE FIELD reviewer ON document TYPE option<record<account | organization>>;

DEFINE FIELD stage ON document TYPE 'draft' | 'review' | 'published' DEFAULT 'draft';

DEFINE FIELD word_count ON document TYPE option<int>;

DEFINE FIELD tags ON document TYPE option<array<string>>;

-- No `DEFAULT []`: an empty array literal infers `array<any, 0>`, which today
-- is not assignable to a `set<string>` (Array and Set stay disjoint in
-- `kind_is_assignable_to`), and the corpus must stay free of error findings.
DEFINE FIELD keywords ON document TYPE set<string>;

DEFINE FIELD summary ON document TYPE option<string | null>;

-- A UNIQUE index makes `WHERE title = <const>` a single-row proof, which is the
-- second cardinality argument `FROM ONLY` accepts (the first is `LIMIT 1`).
-- Defined on the new table so no existing corpus query changes cardinality.
DEFINE INDEX document_title ON document FIELDS title UNIQUE;

-- Guard placement: inside a `DEFINE FIELD` clause, over `$this.<field>`. The
-- recorded kind of each field is the narrowing verdict.
DEFINE FIELD length_band ON document COMPUTED IF $this.word_count != NONE THEN 'known' ELSE 'unknown' END;

-- The same fact through the `= NONE` (negative-polarity) spelling. Equivalent
-- to `length_band`; a differing recorded kind is a spelling gap.
DEFINE FIELD length_band_eq ON document COMPUTED IF $this.word_count = NONE THEN 'unknown' ELSE 'known' END;

-- The subject is `$this.<field>` and the narrowed value is READ in the branch,
-- so this field's kind records whether a `$this` field path narrows at all.
DEFINE FIELD word_count_or_zero ON document COMPUTED IF $this.word_count != NONE THEN $this.word_count ELSE 0 END;

-- Guard placement: inside a `DEFINE FUNCTION` body. The inferred return type is
-- the narrowing verdict. Each of these is the SAME fact in a different
-- spelling; they must agree.
DEFINE FUNCTION fn::facts::guard_throw_then($v: option<string>) {
	IF $v = NONE THEN THROW 'unset' END;
	RETURN $v;
};

-- `{ … }` instead of `THEN … END`. Equivalent to `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_throw_block($v: option<string>) {
	IF $v = NONE { THROW 'unset' };
	RETURN $v;
};

-- A diverging `RETURN` instead of a `THROW`. Equivalent to
-- `guard_throw_then` for everything after the guard.
DEFINE FUNCTION fn::facts::guard_return($v: option<string>) {
	IF $v = NONE { RETURN 'unset' };
	RETURN $v;
};

-- Parenthesised guard condition. `(c)` is a semantic no-op; equivalent to
-- `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_paren($v: option<string>) {
	IF ($v = NONE) THEN THROW 'unset' END;
	RETURN $v;
};

-- Negated guard. `!($v != NONE)` is `$v = NONE`; equivalent to
-- `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_not($v: option<string>) {
	IF !($v != NONE) THEN THROW 'unset' END;
	RETURN $v;
};

-- `IS NONE` rather than `= NONE`; equivalent to `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_is_none($v: option<string>) {
	IF $v IS NONE THEN THROW 'unset' END;
	RETURN $v;
};

-- The positive spelling: guard on `IS NOT NONE` and return from inside the
-- branch. Equivalent to `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_is_not_none($v: option<string>) {
	IF $v IS NOT NONE { RETURN $v };
	THROW 'unset';
};

-- A truthiness guard: `IF $v THEN` claims strictly MORE than `!= NONE`, so
-- anything it proves, `!= NONE` proves too. Equivalent-or-better than
-- `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_truthy($v: option<string>) {
	IF !$v THEN THROW 'unset' END;
	RETURN $v;
};

-- The `type::is_*` family spelling of the same fact. `type::is_none($v)` is
-- `$v = NONE`; equivalent to `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_type_is_none($v: option<string>) {
	IF type::is_none($v) THEN THROW 'unset' END;
	RETURN $v;
};

-- `type::is_string($v)` proves the same thing from the other side; the
-- fall-through of its negation is equivalent to `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_type_is_string($v: option<string>) {
	IF !type::is_string($v) THEN THROW 'unset' END;
	RETURN $v;
};

-- Guard on a LET-bound ALIAS of the parameter, then read the parameter. The
-- two denote the same place, so this is equivalent to `guard_throw_then`.
DEFINE FUNCTION fn::facts::guard_alias($v: option<string>) {
	LET $alias = $v;
	IF $alias = NONE THEN THROW 'unset' END;
	RETURN $v;
};

-- …and the reverse: guard the parameter, read the alias bound AFTER the guard.
DEFINE FUNCTION fn::facts::guard_alias_after($v: option<string>) {
	IF $v = NONE THEN THROW 'unset' END;
	LET $alias = $v;
	RETURN $alias;
};

-- A guard over a FIELD PATH of an object parameter, rather than the parameter
-- itself.
DEFINE FUNCTION fn::facts::guard_field_path($o: { name: option<string> }) {
	IF $o.name = NONE THEN THROW 'unset' END;
	RETURN $o.name;
};

-- Sentinel separation at the function boundary: on `option<string | null>` a
-- `= NONE` guard leaves `null` reachable and a `= NULL` guard leaves `none`
-- reachable. `fn::account::reason_set` / `reason_uncleared` already pin the
-- `= …` spelling; these pin `IS` and the negated-`!=` spellings of the SAME
-- two facts. All four must agree with their `= …` twin.
DEFINE FUNCTION fn::facts::is_none_guard($r: option<string | null>) {
	IF $r IS NONE THEN THROW 'never set' END;
	RETURN $r;
};

DEFINE FUNCTION fn::facts::is_null_guard($r: option<string | null>) {
	IF $r IS NULL THEN THROW 'cleared' END;
	RETURN $r;
};

DEFINE FUNCTION fn::facts::not_ne_none_guard($r: option<string | null>) {
	IF !($r != NONE) THEN THROW 'never set' END;
	RETURN $r;
};

DEFINE FUNCTION fn::facts::not_ne_null_guard($r: option<string | null>) {
	IF !($r != NULL) THEN THROW 'cleared' END;
	RETURN $r;
};
