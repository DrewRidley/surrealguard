-- Calls into the corpus' own `fn::` definitions plus builtin dispatch, both
-- direct and as a method on a typed receiver.

SELECT fn::org::display_name(id) AS label FROM organization;

LET $owner = fn::org::owner(organization:acme);

LET $heads = fn::org::headcount(organization:acme);

LET $initials = fn::person::initials('Ada', 'Lovelace');

LET $pro = fn::org::is_pro(organization:acme);

LET $label = fn::org::label(organization:acme);

LET $upper = string::uppercase('acme');

LET $joined = array::join(['a', 'b'], '-');

LET $when = time::now();

LET $parsed = <int> '42';

RETURN { owner: $owner, heads: $heads, initials: $initials, pro: $pro, label: $label, upper: $upper, joined: $joined, when: $when, parsed: $parsed };
