//! Context parameters bound by SurrealQL DEFINE constructs.
//!
//! Inside a `DEFINE FIELD`, `DEFINE EVENT`, `DEFINE TABLE`/`DEFINE ACCESS`
//! body, SurrealDB binds a fixed set of special `$params` (`$value`,
//! `$event`, `$before`/`$after`, `$auth`, ...). These are never written as
//! `LET` bindings or supplied by the host, so the analyzer's `let_binding`
//! and `inferred_params` facts never carry them. This module models them so
//! editor features (hover) can type a cursor sitting on one.
//!
//! The bindings mirror `surrealdb-core`'s document context: field bodies
//! (`core/src/doc/field.rs`) bind `value`/`after`/`before`/`input`; event
//! bodies (`core/src/doc/event.rs`) additionally bind `event`; `this`/`self`
//! resolve to the base document (`core/src/expr/param.rs`); and the session
//! params `auth`/`token`/`session`/`access` are available throughout.

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral, Table};
use surrealguard_syntax::ast::{DefineStmt, Statement};
use surrealguard_syntax::parse::parse_source;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::ByteRange;

use crate::schema::SchemaIndex;

/// Whether `name` (without the leading `$`) is a reserved *session* param —
/// one SurrealDB binds from the authenticated session (`$auth`) or access
/// token, available in every schema-time context and never a host-supplied
/// query parameter. These are externally typed by the runtime (the concrete
/// `$auth` record depends on the access method), so a *value comparison*
/// against one (`$auth = NONE`, `in = $auth`) must never pin or conflict its
/// kind.
///
/// Only the session set is unconditional: document-context params
/// (`$before`/`$after`/`$value`/...) are context-bound *inside* `DEFINE
/// FIELD`/`EVENT` bodies but are ordinary host params at the top level, so
/// they are deliberately excluded here — comparison-derived reconciliation
/// ([`unify_comparable`](crate::statement_env)) already keeps their record/none
/// comparisons from conflicting without suppressing legitimate host inference.
pub fn is_reserved_session_param(name: &str) -> bool {
    // Mirrors `insert_session_params`.
    matches!(name, "auth" | "token" | "session" | "access" | "scope")
}

/// The context params bound by the DEFINE construct enclosing `offset`,
/// mapped from name (without the leading `$`) to the `Kind` SurrealDB gives
/// them. `None` when the offset sits inside no context-binding construct.
///
/// Best-effort: session params (`$auth`/`$token`/`$session`/`$access`) type
/// to their generic shapes since their concrete record/object types depend
/// on the configured access method, which the schema does not model.
pub fn context_param_map(
    schema: &SchemaIndex,
    source: &SourceId,
    text: &str,
    offset: u32,
) -> Option<BTreeMap<String, Kind>> {
    let parsed = parse_source(source.clone(), text).ok()?;
    let statements = surrealguard_syntax::lower::lower_statements(&parsed);

    // The enclosing construct is the smallest statement whose span covers
    // the cursor — DEFINEs never nest here, but this keeps the choice
    // unambiguous regardless.
    let enclosing = statements
        .iter()
        .filter(|statement| covers(statement.span, offset))
        .min_by_key(|statement| statement.span.len())?;

    let Statement::Define(define) = &enclosing.node else {
        return None;
    };

    match define {
        DefineStmt::Field(field) => {
            let table = field.table.node.as_str();
            let value_kind = field_value_kind(schema, table, field.path.span);
            Some(field_map(table, value_kind))
        }
        DefineStmt::Event(event) => Some(event_map(event.table.node.as_str())),
        // DEFINE TABLE permissions bind only the session params.
        DefineStmt::Table(_) => Some(session_map()),
        // DEFINE ACCESS (and other access-shaped constructs) are outside the
        // modeled DEFINE tier, so they lower to `Other`. Recognize them from
        // the statement text and expose the session params they bind.
        DefineStmt::Other(node) => {
            let span_text = slice(text, enclosing.span);
            if node.cst_kind.to_ascii_uppercase().contains("ACCESS")
                || span_text.to_ascii_uppercase().contains("DEFINE ACCESS")
            {
                Some(session_map())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The `$value` kind for a `DEFINE FIELD` body: the field's declared kind,
/// resolved from the schema by matching the field-name span. `any` when the
/// field is untyped or not indexed (schemaless).
fn field_value_kind(schema: &SchemaIndex, table: &str, name_span: ByteRange) -> Kind {
    schema
        .table(table)
        .and_then(|table| {
            table
                .fields
                .values()
                .find(|field| field.name_span.range() == name_span)
        })
        .and_then(|field| field.kind.clone())
        .unwrap_or(Kind::Any)
}

/// Params bound in a `DEFINE FIELD` body (`VALUE`/`ASSERT`/`DEFAULT`/
/// `PERMISSIONS`).
fn field_map(table: &str, value_kind: Kind) -> BTreeMap<String, Kind> {
    let record = Kind::Record(vec![Table::from(table)]);
    let mut map = BTreeMap::new();
    map.insert("value".to_string(), value_kind);
    map.insert("after".to_string(), record.clone());
    map.insert("before".to_string(), record.clone());
    map.insert("input".to_string(), record.clone());
    map.insert("this".to_string(), record.clone());
    map.insert("self".to_string(), record);
    insert_session_params(&mut map);
    map
}

/// Params bound in a `DEFINE EVENT` body (`WHEN`/`THEN`).
fn event_map(table: &str) -> BTreeMap<String, Kind> {
    let record = Kind::Record(vec![Table::from(table)]);
    let mut map = BTreeMap::new();
    // `$event` is one of these three string literals.
    map.insert(
        "event".to_string(),
        Kind::Either(vec![
            Kind::Literal(KindLiteral::String("CREATE".into())),
            Kind::Literal(KindLiteral::String("UPDATE".into())),
            Kind::Literal(KindLiteral::String("DELETE".into())),
        ]),
    );
    map.insert("value".to_string(), record.clone());
    map.insert("after".to_string(), record.clone());
    map.insert("before".to_string(), record.clone());
    map.insert("input".to_string(), record.clone());
    map.insert("this".to_string(), record.clone());
    map.insert("self".to_string(), record);
    insert_session_params(&mut map);
    map
}

/// A map carrying only the session params — the set a `PERMISSIONS` clause or
/// `DEFINE ACCESS` body binds.
fn session_map() -> BTreeMap<String, Kind> {
    let mut map = BTreeMap::new();
    insert_session_params(&mut map);
    map
}

/// Session/access params available in every schema-time context: `$auth` is
/// the authenticated record, `$session` is the connection's session object
/// with a fixed set of known keys, `$token` is the decoded JWT claims (an
/// open object), and `$access`/`$scope` name the access method.
fn insert_session_params(map: &mut BTreeMap<String, Kind>) {
    // `$auth` is a record; the concrete table depends on the access method,
    // which is unmodeled, so it stays a bare `record`.
    map.insert("auth".to_string(), Kind::Record(Vec::new()));
    // `$token` is the decoded JWT claims and carries *arbitrary* custom claims
    // alongside the standard registered ones, so it must stay an open object —
    // a closed shape would flag a valid custom-claim access. See `session_kind`.
    map.insert("token".to_string(), Kind::Object);
    map.insert("session".to_string(), session_kind());
    map.insert("access".to_string(), Kind::String);
    // `$scope` is the pre-2.0 spelling of `$access`; still accepted.
    map.insert("scope".to_string(), Kind::String);
}

/// The `$session` object's kind: a closed literal object over the fixed keys
/// SurrealDB populates on the connection session.
///
/// Key names and kinds are taken from the authoritative surface — the
/// `session::*` builtins, each of which is literally `$session.pick(KEY)`
/// (`surrealdb-core/src/fnc/session.rs`), and the object SurrealDB assembles in
/// `surrealdb-core/src/dbs/session.rs::values()`:
///
/// | builtin            | key  | kind    |
/// |--------------------|------|---------|
/// | `session::ac()`    | `ac` | string  |
/// | `session::db()`    | `db` | string  |
/// | `session::id()`    | `id` | string  |
/// | `session::ip()`    | `ip` | string  |
/// | `session::ns()`    | `ns` | string  |
/// | `session::origin()`| `or` | string  |
/// | `session::rd()`    | `rd` | record  |
/// | `session::token()` | `tk` | object  |
///
/// `rd` is the authenticated record (same value as `$auth`), so it mirrors
/// `$auth`'s bare open `record`. `tk` is the token/claims value and therefore
/// stays an open `object` for the same reason `$token` does.
///
/// Modeled as a *closed* `Literal(Object(..))` only because unknown-field access
/// on such a kind is lenient — the field-stepper returns `None` and the caller
/// degrades to unresolved without emitting a finding — so an access to a key not
/// listed here (or a deeper path) stays silent. Known keys type precisely;
/// nothing false-flags. This is purely additive typing.
fn session_kind() -> Kind {
    let mut fields = BTreeMap::new();
    fields.insert("ac".to_string(), Kind::String);
    fields.insert("db".to_string(), Kind::String);
    fields.insert("id".to_string(), Kind::String);
    fields.insert("ip".to_string(), Kind::String);
    fields.insert("ns".to_string(), Kind::String);
    // `or` is the origin key (`session::origin()` picks `OR`).
    fields.insert("or".to_string(), Kind::String);
    // `rd` is the authenticated record, mirroring `$auth`.
    fields.insert("rd".to_string(), Kind::Record(Vec::new()));
    // `tk` is the token/claims value — an open object, like `$token`.
    fields.insert("tk".to_string(), Kind::Object);
    Kind::Literal(KindLiteral::Object(fields))
}

/// Whether the byte range covers `offset` (inclusive of both endpoints, so a
/// cursor at either edge of a token still resolves).
fn covers(range: ByteRange, offset: u32) -> bool {
    range.start() <= offset && offset <= range.end()
}

fn slice(text: &str, range: ByteRange) -> &str {
    let start = (range.start() as usize).min(text.len());
    let end = (range.end() as usize).min(text.len());
    text.get(start..end).unwrap_or("")
}

/// Finds the `$name` parameter token covering `offset`, returning its name
/// (without the `$`) and byte range. `None` when the cursor is not on a
/// parameter token.
pub fn param_token_at(text: &str, offset: u32) -> Option<(String, ByteRange)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let start = index;
        let mut end = index + 1;
        while end < bytes.len() && is_ident_byte(bytes[end]) {
            end += 1;
        }
        // A lone `$` with no identifier is not a parameter token.
        if end > start + 1 {
            let offset = offset as usize;
            if offset >= start && offset <= end {
                let name = text[start + 1..end].to_string();
                let range = ByteRange::new(start as u32, end as u32).ok()?;
                return Some((name, range));
            }
        }
        index = end;
    }
    None
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{analyze_query, Workspace};

    fn codes(query: &str) -> Vec<String> {
        let mut workspace = Workspace::default();
        analyze_query(&mut workspace, query)
            .diagnostics
            .iter()
            .map(|finding| finding.code().to_string())
            .collect()
    }

    /// The known fields of the `$session` object kind, or `None` if it is not
    /// modeled as a closed literal object.
    fn session_fields() -> BTreeMap<String, Kind> {
        let mut map = BTreeMap::new();
        insert_session_params(&mut map);
        match map.get("session") {
            Some(Kind::Literal(KindLiteral::Object(fields))) => fields.clone(),
            other => panic!("`$session` is not a closed literal object: {other:?}"),
        }
    }

    // ---- Field composition (STEP 1) ----

    #[test]
    fn session_rd_types_as_the_auth_record() {
        // `$session.rd` is the authenticated record — mirrors `$auth`.
        assert_eq!(session_fields().get("rd"), Some(&Kind::Record(Vec::new())));
    }

    #[test]
    fn session_ip_types_as_string() {
        assert_eq!(session_fields().get("ip"), Some(&Kind::String));
    }

    #[test]
    fn session_string_keys_all_present() {
        let fields = session_fields();
        for key in ["ac", "db", "id", "ip", "ns", "or"] {
            assert_eq!(
                fields.get(key),
                Some(&Kind::String),
                "key `{key}` should be string"
            );
        }
    }

    #[test]
    fn session_token_key_stays_open_object() {
        // `$session.tk` is the token/claims value — never a closed shape.
        assert_eq!(session_fields().get("tk"), Some(&Kind::Object));
    }

    #[test]
    fn session_has_no_unconfirmed_keys() {
        // Exactly the eight evidence-backed keys; nothing speculative.
        let keys: Vec<String> = session_fields().keys().cloned().collect();
        assert_eq!(keys, ["ac", "db", "id", "ip", "ns", "or", "rd", "tk"]);
    }

    #[test]
    fn auth_stays_bare_record_and_token_stays_open_object() {
        let mut map = BTreeMap::new();
        insert_session_params(&mut map);
        assert_eq!(map.get("auth"), Some(&Kind::Record(Vec::new())));
        // `$token` (top-level) is arbitrary JWT claims — must stay open.
        assert_eq!(map.get("token"), Some(&Kind::Object));
        assert_eq!(map.get("access"), Some(&Kind::String));
        assert_eq!(map.get("scope"), Some(&Kind::String));
    }

    // ---- STEP 2 leniency gate: the regression guard ----

    /// Accessing an UNKNOWN field on a closed `Literal(Object(..))` param must
    /// not emit any finding. This is the property that makes modeling `$session`
    /// as a closed object safe.
    #[test]
    fn gate_unknown_field_on_closed_object_is_lenient() {
        // `LET $p = { a: 'x' }` types `$p` as Literal(Object({a: string})).
        let unknown = "LET $p = { a: 'x' }; RETURN $p.zzz;";
        assert!(
            codes(unknown).is_empty(),
            "unknown field access fired: {:?}",
            codes(unknown)
        );
    }

    // ---- Soundness: no valid session/token access starts erroring ----

    #[test]
    fn unknown_session_field_access_emits_no_finding() {
        // An unknown `$session` key in a PERMISSIONS predicate stays silent.
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL ",
            "PERMISSIONS FOR select WHERE $session.zzz = 'x';\n",
        );
        let fired = codes(query);
        assert!(fired.is_empty(), "unexpected findings: {fired:?}");
    }

    #[test]
    fn known_session_field_access_emits_no_finding() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL ",
            "PERMISSIONS FOR select WHERE $session.rd = $auth;\n",
        );
        let fired = codes(query);
        assert!(fired.is_empty(), "unexpected findings: {fired:?}");
    }

    #[test]
    fn token_custom_claim_access_emits_no_finding() {
        let query = concat!(
            "DEFINE TABLE post SCHEMAFULL ",
            "PERMISSIONS FOR select WHERE $token.some_custom_claim = 'x';\n",
        );
        let fired = codes(query);
        assert!(fired.is_empty(), "unexpected findings: {fired:?}");
    }
}
