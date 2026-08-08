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

-- The live spellings of the three families whose retired colon forms are now
-- reported. These are here because retiring the colon spelling very nearly
-- took the underscore one with it: the completion catalog listed ONLY
-- `type::is::x`, folding both spellings onto one key, so removing the dead
-- entry removed the live function too and `age.is_none()` stopped resolving.
-- Each of these must keep both its acceptance and its inferred type.

LET $is_str = type::is_string('x');

LET $is_mail = string::is_email('a@b.c');

LET $is_empty = array::is_empty([]);

LET $from_days = duration::from_days(1);

LET $from_unix = time::from_unix(1);

-- The renamed one-offs, likewise: the name the engine kept.
LET $tb = meta::tb(organization:acme);

LET $rid = rand::id();

RETURN { owner: $owner, heads: $heads, initials: $initials, pro: $pro, label: $label, upper: $upper, joined: $joined, when: $when, parsed: $parsed, is_str: $is_str, is_mail: $is_mail, is_empty: $is_empty, from_days: $from_days, from_unix: $from_unix, tb: $tb, rid: $rid };
