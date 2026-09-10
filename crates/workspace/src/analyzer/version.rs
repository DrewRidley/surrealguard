//! Version compatibility: what the configured target SurrealDB release does
//! and does not have (8001 functions, 8002 removed syntax, 8003 newer
//! syntax).
//!
//! Every check gates on [`AnalysisContext::target_version`]; an unset
//! `analysis.surrealdb_version` is "the latest" and nothing here fires. The
//! function table is also what 5001 reads when there is no target: a spelling
//! the latest release removed is an unknown name there, carrying the rename
//! ([`retired`]) — one table for every removed or renamed name.
//!
//! # Sources
//!
//! Every fact in this module is traceable to SurrealDB's own repository or
//! documentation; nothing is remembered or inferred from naming patterns.
//!
//! **Functions** ([`FUNCTIONS`]) are the diff of the built-in dispatch table
//! `fnc/mod.rs` between consecutive release tags of
//! <https://github.com/surrealdb/surrealdb> — a name present at a tag and
//! absent at the previous minor's last patch was added in that minor; the
//! reverse was removed in it:
//!
//! - `v1.5.6` `core/src/fnc/mod.rs` → `v2.0.5` `core/src/fnc/mod.rs`
//!   (added in 2.0: `array::filter`/`map`/…, `math::acos`/…, `record::*`,
//!   `string::starts_with`/`ends_with`, `type::record`, …; removed in 2.0:
//!   `string::startsWith`/`endsWith`, `session::sc`/`sd`, `meta::table`)
//! - `v2.0.5` → `v2.1.9` `crates/core/src/fnc/mod.rs` (2.1: `array::fold`,
//!   `array::reduce`, `string::distance::*`, `string::similarity::*`)
//! - `v2.1.9` → `v2.2.8` (2.2: `api::invoke`, `object::is_empty`,
//!   `record::refs`, `type::is::range`)
//! - `v2.2.8` → `v2.3.10` (2.3: `array::sort_lexical`/`sort_natural`/
//!   `sort_natural_lexical`, `rand::duration`)
//! - `v2.3.10` → `v3.0.5` `surrealdb/core/src/fnc/mod.rs` (3.0: the
//!   `file::`, `set::`, `api::req::`/`api::res::`, `schema::`, `sequence::`
//!   families, `array::sequence`, `crypto::joaat`, `encoding::cbor::*`,
//!   `object::extend`/`remove`, `record::is_edge`, `search::linear`/`rrf`,
//!   `string::capitalize`, `type::file`/`of`/`set`/`string_lossy`, and the
//!   underscore spellings `type::is_*`, `string::is_*`, `time::from_*`,
//!   `duration::from_*`, `time::is_leap_year`, `geo::is_valid`,
//!   `string::distance::osa`; removed in 3.0: every `::is::`/`::from::`
//!   spelling, `string::distance::osa_distance`, `type::thing`, `rand::guid`,
//!   `record::refs`)
//! - `v3.0.5` → `v3.1.0` (3.1: `encoding::json::encode`/`decode`,
//!   `value::expect`)
//! - `v3.1.0` → `v3.2.0` (3.2: `eval::gql`, `eval::surql`)
//!
//! The 3.0 renames are confirmed by the 3.0.5 parser's own rename table,
//! `surrealdb/core/src/syn/parser/builtin.rs` (`PATHS`: "the final item is
//! `Some` when a path has been renamed to the path on the left"), which
//! rejects the old spelling with the new one as the hint — so a 3.x target
//! genuinely does not accept `type::is::record`.
//!
//! Three facts come from the official docs rather than a tag diff, because
//! the tag diffs cannot pin them finer or the release is newer than the
//! newest tag the diff covered:
//! - `time::set_year`/`set_month`/`set_day`/`set_hour`/`set_minute`/
//!   `set_second`/`set_nanosecond`: "since v3.0.2" —
//!   <https://surrealdb.com/docs/surrealql/functions/database/time>
//! - `api::req::max_body`: "since v3.3.0" —
//!   <https://surrealdb.com/docs/surrealql/functions/database/api>
//! - `vector::sum`: "since v3.3.0" —
//!   <https://surrealdb.com/docs/surrealql/functions/database/vector>
//!
//! **Syntax** ([`check_statement`]) is sourced per construct at its check,
//! from the same repository's `sql/` tree at those tags (a construct whose
//! AST file exists at a tag and not at the previous line's last patch was
//! added in between) and from the docs' "since"/"removed" notes.
//!
//! Anything not listed here is treated as available in every version. That
//! is deliberate: an unsourced annotation would be a guess, and a guess in
//! an error-severity check aborts codegen for a whole workspace.

use surrealguard_syntax::ast;
use surrealguard_syntax::ast::visit::{self, Visitor};
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;
use crate::config::{TargetVersion, Version};

/// One built-in function's version facts, keyed by the name *as written in
/// source* (before lowering canonicalizes `type::is::record` to
/// `type::is_record`): a rename is two rows, the old spelling with
/// `removed`/`replacement` and the new one with `since`/`renamed_from`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionVersion {
    /// The function path as written in SurrealQL.
    pub name: &'static str,
    /// The release that added this name; `None` when it predates every
    /// release the table covers.
    pub since: Option<Version>,
    /// The release that dropped this name; `None` when it is current.
    pub removed: Option<Version>,
    /// For a removed name: the name that took over, when the removal was a
    /// rename.
    pub replacement: Option<&'static str>,
    /// For an added name that is the new spelling of an older function: the
    /// spelling targets before `since` use.
    pub renamed_from: Option<&'static str>,
}

const fn fv(
    name: &'static str,
    since: Option<Version>,
    removed: Option<Version>,
    replacement: Option<&'static str>,
    renamed_from: Option<&'static str>,
) -> FunctionVersion {
    FunctionVersion {
        name,
        since,
        removed,
        replacement,
        renamed_from,
    }
}

/// Every built-in function with a known version boundary, sorted by name
/// (byte order) for binary search. Generated from the tag diffs listed in
/// the module docs; see there for the sources.
#[rustfmt::skip]
pub static FUNCTIONS: &[FunctionVersion] = &[
    fv("api::invoke", Some(Version::new(2, 2, 0)), None, None, None),
    fv("api::req::body", Some(Version::new(3, 0, 0)), None, None, None),
    fv("api::req::max_body", Some(Version::new(3, 3, 0)), None, None, None),
    fv("api::res::body", Some(Version::new(3, 0, 0)), None, None, None),
    fv("api::res::header", Some(Version::new(3, 0, 0)), None, None, None),
    fv("api::res::headers", Some(Version::new(3, 0, 0)), None, None, None),
    fv("api::res::status", Some(Version::new(3, 0, 0)), None, None, None),
    fv("api::timeout", Some(Version::new(3, 0, 0)), None, None, None),
    fv("array::every", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::fill", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::filter", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::find", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::fold", Some(Version::new(2, 1, 0)), None, None, None),
    fv("array::includes", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::index_of", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::is_empty", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::map", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::range", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::reduce", Some(Version::new(2, 1, 0)), None, None, None),
    fv("array::repeat", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::sequence", Some(Version::new(3, 0, 0)), None, None, None),
    fv("array::shuffle", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::some", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::sort_lexical", Some(Version::new(2, 3, 0)), None, None, None),
    fv("array::sort_natural", Some(Version::new(2, 3, 0)), None, None, None),
    fv("array::sort_natural_lexical", Some(Version::new(2, 3, 0)), None, None, None),
    fv("array::swap", Some(Version::new(2, 0, 0)), None, None, None),
    fv("array::windows", Some(Version::new(2, 0, 0)), None, None, None),
    fv("crypto::blake3", Some(Version::new(2, 0, 0)), None, None, None),
    fv("crypto::joaat", Some(Version::new(3, 0, 0)), None, None, None),
    fv("duration::from::days", None, Some(Version::new(3, 0, 0)), Some("duration::from_days"), None),
    fv("duration::from::hours", None, Some(Version::new(3, 0, 0)), Some("duration::from_hours"), None),
    fv("duration::from::micros", None, Some(Version::new(3, 0, 0)), Some("duration::from_micros"), None),
    fv("duration::from::millis", None, Some(Version::new(3, 0, 0)), Some("duration::from_millis"), None),
    fv("duration::from::mins", None, Some(Version::new(3, 0, 0)), Some("duration::from_mins"), None),
    fv("duration::from::nanos", None, Some(Version::new(3, 0, 0)), Some("duration::from_nanos"), None),
    fv("duration::from::secs", None, Some(Version::new(3, 0, 0)), Some("duration::from_secs"), None),
    fv("duration::from::weeks", None, Some(Version::new(3, 0, 0)), Some("duration::from_weeks"), None),
    fv("duration::from_days", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::days")),
    fv("duration::from_hours", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::hours")),
    fv("duration::from_micros", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::micros")),
    fv("duration::from_millis", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::millis")),
    fv("duration::from_mins", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::mins")),
    fv("duration::from_nanos", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::nanos")),
    fv("duration::from_secs", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::secs")),
    fv("duration::from_weeks", Some(Version::new(3, 0, 0)), None, None, Some("duration::from::weeks")),
    fv("encoding::cbor::decode", Some(Version::new(3, 0, 0)), None, None, None),
    fv("encoding::cbor::encode", Some(Version::new(3, 0, 0)), None, None, None),
    fv("encoding::json::decode", Some(Version::new(3, 1, 0)), None, None, None),
    fv("encoding::json::encode", Some(Version::new(3, 1, 0)), None, None, None),
    fv("eval::gql", Some(Version::new(3, 2, 0)), None, None, None),
    fv("eval::surql", Some(Version::new(3, 2, 0)), None, None, None),
    fv("file::bucket", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::copy", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::copy_if_not_exists", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::delete", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::exists", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::get", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::head", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::key", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::list", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::put", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::put_if_not_exists", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::rename", Some(Version::new(3, 0, 0)), None, None, None),
    fv("file::rename_if_not_exists", Some(Version::new(3, 0, 0)), None, None, None),
    fv("geo::is::valid", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("geo::is_valid"), None),
    fv("geo::is_valid", Some(Version::new(3, 0, 0)), None, None, Some("geo::is::valid")),
    fv("math::acos", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::acot", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::asin", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::atan", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::clamp", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::cos", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::cot", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::deg2rad", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::lerp", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::lerpangle", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::ln", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::log", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::log10", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::log2", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::rad2deg", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::sign", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::sin", Some(Version::new(2, 0, 0)), None, None, None),
    fv("math::tan", Some(Version::new(2, 0, 0)), None, None, None),
    fv("meta::table", None, Some(Version::new(2, 0, 0)), Some("record::table"), None),
    fv("object::extend", Some(Version::new(3, 0, 0)), None, None, None),
    fv("object::is_empty", Some(Version::new(2, 2, 0)), None, None, None),
    fv("object::remove", Some(Version::new(3, 0, 0)), None, None, None),
    fv("rand::duration", Some(Version::new(2, 3, 0)), None, None, None),
    fv("rand::guid", None, Some(Version::new(3, 0, 0)), None, None),
    fv("rand::id", Some(Version::new(3, 0, 0)), None, None, None),
    fv("record::exists", Some(Version::new(2, 0, 0)), None, None, None),
    fv("record::id", Some(Version::new(2, 0, 0)), None, None, None),
    fv("record::is_edge", Some(Version::new(3, 0, 0)), None, None, None),
    fv("record::refs", Some(Version::new(2, 2, 0)), Some(Version::new(3, 0, 0)), None, None),
    fv("record::table", Some(Version::new(2, 0, 0)), None, None, Some("meta::table")),
    fv("record::tb", Some(Version::new(2, 0, 0)), None, None, None),
    fv("schema::table::exists", Some(Version::new(3, 0, 0)), None, None, None),
    fv("search::linear", Some(Version::new(3, 0, 0)), None, None, None),
    fv("search::rrf", Some(Version::new(3, 0, 0)), None, None, None),
    fv("sequence::nextval", Some(Version::new(3, 0, 0)), None, None, None),
    fv("session::ac", Some(Version::new(2, 0, 0)), None, None, Some("session::sc")),
    fv("session::rd", Some(Version::new(2, 0, 0)), None, None, Some("session::sd")),
    fv("session::sc", None, Some(Version::new(2, 0, 0)), Some("session::ac"), None),
    fv("session::sd", None, Some(Version::new(2, 0, 0)), Some("session::rd"), None),
    fv("set::add", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::all", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::any", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::at", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::complement", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::contains", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::difference", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::filter", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::find", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::first", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::flatten", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::fold", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::intersect", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::is_empty", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::join", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::last", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::len", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::map", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::max", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::min", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::reduce", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::remove", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::slice", Some(Version::new(3, 0, 0)), None, None, None),
    fv("set::union", Some(Version::new(3, 0, 0)), None, None, None),
    fv("string::capitalize", Some(Version::new(3, 0, 0)), None, None, None),
    fv("string::distance::damerau_levenshtein", Some(Version::new(2, 1, 0)), None, None, None),
    fv("string::distance::normalized_damerau_levenshtein", Some(Version::new(2, 1, 0)), None, None, None),
    fv("string::distance::normalized_levenshtein", Some(Version::new(2, 1, 0)), None, None, None),
    fv("string::distance::osa", Some(Version::new(3, 0, 0)), None, None, Some("string::distance::osa_distance")),
    fv("string::distance::osa_distance", Some(Version::new(2, 1, 0)), Some(Version::new(3, 0, 0)), Some("string::distance::osa"), None),
    fv("string::endsWith", None, Some(Version::new(2, 0, 0)), Some("string::ends_with"), None),
    fv("string::ends_with", Some(Version::new(2, 0, 0)), None, None, Some("string::endsWith")),
    fv("string::html::encode", Some(Version::new(2, 0, 0)), None, None, None),
    fv("string::html::sanitize", Some(Version::new(2, 0, 0)), None, None, None),
    fv("string::is::alpha", None, Some(Version::new(3, 0, 0)), Some("string::is_alpha"), None),
    fv("string::is::alphanum", None, Some(Version::new(3, 0, 0)), Some("string::is_alphanum"), None),
    fv("string::is::ascii", None, Some(Version::new(3, 0, 0)), Some("string::is_ascii"), None),
    fv("string::is::datetime", None, Some(Version::new(3, 0, 0)), Some("string::is_datetime"), None),
    fv("string::is::domain", None, Some(Version::new(3, 0, 0)), Some("string::is_domain"), None),
    fv("string::is::email", None, Some(Version::new(3, 0, 0)), Some("string::is_email"), None),
    fv("string::is::hexadecimal", None, Some(Version::new(3, 0, 0)), Some("string::is_hexadecimal"), None),
    fv("string::is::ip", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("string::is_ip"), None),
    fv("string::is::ipv4", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("string::is_ipv4"), None),
    fv("string::is::ipv6", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("string::is_ipv6"), None),
    fv("string::is::latitude", None, Some(Version::new(3, 0, 0)), Some("string::is_latitude"), None),
    fv("string::is::longitude", None, Some(Version::new(3, 0, 0)), Some("string::is_longitude"), None),
    fv("string::is::numeric", None, Some(Version::new(3, 0, 0)), Some("string::is_numeric"), None),
    fv("string::is::record", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("string::is_record"), None),
    fv("string::is::semver", None, Some(Version::new(3, 0, 0)), Some("string::is_semver"), None),
    fv("string::is::ulid", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("string::is_ulid"), None),
    fv("string::is::url", None, Some(Version::new(3, 0, 0)), Some("string::is_url"), None),
    fv("string::is::uuid", None, Some(Version::new(3, 0, 0)), Some("string::is_uuid"), None),
    fv("string::is_alpha", Some(Version::new(3, 0, 0)), None, None, Some("string::is::alpha")),
    fv("string::is_alphanum", Some(Version::new(3, 0, 0)), None, None, Some("string::is::alphanum")),
    fv("string::is_ascii", Some(Version::new(3, 0, 0)), None, None, Some("string::is::ascii")),
    fv("string::is_datetime", Some(Version::new(3, 0, 0)), None, None, Some("string::is::datetime")),
    fv("string::is_domain", Some(Version::new(3, 0, 0)), None, None, Some("string::is::domain")),
    fv("string::is_email", Some(Version::new(3, 0, 0)), None, None, Some("string::is::email")),
    fv("string::is_hexadecimal", Some(Version::new(3, 0, 0)), None, None, Some("string::is::hexadecimal")),
    fv("string::is_ip", Some(Version::new(3, 0, 0)), None, None, Some("string::is::ip")),
    fv("string::is_ipv4", Some(Version::new(3, 0, 0)), None, None, Some("string::is::ipv4")),
    fv("string::is_ipv6", Some(Version::new(3, 0, 0)), None, None, Some("string::is::ipv6")),
    fv("string::is_latitude", Some(Version::new(3, 0, 0)), None, None, Some("string::is::latitude")),
    fv("string::is_longitude", Some(Version::new(3, 0, 0)), None, None, Some("string::is::longitude")),
    fv("string::is_numeric", Some(Version::new(3, 0, 0)), None, None, Some("string::is::numeric")),
    fv("string::is_record", Some(Version::new(3, 0, 0)), None, None, Some("string::is::record")),
    fv("string::is_semver", Some(Version::new(3, 0, 0)), None, None, Some("string::is::semver")),
    fv("string::is_ulid", Some(Version::new(3, 0, 0)), None, None, Some("string::is::ulid")),
    fv("string::is_url", Some(Version::new(3, 0, 0)), None, None, Some("string::is::url")),
    fv("string::is_uuid", Some(Version::new(3, 0, 0)), None, None, Some("string::is::uuid")),
    fv("string::similarity::jaro_winkler", Some(Version::new(2, 1, 0)), None, None, None),
    fv("string::similarity::sorensen_dice", Some(Version::new(2, 1, 0)), None, None, None),
    fv("string::startsWith", None, Some(Version::new(2, 0, 0)), Some("string::starts_with"), None),
    fv("string::starts_with", Some(Version::new(2, 0, 0)), None, None, Some("string::startsWith")),
    fv("time::from::micros", None, Some(Version::new(3, 0, 0)), Some("time::from_micros"), None),
    fv("time::from::millis", None, Some(Version::new(3, 0, 0)), Some("time::from_millis"), None),
    fv("time::from::nanos", None, Some(Version::new(3, 0, 0)), Some("time::from_nanos"), None),
    fv("time::from::secs", None, Some(Version::new(3, 0, 0)), Some("time::from_secs"), None),
    fv("time::from::ulid", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("time::from_ulid"), None),
    fv("time::from::unix", None, Some(Version::new(3, 0, 0)), Some("time::from_unix"), None),
    fv("time::from::uuid", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("time::from_uuid"), None),
    fv("time::from_micros", Some(Version::new(3, 0, 0)), None, None, Some("time::from::micros")),
    fv("time::from_millis", Some(Version::new(3, 0, 0)), None, None, Some("time::from::millis")),
    fv("time::from_nanos", Some(Version::new(3, 0, 0)), None, None, Some("time::from::nanos")),
    fv("time::from_secs", Some(Version::new(3, 0, 0)), None, None, Some("time::from::secs")),
    fv("time::from_ulid", Some(Version::new(3, 0, 0)), None, None, Some("time::from::ulid")),
    fv("time::from_unix", Some(Version::new(3, 0, 0)), None, None, Some("time::from::unix")),
    fv("time::from_uuid", Some(Version::new(3, 0, 0)), None, None, Some("time::from::uuid")),
    fv("time::is::leap_year", Some(Version::new(2, 0, 0)), Some(Version::new(3, 0, 0)), Some("time::is_leap_year"), None),
    fv("time::is_leap_year", Some(Version::new(3, 0, 0)), None, None, Some("time::is::leap_year")),
    fv("time::set_day", Some(Version::new(3, 0, 2)), None, None, None),
    fv("time::set_hour", Some(Version::new(3, 0, 2)), None, None, None),
    fv("time::set_minute", Some(Version::new(3, 0, 2)), None, None, None),
    fv("time::set_month", Some(Version::new(3, 0, 2)), None, None, None),
    fv("time::set_nanosecond", Some(Version::new(3, 0, 2)), None, None, None),
    fv("time::set_second", Some(Version::new(3, 0, 2)), None, None, None),
    fv("time::set_year", Some(Version::new(3, 0, 2)), None, None, None),
    fv("type::array", Some(Version::new(2, 0, 0)), None, None, None),
    fv("type::bytes", Some(Version::new(2, 0, 0)), None, None, None),
    fv("type::file", Some(Version::new(3, 0, 0)), None, None, None),
    fv("type::geometry", Some(Version::new(2, 0, 0)), None, None, None),
    fv("type::is::array", None, Some(Version::new(3, 0, 0)), Some("type::is_array"), None),
    fv("type::is::bool", None, Some(Version::new(3, 0, 0)), Some("type::is_bool"), None),
    fv("type::is::bytes", None, Some(Version::new(3, 0, 0)), Some("type::is_bytes"), None),
    fv("type::is::collection", None, Some(Version::new(3, 0, 0)), Some("type::is_collection"), None),
    fv("type::is::datetime", None, Some(Version::new(3, 0, 0)), Some("type::is_datetime"), None),
    fv("type::is::decimal", None, Some(Version::new(3, 0, 0)), Some("type::is_decimal"), None),
    fv("type::is::duration", None, Some(Version::new(3, 0, 0)), Some("type::is_duration"), None),
    fv("type::is::float", None, Some(Version::new(3, 0, 0)), Some("type::is_float"), None),
    fv("type::is::geometry", None, Some(Version::new(3, 0, 0)), Some("type::is_geometry"), None),
    fv("type::is::int", None, Some(Version::new(3, 0, 0)), Some("type::is_int"), None),
    fv("type::is::line", None, Some(Version::new(3, 0, 0)), Some("type::is_line"), None),
    fv("type::is::multiline", None, Some(Version::new(3, 0, 0)), Some("type::is_multiline"), None),
    fv("type::is::multipoint", None, Some(Version::new(3, 0, 0)), Some("type::is_multipoint"), None),
    fv("type::is::multipolygon", None, Some(Version::new(3, 0, 0)), Some("type::is_multipolygon"), None),
    fv("type::is::none", None, Some(Version::new(3, 0, 0)), Some("type::is_none"), None),
    fv("type::is::null", None, Some(Version::new(3, 0, 0)), Some("type::is_null"), None),
    fv("type::is::number", None, Some(Version::new(3, 0, 0)), Some("type::is_number"), None),
    fv("type::is::object", None, Some(Version::new(3, 0, 0)), Some("type::is_object"), None),
    fv("type::is::point", None, Some(Version::new(3, 0, 0)), Some("type::is_point"), None),
    fv("type::is::polygon", None, Some(Version::new(3, 0, 0)), Some("type::is_polygon"), None),
    fv("type::is::range", Some(Version::new(2, 2, 0)), Some(Version::new(3, 0, 0)), Some("type::is_range"), None),
    fv("type::is::record", None, Some(Version::new(3, 0, 0)), Some("type::is_record"), None),
    fv("type::is::string", None, Some(Version::new(3, 0, 0)), Some("type::is_string"), None),
    fv("type::is::uuid", None, Some(Version::new(3, 0, 0)), Some("type::is_uuid"), None),
    fv("type::is_array", Some(Version::new(3, 0, 0)), None, None, Some("type::is::array")),
    fv("type::is_bool", Some(Version::new(3, 0, 0)), None, None, Some("type::is::bool")),
    fv("type::is_bytes", Some(Version::new(3, 0, 0)), None, None, Some("type::is::bytes")),
    fv("type::is_collection", Some(Version::new(3, 0, 0)), None, None, Some("type::is::collection")),
    fv("type::is_datetime", Some(Version::new(3, 0, 0)), None, None, Some("type::is::datetime")),
    fv("type::is_decimal", Some(Version::new(3, 0, 0)), None, None, Some("type::is::decimal")),
    fv("type::is_duration", Some(Version::new(3, 0, 0)), None, None, Some("type::is::duration")),
    fv("type::is_float", Some(Version::new(3, 0, 0)), None, None, Some("type::is::float")),
    fv("type::is_geometry", Some(Version::new(3, 0, 0)), None, None, Some("type::is::geometry")),
    fv("type::is_int", Some(Version::new(3, 0, 0)), None, None, Some("type::is::int")),
    fv("type::is_line", Some(Version::new(3, 0, 0)), None, None, Some("type::is::line")),
    fv("type::is_multiline", Some(Version::new(3, 0, 0)), None, None, Some("type::is::multiline")),
    fv("type::is_multipoint", Some(Version::new(3, 0, 0)), None, None, Some("type::is::multipoint")),
    fv("type::is_multipolygon", Some(Version::new(3, 0, 0)), None, None, Some("type::is::multipolygon")),
    fv("type::is_none", Some(Version::new(3, 0, 0)), None, None, Some("type::is::none")),
    fv("type::is_null", Some(Version::new(3, 0, 0)), None, None, Some("type::is::null")),
    fv("type::is_number", Some(Version::new(3, 0, 0)), None, None, Some("type::is::number")),
    fv("type::is_object", Some(Version::new(3, 0, 0)), None, None, Some("type::is::object")),
    fv("type::is_point", Some(Version::new(3, 0, 0)), None, None, Some("type::is::point")),
    fv("type::is_polygon", Some(Version::new(3, 0, 0)), None, None, Some("type::is::polygon")),
    fv("type::is_range", Some(Version::new(3, 0, 0)), None, None, Some("type::is::range")),
    fv("type::is_record", Some(Version::new(3, 0, 0)), None, None, Some("type::is::record")),
    fv("type::is_set", Some(Version::new(3, 0, 0)), None, None, None),
    fv("type::is_string", Some(Version::new(3, 0, 0)), None, None, Some("type::is::string")),
    fv("type::is_uuid", Some(Version::new(3, 0, 0)), None, None, Some("type::is::uuid")),
    fv("type::of", Some(Version::new(3, 0, 0)), None, None, None),
    fv("type::range", Some(Version::new(2, 0, 0)), None, None, None),
    fv("type::record", Some(Version::new(2, 0, 0)), None, None, Some("type::thing")),
    fv("type::set", Some(Version::new(3, 0, 0)), None, None, None),
    fv("type::string_lossy", Some(Version::new(3, 0, 0)), None, None, None),
    fv("type::thing", None, Some(Version::new(3, 0, 0)), Some("type::record"), None),
    fv("type::uuid", Some(Version::new(2, 0, 0)), None, None, None),
    fv("value::diff", Some(Version::new(2, 0, 0)), None, None, None),
    fv("value::expect", Some(Version::new(3, 1, 0)), None, None, None),
    fv("value::patch", Some(Version::new(2, 0, 0)), None, None, None),
    fv("vector::distance::knn", Some(Version::new(2, 0, 0)), None, None, None),
    fv("vector::scale", Some(Version::new(2, 0, 0)), None, None, None),
    fv("vector::sum", Some(Version::new(3, 3, 0)), None, None, None),
];

/// The version facts for a function name as written in source, if any.
pub fn function_version(name: &str) -> Option<&'static FunctionVersion> {
    FUNCTIONS
        .binary_search_by(|entry| entry.name.cmp(name))
        .ok()
        .map(|index| &FUNCTIONS[index])
}

/// Why a call to `name` fails on a target: the message and help for 8001,
/// plus the name inference should dispatch under (a renamed function is still
/// that function; its return kind is unchanged).
pub(crate) struct FunctionMismatch {
    pub message: String,
    pub help: String,
    /// The current spelling to analyze the call as, when the written one is a
    /// removed spelling of a function that still exists.
    pub dispatch_as: Option<&'static str>,
}

/// Checks a function name *as written* against the configured target. `None`
/// when the name has no version facts, or the target has it.
pub(crate) fn check_function(target: TargetVersion, written: &str) -> Option<FunctionMismatch> {
    let entry = function_version(written)?;
    if let Some(removed) = entry.removed {
        if target.includes(removed) {
            return Some(match entry.replacement {
                Some(replacement) => FunctionMismatch {
                    message: format!(
                        "`{written}` does not exist in SurrealDB {target}: it was renamed to \
                         `{replacement}` in {removed}"
                    ),
                    help: format!("call `{replacement}` instead"),
                    dispatch_as: Some(replacement),
                },
                None => FunctionMismatch {
                    message: format!(
                        "`{written}` does not exist in SurrealDB {target}: it was removed in {removed}"
                    ),
                    help: format!(
                        "remove the call, or lower `analysis.surrealdb_version` below {removed}"
                    ),
                    dispatch_as: None,
                },
            });
        }
    }
    if let Some(since) = entry.since {
        if target.predates(since) {
            return Some(match entry.renamed_from {
                Some(older) => FunctionMismatch {
                    message: format!(
                        "`{written}` does not exist in SurrealDB {target}: before {since} it was \
                         spelled `{older}`"
                    ),
                    help: format!(
                        "call `{older}`, or raise `analysis.surrealdb_version` to {since} or newer"
                    ),
                    dispatch_as: None,
                },
                None => FunctionMismatch {
                    message: format!(
                        "`{written}` does not exist in SurrealDB {target}: it was added in {since}"
                    ),
                    help: format!(
                        "raise `analysis.surrealdb_version` to {since} or newer, or avoid the call"
                    ),
                    dispatch_as: None,
                },
            });
        }
    }
    None
}

/// The version facts for a spelling the latest release no longer has — the
/// one table 5001 reads for a name the engine refuses to parse (no target
/// configured) and 8001 reads for a target past the removal.
pub(crate) fn retired(written: &str) -> Option<&'static FunctionVersion> {
    function_version(written).filter(|entry| entry.removed.is_some())
}

/// The rename hint for a name that resolves to no function, whatever the
/// target: `string::startsWith` is unknown *because* it became
/// `string::starts_with`, and saying so beats "did you mean".
pub(crate) fn rename_hint(written: &str) -> Option<String> {
    let entry = function_version(written)?;
    let removed = entry.removed?;
    let replacement = entry.replacement?;
    Some(format!(
        "`{written}` was renamed to `{replacement}` in SurrealDB {removed}"
    ))
}

// ---------------------------------------------------------------------------
// Syntax (8002 / 8003)
// ---------------------------------------------------------------------------

/// Walks one top-level statement (and everything nested in it) and reports
/// every construct the configured target predates (8003) or has removed
/// (8002). No-op without a configured target.
pub(crate) fn check_statement(
    ctx: &mut AnalysisContext<'_>,
    statement: &ast::Spanned<ast::Statement>,
) {
    let Some(target) = ctx.target_version() else {
        return;
    };
    let mut walker = SyntaxVersions { ctx, target };
    walker.visit_statement(statement);
}

/// Each construct's version fact and where it comes from.
///
/// `sql/…` paths are files in <https://github.com/surrealdb/surrealdb>; "at
/// vX not vY" means the file (and so the construct's AST node) exists at tag
/// X and not at tag Y.
struct SyntaxVersions<'c, 'a> {
    ctx: &'c mut AnalysisContext<'a>,
    target: TargetVersion,
}

impl SyntaxVersions<'_, '_> {
    fn text(&self, span: ByteRange) -> &str {
        self.ctx
            .source_text()
            .get(span.start() as usize..span.end() as usize)
            .unwrap_or("")
    }

    /// The span of the first word at `span` — a statement's own keyword —
    /// so a finding about the statement kind does not underline its body.
    fn keyword_span(&self, span: ByteRange) -> ByteRange {
        let text = self.text(span);
        let len = text
            .find(|c: char| c.is_whitespace() || c == ';')
            .unwrap_or(text.len()) as u32;
        ByteRange::new(span.start(), span.start() + len).unwrap_or(span)
    }

    /// The span of the `index`-th whitespace-separated word at `span`.
    fn word_span(&self, span: ByteRange, index: usize) -> Option<ByteRange> {
        let text = self.text(span);
        let mut offset = 0usize;
        let mut seen = 0usize;
        for word in text.split(|c: char| c.is_whitespace() || c == ';') {
            if !word.is_empty() {
                if seen == index {
                    let start = span.start() + offset as u32;
                    return ByteRange::new(start, start + word.len() as u32).ok();
                }
                seen += 1;
            }
            offset += word.len() + 1;
        }
        None
    }

    fn requires(&mut self, span: ByteRange, construct: &str, since: Version) {
        if !self.target.predates(since) {
            return;
        }
        let target = self.target;
        let span = SourceSpan::new(self.ctx.source().clone(), span);
        self.ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                8003,
                format!(
                    "{construct} requires SurrealDB {since}; the configured target is {target}"
                ),
            )
            .with_help(format!(
                "raise `analysis.surrealdb_version` to {since} or newer, or rewrite without it"
            )),
        );
    }

    fn removed(&mut self, span: ByteRange, construct: &str, removed: Version, replacement: &str) {
        if !self.target.includes(removed) {
            return;
        }
        let target = self.target;
        let span = SourceSpan::new(self.ctx.source().clone(), span);
        self.ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                8002,
                format!(
                    "{construct} was removed in SurrealDB {removed}; the configured target is {target}"
                ),
            )
            .with_help(replacement.to_string()),
        );
    }

    /// An unmodeled `DEFINE <kind>`: the kind is the second word.
    fn define_other(&mut self, partial: &ast::PartialNode) {
        let Some(keyword_span) = self.word_span(partial.span, 1) else {
            return;
        };
        let keyword = self.text(keyword_span).to_ascii_uppercase();
        match keyword.as_str() {
            // `sql/statements/define/access.rs` at v2.0.5, not v1.5.6.
            "ACCESS" => self.requires(keyword_span, "`DEFINE ACCESS`", Version::new(2, 0, 0)),
            // `sql/statements/define/config/graphql.rs` at v2.0.5, not v1.5.6.
            "CONFIG" => self.requires(keyword_span, "`DEFINE CONFIG`", Version::new(2, 0, 0)),
            // `sql/statements/define/api.rs` at v2.2.8, not v2.1.9 (the docs
            // page says "since v3.0.0", when it left the experimental flag;
            // the parser accepted it from 2.2, so 2.2 is the no-false-positive
            // reading).
            "API" => self.requires(keyword_span, "`DEFINE API`", Version::new(2, 2, 0)),
            // `sql/statements/define/sequence.rs` at v3.0.5, not v2.3.10;
            // docs: "since v3.0.0".
            "SEQUENCE" => self.requires(keyword_span, "`DEFINE SEQUENCE`", Version::new(3, 0, 0)),
            // `sql/statements/define/bucket.rs` at v3.0.5, not v2.3.10;
            // docs: "since v3.0.0".
            "BUCKET" => self.requires(keyword_span, "`DEFINE BUCKET`", Version::new(3, 0, 0)),
            // Docs (statements/define/scope, statements/define/token): "deprecated
            // in favour of DEFINE ACCESS … in SurrealDB versions 2.x, and has
            // been removed as of SurrealDB 3.0". The 2.3.10 parser still has
            // `t!("SCOPE")`/`t!("TOKEN")` statement arms (converted to ACCESS);
            // the 3.0.5 parser (`syn/parser/stmt/define.rs`) has neither.
            "SCOPE" => self.removed(
                keyword_span,
                "`DEFINE SCOPE`",
                Version::new(3, 0, 0),
                "define the access method with `DEFINE ACCESS <name> ON DATABASE TYPE RECORD ...`",
            ),
            "TOKEN" => self.removed(
                keyword_span,
                "`DEFINE TOKEN`",
                Version::new(3, 0, 0),
                "define the access method with `DEFINE ACCESS <name> ... TYPE JWT ...` (or `TYPE RECORD ... WITH JWT`)",
            ),
            _ => {}
        }
    }

    /// `DEFINE INDEX ... SEARCH ANALYZER` became `FULLTEXT ANALYZER` in 3.0:
    /// the 2.3.10 define parser matches `t!("SEARCH")` and the 3.0.5 one only
    /// `t!("FULLTEXT")` (docs: "Before SurrealDB version 3.0.0 [the FULLTEXT
    /// ANALYZER clause] used the syntax SEARCH ANALYZER"). The AST folds both
    /// into `IndexKind::Search`, so the spelling is read from the statement.
    fn define_index(&mut self, span: ByteRange, index: &ast::DefineIndex) {
        if index.kind != ast::IndexKind::Search {
            return;
        }
        let words: Vec<(usize, String)> = self
            .text(span)
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .scan(0usize, |offset, word| {
                let start = *offset;
                *offset += word.len() + 1;
                Some((start, word.to_string()))
            })
            .filter(|(_, word)| !word.is_empty())
            .collect();
        for window in words.windows(2) {
            let (start, word) = (&window[0].0, window[0].1.as_str());
            if !window[1].1.eq_ignore_ascii_case("ANALYZER") {
                continue;
            }
            let start = span.start() + *start as u32;
            let word_span = ByteRange::new(start, start + word.len() as u32).unwrap_or(span);
            if word.eq_ignore_ascii_case("SEARCH") {
                self.removed(
                    word_span,
                    "`SEARCH ANALYZER`",
                    Version::new(3, 0, 0),
                    "write `FULLTEXT ANALYZER`",
                );
            } else if word.eq_ignore_ascii_case("FULLTEXT") {
                self.requires(word_span, "`FULLTEXT ANALYZER`", Version::new(3, 0, 0));
            }
        }
    }
}

impl Visitor for SyntaxVersions<'_, '_> {
    fn visit_statement(&mut self, statement: &ast::Spanned<ast::Statement>) {
        match &statement.node {
            // `sql/statements/upsert.rs` at v2.0.5, not v1.5.6.
            ast::Statement::Upsert(_) => {
                let span = self.keyword_span(statement.span);
                self.requires(span, "`UPSERT`", Version::new(2, 0, 0));
            }
            // `sql/statements/alter/` at v2.0.5, not v1.5.6.
            ast::Statement::Alter(_) => {
                let span = self.keyword_span(statement.span);
                self.requires(span, "`ALTER`", Version::new(2, 0, 0));
            }
            ast::Statement::Define(ast::DefineStmt::Other(partial)) => self.define_other(partial),
            ast::Statement::Define(ast::DefineStmt::Index(index)) => {
                self.define_index(statement.span, index);
            }
            _ => {}
        }
        visit::walk_statement(self, statement);
    }

    fn visit_define_field(&mut self, field: &ast::DefineField) {
        // Docs (statements/define/field): REFERENCE "(since v2.2.0)";
        // `sql/reference.rs` at v2.2.8, not v2.1.9.
        if field.reference {
            self.requires(
                field.path.span,
                "a `REFERENCE` field",
                Version::new(2, 2, 0),
            );
        }
        // Docs (statements/define/field): COMPUTED "(since v3.0.0)".
        if let Some(computed) = &field.computed {
            self.requires(computed.span, "a `COMPUTED` field", Version::new(3, 0, 0));
        }
        // Docs (statements/define/field): "ASSERT and DEFAULT on the id field
        // (since v3.2.0)".
        let is_id = matches!(
            field.path.node.parts.as_slice(),
            [part] if matches!(&part.node, ast::IdiomPart::Field(name) if name == "id")
        );
        if is_id {
            if let Some(clause) = field.assert.as_ref().or(field.default.as_ref()) {
                self.requires(
                    clause.span,
                    "`ASSERT`/`DEFAULT` on the `id` field",
                    Version::new(3, 2, 0),
                );
            }
        }
        visit::walk_define_field(self, field);
    }

    fn visit_expr(&mut self, expr: &ast::Spanned<ast::Expr>) {
        match &expr.node {
            // `sql/closure.rs` at v2.0.5, not v1.5.6.
            ast::Expr::Closure(_) => self.requires(expr.span, "a closure", Version::new(2, 0, 0)),
            // `sql/id/range.rs` at v2.0.5; v1.5.6 has only `sql/id.rs`.
            ast::Expr::RecordId { range: true, .. } => {
                self.requires(expr.span, "a record-id range", Version::new(2, 0, 0));
            }
            // Docs (datamodel/futures): "The future type is only available up
            // to SurrealDB 2.x. Since version 3.0.0, it has been replaced by
            // defined fields using the COMPUTED clause"; `sql/future.rs` at
            // v2.3.10, not v3.0.5.
            ast::Expr::Cast { ty, .. } if matches!(&ty.node, ast::TypeExpr::Name(name) if name.node.eq_ignore_ascii_case("future")) =>
            {
                self.removed(
                    ty.span,
                    "`<future>`",
                    Version::new(3, 0, 0),
                    "store the expression as a `DEFINE FIELD ... COMPUTED <expr>` instead",
                );
            }
            // Docs (surrealql/operators): the fuzzy operators `~`, `!~`, `?~`,
            // `*~` were "removed since 3.0", pointing at `string::similarity::*`.
            ast::Expr::Binary { op, .. }
                if matches!(
                    op.node,
                    ast::BinaryOp::Match
                        | ast::BinaryOp::NotMatch
                        | ast::BinaryOp::AllMatch
                        | ast::BinaryOp::AnyMatch
                ) =>
            {
                let spelled = self.text(op.span).to_string();
                self.removed(
                    op.span,
                    &format!("the fuzzy-match operator `{spelled}`"),
                    Version::new(3, 0, 0),
                    "compare with `string::similarity::*` (or `string::matches` for a regex) instead",
                );
            }
            _ => {}
        }
        visit::walk_expr(self, expr);
    }

    fn visit_idiom_part(&mut self, part: &ast::Spanned<ast::IdiomPart>) {
        match &part.node {
            // `Part::Optional` in `sql/part.rs` at v2.0.5, not v1.5.6.
            ast::IdiomPart::Optional => {
                self.requires(part.span, "optional chaining (`?.`)", Version::new(2, 0, 0));
            }
            // `Part::Destructure` in `sql/part.rs` at v2.0.5, not v1.5.6.
            ast::IdiomPart::Destructure(_) => {
                self.requires(
                    part.span,
                    "destructuring (`.{a, b}`)",
                    Version::new(2, 0, 0),
                );
            }
            // `Part::Recurse` in `sql/part.rs` at v2.1.9, not v2.0.5.
            ast::IdiomPart::Recurse { .. } => {
                self.requires(
                    part.span,
                    "a recursive path (`.{n..m}`)",
                    Version::new(2, 1, 0),
                );
            }
            // Docs (datamodel/references): "(since v2.2.0)" for the `<~`
            // traversal; `sql/reference.rs` at v2.2.8, not v2.1.9.
            ast::IdiomPart::Graph { step, .. } if step.reference => {
                self.requires(
                    part.span,
                    "a reference traversal (`<~`)",
                    Version::new(2, 2, 0),
                );
            }
            _ => {}
        }
        visit::walk_idiom_part(self, part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_table_is_sorted_unique_and_rename_pairs_agree() {
        for pair in FUNCTIONS.windows(2) {
            assert!(
                pair[0].name < pair[1].name,
                "{} then {}",
                pair[0].name,
                pair[1].name
            );
        }
        for entry in FUNCTIONS {
            assert!(
                entry.since.is_some() || entry.removed.is_some(),
                "{} carries no fact",
                entry.name
            );
            if let Some(replacement) = entry.replacement {
                let removed = entry.removed.expect("a replacement implies a removal");
                let new = function_version(replacement).unwrap_or_else(|| {
                    panic!("{replacement} (replacing {}) is unlisted", entry.name)
                });
                assert_eq!(new.renamed_from, Some(entry.name));
                // The new spelling exists no later than the old one went.
                assert!(new.since.is_none_or(|since| since <= removed));
            }
            if let Some(older) = entry.renamed_from {
                let old = function_version(older)
                    .unwrap_or_else(|| panic!("{older} (renamed to {}) is unlisted", entry.name));
                assert_eq!(old.replacement, Some(entry.name));
            }
        }
    }

    #[test]
    fn check_function_reads_both_directions_of_a_rename() {
        let two_two = TargetVersion::parse("2.2").unwrap();
        let three = TargetVersion::parse("3.0").unwrap();
        let one = TargetVersion::parse("1.5").unwrap();

        // 3.x spelling on a 2.x target: missing, with the old spelling.
        let miss = check_function(two_two, "type::is_record").expect("fires");
        assert!(
            miss.message.contains("spelled `type::is::record`"),
            "{}",
            miss.message
        );
        assert_eq!(miss.dispatch_as, None);
        // 2.x spelling on a 3.x target: renamed, dispatch under the new name.
        let gone = check_function(three, "type::is::record").expect("fires");
        assert!(
            gone.message.contains("renamed to `type::is_record` in 3.0"),
            "{}",
            gone.message
        );
        assert_eq!(gone.dispatch_as, Some("type::is_record"));
        // The right spelling for the target is silent.
        assert!(check_function(two_two, "type::is::record").is_none());
        assert!(check_function(three, "type::is_record").is_none());
        // A plain addition.
        let added = check_function(two_two, "file::get").expect("fires");
        assert!(added.message.contains("added in 3.0"), "{}", added.message);
        // 2.3 additions: missing from 2.2, present from 2.3 on (and on `2`).
        assert!(check_function(two_two, "rand::duration").is_some());
        assert!(check_function(TargetVersion::parse("2.3").unwrap(), "rand::duration").is_none());
        assert!(check_function(TargetVersion::parse("2").unwrap(), "rand::duration").is_none());
        assert!(check_function(two_two, "array::sort_lexical").is_some());
        // A 2.0 rename seen from 1.x.
        assert!(check_function(one, "string::starts_with").is_some());
        assert!(check_function(one, "string::startsWith").is_none());
        assert!(rename_hint("string::startsWith")
            .unwrap()
            .contains("string::starts_with"));
        // Unlisted names are always available.
        assert!(check_function(one, "string::len").is_none());
    }
}
