//! `type` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod range;
pub mod array;
pub mod bool;
pub mod bytes;
pub mod datetime;
pub mod decimal;
pub mod duration;
pub mod record;
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
pub mod set;
pub mod string;
pub mod string_lossy;
pub mod table;
pub mod thing;
pub mod uuid;

pub(crate) fn analyze_type_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "type::array" => array::analyze_type_array(ctx, call, args),
        "type::bool" => bool::analyze_type_bool(ctx, call, args),
        "type::bytes" => bytes::analyze_type_bytes(ctx, call, args),
        "type::datetime" => datetime::analyze_type_datetime(ctx, call, args),
        "type::decimal" => decimal::analyze_type_decimal(ctx, call, args),
        "type::duration" => duration::analyze_type_duration(ctx, call, args),
        "type::field" => field::analyze_type_field(ctx, call, args),
        "type::fields" => fields::analyze_type_fields(ctx, call, args),
        "type::file" => file::analyze_type_file(ctx, call, args),
        "type::float" => float::analyze_type_float(ctx, call, args),
        "type::int" => int::analyze_type_int(ctx, call, args),
        "type::is_array" => is_array::analyze_type_is_array(ctx, call, args),
        "type::is_bool" => is_bool::analyze_type_is_bool(ctx, call, args),
        "type::is_bytes" => is_bytes::analyze_type_is_bytes(ctx, call, args),
        "type::is_collection" => is_collection::analyze_type_is_collection(ctx, call, args),
        "type::is_datetime" => is_datetime::analyze_type_is_datetime(ctx, call, args),
        "type::is_decimal" => is_decimal::analyze_type_is_decimal(ctx, call, args),
        "type::is_duration" => is_duration::analyze_type_is_duration(ctx, call, args),
        "type::is_float" => is_float::analyze_type_is_float(ctx, call, args),
        "type::is_geometry" => is_geometry::analyze_type_is_geometry(ctx, call, args),
        "type::is_int" => is_int::analyze_type_is_int(ctx, call, args),
        "type::is_line" => is_line::analyze_type_is_line(ctx, call, args),
        "type::is_multiline" => is_multiline::analyze_type_is_multiline(ctx, call, args),
        "type::is_multipoint" => is_multipoint::analyze_type_is_multipoint(ctx, call, args),
        "type::is_multipolygon" => is_multipolygon::analyze_type_is_multipolygon(ctx, call, args),
        "type::is_none" => is_none::analyze_type_is_none(ctx, call, args),
        "type::is_null" => is_null::analyze_type_is_null(ctx, call, args),
        "type::is_number" => is_number::analyze_type_is_number(ctx, call, args),
        "type::is_object" => is_object::analyze_type_is_object(ctx, call, args),
        "type::is_point" => is_point::analyze_type_is_point(ctx, call, args),
        "type::is_polygon" => is_polygon::analyze_type_is_polygon(ctx, call, args),
        "type::is_range" => is_range::analyze_type_is_range(ctx, call, args),
        "type::is_record" => is_record::analyze_type_is_record(ctx, call, args),
        "type::is_string" => is_string::analyze_type_is_string(ctx, call, args),
        "type::is_uuid" => is_uuid::analyze_type_is_uuid(ctx, call, args),
        "type::number" => number::analyze_type_number(ctx, call, args),
        "type::of" => of::analyze_type_of(ctx, call, args),
        "type::point" => point::analyze_type_point(ctx, call, args),
        "type::range" => range::analyze_type_range(ctx, call, args),
        "type::record" => record::analyze_type_record(ctx, call, args),
        "type::string" => string::analyze_type_string(ctx, call, args),
        "type::string_lossy" => string_lossy::analyze_type_string_lossy(ctx, call, args),
        "type::table" => table::analyze_type_table(ctx, call, args),
        "type::thing" => thing::analyze_type_thing(ctx, call, args),
        "type::uuid" => uuid::analyze_type_uuid(ctx, call, args),
        "type::geometry" => geometry::analyze_type_geometry(ctx, call, args),
        "type::is_set" => is_set::analyze_type_is_set(ctx, call, args),
        "type::set" => set::analyze_type_set(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
