//! `type` function family: every built-in it dispatches, with its analyzer.

use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::BuiltinEntry;

pub mod array;
pub mod bool;
pub mod bytes;
pub mod datetime;
pub mod decimal;
pub mod duration;
pub mod field;
pub mod fields;
pub mod file;
pub mod float;
pub mod geometry;
pub mod int;
pub mod is_array;
pub mod is_bool;
pub mod is_bytes;
pub mod is_collection;
pub mod is_datetime;
pub mod is_decimal;
pub mod is_duration;
pub mod is_float;
pub mod is_geometry;
pub mod is_int;
pub mod is_line;
pub mod is_multiline;
pub mod is_multipoint;
pub mod is_multipolygon;
pub mod is_none;
pub mod is_null;
pub mod is_number;
pub mod is_object;
pub mod is_point;
pub mod is_polygon;
pub mod is_range;
pub mod is_record;
pub mod is_set;
pub mod is_string;
pub mod is_uuid;
pub mod number;
pub mod of;
pub mod point;
pub mod range;
pub mod record;
pub mod set;
pub mod string;
pub mod string_lossy;
pub mod table;
pub mod uuid;

/// Every `type::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "type::array",
        "Converts a value to an array.",
        array::signature,
        array::analyze_type_array,
    ),
    BuiltinEntry::new(
        "type::bool",
        "Converts a value to a boolean.",
        bool::signature,
        bool::analyze_type_bool,
    ),
    BuiltinEntry::new(
        "type::bytes",
        "Converts a value to bytes.",
        bytes::signature,
        bytes::analyze_type_bytes,
    ),
    BuiltinEntry::new(
        "type::datetime",
        "Converts a value to a datetime.",
        datetime::signature,
        datetime::analyze_type_datetime,
    ),
    BuiltinEntry::new(
        "type::decimal",
        "Converts a value to a decimal.",
        decimal::signature,
        decimal::analyze_type_decimal,
    ),
    BuiltinEntry::new(
        "type::duration",
        "Converts a value to a duration.",
        duration::signature,
        duration::analyze_type_duration,
    ),
    BuiltinEntry::new(
        "type::field",
        "The value of the field named by a path string, on the current row.",
        field::signature,
        field::analyze_type_field,
    ),
    BuiltinEntry::new(
        "type::fields",
        "The values of the fields named by an array of path strings, on the current row.",
        fields::signature,
        fields::analyze_type_fields,
    ),
    BuiltinEntry::new(
        "type::file",
        "A file pointer for a bucket and key.",
        file::signature,
        file::analyze_type_file,
    ),
    BuiltinEntry::new(
        "type::float",
        "Converts a value to a float.",
        float::signature,
        float::analyze_type_float,
    ),
    BuiltinEntry::new(
        "type::int",
        "Converts a value to an integer.",
        int::signature,
        int::analyze_type_int,
    ),
    BuiltinEntry::new(
        "type::is_array",
        "Whether the value is an array.",
        is_array::signature,
        is_array::analyze_type_is_array,
    ),
    BuiltinEntry::new(
        "type::is_bool",
        "Whether the value is a boolean.",
        is_bool::signature,
        is_bool::analyze_type_is_bool,
    ),
    BuiltinEntry::new(
        "type::is_bytes",
        "Whether the value is bytes.",
        is_bytes::signature,
        is_bytes::analyze_type_is_bytes,
    ),
    BuiltinEntry::new(
        "type::is_collection",
        "Whether the value is a geometry collection.",
        is_collection::signature,
        is_collection::analyze_type_is_collection,
    ),
    BuiltinEntry::new(
        "type::is_datetime",
        "Whether the value is a datetime.",
        is_datetime::signature,
        is_datetime::analyze_type_is_datetime,
    ),
    BuiltinEntry::new(
        "type::is_decimal",
        "Whether the value is a decimal.",
        is_decimal::signature,
        is_decimal::analyze_type_is_decimal,
    ),
    BuiltinEntry::new(
        "type::is_duration",
        "Whether the value is a duration.",
        is_duration::signature,
        is_duration::analyze_type_is_duration,
    ),
    BuiltinEntry::new(
        "type::is_float",
        "Whether the value is a float.",
        is_float::signature,
        is_float::analyze_type_is_float,
    ),
    BuiltinEntry::new(
        "type::is_geometry",
        "Whether the value is a geometry.",
        is_geometry::signature,
        is_geometry::analyze_type_is_geometry,
    ),
    BuiltinEntry::new(
        "type::is_int",
        "Whether the value is an integer.",
        is_int::signature,
        is_int::analyze_type_is_int,
    ),
    BuiltinEntry::new(
        "type::is_line",
        "Whether the value is a line geometry.",
        is_line::signature,
        is_line::analyze_type_is_line,
    ),
    BuiltinEntry::new(
        "type::is_multiline",
        "Whether the value is a multi-line geometry.",
        is_multiline::signature,
        is_multiline::analyze_type_is_multiline,
    ),
    BuiltinEntry::new(
        "type::is_multipoint",
        "Whether the value is a multi-point geometry.",
        is_multipoint::signature,
        is_multipoint::analyze_type_is_multipoint,
    ),
    BuiltinEntry::new(
        "type::is_multipolygon",
        "Whether the value is a multi-polygon geometry.",
        is_multipolygon::signature,
        is_multipolygon::analyze_type_is_multipolygon,
    ),
    BuiltinEntry::new(
        "type::is_none",
        "Whether the value is NONE.",
        is_none::signature,
        is_none::analyze_type_is_none,
    ),
    BuiltinEntry::new(
        "type::is_null",
        "Whether the value is NULL.",
        is_null::signature,
        is_null::analyze_type_is_null,
    ),
    BuiltinEntry::new(
        "type::is_number",
        "Whether the value is a number.",
        is_number::signature,
        is_number::analyze_type_is_number,
    ),
    BuiltinEntry::new(
        "type::is_object",
        "Whether the value is an object.",
        is_object::signature,
        is_object::analyze_type_is_object,
    ),
    BuiltinEntry::new(
        "type::is_point",
        "Whether the value is a point geometry.",
        is_point::signature,
        is_point::analyze_type_is_point,
    ),
    BuiltinEntry::new(
        "type::is_polygon",
        "Whether the value is a polygon geometry.",
        is_polygon::signature,
        is_polygon::analyze_type_is_polygon,
    ),
    BuiltinEntry::new(
        "type::is_range",
        "Whether the value is a range.",
        is_range::signature,
        is_range::analyze_type_is_range,
    ),
    BuiltinEntry::new(
        "type::is_record",
        "Whether the value is a record id, optionally of the given table.",
        is_record::signature,
        is_record::analyze_type_is_record,
    ),
    BuiltinEntry::new(
        "type::is_string",
        "Whether the value is a string.",
        is_string::signature,
        is_string::analyze_type_is_string,
    ),
    BuiltinEntry::new(
        "type::is_uuid",
        "Whether the value is a UUID.",
        is_uuid::signature,
        is_uuid::analyze_type_is_uuid,
    ),
    BuiltinEntry::new(
        "type::number",
        "Converts a value to a number.",
        number::signature,
        number::analyze_type_number,
    ),
    BuiltinEntry::new(
        "type::of",
        "The name of the value's type.",
        of::signature,
        of::analyze_type_of,
    ),
    BuiltinEntry::new(
        "type::point",
        "Converts a value or coordinate pair to a point geometry.",
        point::signature,
        point::analyze_type_point,
    ),
    BuiltinEntry::new(
        "type::range",
        "Converts a value to a range.",
        range::signature,
        range::analyze_type_range,
    ),
    BuiltinEntry::new(
        "type::record",
        "Converts a value to a record id, optionally of the given table.",
        record::signature,
        record::analyze_type_record,
    ),
    BuiltinEntry::new(
        "type::string",
        "Converts a value to a string.",
        string::signature,
        string::analyze_type_string,
    ),
    BuiltinEntry::new(
        "type::string_lossy",
        "Converts bytes to a string, replacing invalid UTF-8.",
        string_lossy::signature,
        string_lossy::analyze_type_string_lossy,
    ),
    BuiltinEntry::new(
        "type::table",
        "Converts a value to a table name.",
        table::signature,
        table::analyze_type_table,
    ),
    BuiltinEntry::new(
        "type::uuid",
        "Converts a value to a UUID.",
        uuid::signature,
        uuid::analyze_type_uuid,
    ),
    BuiltinEntry::new(
        "type::geometry",
        "Converts a value to a geometry.",
        geometry::signature,
        geometry::analyze_type_geometry,
    ),
    BuiltinEntry::new(
        "type::is_set",
        "Whether the value is a set.",
        is_set::signature,
        is_set::analyze_type_is_set,
    ),
    BuiltinEntry::new(
        "type::set",
        "Converts a value to a set.",
        set::signature,
        set::analyze_type_set,
    ),
];

/// The table a `type::` constructor's table argument names, when the argument
/// is provably one table and not merely "some table".
///
/// `type::record('person', $id)` is a `record<person>` on every run: the table
/// half is a constant, so the constructed link's table is decided statically
/// and a schema field declared `record<person>` accepts it (engine-verified on
/// 3.2.3 — `CREATE ticket SET owner = type::record('person', 'a')` stores
/// `person:a`, while `type::record('metrics', 'a')` is rejected with
/// "Expected `record<person>` but found `metrics:a`").
///
/// **Prove or stay silent.** `type::record($table, $id)` is a record whose
/// table nobody knows until the parameter is bound, so it stays the
/// unconstrained `record` — not `any`, and never a guess. The const channel
/// resolves a `LET`-bound string too (`LET $t = 'person'` is as constant as the
/// literal), and nothing else.
///
/// The empty string is rejected rather than taken: the engine refuses it
/// ("Found  for the Record ID but this is not a valid table name"), so there is
/// no table to name.
pub(super) fn constant_table_arg(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    index: usize,
) -> Option<surrealdb_types::Table> {
    match crate::analyzer::function::const_value_arg(ctx, call, index)? {
        surrealdb_types::Value::String(name) if !name.is_empty() => Some(name.as_str().into()),
        _ => None,
    }
}
