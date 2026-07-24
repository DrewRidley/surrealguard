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
/// the authenticated record, `$token`/`$session` are objects, and
/// `$access`/`$scope` name the access method.
fn insert_session_params(map: &mut BTreeMap<String, Kind>) {
    // `$auth` is a record; the concrete table depends on the access method,
    // which is unmodeled, so it stays a bare `record`.
    map.insert("auth".to_string(), Kind::Record(Vec::new()));
    map.insert("token".to_string(), Kind::Object);
    map.insert("session".to_string(), Kind::Object);
    map.insert("access".to_string(), Kind::String);
    // `$scope` is the pre-2.0 spelling of `$access`; still accepted.
    map.insert("scope".to_string(), Kind::String);
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
