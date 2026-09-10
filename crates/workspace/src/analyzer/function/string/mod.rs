//! `string` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

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

/// Every `string::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "string::capitalize",
        "The string with its first letter uppercased.",
        capitalize::signature,
        capitalize::analyze_string_capitalize,
    ),
    BuiltinEntry::new(
        "string::concat",
        "Concatenates the values into one string.",
        concat::signature,
        concat::analyze_string_concat,
    ),
    BuiltinEntry::new(
        "string::contains",
        "Whether the string contains a substring.",
        contains::signature,
        contains::analyze_string_contains,
    ),
    BuiltinEntry::new(
        "string::ends_with",
        "Whether the string ends with a suffix.",
        ends_with::signature,
        ends_with::analyze_string_ends_with,
    ),
    BuiltinEntry::new(
        "string::is_alpha",
        "Whether the string is entirely alphabetic.",
        is_alpha::signature,
        is_alpha::analyze_string_is_alpha,
    ),
    BuiltinEntry::new(
        "string::is_alphanum",
        "Whether the string is entirely alphanumeric.",
        is_alphanum::signature,
        is_alphanum::analyze_string_is_alphanum,
    ),
    BuiltinEntry::new(
        "string::is_ascii",
        "Whether the string is entirely ASCII.",
        is_ascii::signature,
        is_ascii::analyze_string_is_ascii,
    ),
    BuiltinEntry::new(
        "string::is_datetime",
        "Whether the string parses as a datetime in the given format.",
        is_datetime::signature,
        is_datetime::analyze_string_is_datetime,
    ),
    BuiltinEntry::new(
        "string::is_domain",
        "Whether the string is a domain name.",
        is_domain::signature,
        is_domain::analyze_string_is_domain,
    ),
    BuiltinEntry::new(
        "string::is_email",
        "Whether the string is an email address.",
        is_email::signature,
        is_email::analyze_string_is_email,
    ),
    BuiltinEntry::new(
        "string::is_hexadecimal",
        "Whether the string is hexadecimal.",
        is_hexadecimal::signature,
        is_hexadecimal::analyze_string_is_hexadecimal,
    ),
    BuiltinEntry::new(
        "string::is_ip",
        "Whether the string is an IP address.",
        is_ip::signature,
        is_ip::analyze_string_is_ip,
    ),
    BuiltinEntry::new(
        "string::is_ipv4",
        "Whether the string is an IPv4 address.",
        is_ipv4::signature,
        is_ipv4::analyze_string_is_ipv4,
    ),
    BuiltinEntry::new(
        "string::is_ipv6",
        "Whether the string is an IPv6 address.",
        is_ipv6::signature,
        is_ipv6::analyze_string_is_ipv6,
    ),
    BuiltinEntry::new(
        "string::is_latitude",
        "Whether the string is a latitude.",
        is_latitude::signature,
        is_latitude::analyze_string_is_latitude,
    ),
    BuiltinEntry::new(
        "string::is_longitude",
        "Whether the string is a longitude.",
        is_longitude::signature,
        is_longitude::analyze_string_is_longitude,
    ),
    BuiltinEntry::new(
        "string::is_numeric",
        "Whether the string is entirely numeric.",
        is_numeric::signature,
        is_numeric::analyze_string_is_numeric,
    ),
    BuiltinEntry::new(
        "string::is_record",
        "Whether the string is a record id, optionally of the given table.",
        is_record::signature,
        is_record::analyze_string_is_record,
    ),
    BuiltinEntry::new(
        "string::is_semver",
        "Whether the string is a semantic version.",
        is_semver::signature,
        is_semver::analyze_string_is_semver,
    ),
    BuiltinEntry::new(
        "string::is_ulid",
        "Whether the string is a ULID.",
        is_ulid::signature,
        is_ulid::analyze_string_is_ulid,
    ),
    BuiltinEntry::new(
        "string::is_url",
        "Whether the string is a URL.",
        is_url::signature,
        is_url::analyze_string_is_url,
    ),
    BuiltinEntry::new(
        "string::is_uuid",
        "Whether the string is a UUID.",
        is_uuid::signature,
        is_uuid::analyze_string_is_uuid,
    ),
    BuiltinEntry::new(
        "string::join",
        "Joins the values into a string with the given separator.",
        join::signature,
        join::analyze_string_join,
    ),
    BuiltinEntry::new(
        "string::len",
        "The number of characters in the string.",
        len::signature,
        len::analyze_string_len,
    ),
    BuiltinEntry::new(
        "string::lowercase",
        "The string in lowercase.",
        lowercase::signature,
        lowercase::analyze_string_lowercase,
    ),
    BuiltinEntry::new(
        "string::matches",
        "Whether the string matches a regex.",
        matches::signature,
        matches::analyze_string_matches,
    ),
    BuiltinEntry::new(
        "string::repeat",
        "The string repeated the given number of times.",
        repeat::signature,
        repeat::analyze_string_repeat,
    ),
    BuiltinEntry::new(
        "string::replace",
        "The string with every occurrence of a substring or regex replaced.",
        replace::signature,
        replace::analyze_string_replace,
    ),
    BuiltinEntry::new(
        "string::reverse",
        "The string reversed.",
        reverse::signature,
        reverse::analyze_string_reverse,
    ),
    BuiltinEntry::new(
        "string::slice",
        "A substring from a start index, of an optional length.",
        slice::signature,
        slice::analyze_string_slice,
    ),
    BuiltinEntry::new(
        "string::slug",
        "The string as a URL slug.",
        slug::signature,
        slug::analyze_string_slug,
    ),
    BuiltinEntry::new(
        "string::split",
        "Splits the string on a delimiter.",
        split::signature,
        split::analyze_string_split,
    ),
    BuiltinEntry::new(
        "string::starts_with",
        "Whether the string starts with a prefix.",
        starts_with::signature,
        starts_with::analyze_string_starts_with,
    ),
    BuiltinEntry::new(
        "string::trim",
        "The string with surrounding whitespace removed.",
        trim::signature,
        trim::analyze_string_trim,
    ),
    BuiltinEntry::new(
        "string::uppercase",
        "The string in uppercase.",
        uppercase::signature,
        uppercase::analyze_string_uppercase,
    ),
    BuiltinEntry::new(
        "string::words",
        "Splits the string into words.",
        words::signature,
        words::analyze_string_words,
    ),
    BuiltinEntry::new(
        "string::distance::damerau_levenshtein",
        "The Damerau-Levenshtein edit distance between two strings.",
        distance_damerau_levenshtein::signature,
        distance_damerau_levenshtein::analyze_string_distance_damerau_levenshtein,
    ),
    BuiltinEntry::new(
        "string::distance::hamming",
        "The Hamming distance between two strings of equal length.",
        distance_hamming::signature,
        distance_hamming::analyze_string_distance_hamming,
    ),
    BuiltinEntry::new(
        "string::distance::levenshtein",
        "The Levenshtein edit distance between two strings.",
        distance_levenshtein::signature,
        distance_levenshtein::analyze_string_distance_levenshtein,
    ),
    BuiltinEntry::new(
        "string::distance::normalized_damerau_levenshtein",
        "The Damerau-Levenshtein distance normalized to 0..1.",
        distance_normalized_damerau_levenshtein::signature,
        distance_normalized_damerau_levenshtein::analyze_string_distance_normalized_damerau_levenshtein,
    ),
    BuiltinEntry::new(
        "string::distance::normalized_levenshtein",
        "The Levenshtein distance normalized to 0..1.",
        distance_normalized_levenshtein::signature,
        distance_normalized_levenshtein::analyze_string_distance_normalized_levenshtein,
    ),
    BuiltinEntry::new(
        "string::distance::osa",
        "The optimal string alignment distance between two strings.",
        distance_osa::signature,
        distance_osa::analyze_string_distance_osa,
    ),
    BuiltinEntry::new(
        "string::html::encode",
        "Escapes HTML special characters in a string.",
        html_encode::signature,
        html_encode::analyze_string_html_encode,
    ),
    BuiltinEntry::new(
        "string::html::sanitize",
        "Strips unsafe HTML from a string.",
        html_sanitize::signature,
        html_sanitize::analyze_string_html_sanitize,
    ),
    BuiltinEntry::new(
        "string::semver::compare",
        "Compares two semantic versions: -1, 0, or 1.",
        semver_compare::signature,
        semver_compare::analyze_string_semver_compare,
    ),
    BuiltinEntry::new(
        "string::semver::inc::major",
        "The version with its major component incremented.",
        semver_inc_major::signature,
        semver_inc_major::analyze_string_semver_inc_major,
    ),
    BuiltinEntry::new(
        "string::semver::inc::minor",
        "The version with its minor component incremented.",
        semver_inc_minor::signature,
        semver_inc_minor::analyze_string_semver_inc_minor,
    ),
    BuiltinEntry::new(
        "string::semver::inc::patch",
        "The version with its patch component incremented.",
        semver_inc_patch::signature,
        semver_inc_patch::analyze_string_semver_inc_patch,
    ),
    BuiltinEntry::new(
        "string::semver::major",
        "The major component of a semantic version.",
        semver_major::signature,
        semver_major::analyze_string_semver_major,
    ),
    BuiltinEntry::new(
        "string::semver::minor",
        "The minor component of a semantic version.",
        semver_minor::signature,
        semver_minor::analyze_string_semver_minor,
    ),
    BuiltinEntry::new(
        "string::semver::patch",
        "The patch component of a semantic version.",
        semver_patch::signature,
        semver_patch::analyze_string_semver_patch,
    ),
    BuiltinEntry::new(
        "string::semver::set::major",
        "The version with its major component replaced.",
        semver_set_major::signature,
        semver_set_major::analyze_string_semver_set_major,
    ),
    BuiltinEntry::new(
        "string::semver::set::minor",
        "The version with its minor component replaced.",
        semver_set_minor::signature,
        semver_set_minor::analyze_string_semver_set_minor,
    ),
    BuiltinEntry::new(
        "string::semver::set::patch",
        "The version with its patch component replaced.",
        semver_set_patch::signature,
        semver_set_patch::analyze_string_semver_set_patch,
    ),
    BuiltinEntry::new(
        "string::similarity::fuzzy",
        "A fuzzy-match similarity score between two strings.",
        similarity_fuzzy::signature,
        similarity_fuzzy::analyze_string_similarity_fuzzy,
    ),
    BuiltinEntry::new(
        "string::similarity::jaro",
        "The Jaro similarity between two strings, 0..1.",
        similarity_jaro::signature,
        similarity_jaro::analyze_string_similarity_jaro,
    ),
    BuiltinEntry::new(
        "string::similarity::jaro_winkler",
        "The Jaro-Winkler similarity between two strings, 0..1.",
        similarity_jaro_winkler::signature,
        similarity_jaro_winkler::analyze_string_similarity_jaro_winkler,
    ),
    BuiltinEntry::new(
        "string::similarity::smithwaterman",
        "The Smith-Waterman alignment score between two strings.",
        similarity_smithwaterman::signature,
        similarity_smithwaterman::analyze_string_similarity_smithwaterman,
    ),
    BuiltinEntry::new(
        "string::similarity::sorensen_dice",
        "The Sorensen-Dice coefficient of two strings, 0..1.",
        similarity_sorensen_dice::signature,
        similarity_sorensen_dice::analyze_string_similarity_sorensen_dice,
    ),
];
