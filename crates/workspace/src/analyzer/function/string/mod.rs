//! `string` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod capitalize;
pub mod concat;
pub mod contains;
pub mod ends_with;
pub mod is_alpha;
pub mod is_alphanum;
pub mod is_ascii;
pub mod is_datetime;
pub mod is_domain;
pub mod is_email;
pub mod is_hexadecimal;
pub mod is_ip;
pub mod is_ipv4;
pub mod is_ipv6;
pub mod is_latitude;
pub mod is_longitude;
pub mod is_numeric;
pub mod is_record;
pub mod is_semver;
pub mod is_ulid;
pub mod is_url;
pub mod is_uuid;
pub mod join;
pub mod len;
pub mod lowercase;
pub mod matches;
pub mod repeat;
pub mod replace;
pub mod reverse;
pub mod slice;
pub mod slug;
pub mod split;
pub mod starts_with;
pub mod trim;
pub mod uppercase;
pub mod words;

pub fn analyze_string_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "string::capitalize" => capitalize::analyze_string_capitalize(ctx, call, args),
        "string::concat" => concat::analyze_string_concat(ctx, call, args),
        "string::contains" => contains::analyze_string_contains(ctx, call, args),
        "string::ends_with" => ends_with::analyze_string_ends_with(ctx, call, args),
        "string::is_alpha" => is_alpha::analyze_string_is_alpha(ctx, call, args),
        "string::is_alphanum" => is_alphanum::analyze_string_is_alphanum(ctx, call, args),
        "string::is_ascii" => is_ascii::analyze_string_is_ascii(ctx, call, args),
        "string::is_datetime" => is_datetime::analyze_string_is_datetime(ctx, call, args),
        "string::is_domain" => is_domain::analyze_string_is_domain(ctx, call, args),
        "string::is_email" => is_email::analyze_string_is_email(ctx, call, args),
        "string::is_hexadecimal" => is_hexadecimal::analyze_string_is_hexadecimal(ctx, call, args),
        "string::is_ip" => is_ip::analyze_string_is_ip(ctx, call, args),
        "string::is_ipv4" => is_ipv4::analyze_string_is_ipv4(ctx, call, args),
        "string::is_ipv6" => is_ipv6::analyze_string_is_ipv6(ctx, call, args),
        "string::is_latitude" => is_latitude::analyze_string_is_latitude(ctx, call, args),
        "string::is_longitude" => is_longitude::analyze_string_is_longitude(ctx, call, args),
        "string::is_numeric" => is_numeric::analyze_string_is_numeric(ctx, call, args),
        "string::is_record" => is_record::analyze_string_is_record(ctx, call, args),
        "string::is_semver" => is_semver::analyze_string_is_semver(ctx, call, args),
        "string::is_ulid" => is_ulid::analyze_string_is_ulid(ctx, call, args),
        "string::is_url" => is_url::analyze_string_is_url(ctx, call, args),
        "string::is_uuid" => is_uuid::analyze_string_is_uuid(ctx, call, args),
        "string::join" => join::analyze_string_join(ctx, call, args),
        "string::len" => len::analyze_string_len(ctx, call, args),
        "string::lowercase" => lowercase::analyze_string_lowercase(ctx, call, args),
        "string::matches" => matches::analyze_string_matches(ctx, call, args),
        "string::repeat" => repeat::analyze_string_repeat(ctx, call, args),
        "string::replace" => replace::analyze_string_replace(ctx, call, args),
        "string::reverse" => reverse::analyze_string_reverse(ctx, call, args),
        "string::slice" => slice::analyze_string_slice(ctx, call, args),
        "string::slug" => slug::analyze_string_slug(ctx, call, args),
        "string::split" => split::analyze_string_split(ctx, call, args),
        "string::starts_with" => starts_with::analyze_string_starts_with(ctx, call, args),
        "string::trim" => trim::analyze_string_trim(ctx, call, args),
        "string::uppercase" => uppercase::analyze_string_uppercase(ctx, call, args),
        "string::words" => words::analyze_string_words(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
