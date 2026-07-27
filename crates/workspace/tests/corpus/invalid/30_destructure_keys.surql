-- A `.{…}` destructure names its own key set, so a field read out of the
-- object it built is either one of those keys or provably absent — there is no
-- schema left to be lenient about. The engine returns NONE for the absent ones
-- (3.0.5: `SELECT author.{name}.age FROM post` -> `[{r: null}]`), and the
-- site's type would otherwise be a silent `any`.
--
-- The schema half of the valid corpus is in scope here, so these are wrong
-- queries about real tables.

-- The selection holds `username`; `email` is not in it, however real a field
-- of `account` it is.
SELECT owner.{ username }.email AS a FROM organization;

-- An EMPTY destructure selected nothing at all, so nothing can be read back.
SELECT owner.{}.username AS n FROM organization;

-- The same contract on a graph tail, where the destructure sits behind a
-- traversal rather than a record link.
SELECT ->employee_of->organization.{ name }.tier AS t FROM account;

-- The other half: a key the destructure selected but the TARGET does not
-- declare. The selection itself is reported — and reading `nope` back out
-- afterwards is legitimate, it is a key the object has.
SELECT owner.{ nope }.nope AS n FROM organization;

-- A destructure with a tail was silent about its selection purely because the
-- destructure was not the final part; unaliased makes no difference.
SELECT ->employee_of->organization.{ nope }.nope FROM account;
