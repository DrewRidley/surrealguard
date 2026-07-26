-- Equivalence class: DIVERGING GUARD FORM.
--
-- A guard narrows the code after it only because the guarded branch never falls
-- through. `THROW`, `RETURN`, `CONTINUE` and `BREAK` all diverge, and
-- `THEN … END` and `{ … }` are two spellings of the same branch body. So every
-- binding read after a guard here must record the same kind regardless of which
-- of the eight combinations produced it.
--
-- The function-body twins of the `THROW`/`RETURN` × `THEN`/`{}` matrix are in
-- `schema/08_facts.surql` (`fn::facts::guard_throw_then`,
-- `guard_throw_block`, `guard_return`).

-- THROW + `THEN … END`.
LET $a = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

IF $a = NONE THEN THROW 'unset' END;

LET $after_throw_then = $a;

-- THROW + `{ … }`. Equivalent to `$after_throw_then`.
LET $b = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

IF $b = NONE { THROW 'unset' };

LET $after_throw_block = $b;

-- RETURN + `{ … }`, inside a block (a top-level `RETURN` is not a divergence
-- the statement loop models). Equivalent to `$after_throw_then`.
LET $after_return_block = {
	LET $c = (SELECT VALUE logo FROM ONLY organization LIMIT 1);
	IF $c = NONE { RETURN 'unset' };
	RETURN $c;
};

-- RETURN + `THEN … END`. Equivalent to `$after_return_block`.
LET $after_return_then = {
	LET $d = (SELECT VALUE logo FROM ONLY organization LIMIT 1);
	IF $d = NONE THEN RETURN 'unset' END;
	RETURN $d;
};

-- An `IF … ELSE` where BOTH branches diverge: everything after it is dead, and
-- the guard proves nothing about the fall-through because there is none.
LET $e = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $both_diverge = {
	IF $e = NONE { THROW 'unset' } ELSE { RETURN $e };
};

-- CONTINUE + `THEN … END`, inside a `FOR`. Equivalent to `$after_throw_then`.
LET $logos = (SELECT VALUE logo FROM organization);

FOR $l IN $logos {
	IF $l = NONE THEN CONTINUE END;
	LET $after_continue_then = $l;
};

-- CONTINUE + `{ … }`. Equivalent to `$after_continue_then`.
FOR $l2 IN $logos {
	IF $l2 = NONE { CONTINUE };
	LET $after_continue_block = $l2;
};

-- BREAK + `THEN … END`. `BREAK` diverges exactly as `CONTINUE` does for the
-- statements that follow it in the body, so this is equivalent too.
FOR $l3 IN $logos {
	IF $l3 = NONE THEN BREAK END;
	LET $after_break_then = $l3;
};

-- BREAK + `{ … }`. Equivalent to `$after_break_then`.
FOR $l4 IN $logos {
	IF $l4 = NONE { BREAK };
	LET $after_break_block = $l4;
};

-- A NON-diverging branch proves nothing about the fall-through: after this
-- `IF`, `$f` is still optional. The must-stay-wide boundary of the whole class.
LET $f = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

IF $f = NONE THEN { CREATE organization_role SET name = 'x', organization = organization:root; } END;

LET $after_non_diverging = $f;

RETURN {
	throw_then: $after_throw_then,
	throw_block: $after_throw_block,
	return_block: $after_return_block,
	return_then: $after_return_then,
	both_diverge: $both_diverge,
	non_diverging: $after_non_diverging
};
