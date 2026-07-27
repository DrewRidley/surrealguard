-- Every clause that supplies a field's value carries the same contract, so a
-- constant that cannot inhabit the declared type must report from all three.
-- `COMPUTED` was absent from the clause loop entirely: identical mistakes were
-- reported from `VALUE`/`DEFAULT` and silent from `COMPUTED`.

DEFINE TABLE bad_field SCHEMALESS;

DEFINE FIELD by_value ON bad_field TYPE int VALUE 'notanint';

DEFINE FIELD by_default ON bad_field TYPE int DEFAULT 'notanint';

DEFINE FIELD by_computed ON bad_field TYPE int COMPUTED 'notanint';

-- The literal-union case, which is what the whole contract work started from.
DEFINE FIELD hue_value ON bad_field TYPE 'red' | 'blue' VALUE 'green';

DEFINE FIELD hue_default ON bad_field TYPE 'red' | 'blue' DEFAULT 'green';

DEFINE FIELD hue_computed ON bad_field TYPE 'red' | 'blue' COMPUTED 'green';
