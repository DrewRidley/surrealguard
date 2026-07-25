-- Core identity tables. Shapes mirror the real-world corpus: `option<record<…>>`
-- links, `DEFAULT`/`VALUE` fields, and a `COMPUTED` field that reads siblings
-- through `$this`.
--
-- Declaration order matters to the analyzer (a `record<T>` naming a table
-- defined later in workspace order is reported as undefined), so every table
-- here is declared before it is referenced.

DEFINE TABLE person SCHEMAFULL;

DEFINE FIELD first_name ON person TYPE string;

DEFINE FIELD middle_name ON person TYPE option<string>;

DEFINE FIELD last_name ON person TYPE string;

DEFINE FIELD date_of_birth ON person TYPE option<datetime>;

DEFINE FIELD full_name ON person COMPUTED string::concat($this.first_name, ' ', $this.last_name);

DEFINE FIELD created_at ON person TYPE datetime DEFAULT time::now();

DEFINE TABLE account SCHEMAFULL;

DEFINE FIELD username ON account TYPE string;

DEFINE FIELD email ON account TYPE string;

DEFINE FIELD person ON account TYPE option<record<person>>;

DEFINE FIELD active ON account TYPE bool DEFAULT true;

DEFINE FIELD settings ON account TYPE {
	timezone: string,
	locale: option<string>,
	notify: bool
} DEFAULT { timezone: 'UTC', locale: NONE, notify: true };

DEFINE FIELD created_at ON account TYPE datetime DEFAULT time::now();

DEFINE FIELD updated_at ON account TYPE datetime VALUE time::now();
