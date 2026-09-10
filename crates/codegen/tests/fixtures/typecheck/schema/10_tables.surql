-- One value of every convention the generator spells: a record link, an
-- option, an array, a literal union, a closed object, and each SDK value class
-- (`Decimal`, `Uuid`, `Duration`, `Date`). Tables are declared before they are
-- referenced because `record<T>` resolves in workspace order.

DEFINE TABLE team SCHEMAFULL;

DEFINE FIELD name ON team TYPE string;

DEFINE FIELD budget ON team TYPE decimal;

DEFINE TABLE person SCHEMAFULL;

DEFINE FIELD name ON person TYPE string;

DEFINE FIELD nick ON person TYPE option<string>;

DEFINE FIELD age ON person TYPE int;

DEFINE FIELD joined ON person TYPE datetime;

DEFINE FIELD team ON person TYPE record<team>;

DEFINE FIELD mentor ON person TYPE option<record<person>>;

DEFINE FIELD tags ON person TYPE array<string>;

DEFINE FIELD status ON person TYPE 'active' | 'retired' DEFAULT 'active';

DEFINE FIELD settings ON person TYPE {
	timezone: string,
	locale: option<string>,
	notify: bool
};

DEFINE FIELD external_id ON person TYPE uuid;

DEFINE FIELD tenure ON person TYPE duration;
