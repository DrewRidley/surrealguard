use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::resolve::resolve_expr;
use crate::span::Span;
use crate::types::Kind;

/// Analyze a KILL statement.
///
/// Grammar: `kill_statement = keyword_kill + value`
///
/// KILL cancels a live query by its UUID. The argument must resolve to a UUID type.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if !child.kind().starts_with("keyword_") {
            let typ = resolve_expr(&child, source, ctx, None);
            if typ != Kind::Uuid && typ != Kind::Any {
                ctx.emit(Diagnostic::warning(
                    Span::from_node(&child),
                    Code::TypeMismatch,
                    format!("KILL expects a UUID argument, got `{}`", typ),
                ));
            }
        }
    }
    Kind::Null
}

#[cfg(test)]
mod tests {
    #[test]
    fn kill_with_uuid_no_warning() {
        let result = crate::analyze(r#"KILL u"e72bee20-f49b-11ec-b939-0242ac120002";"#).unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("KILL"))
            .collect();
        assert!(warnings.is_empty(), "KILL with UUID literal should not warn: {:?}", warnings);
    }

    #[test]
    fn kill_with_string_warns() {
        let result = crate::analyze(r#"KILL "not-a-uuid";"#).unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("KILL"))
            .collect();
        assert!(!warnings.is_empty(), "KILL with string should warn about type mismatch");
    }

    #[test]
    fn kill_with_number_warns() {
        let result = crate::analyze(r#"KILL 42;"#).unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("KILL"))
            .collect();
        assert!(!warnings.is_empty(), "KILL with number should warn about type mismatch");
    }

    #[test]
    fn kill_with_param_no_warning() {
        let result = crate::analyze(r#"KILL $live_id;"#).unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("KILL"))
            .collect();
        assert!(warnings.is_empty(), "KILL with param (Any type) should not warn: {:?}", warnings);
    }
}
