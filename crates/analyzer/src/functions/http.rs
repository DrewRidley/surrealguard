use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_min_args, is_string};

const KNOWN_FUNCS: &[&str] = &["head", "delete", "get", "patch", "post", "put"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "head" => {
            expect_min_args("http::head", arg_types, 1, node, ctx);
            expect_arg_type("http::head", arg_types, 0, "string", is_string, node, ctx);
            Kind::Object
        }
        "delete" | "get" | "patch" | "post" | "put" => {
            let full_name = format!("http::{}", func);
            expect_min_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::Any
        }

        _ => {
            emit_unknown_function(
                &format!("http::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::diagnostic::Code;

    #[test]
    fn http_head_returns_object() {
        let result = crate::analyze("LET $x = http::head('https://example.com');").unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn http_head_validates_url_string() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT http::head(x) FROM t;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected wrong arg type warning for http::head(int), got: {:?}",
            result.diagnostics
        );
    }
}
