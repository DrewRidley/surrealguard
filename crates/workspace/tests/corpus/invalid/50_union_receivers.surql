-- A union receiver is a receiver. Every contract a single kind has to meet,
-- a union has to meet on every arm — and until this file existed, a union met
-- none of them: the whole class of reads below resolved silently.
--
-- The valid half is `queries/82_union_receivers.surql`; the two are a pair.
-- Anything reported here that is legitimate on ONE arm would be a false
-- positive, which is exactly what that file pins.

-- A multi-table link, `record<account | organization>`. Neither table declares
-- `nope`, so the read is provably absent on every arm — the same contract, and
-- the same 1002, as a missing field behind a single-table link.
SELECT owner.nope FROM document;

-- Through the `option` wrapper: `option<record<account | organization>>` crosses
-- into the same two tables.
SELECT reviewer.nope FROM document;

-- Behind a splat, which names no path segment: `nope` is still the linked
-- tables' field to have.
SELECT owner.*.nope FROM document;

-- A method on a field read through the link. `username` is `account`'s alone,
-- so the value is `string | none` for an `organization` owner — and `none` has
-- no `len`, which is what makes the call wrong rather than the field.
SELECT owner.username.len() FROM document;

-- A method no arm answers. `summary` is `option<string | null>` — `none` and
-- `null` have no `len`, so the union does not have it either, and the engine
-- would refuse the call for any row where the field is unset.
SELECT summary.len() FROM document;

-- A method only SOME arms answer is still not a method the union has:
-- `word_count` is `option<int>`, and `int` has no `uppercase`.
SELECT word_count.uppercase() FROM document;
