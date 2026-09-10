-- Spellings SurrealDB has retired. These are not merely unregistered names:
-- 3.2.3 refuses to *parse* a call to one, so the query never runs at all.
-- We accepted every one of them, and the completion list offered 45 of them
-- as suggestions — the worst version of this, since it puts syntax that
-- cannot execute in front of the author as a recommendation.
--
-- Each line was probed against a live 3.2.3 and answered with
--   Parse error: Invalid function/constant path[, did you maybe mean `X`]
-- and each replacement below is the engine's own suggestion where it offers
-- one. Reported as 5001, the contract a name that does not exist already had.

-- The `::from::` families, retired wholesale.
RETURN duration::from::days(1);

RETURN time::from::unix(1);

-- The `::is::` family. These were the invisible ones: lowering canonicalized
-- `::is::` to `::is_` before any analyzer saw the path, so the dead spelling
-- and the live one were the same string by the time anything could tell them
-- apart. Three namespaces, to show it is the whole family and not one table.
RETURN type::is::string('x');

RETURN string::is::email('a@b.c');

RETURN array::is::empty([]);

-- Renamed one-offs, each with the engine's replacement.
RETURN meta::table(account:1);

RETURN rand::guid();

RETURN sequence::next('s');

RETURN session::sc();

RETURN session::sd();

-- Removed outright, with no replacement inside their namespace: `not::not(x)`
-- is the `!x` operator, `count::count(…)` is bare `count(…)`, and reference
-- traversal is the `<~` idiom. The engine offers no suggestion for these
-- either, so neither do we.
RETURN not::not(true);

RETURN count::count([1, 2]);

RETURN record::refs(account:1);
