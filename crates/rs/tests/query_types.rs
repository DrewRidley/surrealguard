//! `query!` yields a `Query<T>` whose nameless `T` deserializes real rows.

use surrealguard_rs::query;

#[test]
fn object_result_deserializes_into_a_nameless_typed_struct() {
    let q = query!("RETURN { name: 'ada', age: 42 };");
    let row = q.from_json(r#"{ "name": "ada", "age": 42 }"#).unwrap();
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
        .from_json(r#"{ "user": { "name": "ada" }, "tags": ["a", "b"] }"#)
        .unwrap();
    let nested_name: String = row.user.name;
    let tags: Vec<String> = row.tags;
    assert_eq!(nested_name, "ada");
    assert_eq!(tags, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn array_result_is_a_vec_of_the_nameless_row() {
    let q = query!("RETURN [{ city: 'london' }];");
    let rows = q.from_json(r#"[{ "city": "london" }]"#).unwrap();
    assert_eq!(rows.len(), 1);
    let city: String = rows[0].city.clone();
    assert_eq!(city, "london");
}

#[test]
fn the_query_text_is_carried_verbatim() {
    let q = query!("RETURN 1 + 1;");
    assert_eq!(q.text, "RETURN 1 + 1;");
}
