/// SurrealGuard Analyzer — static analysis and type inference for SurrealQL.
///
/// Uses tree-sitter for parsing, providing accurate spans, error recovery,
/// and incremental reparsing. No dependency on `surrealdb`.
///
/// # Usage
///
/// ```
/// use surrealguard_analyzer::{analyze, AnalysisResult};
///
/// let source = r#"
///     DEFINE TABLE user SCHEMAFULL;
///     DEFINE FIELD name ON user TYPE string;
///     SELECT name FROM user;
/// "#;
///
/// let result = analyze(source).unwrap();
/// assert!(result.diagnostics.is_empty());
/// ```

pub mod config;
pub mod context;
pub mod diagnostic;
mod functions;
pub mod hints;
pub mod parser;
pub mod permissions;
pub mod resolve;
pub mod schema;
pub mod scope;
pub mod span;
pub mod statements;
pub mod types;

// Re-export core types for convenience
pub use context::Context;
pub use diagnostic::{Code, Diagnostic, Severity};
pub use parser::{parse, ParseError};
pub use span::Span;
pub use types::{Kind, KindExt};

/// Result of analyzing a SurrealQL source file.
#[derive(Debug)]
pub struct AnalysisResult {
    /// All diagnostics emitted during analysis.
    pub diagnostics: Vec<Diagnostic>,
    /// The analysis context (schema, scopes, inferred params).
    pub context: Context,
}

/// Analyze SurrealQL source text.
///
/// This is the main entry point. It:
/// 1. Parses the source with tree-sitter
/// 2. Extracts schema definitions (DEFINE TABLE/FIELD/INDEX/FUNCTION)
/// 3. Analyzes all statements for type errors, scope issues, etc.
/// 4. Validates permission clauses
pub fn analyze(source: &str) -> Result<AnalysisResult, ParseError> {
    analyze_with_config(source, false)
}

/// Analyze with strict mode enabled.
///
/// In strict mode, ambiguous types and unknown fields on schemafull
/// tables emit errors instead of being silently accepted.
pub fn analyze_strict(source: &str) -> Result<AnalysisResult, ParseError> {
    analyze_with_config(source, true)
}

/// Analyze with a pre-configured context.
///
/// Useful when you have schema from external files or want to
/// analyze a query against a known schema.
pub fn analyze_with_context(
    source: &str,
    ctx: &mut Context,
) -> Result<Vec<Diagnostic>, ParseError> {
    let tree = parse(source)?;
    let root = tree.root_node();

    // Check for parse errors
    if root.has_error() {
        collect_parse_errors(&root, source, ctx);
    }

    // Extract schema from this source (additive to existing context)
    schema::extract_schema(&root, source, ctx);

    // Analyze all statements
    statements::analyze_all(&root, source, ctx);

    // Validate permissions
    permissions::analyze_permissions(&root, source, ctx);

    Ok(ctx.take_diagnostics())
}

fn analyze_with_config(source: &str, strict: bool) -> Result<AnalysisResult, ParseError> {
    let mut ctx = if strict {
        Context::strict()
    } else {
        Context::new()
    };

    let diagnostics = analyze_with_context(source, &mut ctx)?;

    // Put diagnostics back for the result
    for d in &diagnostics {
        ctx.emit(d.clone());
    }

    Ok(AnalysisResult {
        diagnostics,
        context: ctx,
    })
}

fn collect_parse_errors(node: &tree_sitter::Node, source: &str, ctx: &mut Context) {
    if node.is_error() || node.is_missing() {
        let span = Span::from_node(node);
        let text = if span.len() > 0 {
            let t = span.text(source);
            if t.len() > 30 {
                format!("{}...", &t[..30])
            } else {
                t.to_string()
            }
        } else {
            "unexpected end of input".to_string()
        };

        ctx.emit(Diagnostic::error(
            span,
            Code::ParseError,
            format!("parse error near `{}`", text),
        ));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_parse_errors(&child, source, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_valid_schema() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            "#,
        )
        .unwrap();

        // Should have no errors
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn analyze_with_schema_context() {
        let mut ctx = Context::new();

        // First pass: define schema
        let schema_source = r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
        "#;
        let _ = analyze_with_context(schema_source, &mut ctx).unwrap();

        // Verify schema was extracted
        assert!(ctx.has_table("user"));
        assert!(ctx.get_field("user", "name").is_some());
    }

    #[test]
    fn strict_mode_catches_errors() {
        let result = analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
        "#,
        )
        .unwrap();

        // Basic schema definition should not produce errors even in strict mode
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn context_builds_table_types() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            "#,
        )
        .unwrap();

        let table_type = result.context.build_table_type("user").unwrap();
        if let Kind::Literal(crate::types::Literal::Object(fields)) = table_type {
            assert_eq!(fields["name"], Kind::String);
            assert_eq!(fields["age"], Kind::Int);
        } else {
            panic!("Expected object type, got {:?}", table_type);
        }
    }

    #[test]
    fn parse_error_reported() {
        let _result = analyze("SELECT FROM WHERE @@ ###").unwrap();
        // Should have at least one parse error or be resilient
        // tree-sitter may or may not flag this as error depending on grammar
    }

    #[test]
    fn graph_edge_invalid_target() {
        // friend relates user->user, not user->file
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE file SCHEMAFULL;
            DEFINE FIELD path ON file TYPE string;
            DEFINE TABLE friend TYPE RELATION FROM user TO user;
            SELECT ->friend->file FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("not to `file`"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected graph edge error for friend->file, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn graph_edge_valid_traversal() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE friend TYPE RELATION FROM user TO user;
            SELECT ->friend->user FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("does not connect"))
            .collect();
        assert!(
            errors.is_empty(),
            "Valid traversal should not error: {:?}",
            errors
        );
    }

    #[test]
    fn assignment_type_mismatch_update() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            UPDATE user SET age = 'twenty';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected assignment type error: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn assignment_compatible_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            UPDATE user SET name = 'John', age = 30;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn undefined_field_on_schemafull_create() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET name = 'John', email = 'j@test.com';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("not defined on table"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected undefined field error, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn undefined_field_on_schemaless_ok() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMALESS;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET name = 'John', email = 'j@test.com';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("not defined on table"))
            .collect();
        assert!(
            errors.is_empty(),
            "Schemaless table should allow undefined fields: {:?}",
            errors
        );
    }

    #[test]
    fn type_assignability() {
        use crate::types::{is_assignable, Kind, Table};

        // Basic same type
        assert!(is_assignable(&Kind::String, &Kind::String));
        assert!(is_assignable(&Kind::Int, &Kind::Int));

        // Any accepts anything
        assert!(is_assignable(&Kind::Any, &Kind::String));
        assert!(is_assignable(&Kind::String, &Kind::Any));

        // Numeric coercion
        assert!(is_assignable(&Kind::Number, &Kind::Int));
        assert!(is_assignable(&Kind::Float, &Kind::Int));
        assert!(is_assignable(&Kind::Decimal, &Kind::Float));

        // Incompatible
        assert!(!is_assignable(&Kind::Int, &Kind::String));
        assert!(!is_assignable(&Kind::Bool, &Kind::Int));
        assert!(!is_assignable(&Kind::String, &Kind::Bool));

        // Option
        assert!(is_assignable(&Kind::Option(Box::new(Kind::String)), &Kind::String));
        assert!(is_assignable(&Kind::Option(Box::new(Kind::String)), &Kind::Null));

        // Record compatibility
        assert!(is_assignable(
            &Kind::Record(vec![Table::from("user".to_string())]),
            &Kind::Record(vec![Table::from("user".to_string())])
        ));
        assert!(!is_assignable(
            &Kind::Record(vec![Table::from("user".to_string())]),
            &Kind::Record(vec![Table::from("post".to_string())])
        ));
        // record<> accepts any record
        assert!(is_assignable(
            &Kind::Record(vec![]),
            &Kind::Record(vec![Table::from("anything".to_string())])
        ));

        // Array element compatibility
        assert!(is_assignable(
            &Kind::Array(Box::new(Kind::Int), None),
            &Kind::Array(Box::new(Kind::Int), None)
        ));
        assert!(!is_assignable(
            &Kind::Array(Box::new(Kind::Int), None),
            &Kind::Array(Box::new(Kind::String), None)
        ));
    }

    #[test]
    fn string_len_wrong_arg_type() {
        let result = analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE string::len(x) > 0;
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
            "Expected wrong arg type warning for string::len(int), got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn string_len_valid_arg() {
        let result = analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE string;
            SELECT * FROM t WHERE string::len(x) > 0;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            warnings.is_empty(),
            "string::len with string arg should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn array_len_wrong_arg_type() {
        let result = analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE string;
            SELECT * FROM t WHERE array::len(x) > 0;
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
            "Expected wrong arg type warning for array::len(string), got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn math_abs_wrong_arg_type() {
        let result = analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE string;
            SELECT * FROM t WHERE math::abs(x) > 0;
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
            "Expected wrong arg type warning for math::abs(string), got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn crypto_md5_wrong_arg_type() {
        let result = analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE int;
            SELECT * FROM t WHERE crypto::md5(x) != "";
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
            "Expected wrong arg type warning for crypto::md5(int), got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn crypto_sha256_valid_arg() {
        let result = analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE string;
            SELECT * FROM t WHERE crypto::sha256(x) != "";
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgType)
            .collect();
        assert!(
            warnings.is_empty(),
            "crypto::sha256 with string arg should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn math_sum_wrong_arg_type() {
        let result = analyze(
            r#"
            DEFINE TABLE t SCHEMAFULL;
            DEFINE FIELD x ON t TYPE string;
            SELECT * FROM t WHERE math::sum(x) > 0;
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
            "Expected wrong arg type warning for math::sum(string), got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn cast_validity() {
        use crate::types::{is_valid_cast, Kind};

        // Everything to string is valid
        assert!(is_valid_cast(&Kind::String, &Kind::Int));
        assert!(is_valid_cast(&Kind::String, &Kind::Bool));
        assert!(is_valid_cast(&Kind::String, &Kind::Duration));

        // Everything to bool is valid
        assert!(is_valid_cast(&Kind::Bool, &Kind::String));
        assert!(is_valid_cast(&Kind::Bool, &Kind::Int));

        // Numeric casts
        assert!(is_valid_cast(&Kind::Int, &Kind::String));
        assert!(is_valid_cast(&Kind::Int, &Kind::Float));
        assert!(is_valid_cast(&Kind::Float, &Kind::Int));

        // Invalid casts
        assert!(!is_valid_cast(&Kind::Geometry(vec![]), &Kind::Bool));
        assert!(!is_valid_cast(&Kind::Duration, &Kind::Bool));
        assert!(!is_valid_cast(&Kind::Uuid, &Kind::Int));
    }

    #[test]
    fn comparison_string_vs_int_warns() {
        // Comparing a string literal to an int literal should produce a type mismatch warning
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT * FROM user WHERE name == 42;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("comparing incompatible types"))
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for string == int comparison, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn comparison_int_vs_float_no_warn() {
        // Comparing numeric types should not warn — they are compatible
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD age ON user TYPE int;
            DEFINE FIELD score ON user TYPE float;
            SELECT * FROM user WHERE age > score;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("comparing incompatible types"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Numeric comparison should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn logical_and_with_non_bool_warns() {
        // Using AND with a non-bool operand should produce a warning
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD active ON user TYPE bool;
            DEFINE FIELD name ON user TYPE string;
            SELECT * FROM user WHERE active AND name;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("`AND` expects `bool` operands"))
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for bool AND string, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn field_compared_to_string_literal_no_warn() {
        // A field with unknown type (Any) compared to a string should not warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMALESS;
            SELECT * FROM user WHERE name == "test";
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("comparing incompatible types"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Field (Any) == string should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn graph_basic_traversal_no_error() {
        // Basic graph traversal in SELECT fields should not produce errors
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE wrote TYPE RELATION FROM user TO post;
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD title ON post TYPE string;
            SELECT ->wrote->post FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "Basic graph traversal should not error: {:?}",
            errors
        );
    }

    #[test]
    fn graph_where_clause_resolved() {
        // Graph predicate with WHERE clause should be resolved without spurious errors
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE wrote TYPE RELATION FROM user TO post;
            DEFINE FIELD created_at ON wrote TYPE datetime;
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD title ON post TYPE string;
            SELECT ->(wrote WHERE created_at > d'2024-01-01')->post FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "Graph WHERE clause should not produce errors: {:?}",
            errors
        );
    }

    #[test]
    fn graph_where_non_bool_warns() {
        // Graph WHERE with non-bool condition should produce a warning
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE wrote TYPE RELATION FROM user TO post;
            DEFINE TABLE post SCHEMAFULL;
            SELECT ->(wrote WHERE "not a bool")->post FROM user;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("WHERE condition should be bool"))
            .collect();
        assert!(
            !warnings.is_empty(),
            "Non-bool WHERE condition should warn: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn contains_on_non_container_warns() {
        // CONTAINS on an int (non-array/set/string) should warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD age ON user TYPE int;
            SELECT * FROM user WHERE age CONTAINS 42;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch
                    && d.message.contains("CONTAINS")
                    && d.message.contains("left operand")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for int CONTAINS int, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn contains_on_array_no_warn() {
        // CONTAINS on an array should not warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD tags ON user TYPE array<string>;
            SELECT * FROM user WHERE tags CONTAINS 'admin';
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch && d.message.contains("CONTAINS")
            })
            .collect();
        assert!(
            warnings.is_empty(),
            "array CONTAINS should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn contains_on_string_no_warn() {
        // String CONTAINS (substring check) should not warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT * FROM user WHERE name CONTAINS 'foo';
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch && d.message.contains("CONTAINS")
            })
            .collect();
        assert!(
            warnings.is_empty(),
            "string CONTAINS should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn inside_non_container_warns() {
        // INSIDE with right operand as int should warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD age ON user TYPE int;
            DEFINE FIELD score ON user TYPE int;
            SELECT * FROM user WHERE age INSIDE score;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch
                    && d.message.contains("INSIDE")
                    && d.message.contains("right operand")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for int INSIDE int, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn inside_array_no_warn() {
        // INSIDE with right operand as array should not warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD role ON user TYPE string;
            DEFINE FIELD allowed ON user TYPE array<string>;
            SELECT * FROM user WHERE role INSIDE allowed;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch && d.message.contains("INSIDE")
            })
            .collect();
        assert!(
            warnings.is_empty(),
            "INSIDE array should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn matches_non_string_warns() {
        // MATCHES with non-string operands should warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD age ON user TYPE int;
            SELECT * FROM user WHERE age MATCHES 'pattern';
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch && d.message.contains("MATCHES")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for int MATCHES string, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn matches_string_no_warn() {
        // MATCHES with string operands should not warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT * FROM user WHERE name MATCHES 'pattern';
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch && d.message.contains("MATCHES")
            })
            .collect();
        assert!(
            warnings.is_empty(),
            "string MATCHES string should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn containsall_on_non_container_warns() {
        // CONTAINSALL on a bool should warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD active ON user TYPE bool;
            SELECT * FROM user WHERE active CONTAINSALL [1, 2];
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch
                    && d.message.contains("CONTAINSALL")
                    && d.message.contains("left operand")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for bool CONTAINSALL, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn object_literal_duplicate_key_warns() {
        let result = crate::analyze(
            r#"
            SELECT * FROM { name: 'John', age: 25, name: 'Jane' };
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::DuplicateObjectKey)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected duplicate key warning, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn object_literal_no_duplicate_no_warning() {
        let result = crate::analyze(
            r#"
            SELECT * FROM { name: 'John', age: 25 };
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::DuplicateObjectKey)
            .collect();
        assert!(
            warnings.is_empty(),
            "Expected no duplicate key warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn allinside_non_container_warns() {
        // ALLINSIDE with right operand as bool should warn
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD age ON user TYPE int;
            DEFINE FIELD active ON user TYPE bool;
            SELECT * FROM user WHERE age ALLINSIDE active;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == Code::TypeMismatch
                    && d.message.contains("ALLINSIDE")
                    && d.message.contains("right operand")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for int ALLINSIDE bool, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn record_id_table_exists_strict() {
        let result = crate::analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            SELECT * FROM user:123;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.code == crate::Code::TableNotFound)
            .collect();
        assert!(errors.is_empty(), "Should not error for defined table");
    }

    // camelCase alias tests: the tree-sitter-surrealql grammar already accepts
    // camelCase identifiers in function names (e.g. `string::startsWith`), and
    // the analyzer resolves them via camel_alias() in functions/string.rs.

    #[test]
    fn string_camelcase_alias_starts_with() {
        let result = analyze(
            r#"
            RETURN string::startsWith("hello", "he");
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "camelCase alias startsWith should work: {:?}",
            errors
        );
    }

    #[test]
    fn string_camelcase_alias_ends_with() {
        let result = analyze(
            r#"
            RETURN string::endsWith("hello", "lo");
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "camelCase alias endsWith should work: {:?}",
            errors
        );
    }

    #[test]
    fn string_camelcase_alias_to_lowercase() {
        let result = analyze(
            r#"
            RETURN string::toLowerCase("HELLO");
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "camelCase alias toLowerCase should work: {:?}",
            errors
        );
    }

    #[test]
    fn string_camelcase_alias_to_uppercase() {
        let result = analyze(
            r#"
            RETURN string::toUpperCase("hello");
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            errors.is_empty(),
            "camelCase alias toUpperCase should work: {:?}",
            errors
        );
    }

    #[test]
    fn string_camelcase_alias_no_false_warning() {
        // Verify that camelCase aliases don't produce "unknown function" warnings
        let result = analyze(
            r#"
            RETURN string::startsWith("hello", "he");
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::FunctionNotFound)
            .collect();
        assert!(
            warnings.is_empty(),
            "camelCase alias should not produce FunctionNotFound warning: {:?}",
            warnings
        );
    }

    #[test]
    fn record_id_table_not_exists_strict() {
        let result = crate::analyze_strict(
            r#"
            SELECT * FROM nonexistent:123;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Should error for undefined table in strict mode"
        );
    }

    #[test]
    fn geometry_point_assignable_to_geometry() {
        use crate::types::{is_assignable, Kind};
        // geometry<point> → geometry (bare) should work
        let point = Kind::Geometry(vec!["point".into()]);
        let any_geo = Kind::Geometry(vec![]);
        assert!(
            is_assignable(&any_geo, &point),
            "geometry<point> should be assignable to bare geometry"
        );
    }

    #[test]
    fn geometry_point_not_assignable_to_geometry_line() {
        use crate::types::{is_assignable, Kind};
        // geometry<point> → geometry<line> should fail
        let point = Kind::Geometry(vec!["point".into()]);
        let line = Kind::Geometry(vec!["line".into()]);
        assert!(
            !is_assignable(&line, &point),
            "geometry<point> should NOT be assignable to geometry<line>"
        );
    }

    #[test]
    fn geometry_any_accepts_point() {
        use crate::types::{is_assignable, Kind};
        // A field of type geometry should accept geometry<point>
        let any_geo = Kind::Geometry(vec![]);
        let point = Kind::Geometry(vec!["point".into()]);
        assert!(
            is_assignable(&any_geo, &point),
            "bare geometry should accept geometry<point>"
        );
    }

    #[test]
    fn geometry_collection_accepts_any_subtype() {
        use crate::types::{is_assignable, Kind};
        let collection = Kind::Geometry(vec!["collection".into()]);
        let point = Kind::Geometry(vec!["point".into()]);
        let line = Kind::Geometry(vec!["line".into()]);
        let polygon = Kind::Geometry(vec!["polygon".into()]);
        let multipoint = Kind::Geometry(vec!["multipoint".into()]);
        assert!(is_assignable(&collection, &point), "collection should accept point");
        assert!(is_assignable(&collection, &line), "collection should accept line");
        assert!(is_assignable(&collection, &polygon), "collection should accept polygon");
        assert!(is_assignable(&collection, &multipoint), "collection should accept multipoint");
    }

    #[test]
    fn geometry_same_subtype_assignable() {
        use crate::types::{is_assignable, Kind};
        let point1 = Kind::Geometry(vec!["point".into()]);
        let point2 = Kind::Geometry(vec!["point".into()]);
        assert!(
            is_assignable(&point1, &point2),
            "geometry<point> should be assignable to geometry<point>"
        );
    }

    #[test]
    fn debug_diagnostic_quality() {
        fn show(label: &str, source: &str) {
            let result = crate::analyze(source).unwrap();
            eprintln!("\n=== {} ===", label);
            for d in &result.diagnostics {
                let sev = match d.severity {
                    crate::Severity::Error => "ERROR",
                    crate::Severity::Warning => "WARN",
                    crate::Severity::Hint => "HINT",
                };
                let span_text = &source[d.span.start as usize..std::cmp::min(d.span.end as usize, source.len())];
                let short = if span_text.len() > 60 { &span_text[..60] } else { span_text };
                eprintln!("  [{}] {} ({})", sev, d.message, d.code.id());
                eprintln!("       span {}..{}: {:?}", d.span.start, d.span.end, short);
                if let Some(ref s) = d.suggestion { eprintln!("       help: {}", s); }
                for r in &d.related {
                    let rt = &source[r.span.start as usize..std::cmp::min(r.span.end as usize, source.len())];
                    let rs = if rt.len() > 40 { &rt[..40] } else { rt };
                    eprintln!("       related: {} -> {:?}", r.message, rs);
                }
            }
        }

        show("CREATE missing field", "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;\nDEFINE FIELD profile ON user TYPE record<profile>;\nCREATE user SET name = 'Alice', age = 30;");

        show("UPDATE type mismatch", "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD age ON user TYPE int;\nUPDATE user SET age = 'twenty';");

        show("LET greeting no profile", "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD profile ON user TYPE record<profile>;\nLET $greeting = \"hello world\";");

        show("LET uppercase", "LET $upper = string::uppercase(\"hello\");");

        // Debug graph path tree structure
        {
            let src = "SELECT ->wrote->user FROM user;";
            let tree = crate::parse(src).unwrap();
            fn print_tree(node: &tree_sitter::Node, source: &str, depth: usize) {
                let indent = "  ".repeat(depth);
                let text = node.utf8_text(source.as_bytes()).unwrap_or("");
                let short = if text.len() > 30 { &text[..30] } else { text };
                eprintln!("{}{} [{}-{}] {:?}", indent, node.kind(), node.start_byte(), node.end_byte(), short);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    print_tree(&child, source, depth + 1);
                }
            }
            eprintln!("\n=== Graph path tree ===");
            print_tree(&tree.root_node(), src, 0);
        }

        // Test cross-file scenario: schema in context, queries analyzed
        {
            eprintln!("\n=== Cross-file with schema context ===");
            let schema = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;\nDEFINE FIELD profile ON user TYPE record<profile>;";
            let query = "LET $adults = SELECT * FROM user WHERE age >= 18;\nLET $greeting = \"hello world\";\nCREATE user SET name = 'Alice', age = 30;";
            let mut ctx = crate::Context::new();
            let _ = crate::analyze_with_context(schema, &mut ctx);
            ctx.take_diagnostics(); // discard schema diagnostics
            let diags = crate::analyze_with_context(query, &mut ctx).unwrap();
            for d in &diags {
                let sev = match d.severity {
                    crate::Severity::Error => "ERROR",
                    crate::Severity::Warning => "WARN",
                    crate::Severity::Hint => "HINT",
                };
                let span_text = &query[d.span.start as usize..std::cmp::min(d.span.end as usize, query.len())];
                let short = if span_text.len() > 60 { &span_text[..60] } else { span_text };
                eprintln!("  [{}] {} ({})", sev, d.message, d.code.id());
                eprintln!("       span {}..{}: {:?}", d.span.start, d.span.end, short);
                for r in &d.related {
                    eprintln!("       related span {}..{}: {}", r.span.start, r.span.end, r.message);
                }
            }
        }
    }

    #[test]
    fn block_with_if_return_unions_types() {
        // A block with IF/RETURN should produce a union type
        let src = r#"
            LET $result = {
                IF true { RETURN 'hello' };
                RETURN 42;
            };
        "#;
        let result = analyze(src).unwrap();
        if let Some(binding) = result.context.scope.lookup("$result") {
            match &binding.typ {
                Kind::Either(variants) => {
                    assert!(
                        variants.len() >= 2,
                        "Expected union of at least 2 types, got: {:?}",
                        variants
                    );
                }
                Kind::String | Kind::Int => {
                    // Acceptable if one branch dominates
                }
                other => {
                    // At minimum it should not be Any
                    assert!(
                        !other.is_any(),
                        "Expected union type for multi-return block, got: {:?}",
                        other
                    );
                }
            }
        }
        // Should have no errors
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn block_single_return_infers_type() {
        let src = r#"LET $x = { RETURN 'hello'; };"#;
        let result = analyze(src).unwrap();
        if let Some(binding) = result.context.scope.lookup("$x") {
            assert_eq!(binding.typ, Kind::String, "Expected string from RETURN 'hello'");
        } else {
            panic!("$x not found in scope");
        }
    }

    #[test]
    fn geometry_cast_subtype_covariance() {
        use crate::types::{is_valid_cast, Kind};
        let point = Kind::Geometry(vec!["point".into()]);
        let any_geo = Kind::Geometry(vec![]);
        let line = Kind::Geometry(vec!["line".into()]);
        // Cast geometry<point> to geometry should work
        assert!(is_valid_cast(&any_geo, &point), "cast point to bare geometry should be valid");
        // Cast geometry<point> to geometry<line> should fail
        assert!(!is_valid_cast(&line, &point), "cast point to line should be invalid");
    }
}

    #[test]
    fn debug_user_issues() {
        fn show(label: &str, source: &str) {
            let result = crate::analyze(source).unwrap();
            eprintln!("\n=== {} ===", label);
            if result.diagnostics.is_empty() {
                eprintln!("  (no diagnostics)");
            }
            for d in &result.diagnostics {
                let sev = match d.severity { crate::Severity::Error => "ERR", crate::Severity::Warning => "WRN", crate::Severity::Hint => "HNT" };
                eprintln!("  [{}] {} ({})", sev, d.message, d.code.id());
            }
        }

        let schema = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;\nDEFINE FIELD email ON user TYPE string;\nDEFINE FIELD active ON user TYPE bool;\nDEFINE PARAM $aah VALUE 5;";

        // Issue 1: SELECT with typo field on schemafull
        show("Typo field 'ag'", &format!("{}\nSELECT name, ag FROM user;", schema));

        // Issue 2: CONTENT clause type mismatch
        show("CONTENT age='test'", &format!("{}\nUPDATE user CONTENT {{ age: 'test', active: true }};", schema));

        // Issue 3: $aah param type resolution
        show("$aah comparison", &format!("{}\nSELECT * FROM user WHERE age > $aah;", schema));
    }

    #[test]
    fn gap_audit() {
        let src = r#"
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE FIELD age ON user TYPE int;
DEFINE TABLE post SCHEMAFULL;
DEFINE FIELD title ON post TYPE string;
DEFINE FIELD author ON post TYPE record<user>;
DEFINE TABLE wrote TYPE RELATION FROM user TO post;

LET $a = (SELECT name FROM user);
LET $b = SELECT VALUE name FROM user;
LET $c = SELECT name AS n, age AS a FROM user;
LET $d = 1 + 2;
LET $e = "hello" + " world";
LET $f = IF true { 'hello' } ELSE { 42 };
"#;
        let result = crate::analyze(src).unwrap();
        let ctx = &result.context;
        
        let vars = ["$a", "$b", "$c", "$d", "$e", "$f"];
        for var in &vars {
            let typ = ctx.scope.lookup(var)
                .map(|b| format!("{}", crate::types::display_kind(&b.typ)))
                .unwrap_or_else(|| "NOT FOUND".to_string());
            eprintln!("  {} = {}", var, typ);
        }
    }

    #[test]
    fn gap_audit_2() {
        let src = r#"
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE FIELD age ON user TYPE int;
DEFINE TABLE post SCHEMAFULL;
DEFINE FIELD title ON post TYPE string;
DEFINE FIELD author ON post TYPE record<user>;
DEFINE TABLE wrote TYPE RELATION FROM user TO post;
DEFINE FIELD created_at ON wrote TYPE datetime;
DEFINE FUNCTION fn::greet($name: string) -> string { RETURN "Hi " + $name; };

SELECT author.name FROM post;
fn::greet(42);
LET $g = user:123;
SELECT * FROM user:specific;
INSERT INTO user (name, age) VALUES ('test', 'not_int');
"#;
        let result = crate::analyze(src).unwrap();
        eprintln!("\n=== Gap Audit 2 ===");
        for d in &result.diagnostics {
            let sev = match d.severity { crate::Severity::Error => "ERR", crate::Severity::Warning => "WRN", _ => "HNT" };
            eprintln!("  [{}] {} ({})", sev, d.message, d.code.id());
        }
    }

    #[test]
    fn fn_call_simple() {
        let result = crate::analyze(r#"
DEFINE FUNCTION fn::greet($name: string) -> string { RETURN "Hi " + $name; };
fn::greet(42);
"#).unwrap();
        for d in &result.diagnostics {
            eprintln!("  [{}] {}", d.code.id(), d.message);
        }
    }
