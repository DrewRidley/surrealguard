-- The engagement half: a relation table with real IN/OUT tables and its own
-- fields (so `RELATE … SET`/`CONTENT`, `<->`, and `DELETE` on an edge have
-- something to type against), a CHANGEFEED table driven by a sequence with
-- three events reading `$before`/`$after`/`$event`, a view table, a `DEFINE
-- PARAM` the queries consume, and the parse-only `DEFINE ACCESS`/`DEFINE USER`.
--
-- Verified against a live SurrealDB 3.2.3.

DEFINE TABLE likes SCHEMAFULL TYPE RELATION IN account OUT article;

DEFINE FIELD weight ON likes TYPE float DEFAULT 1.0;

DEFINE FIELD note ON likes TYPE option<string>;

DEFINE FIELD liked_at ON likes TYPE datetime DEFAULT time::now();

-- One like per (account, article) pair.
DEFINE INDEX likes_pair ON likes FIELDS in, out UNIQUE;

DEFINE SEQUENCE ticket_number;

DEFINE PARAM $max_priority VALUE 5;

DEFINE PARAM $search_limit VALUE 10;

DEFINE TABLE ticket SCHEMAFULL CHANGEFEED 3d;

-- A sequence-backed number: `DEFAULT` is the one clause where `sequence::*`
-- is an idiom rather than a re-running side effect, and READONLY pins it.
DEFINE FIELD number ON ticket TYPE int READONLY DEFAULT sequence::nextval('ticket_number');

DEFINE FIELD subject ON ticket TYPE string;

DEFINE FIELD priority ON ticket TYPE int DEFAULT 3 ASSERT $value >= 1 AND $value <= 5;

DEFINE FIELD status ON ticket TYPE 'open' | 'closed' DEFAULT 'open';

DEFINE FIELD reporter ON ticket TYPE record<account>;

DEFINE FIELD article ON ticket TYPE option<record<article>>;

DEFINE FIELD opened_at ON ticket TYPE datetime READONLY DEFAULT time::now();

DEFINE FIELD closed_at ON ticket TYPE option<datetime>;

DEFINE FIELD label ON ticket COMPUTED string::concat('#', <string> $this.number, ' ', $this.subject);

DEFINE TABLE ticket_log SCHEMAFULL;

DEFINE FIELD ticket ON ticket_log TYPE record<ticket>;

-- `$event` is the literal union `'CREATE' | 'UPDATE' | 'DELETE'`, so the
-- events below can write it straight into this field.
DEFINE FIELD kind ON ticket_log TYPE 'CREATE' | 'UPDATE' | 'DELETE';

DEFINE FIELD from_priority ON ticket_log TYPE option<int>;

DEFINE FIELD to_priority ON ticket_log TYPE option<int>;

DEFINE FIELD at ON ticket_log TYPE datetime DEFAULT time::now();

DEFINE EVENT ticket_created ON ticket WHEN $event = 'CREATE' THEN {
	CREATE ticket_log SET ticket = $after.id, kind = $event, to_priority = $after.priority;
};

DEFINE EVENT ticket_reprioritised ON ticket WHEN $event = 'UPDATE' AND $before.priority != $after.priority THEN {
	CREATE ticket_log SET ticket = $after.id, kind = $event, from_priority = $before.priority, to_priority = $after.priority;
};

DEFINE EVENT ticket_deleted ON ticket WHEN $event = 'DELETE' THEN {
	CREATE ticket_log SET ticket = $before.id, kind = 'DELETE', from_priority = $before.priority;
};

-- A view: its rows are the projection of the SELECT it is defined as.
DEFINE TABLE ticket_stats AS SELECT status, count() AS total FROM ticket GROUP BY status;

-- Parse-only definitions: access and system users carry no row shape.
DEFINE ACCESS member ON DATABASE TYPE RECORD
	SIGNUP (CREATE account SET username = $username, email = $email)
	SIGNIN (SELECT * FROM account WHERE username = $username AND email = $email)
	DURATION FOR TOKEN 15m, FOR SESSION 12h;

DEFINE USER reporter ON DATABASE PASSWORD 'change-me' ROLES VIEWER;
