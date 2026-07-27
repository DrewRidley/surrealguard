//! `query!` yields a `Query<T>` whose nameless `T` decodes real SurrealDB
//! values.
//!
//! These exercise the typing and decoding halves without a database, by
//! handing [`Query::decode`] the `Value`s a server would have sent — one per
//! top-level statement. `tests/execution.rs` covers the same ground against a
//! live server.

use surrealguard_rs::surrealdb_types::{
    Datetime, Decimal, Duration, Number, Object, RecordId, SurrealValue, Value,
};
use surrealguard_rs::{query, query_file, surql, Rows};

/// One statement's worth of response.
fn one(value: Value) -> Vec<Value> {
    vec![value]
}

fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = Object::new();
    for (key, value) in fields {
        object.insert(key, value);
    }
    Value::Object(object)
}

#[test]
fn object_result_decodes_into_a_nameless_typed_struct() {
    let q = query!("RETURN { name: 'ada', age: 42 };");
    let row = q
        .decode(one(object([
            ("name", Value::String("ada".into())),
            ("age", 42i64.into_value()),
        ])))
        .unwrap();
    // The type has no name we can write — assert its fields' types and values.
    let name: String = row.name;
    let age: i64 = row.age;
    assert_eq!(name, "ada");
    assert_eq!(age, 42);
}

#[test]
fn nested_objects_and_arrays_compose() {
    let q = query!("RETURN { user: { name: 'ada' }, tags: ['a', 'b'] };");
    let row = q
        .decode(one(object([
            ("user", object([("name", Value::String("ada".into()))])),
            (
                "tags",
                vec![
                    Value::String("a".into()),
                    Value::String("b".into()),
                ]
                .into_value(),
            ),
        ])))
        .unwrap();
    let nested_name: String = row.user.name;
    let tags: Vec<String> = row.tags;
    assert_eq!(nested_name, "ada");
    assert_eq!(tags, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn array_result_is_a_vec_of_the_nameless_row() {
    let q = query!("RETURN [{ city: 'london' }];");
    let rows = q
        .decode(one(
            vec![object([("city", Value::String("london".into()))])].into_value(),
        ))
        .unwrap();
    assert_eq!(rows.len(), 1);
    let city: String = rows[0].city.clone();
    assert_eq!(city, "london");
}

#[test]
fn the_query_text_is_carried_verbatim() {
    let q = query!("RETURN 1 + 1;");
    assert_eq!(q.text(), "RETURN 1 + 1;");
    assert_eq!(q.statements(), 1);
}

#[test]
fn surql_still_expands_to_validated_text() {
    let text: &str = surql!("RETURN 1 + 1;");
    assert_eq!(text, "RETURN 1 + 1;");
}

// ---------------------------------------------------------------------------
// Decode types: each generated field must be the exact type the SDK produces.
// The SDK coerces nothing, so an approximate mapping is a runtime failure.
// ---------------------------------------------------------------------------

#[test]
fn a_record_link_decodes_as_a_record_id_not_a_string() {
    let q = query!("SELECT id FROM user;");
    let id = RecordId::new("user", "ada");
    let rows = q
        .decode(one(vec![object([("id", id.clone().into_value())])].into_value()))
        .unwrap();
    // Typed as `RecordId`; decoding this same value into a `String` is the bug
    // `a_record_id_does_not_decode_as_a_string` pins.
    let got: RecordId = rows[0].id.clone();
    assert_eq!(got, id);
}

#[test]
fn a_record_id_does_not_decode_as_a_string() {
    // The reason `record<t>` maps to `RecordId`: the SDK's `String` conversion
    // accepts `Value::String` and nothing else.
    let value = RecordId::new("user", "ada").into_value();
    let error = String::from_value(value).unwrap_err();
    assert!(
        error.message().contains("Expected string"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_duration_decodes_as_a_duration_not_a_string() {
    let q = query!("SELECT ttl FROM user;");
    let ttl = Duration::from_secs(3600);
    let rows = q
        .decode(one(
            vec![object([("ttl", ttl.into_value())])].into_value()
        ))
        .unwrap();
    let got: Duration = rows[0].ttl;
    assert_eq!(got, ttl);
}

#[test]
fn a_decimal_decodes_as_a_decimal_not_an_f64() {
    let q = query!("SELECT score FROM user;");
    let score = Decimal::new(15, 1);
    let rows = q
        .decode(one(
            vec![object([("score", score.into_value())])].into_value(),
        ))
        .unwrap();
    let got: Decimal = rows[0].score;
    assert_eq!(got, score);
    // And the reason: a decimal is not a float to the SDK.
    assert!(f64::from_value(score.into_value()).is_err());
}

#[test]
fn a_datetime_decodes_as_a_datetime() {
    let q = query!("SELECT created FROM user;");
    let created = Datetime::default();
    let rows = q
        .decode(one(
            vec![object([("created", created.into_value())])].into_value(),
        ))
        .unwrap();
    let got: Datetime = rows[0].created;
    assert_eq!(got, created);
}

#[test]
fn an_unrefined_number_decodes_as_number_so_every_variant_fits() {
    // `math::mean` is the unrefined one: it returns a float over an int column
    // (`math::mean([1,2])` -> 1.5 on 3.0.5), so `number` is the honest kind.
    // `math::sum([1,2])` is NOT — it is an `int`, and typing it `Number` would
    // hide that from the caller.
    let q = query!("RETURN { n: math::mean([1, 2]) };");
    // Whatever numeric variant the server picks, the field accepts it.
    let row = q
        .decode(one(object([("n", Number::Int(3).into_value())])))
        .unwrap();
    let got: Number = row.n;
    assert_eq!(got, Number::Int(3));
}

#[test]
fn an_optional_field_decodes_from_an_absent_key() {
    let q = query!("SELECT nickname FROM user;");
    // The server omits a NONE-valued field entirely.
    let rows = q.decode(one(vec![object([])].into_value())).unwrap();
    let got: Option<String> = rows[0].nickname.clone();
    assert_eq!(got, None);
}

#[test]
fn an_optional_field_decodes_from_null() {
    let q = query!("SELECT nickname FROM user;");
    // The SDK's own `Option<T>` rejects `Value::Null`; the generated decoder
    // normalises it, so a SurrealQL NULL lands as `None` rather than an error.
    let rows = q
        .decode(one(vec![object([("nickname", Value::Null)])].into_value()))
        .unwrap();
    assert_eq!(rows[0].nickname, None);
}

#[test]
fn a_decode_mismatch_names_the_field_and_the_query() {
    let q = query!("SELECT name FROM user;");
    let error = q
        .decode(one(vec![object([("name", 42i64.into_value())])].into_value()))
        .unwrap_err();
    let rendered = error.to_string();
    assert!(rendered.contains("field `name`"), "{rendered}");
    assert!(rendered.contains("SELECT name FROM user;"), "{rendered}");
    assert_eq!(error.query(), "SELECT name FROM user;");
}

// ---------------------------------------------------------------------------
// Shape: row sets, ONLY, scalars, multiple statements.
// ---------------------------------------------------------------------------

#[test]
fn a_select_is_a_row_set() {
    let q = query!("SELECT name FROM user;");
    let rows = q
        .decode(one(
            vec![object([("name", Value::String("ada".into()))])].into_value(),
        ))
        .unwrap();
    // `Rows` is what gates `fetch_all` and friends.
    let names: Vec<String> = rows.into_rows().into_iter().map(|row| row.name).collect();
    assert_eq!(names, vec!["ada".to_string()]);
}

#[test]
fn select_from_only_is_an_option_shaped_row_set() {
    let q = query!("SELECT name FROM ONLY user:ada;");
    let row = q
        .decode(one(object([("name", Value::String("ada".into()))])))
        .unwrap();
    let row = row.expect("ONLY matched a record");
    assert_eq!(row.name, "ada");
    // Absent: the same query decodes NONE as `None` and yields no rows.
    let q = query!("SELECT name FROM ONLY user:ada;");
    let missing = q.decode(one(Value::None)).unwrap();
    assert!(missing.into_rows().is_empty());
}

#[test]
fn a_non_responding_statement_still_occupies_a_result_slot() {
    // The server returns one slot per top-level statement, including the LET.
    let q = query!("LET $x = 1; RETURN $x + 1;");
    assert_eq!(q.statements(), 2);
    let value = q.decode(vec![Value::None, 2i64.into_value()]).unwrap();
    assert_eq!(value, 2);
}

#[test]
fn multiple_responding_statements_decode_into_a_tuple() {
    let q = query!("SELECT name FROM user; SELECT title FROM post;");
    assert_eq!(q.statements(), 2);
    let (users, posts) = q
        .decode(vec![
            vec![object([("name", Value::String("ada".into()))])].into_value(),
            vec![object([("title", Value::String("hello".into()))])].into_value(),
        ])
        .unwrap();
    assert_eq!(users[0].name, "ada");
    assert_eq!(posts[0].title, "hello");
}

#[test]
fn a_let_before_two_selects_keeps_the_tuple_aligned_with_the_statements() {
    // Slot 0 is the LET's empty result; the tuple must skip it.
    let q = query!("LET $n = 1; SELECT name FROM user; SELECT title FROM post;");
    assert_eq!(q.statements(), 3);
    let (users, posts) = q
        .decode(vec![
            Value::None,
            vec![object([("name", Value::String("ada".into()))])].into_value(),
            vec![object([("title", Value::String("hello".into()))])].into_value(),
        ])
        .unwrap();
    assert_eq!(users[0].name, "ada");
    assert_eq!(posts[0].title, "hello");
}

// ---------------------------------------------------------------------------
// Parameters.
// ---------------------------------------------------------------------------

#[test]
fn a_parameter_is_bound_under_its_name() {
    let q = query!("SELECT name FROM user WHERE age > $min;", min = 18);
    assert_eq!(q.bound(), vec!["min"]);
}

#[test]
fn a_string_parameter_accepts_a_str_literal() {
    // The bind pins to `String` through `Into`, so `&str` still works.
    let q = query!("SELECT age FROM user WHERE name = $who;", who = "ada");
    assert_eq!(q.bound(), vec!["who"]);
}

#[test]
fn several_parameters_bind_in_call_order() {
    let q = query!(
        "SELECT name FROM user WHERE age > $min AND name = $who;",
        min = 18,
        who = "ada",
    );
    assert_eq!(q.bound(), vec!["min", "who"]);
}

#[test]
fn a_parameter_expression_may_be_a_local_binding() {
    let cutoff = 21i64;
    let q = query!("SELECT name FROM user WHERE age > $min;", min = cutoff);
    assert_eq!(q.bound(), vec!["min"]);
}

// ---------------------------------------------------------------------------
// query_file!
// ---------------------------------------------------------------------------

#[test]
fn query_file_reads_checks_and_types_a_file_relative_to_the_crate_root() {
    let q = query_file!("tests/queries/adults.surql", min = 18);
    assert!(q.text().contains("SELECT name, age FROM user"));
    assert_eq!(q.bound(), vec!["min"]);
    let rows = q
        .decode(one(
            vec![object([
                ("name", Value::String("ada".into())),
                ("age", 42i64.into_value()),
            ])]
            .into_value(),
        ))
        .unwrap();
    let name: String = rows[0].name.clone();
    let age: i64 = rows[0].age;
    assert_eq!((name.as_str(), age), ("ada", 42));
}
