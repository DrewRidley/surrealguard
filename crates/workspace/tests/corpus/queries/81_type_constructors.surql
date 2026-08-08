-- `type::` constructors. Every one of these is valid SurrealQL, so the value
-- of the file is the *types*: the precision snapshot records what each call
-- infers, and a constructed link losing its table is exactly the regression
-- that turns a correct `SET owner = type::record('account', $id)` back into a
-- false 2001.

LET $account_id = 'ada';

-- A string literal decides the table: `record<account>`, not `record`.
LET $by_literal = type::record('account', $account_id);

-- A `LET`-bound constant is as constant as the literal.
LET $account_table = 'account';
LET $by_binding = type::record($account_table, $account_id);

-- …and the constructed link satisfies a field declared with that table.
UPDATE organization_unit:ops SET organization = type::record('organization', $account_id);

-- The one-argument form passes a record through with its table intact.
LET $round_trip = type::record(account:ada);

-- `type::table` is a table value, and a constant argument names it.
LET $table_value = type::table('organization');

-- The scalar casts each have their kind regardless of the argument.
LET $as_string = type::string($account_id);
LET $as_int = type::int('42');
LET $as_float = type::float('42');
LET $as_decimal = type::decimal('42');
LET $as_number = type::number('42');
LET $as_bool = type::bool('true');
LET $as_datetime = type::datetime('2026-01-01T00:00:00Z');
LET $as_duration = type::duration('1h');
LET $as_uuid = type::uuid('018a6680-bef9-701b-9025-e1754f296a0f');
LET $as_bytes = type::bytes('abc');
LET $as_point = type::point([1, 2]);
LET $as_range = type::range([1, 5]);
LET $as_array = type::array([1, 2]);
LET $as_set = type::set([1, 2, 2]);
LET $as_of = type::of(1);

RETURN {
    by_literal: $by_literal,
    by_binding: $by_binding,
    round_trip: $round_trip,
    table_value: $table_value,
    as_string: $as_string,
    as_int: $as_int,
    as_float: $as_float,
    as_decimal: $as_decimal,
    as_number: $as_number,
    as_bool: $as_bool,
    as_datetime: $as_datetime,
    as_duration: $as_duration,
    as_uuid: $as_uuid,
    as_bytes: $as_bytes,
    as_point: $as_point,
    as_range: $as_range,
    as_array: $as_array,
    as_set: $as_set,
    as_of: $as_of
};
