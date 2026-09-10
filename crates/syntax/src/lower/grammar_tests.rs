//! One test per grammar item the conformance work added: each query must
//! parse without an `ERROR`/`MISSING` node, and the lowered AST must have
//! the shape analysis reads. A parse error is fatal to the whole source, so
//! every item here used to make the analyzer wrong on valid SurrealQL.

use crate::ast::*;
use crate::lower::{lower_first_expr, lower_first_statement, lower_statements};
use crate::parse::{parse_source, ParsedSource};
use crate::source::SourceId;

fn parse(query: &str) -> ParsedSource {
    let parsed = parse_source(SourceId::new("grammar:test"), query).expect("parser returns a tree");
    assert!(
        !parsed.has_error(),
        "`{query}` must parse cleanly: {:?}",
        parsed.syntax_diagnostics()
    );
    parsed
}

fn parses(queries: &[&str]) {
    for query in queries {
        parse(query);
    }
}

fn expr(query: &str, cst_kind: &str) -> Expr {
    let parsed = parse(query);
    lower_first_expr(&parsed, cst_kind)
        .unwrap_or_else(|| panic!("no {cst_kind} in `{query}`"))
        .node
}

fn statement(query: &str, cst_kind: &str) -> Statement {
    let parsed = parse(query);
    lower_first_statement(&parsed, cst_kind)
        .unwrap_or_else(|| panic!("no {cst_kind} in `{query}`"))
        .node
}

fn binary_op(query: &str) -> BinaryOp {
    match expr(query, "BinaryExpression") {
        Expr::Binary { op, .. } => op.node,
        other => panic!("expected a binary expression for `{query}`, got {other:?}"),
    }
}

fn define(query: &str) -> DefineStmt {
    match statement(query, "DefineStatement") {
        Statement::Define(def) => def,
        other => panic!("expected a DEFINE for `{query}`, got {other:?}"),
    }
}

// ---- 1. `%`, `**` ------------------------------------------------------------

#[test]
fn modulo_and_power_have_their_own_operators() {
    assert_eq!(binary_op("RETURN 5 % 2;"), BinaryOp::Rem);
    // `**` binds tighter than `%`: `(2 ** 3) % 2`.
    let Expr::Binary { lhs, op, .. } = expr("RETURN 2 ** 3 % 2;", "BinaryExpression") else {
        panic!("expected a binary expression");
    };
    assert_eq!(op.node, BinaryOp::Rem);
    assert!(matches!(lhs.node, Expr::Binary { op, .. } if op.node == BinaryOp::Pow));
}

// ---- 2. prefix `NOT`, `-`, `+` on non-literals ------------------------------

#[test]
fn prefix_operators_apply_to_any_operand() {
    let cases = [
        ("RETURN NOT true;", PrefixOp::Not),
        ("RETURN not $x;", PrefixOp::Not),
        ("RETURN -$x;", PrefixOp::Neg),
        ("RETURN -(1 + 2);", PrefixOp::Neg),
        ("RETURN -a.b;", PrefixOp::Neg),
        ("RETURN +$x;", PrefixOp::Pos),
    ];
    for (query, expected) in cases {
        let Expr::Prefix { op, .. } = expr(query, "PrefixExpression") else {
            panic!("expected a prefix expression for `{query}`");
        };
        assert_eq!(op.node, expected, "`{query}`");
    }

    // `NOT a AND b` is `(NOT a) AND b`, as the engine reads it.
    let Expr::Binary { lhs, op, .. } = expr("RETURN NOT a AND b;", "BinaryExpression") else {
        panic!("expected a binary expression");
    };
    assert_eq!(op.node, BinaryOp::And);
    assert!(matches!(lhs.node, Expr::Prefix { .. }));

    // A sign directly on a literal is still the literal (`-5` is `Int(-5)`),
    // and a binary minus followed by a prefix minus is two operators.
    assert_eq!(
        expr("RETURN -5;", "Number"),
        Expr::Literal(Literal::Int(-5))
    );
    let Expr::Binary { op, rhs, .. } = expr("RETURN 1 - -$x;", "BinaryExpression") else {
        panic!("expected a binary expression");
    };
    assert_eq!(op.node, BinaryOp::Sub);
    assert!(matches!(rhs.node, Expr::Prefix { .. }));
}

// ---- 3. numeric suffixes ---------------------------------------------------------

#[test]
fn numeric_suffixes_and_separators_lower_to_their_kind_and_value() {
    assert_eq!(
        expr("RETURN 1.5dec;", "Number"),
        Expr::Literal(Literal::Decimal)
    );
    assert_eq!(
        expr("RETURN 9.7e-7dec;", "Number"),
        Expr::Literal(Literal::Decimal)
    );
    assert_eq!(
        expr("RETURN 1dec;", "Number"),
        Expr::Literal(Literal::Decimal)
    );
    assert_eq!(
        expr("RETURN 1.5f;", "Number"),
        Expr::Literal(Literal::Float(1.5))
    );
    assert_eq!(
        expr("RETURN 100000f;", "Number"),
        Expr::Literal(Literal::Float(100000.0))
    );
    assert_eq!(
        expr("RETURN 1_000;", "Number"),
        Expr::Literal(Literal::Int(1000))
    );
    // An integer literal past `i64` is a decimal-kinded value, not a zero.
    assert_eq!(
        expr("RETURN 1749284739243842973049283492847029475;", "Number"),
        Expr::Literal(Literal::Decimal)
    );
}

// ---- 4. string prefixes ----------------------------------------------------------

#[test]
fn string_prefixes_select_the_literal_kind() {
    assert_eq!(
        expr("RETURN s'abc';", "String"),
        Expr::Literal(Literal::String("abc".into()))
    );
    assert_eq!(
        expr("RETURN b'abc';", "String"),
        Expr::Literal(Literal::Bytes("abc".into()))
    );
    assert_eq!(
        expr("RETURN f'bucket:/a.txt';", "String"),
        Expr::Literal(Literal::File("bucket:/a.txt".into()))
    );
    // Backslash escapes stay inside the string.
    assert!(matches!(
        expr(r#"RETURN "a\"b";"#, "String"),
        Expr::Literal(Literal::String(s)) if s == r#"a\"b"#
    ));
    assert!(matches!(
        expr(r"RETURN 'a\'b';", "String"),
        Expr::Literal(Literal::String(s)) if s == r"a\'b"
    ));
}

// ---- 5. `?.` optional chaining ---------------------------------------------------

#[test]
fn optional_chaining_is_a_path_part() {
    let Expr::Idiom(idiom) = expr("RETURN a?.b;", "Path") else {
        panic!("expected an idiom");
    };
    let parts: Vec<_> = idiom.parts.iter().map(|p| p.node.clone()).collect();
    assert_eq!(
        parts,
        vec![
            IdiomPart::Field("a".into()),
            IdiomPart::Optional,
            IdiomPart::Field("b".into()),
        ]
    );
}

// ---- 6. bare `not()` / `sleep()` -----------------------------------------------

#[test]
fn not_and_sleep_are_callable_without_a_module() {
    let Expr::Call(call) = expr("RETURN not(true);", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.path.node, "not");
    assert_eq!(call.args.len(), 1);
    let Expr::Call(call) = expr("RETURN sleep(1s);", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.path.node, "sleep");
    // Two arguments stay a call, never `NOT` applied to a point.
    let Expr::Call(call) = expr("RETURN not(1, 2);", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.args.len(), 2);
}

// ---- 7. KILL / LIVE SELECT -------------------------------------------------------

#[test]
fn kill_takes_a_param_or_a_uuid() {
    let Statement::Kill(kill) = statement("KILL $id;", "KillStatement") else {
        panic!("expected KILL");
    };
    assert_eq!(kill.id.map(|id| id.node), Some(Expr::Param("id".into())));
    let Statement::Kill(kill) = statement("KILL u\"0189-aa\";", "KillStatement") else {
        panic!("expected KILL");
    };
    assert!(matches!(
        kill.id.map(|id| id.node),
        Some(Expr::Literal(Literal::Uuid(_)))
    ));
}

#[test]
fn live_select_lowers_its_projection_filter_and_fetch() {
    let Statement::LiveSelect(live) = statement(
        "LIVE SELECT name, age FROM person WHERE active = true FETCH profile;",
        "LiveSelectStatement",
    ) else {
        panic!("expected LIVE SELECT");
    };
    assert!(live.diff.is_none() && !live.value);
    assert_eq!(live.projections.len(), 2);
    assert_eq!(live.table().map(|t| t.node.as_str()), Some("person"));
    assert!(matches!(
        live.where_clause,
        Some(Spanned {
            node: Expr::Binary { .. },
            ..
        })
    ));
    assert_eq!(live.fetch.len(), 1);

    let Statement::LiveSelect(live) =
        statement("LIVE SELECT DIFF FROM $tb;", "LiveSelectStatement")
    else {
        panic!("expected LIVE SELECT");
    };
    assert!(live.diff.is_some());
    assert!(live.projections.is_empty());
    assert!(matches!(live.from[0].node, Expr::Param(ref p) if p == "tb"));
    assert!(live.table().is_none());

    let Statement::LiveSelect(live) =
        statement("LIVE SELECT VALUE name FROM person;", "LiveSelectStatement")
    else {
        panic!("expected LIVE SELECT");
    };
    assert!(live.value);
    assert_eq!(live.projections.len(), 1);
}

// ---- 8. WITH INDEX / NOINDEX -----------------------------------------------------

#[test]
fn index_hints_parse_on_every_row_statement() {
    parses(&[
        "SELECT * FROM t WITH INDEX a, b WHERE x = 1;",
        "SELECT * FROM t WITH NOINDEX;",
        "UPDATE t WITH NOINDEX SET a = 1 WHERE x = 1;",
        "UPSERT t WITH INDEX a SET a = 1;",
        "DELETE t WITH INDEX a WHERE x = 1;",
        "DELETE FROM ONLY t:1 WITH INDEX a WHERE x RETURN NULL EXPLAIN;",
    ]);
    // The hint does not displace the clauses around it.
    let Statement::Update(update) = statement(
        "UPDATE t WITH NOINDEX SET a = 1 WHERE x = 1;",
        "UpdateStatement",
    ) else {
        panic!("expected UPDATE");
    };
    assert!(matches!(update.data, Some(DataClause::Set(_))));
    assert!(update.where_clause.is_some());
}

// ---- 9. INSERT forms -------------------------------------------------------------

#[test]
fn insert_accepts_param_targets_subquery_payloads_and_duplicate_key_updates() {
    let Statement::Insert(insert) =
        statement("INSERT IGNORE INTO $tb { a: 1 };", "InsertStatement")
    else {
        panic!("expected INSERT");
    };
    assert!(insert.ignore);
    assert!(matches!(insert.target.map(|t| t.node), Some(Expr::Param(p)) if p == "tb"));

    let Statement::Insert(insert) =
        statement("INSERT INTO t (SELECT * FROM u);", "InsertStatement")
    else {
        panic!("expected INSERT");
    };
    let InsertData::Values(values) = &insert.data else {
        panic!("expected a values payload, got {:?}", insert.data);
    };
    assert!(matches!(values[0].node, Expr::Subquery(_)));

    // Both halves survive: the row payload AND the update assignments.
    let Statement::Insert(insert) = statement(
        "INSERT INTO t { a: 1 } ON DUPLICATE KEY UPDATE b = 2, c += 1, d.e +?= 3;",
        "InsertStatement",
    ) else {
        panic!("expected INSERT");
    };
    let InsertData::Values(values) = &insert.data else {
        panic!("expected a values payload, got {:?}", insert.data);
    };
    assert!(matches!(values[0].node, Expr::Object(_)));
    assert_eq!(insert.on_duplicate_update.len(), 3);
    assert_eq!(insert.on_duplicate_update[0].op.node, AssignOp::Assign);
    assert_eq!(insert.on_duplicate_update[1].op.node, AssignOp::Add);
    assert_eq!(insert.on_duplicate_update[2].op.node, AssignOp::Extend);
    assert_eq!(insert.on_duplicate_update[2].target.node.parts.len(), 2);
}

// ---- 10. `array<T, N>` -----------------------------------------------------------

#[test]
fn bounded_collection_types_keep_their_element_type() {
    let DefineStmt::Function(function) =
        define("DEFINE FUNCTION fn::f($a: array<bool, 3>, $b: set<int,2>) { RETURN 1 };")
    else {
        panic!("expected DEFINE FUNCTION");
    };
    for (param, element) in [(0, "bool"), (1, "int")] {
        let Some(TypeExpr::Parameterized { args, .. }) =
            function.params[param].1.as_ref().map(|t| &t.node)
        else {
            panic!("expected a parameterized type");
        };
        assert_eq!(args.len(), 1, "the length bound is not a type argument");
        assert!(matches!(&args[0].node, TypeExpr::Name(n) if n.node == element));
    }
    let DefineStmt::Field(field) = define("DEFINE FIELD a ON t TYPE array<string, 5>;") else {
        panic!("expected DEFINE FIELD");
    };
    assert!(matches!(
        field.ty.map(|t| t.node),
        Some(TypeExpr::Parameterized { args, .. }) if args.len() == 1
    ));
}

// ---- 11. COMMENT clauses, NS/DB short forms --------------------------------------

#[test]
fn comment_clauses_and_short_define_forms_parse() {
    parses(&[
        "DEFINE TABLE t COMMENT 'x';",
        "DEFINE FIELD a ON t TYPE int COMMENT 'x';",
        "DEFINE INDEX i ON t FIELDS a COMMENT 'x';",
        "DEFINE EVENT e ON t WHEN true THEN (RETURN 1) COMMENT 'x';",
        "DEFINE PARAM $p VALUE 1 COMMENT 'x';",
        "DEFINE ANALYZER a TOKENIZERS blank COMMENT 'x';",
        "DEFINE FUNCTION fn::f() { RETURN 1 } COMMENT 'x' PERMISSIONS FULL;",
        "DEFINE USER u ON ROOT COMMENT 'c' PASSHASH 'h' ROLES editor, OWNER DURATION FOR TOKEN 15m, FOR SESSION NONE;",
        "DEFINE NS n; DEFINE DB d; DEFINE NAMESPACE n2 COMMENT 'x'; DEFINE DATABASE d2 COMMENT 'x';",
        "REMOVE NS n; REMOVE DB d; REMOVE USER u ON NS; REMOVE ACCESS a ON DB; REMOVE FUNCTION fn::f();",
    ]);
    assert!(matches!(define("DEFINE NS n;"), DefineStmt::Other(_)));
    assert!(matches!(define("DEFINE DB d;"), DefineStmt::Other(_)));
    let DefineStmt::Table(table) = define("DEFINE TABLE t SCHEMAFULL COMMENT 'x';") else {
        panic!("expected DEFINE TABLE");
    };
    assert!(table.schemafull);
}

// ---- 12. constants, comments -------------------------------------------------------

#[test]
fn constants_lower_case_folded() {
    assert!(matches!(
        expr("RETURN MaTh::Pi;", "Constant"),
        Expr::Constant(path) if path.node == "math::pi"
    ));
    assert!(matches!(
        expr("RETURN time::EPOCH;", "Constant"),
        Expr::Constant(path) if path.node == "time::epoch"
    ));
    // A bare constant is a statement of its own, and a comparison operand.
    let statements = lower_statements(&parse("math::pi;"));
    assert!(
        matches!(&statements[0].node, Statement::Expr(e) if matches!(e.node, Expr::Constant(_)))
    );
    assert_eq!(binary_op("RETURN math::PI > 3.14;"), BinaryOp::Gt);
    // A real call is still a call.
    assert!(matches!(
        expr("RETURN math::abs(1);", "FunctionCall"),
        Expr::Call(_)
    ));
}

#[test]
fn every_comment_syntax_is_an_extra() {
    parses(&[
        "-- c\nRETURN 1;",
        "// c\nRETURN 1;",
        "# c\nRETURN 1;",
        "/* c */ RETURN 1;",
        "// leading comment then a bare expression\n3 * 5 = 15;",
    ]);
}

// ---- 13. index options, sequences, ratelimits, events, filtered FETCH --------------

#[test]
fn vector_index_options_lower_to_the_vector_kind() {
    for query in [
        "DEFINE INDEX i ON t FIELDS v HNSW DIMENSION 3 DISTANCE MANHATTAN;",
        "DEFINE INDEX i ON t FIELDS v HNSW DIMENSION 128 EFC 250 TYPE F32 DISTANCE COSINE M 6 M0 12 LM 0.5 EXTEND_CANDIDATES KEEP_PRUNED_CONNECTIONS HASHED_VECTOR;",
        "DEFINE INDEX i ON t FIELDS v DISKANN DIMENSION 128;",
        "DEFINE INDEX i ON t FIELDS v DISKANN DIMENSION 128 DEGREE 32 L_BUILD 88 ALPHA 1.4 TYPE F16 DISTANCE COSINE_NORMALIZED;",
        "DEFINE INDEX i ON t FIELDS v MTREE DIMENSION 4 DISTANCE EUCLIDEAN;",
    ] {
        let DefineStmt::Index(index) = define(query) else {
            panic!("expected DEFINE INDEX for `{query}`");
        };
        assert_eq!(index.kind, IndexKind::Vector, "`{query}`");
    }
    let DefineStmt::Index(index) =
        define("DEFINE INDEX i ON t FIELDS body FULLTEXT ANALYZER ascii BM25 HIGHLIGHTS;")
    else {
        panic!("expected DEFINE INDEX");
    };
    assert_eq!(index.kind, IndexKind::Search);
}

#[test]
fn sequences_ratelimits_and_event_options_parse() {
    parses(&[
        "DEFINE SEQUENCE sq;",
        "DEFINE SEQUENCE sq1 START -250; DEFINE SEQUENCE sq2 BATCH 50; DEFINE SEQUENCE sq3 BATCH 10 START 1000;",
        "REMOVE SEQUENCE sq;",
        "DEFINE TABLE post RATELIMIT FOR SELECT WHERE $auth IS NONE BY $session.ip LIMIT 2 PER 1s MAX 4;",
        "DEFINE TABLE knows RATELIMIT FOR SELECT BY $auth.id LIMIT 100 PER 1m, FOR UPDATE BY $auth.id LIMIT 100 PER 1h;",
        "DEFINE FIELD name ON post RATELIMIT FOR DELETE BY $auth.id LIMIT 1 PER 1s;",
        "DEFINE EVENT e ON t WHEN null THEN null, none ASYNC RETRY 5 MAXDEPTH 64;",
        "DEFINE EVENT e ON TABLE tb WHEN true THEN RETURN 'foo';",
        "DEFINE EVENT e ON t THEN { RETURN 1 } ASYNC RETRY 3;",
        "DEFINE TABLE t DROP SCHEMAFUL CHANGEFEED 1s INCLUDE ORIGINAL PERMISSIONS FOR DELETE FULL, FOR SELECT WHERE a = 1;",
    ]);
    assert!(matches!(
        define("DEFINE SEQUENCE sq;"),
        DefineStmt::Other(_)
    ));
    let DefineStmt::Event(event) =
        define("DEFINE EVENT e ON TABLE tb WHEN true THEN RETURN 'foo';")
    else {
        panic!("expected DEFINE EVENT");
    };
    assert!(matches!(
        event.then.map(|t| t.node),
        Some(Expr::Literal(Literal::String(s))) if s == "foo"
    ));
}

#[test]
fn fetch_accepts_a_filtered_path() {
    let Statement::Select(select) = statement(
        "SELECT * FROM t FETCH a[WHERE b = 1], c;",
        "SelectStatement",
    ) else {
        panic!("expected SELECT");
    };
    assert_eq!(select.fetch.len(), 2);
    assert!(matches!(
        select.fetch[0].node.parts[1].node,
        IdiomPart::Where(_)
    ));
}

/// `FETCH RETURN` fetches the field named `RETURN`: the keyword root arrives
/// as a `Keyword` node and lowers to an ordinary field, on RETURN's own FETCH
/// as much as on SELECT's.
#[test]
fn fetch_accepts_a_keyword_named_root() {
    let Statement::Select(select) =
        statement("SELECT * FROM t FETCH RETURN, a.b;", "SelectStatement")
    else {
        panic!("expected SELECT");
    };
    assert_eq!(select.fetch.len(), 2);
    assert!(matches!(
        &select.fetch[0].node.parts[0].node,
        IdiomPart::Field(name) if name == "RETURN"
    ));
    assert!(matches!(
        &select.fetch[1].node.parts[..],
        [a, b] if matches!(&a.node, IdiomPart::Field(x) if x == "a")
            && matches!(&b.node, IdiomPart::Field(y) if y == "b")
    ));

    // RETURN carries a FETCH of its own; the AST does not model it yet, but
    // the statement must parse and its value must still lower.
    let Statement::Return(ret) = statement("RETURN RETRUN FETCH RETURN;", "ReturnStatement") else {
        panic!("expected RETURN");
    };
    assert!(ret.value.is_some());
}

// ---- 14. access / users / info ---------------------------------------------------------

#[test]
fn access_and_user_definitions_parse_as_unmodeled_defines() {
    for query in [
        "DEFINE ACCESS a ON DATABASE TYPE JWT ALGORITHM HS256 KEY \"foo\" DURATION FOR TOKEN 10s;",
        "DEFINE ACCESS a ON DB TYPE RECORD SIGNUP (CREATE user SET email = $email) SIGNIN (SELECT * FROM user WHERE email = $email) WITH JWT ALGORITHM HS512 KEY 'k' WITH ISSUER KEY 'i' DURATION FOR TOKEN 15m, FOR SESSION 12h;",
        "DEFINE ACCESS a ON DB TYPE RECORD WITH REFRESH DURATION FOR GRANT 10d;",
        "DEFINE ACCESS a ON NS TYPE BEARER FOR USER COMMENT \"foo\";",
        "DEFINE ACCESS a ON DATABASE TYPE JWT URL \"http://example.com/.well-known/jwks.json\" WITH ISSUER ALGORITHM HS384 KEY \"foo\";",
        "DEFINE USER u ON DB PASSHASH 'secret' ROLES VIEWER;",
    ] {
        assert!(matches!(define(query), DefineStmt::Other(_)), "`{query}`");
    }
    parses(&[
        "ACCESS a ON NAMESPACE GRANT FOR USER b;",
        "ACCESS a ON NAMESPACE GRANT FOR RECORD b:c;",
        "ACCESS a ON DATABASE SHOW ALL; ACCESS a ON DATABASE SHOW GRANT b; ACCESS a ON DATABASE SHOW WHERE true;",
        "ACCESS a ON DATABASE REVOKE ALL; ACCESS a ON DATABASE REVOKE WHERE true;",
        "ACCESS a ON DATABASE PURGE EXPIRED, REVOKED FOR 90d;",
        "INFO FOR USER u ON ROOT; INFO FOR USER u; INFO FOR INDEX i ON t;",
        "ALTER INDEX test ON user PREPARE REMOVE;",
        "SHOW CHANGES FOR DATABASE SINCE d\"2012-04-23T18:25:43.0000511Z\";",
    ]);
    // An ACCESS statement is outside the modeled tier: explicit, never silent.
    let statements = lower_statements(&parse("ACCESS a ON DATABASE SHOW ALL;"));
    assert!(matches!(
        &statements[0].node,
        Statement::Partial(partial) if partial.cst_kind == "AccessStatement"
    ));
}

// ---- lowering: operators, flags, ranges, aliases ----------------------------------------

#[test]
fn every_operator_has_its_own_variant() {
    let cases = [
        ("RETURN a = b;", BinaryOp::Eq),
        ("RETURN a == b;", BinaryOp::Exact),
        ("RETURN a IS b;", BinaryOp::Is),
        ("RETURN a IS NOT b;", BinaryOp::IsNot),
        ("RETURN a IN b;", BinaryOp::In),
        ("RETURN a NOT IN b;", BinaryOp::NotIn),
        ("RETURN a CONTAINS b;", BinaryOp::Contains),
        ("RETURN a CONTAINSNOT b;", BinaryOp::ContainsNot),
        ("RETURN a CONTAINSALL b;", BinaryOp::ContainsAll),
        ("RETURN a CONTAINSANY b;", BinaryOp::ContainsAny),
        ("RETURN a CONTAINSNONE b;", BinaryOp::ContainsNone),
        ("RETURN a INSIDE b;", BinaryOp::Inside),
        ("RETURN a NOTINSIDE b;", BinaryOp::NotInside),
        ("RETURN a ALLINSIDE b;", BinaryOp::AllInside),
        ("RETURN a ANYINSIDE b;", BinaryOp::AnyInside),
        ("RETURN a NONEINSIDE b;", BinaryOp::NoneInside),
        ("RETURN a OUTSIDE b;", BinaryOp::Outside),
        ("RETURN a INTERSECTS b;", BinaryOp::Intersects),
        ("RETURN a ~ b;", BinaryOp::Match),
        ("RETURN a !~ b;", BinaryOp::NotMatch),
        ("RETURN a *~ b;", BinaryOp::AllMatch),
        ("RETURN a ?~ b;", BinaryOp::AnyMatch),
        ("RETURN a ?= b;", BinaryOp::AnyEq),
        ("RETURN a *= b;", BinaryOp::AllEq),
        ("RETURN a @@ b;", BinaryOp::Matches(None)),
        ("RETURN a @1@ b;", BinaryOp::Matches(Some(1))),
        ("RETURN a ?: b;", BinaryOp::TruthyCoalesce),
        ("RETURN a ?? b;", BinaryOp::NullCoalesce),
        ("RETURN a ∈ b;", BinaryOp::Inside),
        ("RETURN a ∋ b;", BinaryOp::Contains),
        ("RETURN a × b;", BinaryOp::Mul),
        (
            "RETURN a <|3|> b;",
            BinaryOp::Knn(Knn {
                k: Some(3),
                ef: None,
                distance: None,
            }),
        ),
        (
            "RETURN a <|3, 40|> b;",
            BinaryOp::Knn(Knn {
                k: Some(3),
                ef: Some(40),
                distance: None,
            }),
        ),
        (
            "RETURN a <|3, COSINE|> b;",
            BinaryOp::Knn(Knn {
                k: Some(3),
                ef: None,
                distance: Some("COSINE".into()),
            }),
        ),
    ];
    for (query, expected) in cases {
        assert_eq!(binary_op(query), expected, "`{query}`");
    }
}

#[test]
fn if_not_exists_and_overwrite_flags_reach_every_define() {
    let DefineStmt::Table(table) = define("DEFINE TABLE IF NOT EXISTS t;") else {
        panic!("expected DEFINE TABLE");
    };
    assert!(table.if_not_exists && !table.overwrite);
    let DefineStmt::Field(field) = define("DEFINE FIELD IF NOT EXISTS a ON t TYPE int;") else {
        panic!("expected DEFINE FIELD");
    };
    assert!(field.if_not_exists && !field.overwrite);
    let DefineStmt::Index(index) = define("DEFINE INDEX OVERWRITE i ON t FIELDS a;") else {
        panic!("expected DEFINE INDEX");
    };
    assert!(index.overwrite && !index.if_not_exists);
    let DefineStmt::Event(event) =
        define("DEFINE EVENT IF NOT EXISTS e ON t WHEN true THEN (RETURN 1);")
    else {
        panic!("expected DEFINE EVENT");
    };
    assert!(event.if_not_exists);
    let DefineStmt::Param(param) = define("DEFINE PARAM OVERWRITE $p VALUE 1;") else {
        panic!("expected DEFINE PARAM");
    };
    assert!(param.overwrite);
    let DefineStmt::Function(function) =
        define("DEFINE FUNCTION IF NOT EXISTS fn::f() { RETURN 1 };")
    else {
        panic!("expected DEFINE FUNCTION");
    };
    assert!(function.if_not_exists);
    let DefineStmt::Analyzer(analyzer) = define("DEFINE ANALYZER OVERWRITE a TOKENIZERS blank;")
    else {
        panic!("expected DEFINE ANALYZER");
    };
    assert!(analyzer.overwrite);
}

#[test]
fn ranges_lower_to_range_expressions_and_record_ranges_stay_flagged() {
    let Expr::Range(range) = expr("RETURN 1..5;", "Range") else {
        panic!("expected a range");
    };
    assert!(range.start.is_some() && range.end.is_some());
    assert!(!range.start_exclusive && !range.end_inclusive);
    let Expr::Range(range) = expr("RETURN 1>..=5;", "Range") else {
        panic!("expected a range");
    };
    assert!(range.start_exclusive && range.end_inclusive);
    let Expr::Range(range) = expr("RETURN ..5;", "Range") else {
        panic!("expected a range");
    };
    assert!(range.start.is_none());

    assert!(matches!(
        expr("RETURN t:1..5;", "RecordId"),
        Expr::RecordId { range: true, .. }
    ));
    assert!(matches!(
        expr("SELECT * FROM |t:10|;", "RangeRecordId"),
        Expr::RecordId { range: true, table, .. } if table.node == "t"
    ));
}

#[test]
fn a_graph_step_alias_is_kept() {
    let Expr::Idiom(idiom) = expr("SELECT ->(likes AS liked)->post FROM person;", "Path") else {
        panic!("expected an idiom");
    };
    let IdiomPart::Graph { step, .. } = &idiom.parts[0].node else {
        panic!("expected a graph step");
    };
    assert_eq!(step.targets[0].node, "likes");
    assert_eq!(step.alias.as_ref().map(|a| a.node.as_str()), Some("liked"));
    let IdiomPart::Graph { step, .. } = &idiom.parts[1].node else {
        panic!("expected a graph step");
    };
    assert!(step.alias.is_none());
}

#[test]
fn an_if_is_a_value_in_a_projection_and_a_condition() {
    let Statement::Select(select) = statement(
        "SELECT if true { 'Yay' } else { 'Oops' } AS if_else FROM t WHERE IF true THEN 'YAY' ELSE 'OOPS' END AND a = 1;",
        "SelectStatement",
    ) else {
        panic!("expected SELECT");
    };
    let Projection::Expr { expr, alias } = &select.projections[0] else {
        panic!("expected a projection");
    };
    assert!(matches!(expr.node, Expr::Subquery(_)));
    assert_eq!(alias.as_ref().map(|a| a.node.as_str()), Some("if_else"));
    assert!(select.where_clause.is_some());
}

#[test]
fn flatten_and_path_assignment_targets_lower() {
    let Statement::Update(update) = statement("UPDATE t UNSET c..., d;", "UpdateStatement") else {
        panic!("expected UPDATE");
    };
    let Some(DataClause::Unset(paths)) = update.data else {
        panic!("expected UNSET, got {:?}", update.data);
    };
    assert!(matches!(paths[0].node.parts[1].node, IdiomPart::Flatten));
    let Statement::Update(update) = statement("UPDATE t SET a.b += 1;", "UpdateStatement") else {
        panic!("expected UPDATE");
    };
    let Some(DataClause::Set(assignments)) = update.data else {
        panic!("expected SET");
    };
    assert_eq!(assignments[0].target.node.parts.len(), 2);
}

// ---- corpus findings: record-id strings, NOT (x), casts, sources, EXPLAIN, calls ----

fn cast(query: &str) -> (TypeExpr, Expr) {
    match expr(query, "TypeCast") {
        Expr::Cast { ty, expr } => (ty.node, expr.node),
        other => panic!("expected a cast for `{query}`, got {other:?}"),
    }
}

fn select(query: &str) -> SelectStmt {
    match statement(query, "SelectStatement") {
        Statement::Select(stmt) => stmt,
        other => panic!("expected a SELECT for `{query}`, got {other:?}"),
    }
}

#[test]
fn an_r_prefixed_string_is_a_record_id_not_a_regex() {
    // `type::of(r'account:ada')` is `record` on 3.2.3; regex literals are `/…/`.
    let query = "RETURN r'account:ada';";
    let Expr::RecordId { table, id, range } = expr(query, "String") else {
        panic!("expected a record id");
    };
    assert_eq!(table.node, "account");
    assert!(!range);
    // Both halves are spanned inside the quotes: `r'` is two bytes.
    assert_eq!(
        &query[table.span.start() as usize..table.span.end() as usize],
        "account"
    );
    assert_eq!(&query[id.start() as usize..id.end() as usize], "ada");

    let Expr::RecordId { table, id, .. } = expr("RETURN r\"ticket:1\";", "String") else {
        panic!("expected a record id");
    };
    assert_eq!(table.node, "ticket");
    assert_eq!(id.len(), 1);

    // Without a `table:id` shape the engine reports a parse error
    // (`r'notarecord'`, `r'a:'`, `r':b'`); nothing here is a string.
    for query in ["RETURN r'notarecord';", "RETURN r'a:';", "RETURN r':b';"] {
        assert!(
            matches!(expr(query, "String"), Expr::Partial(_)),
            "`{query}` must not lower to a value"
        );
    }
    // A real regex still is one.
    assert_eq!(
        expr("RETURN /ab+/;", "Regex"),
        Expr::Literal(Literal::Regex("ab+".into()))
    );
}

#[test]
fn builtin_function_names_fold_to_lowercase_and_custom_ones_keep_their_case() {
    // `NOT (x)` lexes as a call to a function named `NOT`, which IS the `not`
    // builtin on the engine (`NOT true` without parentheses is a parse error
    // on 3.2.3, so there is no prefix operator to prefer).
    for query in [
        "RETURN NOT (true);",
        "RETURN not(true);",
        "RETURN NOT(1 = 1);",
    ] {
        let Expr::Call(call) = expr(query, "FunctionCall") else {
            panic!("expected a call for `{query}`");
        };
        assert_eq!(call.path.node, "not");
        assert_eq!(call.args.len(), 1);
    }
    let Expr::Call(call) = expr("RETURN STRING::LEN('a');", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.path.node, "string::len");
    let Expr::Call(call) = expr("RETURN Type::IS::Record(1);", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.path.node, "type::is_record");
    // `fn::Foo` and `fn::foo` are different functions on the engine.
    let Expr::Call(call) = expr("RETURN fn::Foo();", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.path.node, "fn::Foo");
}

#[test]
fn parameterized_cast_targets_lower_as_type_expressions() {
    let (ty, value) = cast("RETURN <set<int>> [1, 1];");
    let TypeExpr::Parameterized { name, args } = ty else {
        panic!("expected set<int>, got {ty:?}");
    };
    assert_eq!(name.node, "set");
    assert!(matches!(&args[0].node, TypeExpr::Name(n) if n.node == "int"));
    assert!(matches!(value, Expr::Array(_)));

    let (ty, _) = cast("RETURN <option<int>> NONE;");
    assert!(
        matches!(ty, TypeExpr::Optional(inner) if matches!(&inner.node, TypeExpr::Name(n) if n.node == "int"))
    );

    let (ty, _) = cast("RETURN <record<article>> 'article:hello';");
    assert!(matches!(&ty, TypeExpr::Parameterized { name, .. } if name.node == "record"));

    let (ty, _) = cast("RETURN <int | string> 5;");
    assert!(matches!(&ty, TypeExpr::Union(variants) if variants.len() == 2));

    let (ty, value) = cast("RETURN <geometry<point>> (1.0, 2.0);");
    assert!(matches!(&ty, TypeExpr::Parameterized { name, .. } if name.node == "geometry"));
    // The point literal itself has no lowering yet: an explicit `Partial`.
    assert!(matches!(value, Expr::Partial(ref p) if p.cst_kind == "Point"));

    let (ty, _) = cast("RETURN <array<string>> [1, 2];");
    assert!(
        matches!(&ty, TypeExpr::Parameterized { name, args } if name.node == "array" && args.len() == 1)
    );

    // The plain form is unchanged.
    let (ty, _) = cast("RETURN <int> '1';");
    assert!(matches!(&ty, TypeExpr::Name(n) if n.node == "int"));
}

#[test]
fn a_cast_reaches_a_range_a_path_and_a_prefix_but_not_a_binary_operator() {
    // 3.2.3: `<array> 1..5` is `[1, 2, 3, 4]`, `<string> $x.y` casts the
    // field, `<string> -$x.y` casts the negation, and `<string> 1 + 2` fails
    // with "Cannot perform addition with 'string' and 'int'" — so the cast
    // is `(<string> 1) + 2`.
    let (_, value) = cast("RETURN <array> 1..5;");
    assert!(matches!(value, Expr::Range(_)), "got {value:?}");
    let (_, value) = cast("RETURN <array> 1>..=5;");
    assert!(matches!(
        value,
        Expr::Range(Range {
            start_exclusive: true,
            end_inclusive: true,
            ..
        })
    ));

    let (_, value) = cast("RETURN <string> $x.y;");
    assert!(matches!(value, Expr::Idiom(_)), "got {value:?}");
    let (_, value) = cast("RETURN <string> $x.z[0];");
    assert!(matches!(value, Expr::Idiom(_)), "got {value:?}");

    let (_, value) = cast("RETURN <string> -$x;");
    assert!(matches!(value, Expr::Prefix { .. }), "got {value:?}");
    let (_, value) = cast("RETURN <string> !true;");
    assert!(matches!(value, Expr::Prefix { .. }), "got {value:?}");

    match expr("RETURN <string> 1 + 2;", "BinaryExpression") {
        Expr::Binary { lhs, rhs, .. } => {
            assert!(matches!(lhs.node, Expr::Cast { .. }));
            assert_eq!(rhs.node, Expr::Literal(Literal::Int(2)));
        }
        other => panic!("expected a binary expression, got {other:?}"),
    }
    match expr("RETURN <int> 1 IN [1];", "BinaryExpression") {
        Expr::Binary { lhs, .. } => assert!(matches!(lhs.node, Expr::Cast { .. })),
        other => panic!("expected a binary expression, got {other:?}"),
    }

    // Casts nest, and a cast is a prefix operand.
    let (_, value) = cast("RETURN <int> <string> 1;");
    assert!(matches!(value, Expr::Cast { .. }));
    match expr("RETURN -<int> 1;", "PrefixExpression") {
        Expr::Prefix { expr, .. } => assert!(matches!(expr.node, Expr::Cast { .. })),
        other => panic!("expected a prefix expression, got {other:?}"),
    }
    // Everywhere a cast already appeared keeps parsing.
    parses(&[
        "RETURN <future> { 1 };",
        "SELECT <string> views AS v FROM a;",
        "UPDATE a SET x = <int> $y;",
        "RETURN array::len(<array> (1..5));",
        "RETURN <string> (1, 2)..(3, 4);",
    ]);
}

#[test]
fn an_array_a_subquery_and_a_param_are_select_sources() {
    let stmt = select("SELECT title FROM [article:hello, article:world];");
    assert_eq!(stmt.from.len(), 1);
    let Expr::Array(items) = &stmt.from[0].node else {
        panic!("expected an array source, got {:?}", stmt.from[0].node);
    };
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .all(|item| matches!(&item.node, Expr::RecordId { table, .. } if table.node == "article")));

    let stmt = select("SELECT * FROM (SELECT * FROM a);");
    assert!(matches!(stmt.from[0].node, Expr::Subquery(_)));
    let stmt = select("SELECT * FROM $p;");
    assert!(matches!(&stmt.from[0].node, Expr::Param(p) if p == "p"));
    let stmt = select("SELECT * FROM [a:1, a:2], a;");
    assert_eq!(stmt.from.len(), 2);

    // Other statements share the source predicate.
    let Statement::Update(update) = statement("UPDATE [a:1, a:2] SET x = 1;", "UpdateStatement")
    else {
        panic!("expected UPDATE");
    };
    assert!(matches!(update.targets[0].node, Expr::Array(_)));
    // …but a payload array after the target is not a second target.
    let Statement::Insert(insert) =
        statement("INSERT INTO t [{ a: 1 }, { a: 2 }];", "InsertStatement")
    else {
        panic!("expected INSERT");
    };
    assert!(
        matches!(&insert.target.as_ref().map(|t| &t.node), Some(Expr::Table(t)) if t.node == "t")
    );
}

#[test]
fn explain_is_accepted_as_a_prefix_and_as_the_trailing_clause() {
    // 3.2.3 accepts `EXPLAIN SELECT …` and `SELECT … EXPLAIN [FULL]`;
    // `EXPLAIN FULL SELECT …` is a parse error there, so it stays one here.
    let stmt = select("EXPLAIN SELECT title FROM article WHERE slug = 'hello';");
    assert!(stmt.explain.is_some());
    assert!(matches!(&stmt.from[0].node, Expr::Table(t) if t.node == "article"));
    assert!(stmt.where_clause.is_some());
    let stmt = select("SELECT title FROM article EXPLAIN FULL;");
    assert!(stmt.explain.is_some());
    let stmt = select("SELECT title FROM article;");
    assert!(stmt.explain.is_none());
    parses(&[
        "RETURN EXPLAIN SELECT * FROM a;",
        "LET $p = (EXPLAIN SELECT * FROM a);",
    ]);
    let parsed = parse_source(
        SourceId::new("grammar:test"),
        "EXPLAIN FULL SELECT * FROM a;",
    )
    .expect("tree");
    assert!(parsed.has_error());
}

#[test]
fn a_parenthesized_value_followed_by_arguments_is_a_call() {
    // `(|$x| $x + 1)(41)` is 42 on 3.2.3; `(1 + 2)(3)` parses there and
    // fails at run time. Like `$f(41)`, the callee is not a named function,
    // so the call carries an empty path and the analyzer yields `any`.
    let Expr::Call(call) = expr("RETURN (|$x| $x + 1)(41);", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.path.node, "");
    assert_eq!(call.args.len(), 1);
    assert_eq!(call.args[0].node, Expr::Literal(Literal::Int(41)));
    let Expr::Call(call) = expr("RETURN (1 + 2)(3);", "FunctionCall") else {
        panic!("expected a call");
    };
    assert_eq!(call.args.len(), 1);
    // Plain grouping is still grouping.
    assert!(matches!(
        expr("SELECT * FROM a WHERE (x) = 1;", "BinaryExpression"),
        Expr::Binary { .. }
    ));
}
