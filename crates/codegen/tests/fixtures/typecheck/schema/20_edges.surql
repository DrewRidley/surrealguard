-- An edge table: traversals (`->knows->person`) and the implicit `in`/`out`
-- record links on the edge row itself.

DEFINE TABLE knows SCHEMAFULL TYPE RELATION FROM person TO person;

DEFINE FIELD since ON knows TYPE datetime;

DEFINE FIELD weight ON knows TYPE float;
