-- Functions, params and analyzers. Both the written-return and the
-- inferred-return paths are represented, because the inferred one is the
-- precision-sensitive half.

DEFINE PARAM $default_tier VALUE 'free';

DEFINE ANALYZER simple TOKENIZERS blank, class FILTERS lowercase, ascii;

-- Written return type: the declared kind is the contract.
DEFINE FUNCTION fn::org::display_name($org: record<organization>) -> string {
	RETURN $org.name;
};

-- No written return type: the snapshot records what inference derived.
DEFINE FUNCTION fn::org::owner($org: record<organization>) {
	RETURN $org.owner;
};

DEFINE FUNCTION fn::org::headcount($org: record<organization>) {
	RETURN (SELECT VALUE count() FROM employee_of WHERE out = $org GROUP ALL)[0] ?? 0;
};

DEFINE FUNCTION fn::person::initials($first: string, $last: string) {
	LET $a = string::slice($first, 0, 1);
	LET $b = string::slice($last, 0, 1);
	RETURN string::uppercase(string::concat($a, $b));
};

DEFINE FUNCTION fn::org::is_pro($org: record<organization>) {
	RETURN $org.tier = 'pro' OR $org.tier = 'enterprise';
};

-- Calls another `fn::` — exercises cross-function return propagation.
DEFINE FUNCTION fn::org::label($org: record<organization>) {
	RETURN string::concat(fn::org::display_name($org), ' (', $org.tier, ')');
};

-- Sentinel guards: each eliminates ONLY its own sentinel. `IF $r = NONE THEN
-- THROW` leaves `null` reachable (a NULL passes the guard), and `IF $r = NULL
-- THEN THROW` leaves `none` reachable. Collapsing either to "not none and not
-- null" is a type the database can violate.
DEFINE FUNCTION fn::account::reason_set($r: option<string | null>) {
	IF $r = NONE THEN THROW 'never set' END;
	RETURN $r;
};

DEFINE FUNCTION fn::account::reason_uncleared($r: option<string | null>) {
	IF $r = NULL THEN THROW 'explicitly cleared' END;
	RETURN $r;
};

-- `??` is not a guard: it falls through on BOTH sentinels, so the coalesced
-- value really is a plain `string`.
DEFINE FUNCTION fn::account::reason_or_default($r: option<string | null>) {
	RETURN $r ?? 'none given';
};
