-- Schema for this crate's own tests. The macros discover it by the
-- `schema.surql` convention path, so every `query!` in `crates/rs` is checked
-- and typed against it.

DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE FIELD age ON user TYPE int;
DEFINE FIELD ratio ON user TYPE float;
DEFINE FIELD score ON user TYPE decimal;
DEFINE FIELD created ON user TYPE datetime;
DEFINE FIELD ttl ON user TYPE duration;
DEFINE FIELD nickname ON user TYPE option<string>;
DEFINE FIELD tags ON user TYPE array<string>;
DEFINE FIELD best ON user TYPE option<record<post>>;

DEFINE TABLE post SCHEMAFULL;
DEFINE FIELD title ON post TYPE string;
DEFINE FIELD author ON post TYPE record<user>;
