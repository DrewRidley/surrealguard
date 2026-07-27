-- `DEFINE PARAM` has no TYPE clause (engine-verified: `DEFINE PARAM $x TYPE
-- int VALUE 5` is a parse error on 3.0.5), so the kind its value is checked
-- against is whatever its *uses* require.
--
-- The finding therefore belongs on the definition. It used to land on the use
-- as an E6001 reading "one use needs `string`, this one needs `int`" — blaming
-- a line that is correct, and calling the definition "one use".

DEFINE PARAM $bad_limit VALUE 'notanint';

DEFINE PARAM $bad_org VALUE 'not-a-record';

SELECT name FROM organization LIMIT $bad_limit;

RETURN fn::org::display_name($bad_org);
