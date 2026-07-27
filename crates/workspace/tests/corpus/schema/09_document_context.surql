-- Equivalence class: THE DOCUMENT CONTEXT.
--
-- SurrealDB binds a fixed set of params itself, and *where* it binds each one
-- is the only thing that separates them. Five hand-written lists used to answer
-- that question and no two agreed, so the same name behaved differently
-- depending on which list a given check happened to consult: `$this` was
-- protected from `LET` but not context-only (a top-level `$this` was demanded
-- from the caller), `$self` was bound in three constructs and protected in
-- none, `$scope` was seeded and shadowable.
--
-- Every name below must resolve inside the construct that binds it, in every
-- construct that binds it — the hover map and the analyzer's binder are now one
-- table, so a name an editor offers is a name that resolves.

DEFINE TABLE document_note SCHEMAFULL;

DEFINE FIELD body ON document_note TYPE string;

DEFINE FIELD author ON document_note TYPE record<account>;

-- `$this` and `$self` name the same record, so the two spellings must produce
-- the same kind.
DEFINE FIELD this_body ON document_note COMPUTED $this.body;

DEFINE FIELD self_body ON document_note COMPUTED $self.body;

-- `$before`/`$after` are the record too, in a field clause.
DEFINE FIELD before_body ON document_note VALUE $before.body;

-- `$value` is the field's own declared kind; `$input` is the value being
-- written, before coercion — not the record containing it.
DEFINE FIELD trimmed ON document_note TYPE string VALUE string::trim($value);

-- The same names, inside a `DEFINE EVENT` body. `$before`/`$after`/`$value`
-- carry the row's full field object there; `$this`/`$self` still name the
-- record.
DEFINE EVENT document_note_touch ON document_note WHEN $event = 'UPDATE' THEN {
	LET $event_before = $before.body;
	LET $event_after = $after.body;
	LET $event_this = $this.body;
	LET $event_self = $self.body;
	LET $event_input = $input.body;
	RETURN $event_before;
};

-- And inside a `PERMISSIONS` predicate, where the session params are bound too.
DEFINE TABLE document_audit SCHEMAFULL
	PERMISSIONS FOR select WHERE $this.id != NONE AND $auth != NONE;

DEFINE FIELD note ON document_audit TYPE record<document_note>;
