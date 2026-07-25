-- Bindings at every nesting depth: top level, inside a block, and a `FOR` loop
-- variable. Each one is a snapshot site, so a binding that silently degrades to
-- `any` is caught even though no diagnostic fires.

LET $orgs = (SELECT name, tier, owner FROM organization);

LET $count = array::len($orgs);

LET $names = array::map($orgs, |$o| $o.name);

LET $first = $orgs[0];

LET $tiers = (SELECT VALUE tier FROM organization);

FOR $org IN $orgs {
	LET $label = string::concat($org.name, ':', $org.tier);
	LET $inner = (SELECT VALUE name FROM organization_unit WHERE organization = $org.owner LIMIT 1);
	CREATE organization_role SET name = $label, organization = organization:root;
};

LET $block = {
	LET $nested = (SELECT VALUE headcount FROM organization_unit);
	LET $total = math::sum($nested);
	RETURN $total;
};

LET $destructured = (SELECT address.{ city, country } AS place FROM organization);

LET $obj = { alpha: 1, beta: 'two', gamma: [true, false] };

LET $arr = [1, 2, 3];

RETURN { count: $count, names: $names, first: $first, tiers: $tiers, block: $block, obj: $obj, arr: $arr, destructured: $destructured };
