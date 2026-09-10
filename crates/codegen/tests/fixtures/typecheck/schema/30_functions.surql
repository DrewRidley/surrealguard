-- `fn::` definitions: one with a written return type, one whose return is
-- inferred from the body.

DEFINE FUNCTION fn::greet($name: string) -> string {
	RETURN string::concat('hi ', $name);
};

DEFINE FUNCTION fn::headcount($team: record<team>) {
	RETURN (SELECT VALUE count() FROM person WHERE team = $team GROUP ALL)[0] ?? 0;
};
