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
