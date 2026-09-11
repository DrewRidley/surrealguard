//! The SELECT clause contracts that depend on the *combination* of clauses:
//! what a `GROUP` clause does to the projections beside it (4029, 4028) and
//! what a page cut without an order is (7016).
//!
//! Each contract gets the query that violates it and at least one near miss
//! that must stay silent — the near misses are the point, since every one of
//! these fires on a shape that is one token away from a correct query.

use surrealdb_types::{Kind, KindLiteral};
use surrealql_analyzer_workspace::{analyze_workspace, Workspace};

const SCHEMA: &str = "\
DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD city ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD score ON person TYPE option<float>;
DEFINE FIELD tags ON person TYPE array<int>;
DEFINE FIELD address ON person TYPE { city: string, zip: string };
DEFINE FIELD joined ON person TYPE datetime;
";

/// Analyzes `query` against [`SCHEMA`] and returns the codes it reports.
fn codes(query: &str) -> Vec<u16> {
    let mut workspace = Workspace::default();
    workspace.add_file_source("schema.surql".into(), SCHEMA.to_string());
    let id = workspace.add_file_source("query.surql".into(), query.to_string());
    let analysis = analyze_workspace(&workspace);
    let output = analysis.sources.get(&id).expect("query source analyzed");
    let mut codes: Vec<u16> = output
        .diagnostics
        .iter()
        .map(|finding| finding.code().number())
        .collect();
    codes.sort_unstable();
    codes
}

/// The response kind of the single statement in `query`.
fn response(query: &str) -> Kind {
    let mut workspace = Workspace::default();
    workspace.add_file_source("schema.surql".into(), SCHEMA.to_string());
    let id = workspace.add_file_source("query.surql".into(), query.to_string());
    let analysis = analyze_workspace(&workspace);
    let output = analysis.sources.get(&id).expect("query source analyzed");
    output.statements[0]
        .response_kind
        .clone()
        .expect("a SELECT responds")
}

fn row_fields(query: &str) -> std::collections::BTreeMap<String, Kind> {
    match response(query) {
        Kind::Array(element, _) => match *element {
            Kind::Literal(KindLiteral::Object(fields)) => fields,
            other => panic!("expected an object row, got {other:?}"),
        },
        other => panic!("expected an array of rows, got {other:?}"),
    }
}

fn fires(query: &str, code: u16) -> bool {
    codes(query).contains(&code)
}

// ---- 4029: every projection under GROUP BY is a key or an aggregate ----

#[test]
fn a_plain_non_key_field_under_group_by_is_reported_once() {
    let codes = codes("SELECT name, count() FROM person GROUP BY city;");
    assert_eq!(
        codes.iter().filter(|code| **code == 4029).count(),
        1,
        "{codes:?}"
    );
    // The key not being projected is 4013's, and it is still reported.
    assert!(codes.contains(&4013), "{codes:?}");
}

#[test]
fn an_expression_over_a_non_key_field_is_reported() {
    assert!(fires(
        "SELECT city, string::uppercase(name) AS n FROM person GROUP BY city;",
        4029
    ));
    assert!(fires(
        "SELECT city, age * 2 AS doubled FROM person GROUP BY city;",
        4029
    ));
}

#[test]
fn keys_aggregates_and_expressions_over_them_stay_silent() {
    for query in [
        "SELECT city, count() FROM person GROUP BY city;",
        "SELECT city, math::max(age) FROM person GROUP BY city;",
        "SELECT city, math::sum(age) * 2 AS doubled FROM person GROUP BY city;",
        "SELECT city, count() + 1 AS n FROM person GROUP BY city;",
        "SELECT string::uppercase(city) AS c FROM person GROUP BY c;",
        "SELECT time::year(joined) AS year FROM person GROUP BY year;",
        // A sub-path of a key is constant within the group; a parent of a
        // key covers it.
        "SELECT address.city FROM person GROUP BY address;",
        "SELECT address FROM person GROUP BY address.city;",
        // GROUP ALL has no keys; accumulation is what the author asked for.
        "SELECT count() FROM person GROUP ALL;",
        "SELECT name FROM person GROUP ALL;",
        // No GROUP clause at all.
        "SELECT name, city FROM person;",
    ] {
        assert!(!fires(query, 4029), "4029 must not fire for {query}");
    }
}

#[test]
fn a_wildcard_beside_a_non_key_field_is_only_the_wildcard_error() {
    let codes = codes("SELECT *, name FROM person GROUP BY city;");
    assert!(codes.contains(&4025), "{codes:?}");
    assert!(!codes.contains(&4029), "{codes:?}");
}

#[test]
fn an_accumulated_projection_is_typed_as_the_collected_column() {
    let fields = row_fields("SELECT city, name FROM person GROUP BY city;");
    assert_eq!(fields["city"], Kind::String);
    assert_eq!(fields["name"], Kind::Array(Box::new(Kind::String), None));

    // Aliased, the wrap lands on the alias.
    let fields = row_fields("SELECT city, age * 2 AS doubled FROM person GROUP BY city;");
    assert!(
        matches!(&fields["doubled"], Kind::Array(inner, None) if matches!(**inner, Kind::Int | Kind::Number)),
        "{:?}",
        fields["doubled"]
    );

    // A nested path wraps its leaf; a whole object field wraps the object.
    let fields = row_fields("SELECT city, address.zip FROM person GROUP BY city;");
    let Kind::Literal(KindLiteral::Object(address)) = &fields["address"] else {
        panic!("address should stay an object: {:?}", fields["address"]);
    };
    assert_eq!(address["zip"], Kind::Array(Box::new(Kind::String), None));
    let fields = row_fields("SELECT city, address FROM person GROUP BY city;");
    assert!(
        matches!(&fields["address"], Kind::Array(inner, None) if matches!(**inner, Kind::Literal(KindLiteral::Object(_)))),
        "{:?}",
        fields["address"]
    );

    // A key and an aggregate keep their per-group kinds.
    let fields = row_fields("SELECT city, math::sum(age) AS total FROM person GROUP BY city;");
    assert_eq!(fields["total"], Kind::Int);
}

#[test]
fn an_accumulated_value_projection_is_the_collected_column() {
    assert_eq!(
        response("SELECT VALUE name FROM person GROUP BY city;"),
        Kind::Array(Box::new(Kind::Array(Box::new(Kind::String), None)), None)
    );
    assert_eq!(
        response("SELECT VALUE city FROM person GROUP BY city;"),
        Kind::Array(Box::new(Kind::String), None)
    );
}

// ---- 4028: an aggregate over a column needs a GROUP clause ----

#[test]
fn an_aggregate_over_a_scalar_column_without_group_is_an_error() {
    let codes = codes("SELECT math::sum(age) FROM person;");
    assert_eq!(
        codes.iter().filter(|code| **code == 4028).count(),
        1,
        "{codes:?}"
    );
    // One mistake, one code: the signature contract is not also reported.
    assert!(!codes.contains(&5002), "{codes:?}");

    for query in [
        "SELECT math::mean(score) AS avg FROM person;",
        "SELECT time::max(joined) FROM person;",
        "SELECT array::distinct(city) FROM person;",
        "SELECT math::sum(age) * 2 AS doubled FROM person;",
        "SELECT name, math::max(age) FROM person WHERE age > 3;",
    ] {
        assert!(fires(query, 4028), "4028 must fire for {query}");
    }
}

#[test]
fn a_grouped_aggregate_or_a_collection_argument_stays_silent() {
    for query in [
        "SELECT math::sum(age) FROM person GROUP ALL;",
        "SELECT city, math::sum(age) FROM person GROUP BY city;",
        // `tags` is already a column of arrays: the per-row reading is right.
        "SELECT math::sum(tags) FROM person;",
        "SELECT math::max(tags) AS top FROM person;",
        // A parameter or a computed argument is not a provable scalar column.
        "SELECT math::sum($values) FROM person;",
        "SELECT math::sum(age * 2) FROM person;",
        // `count()` belongs to 4023.
        "SELECT count() FROM person;",
    ] {
        let codes = codes(query);
        assert!(
            !codes.contains(&4028),
            "4028 must not fire for {query}: {codes:?}"
        );
    }
    assert!(fires("SELECT count() FROM person;", 4023));
}

#[test]
fn a_collection_column_aggregate_reads_element_wise_with_or_without_group() {
    // Without GROUP the argument contract is met by the column itself.
    let ungrouped = codes("SELECT math::sum(tags) FROM person;");
    assert!(!ungrouped.contains(&5002), "{ungrouped:?}");
    let grouped = codes("SELECT math::sum(tags) FROM person GROUP ALL;");
    assert!(!grouped.contains(&5002), "{grouped:?}");
}

// ---- 7016: a page needs an order ----

#[test]
fn a_page_without_an_order_is_a_hint_on_a_table_target() {
    for query in [
        "SELECT name FROM person LIMIT 10;",
        "SELECT name FROM person START 20;",
        "SELECT name FROM person WHERE age > 3 LIMIT 10 START 20;",
        "SELECT city, count() FROM person GROUP BY city LIMIT 5;",
    ] {
        assert!(fires(query, 7016), "7016 must fire for {query}");
    }
}

#[test]
fn an_ordered_page_a_record_target_and_a_single_row_stay_silent() {
    for query in [
        "SELECT name FROM person ORDER BY name LIMIT 10;",
        "SELECT name FROM person ORDER BY age DESC LIMIT 10 START 20;",
        "SELECT * FROM person:1 LIMIT 1;",
        "SELECT name FROM ONLY person LIMIT 1;",
        "SELECT name FROM person LIMIT 1;",
        "SELECT name FROM person WHERE age > 3 LIMIT 1;",
        "SELECT count() FROM person GROUP ALL LIMIT 1;",
        "SELECT name FROM person;",
        "SELECT name FROM $people LIMIT 10;",
    ] {
        let codes = codes(query);
        assert!(
            !codes.contains(&7016),
            "7016 must not fire for {query}: {codes:?}"
        );
    }
}

#[test]
fn a_live_select_never_pages() {
    // LIVE SELECT has no LIMIT/START/ORDER at all (the grammar rejects them),
    // so the check has nothing to say there.
    assert!(!fires("LIVE SELECT name FROM person;", 7016));
}
