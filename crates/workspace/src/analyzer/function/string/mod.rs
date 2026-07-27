//! `string` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod capitalize;
pub mod concat;
pub mod contains;
pub mod distance_damerau_levenshtein;
pub mod distance_hamming;
pub mod distance_levenshtein;
pub mod distance_normalized_damerau_levenshtein;
pub mod distance_normalized_levenshtein;
pub mod distance_osa;
pub mod ends_with;
pub mod html_encode;
pub mod html_sanitize;
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
pub mod semver_compare;
pub mod semver_inc_major;
pub mod semver_inc_minor;
pub mod semver_inc_patch;
pub mod semver_major;
pub mod semver_minor;
pub mod semver_patch;
pub mod semver_set_major;
pub mod semver_set_minor;
pub mod semver_set_patch;
pub mod similarity_fuzzy;
pub mod similarity_jaro;
pub mod similarity_jaro_winkler;
pub mod similarity_smithwaterman;
pub mod similarity_sorensen_dice;
pub mod slice;
pub mod slug;
pub mod split;
pub mod starts_with;
pub mod trim;
pub mod uppercase;
pub mod words;

pub(crate) fn analyze_string_function(
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
        "string::distance::damerau_levenshtein" => distance_damerau_levenshtein::analyze_string_distance_damerau_levenshtein(ctx, call, args),
        "string::distance::hamming" => distance_hamming::analyze_string_distance_hamming(ctx, call, args),
        "string::distance::levenshtein" => distance_levenshtein::analyze_string_distance_levenshtein(ctx, call, args),
        "string::distance::normalized_damerau_levenshtein" => distance_normalized_damerau_levenshtein::analyze_string_distance_normalized_damerau_levenshtein(ctx, call, args),
        "string::distance::normalized_levenshtein" => distance_normalized_levenshtein::analyze_string_distance_normalized_levenshtein(ctx, call, args),
        "string::distance::osa" => distance_osa::analyze_string_distance_osa(ctx, call, args),
        "string::html::encode" => html_encode::analyze_string_html_encode(ctx, call, args),
        "string::html::sanitize" => html_sanitize::analyze_string_html_sanitize(ctx, call, args),
        "string::semver::compare" => semver_compare::analyze_string_semver_compare(ctx, call, args),
        "string::semver::inc::major" => semver_inc_major::analyze_string_semver_inc_major(ctx, call, args),
        "string::semver::inc::minor" => semver_inc_minor::analyze_string_semver_inc_minor(ctx, call, args),
        "string::semver::inc::patch" => semver_inc_patch::analyze_string_semver_inc_patch(ctx, call, args),
        "string::semver::major" => semver_major::analyze_string_semver_major(ctx, call, args),
        "string::semver::minor" => semver_minor::analyze_string_semver_minor(ctx, call, args),
        "string::semver::patch" => semver_patch::analyze_string_semver_patch(ctx, call, args),
        "string::semver::set::major" => semver_set_major::analyze_string_semver_set_major(ctx, call, args),
        "string::semver::set::minor" => semver_set_minor::analyze_string_semver_set_minor(ctx, call, args),
        "string::semver::set::patch" => semver_set_patch::analyze_string_semver_set_patch(ctx, call, args),
        "string::similarity::fuzzy" => similarity_fuzzy::analyze_string_similarity_fuzzy(ctx, call, args),
        "string::similarity::jaro" => similarity_jaro::analyze_string_similarity_jaro(ctx, call, args),
        "string::similarity::jaro_winkler" => similarity_jaro_winkler::analyze_string_similarity_jaro_winkler(ctx, call, args),
        "string::similarity::smithwaterman" => similarity_smithwaterman::analyze_string_similarity_smithwaterman(ctx, call, args),
        "string::similarity::sorensen_dice" => similarity_sorensen_dice::analyze_string_similarity_sorensen_dice(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
