-- Union receivers, on the side where the contract IS met. A union answers a
-- method when every arm answers it, and a multi-table link answers a field when
-- any arm declares it — the invalid corpus holds the mirror image of both.

-- `stage` is `'draft' | 'review' | 'published'`: three arms, all strings, so a
-- string method resolves and the result is a `string`.
SELECT stage.uppercase() AS shouted FROM document;

-- `owner` is `record<account | organization>`. `created_at` is declared on both
-- tables with the same kind, so the read is provable across the whole union.
SELECT owner.created_at AS owned_since FROM document;

-- One arm is enough. `username` is `account`'s and not `organization`'s; the
-- read is legitimate polymorphic SurrealQL whose value is simply NONE for an
-- organization owner, and reporting it would be reporting a program that works.
SELECT owner.username AS owner_username FROM document;
SELECT owner.legal_name AS owner_legal_name FROM document;

-- The same, through the `option` wrapper the link is declared with.
SELECT reviewer.username AS reviewer_username FROM document;
