//! Execution against a live SurrealDB server.
//!
//! These are `#[ignore]`d because they need a server, the same way `sqlx`'s
//! integration tests need a live database. Run them with one up:
//!
//! ```text
//! surreal start --user root --pass root --bind 127.0.0.1:8111 memory &
//! cargo test -p surrealql-analyzer-rs -- --ignored
//! ```
//!
//! Point them elsewhere with `SURREALQL_ANALYZER_TEST_WS` (default
//! `127.0.0.1:8111`). Each test uses its own namespace and database, so they
//! neither collide nor need cleaning up between runs.
//!
//! What they prove that `tests/query_types.rs` cannot: that the `Value`s a real
//! server sends decode into the types the analyzer inferred. A test that only
//! type-checks would pass against a wrong mapping.

use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;
use surrealql_analyzer_rs::surrealdb_types::{Datetime, Decimal, Duration, RecordId};
use surrealql_analyzer_rs::{query, query_file, ErrorKind};

const SCHEMA: &str = include_str!("../schema.surql");

/// Connects, isolates into `namespace`, and applies this crate's schema.
async fn connect(namespace: &str) -> Surreal<Client> {
    let address =
        std::env::var("SURREALQL_ANALYZER_TEST_WS").unwrap_or_else(|_| "127.0.0.1:8111".to_owned());
    let db = Surreal::new::<Ws>(address.as_str())
        .await
        .expect("a SurrealDB server on SURREALQL_ANALYZER_TEST_WS — see this file's docs");
    db.signin(Root {
        username: "root".to_owned(),
        password: "root".to_owned(),
    })
    .await
    .expect("root sign-in");
    // Start from nothing, so a run against a server that is still up from the
    // last one behaves identically to a run against a fresh one.
    db.query(format!("REMOVE NAMESPACE IF EXISTS {namespace};"))
        .await
        .expect("reset")
        .check()
        .expect("reset");
    db.use_ns(namespace).use_db(namespace).await.expect("ns/db");
    db.query(SCHEMA)
        .await
        .expect("schema")
        .check()
        .expect("schema");
    db
}

async fn seed_ada(db: &Surreal<Client>) {
    db.query(
        "CREATE user:ada SET name = 'ada', age = 42, ratio = 1.5f, score = 9.99dec,
             created = d'2024-01-01T00:00:00Z', ttl = 1h, tags = ['x'], nickname = NONE;",
    )
    .await
    .expect("seed")
    .check()
    .expect("seed");
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn fetch_all_returns_every_row_typed() {
    let db = connect("sg_fetch_all").await;
    seed_ada(&db).await;

    let rows = query!("SELECT name, age FROM user;")
        .fetch_all(&db)
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    let name: String = rows[0].name.clone();
    let age: i64 = rows[0].age;
    assert_eq!((name.as_str(), age), ("ada", 42));
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn fetch_one_returns_the_first_row() {
    let db = connect("sg_fetch_one").await;
    seed_ada(&db).await;

    let row = query!("SELECT name FROM user;")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(row.name, "ada");
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn fetch_one_reports_row_not_found_with_the_query_text() {
    let db = connect("sg_row_not_found").await;

    let error = query!("SELECT name FROM user;")
        .fetch_one(&db)
        .await
        .unwrap_err();

    assert!(matches!(error.kind(), ErrorKind::RowNotFound));
    assert_eq!(error.query(), "SELECT name FROM user;");
    assert!(error.to_string().contains("SELECT name FROM user;"));
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn fetch_optional_is_none_when_nothing_matches() {
    let db = connect("sg_fetch_optional").await;

    let row = query!("SELECT name FROM user;")
        .fetch_optional(&db)
        .await
        .unwrap();
    assert!(row.is_none());
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn every_scalar_kind_decodes_from_what_the_server_actually_sends() {
    // The correctness test. Record ids, durations, decimals and datetimes each
    // arrive as their own `Value` variant, and the SDK coerces nothing — so this
    // failing would mean the generated types are wrong.
    let db = connect("sg_scalars").await;
    seed_ada(&db).await;

    let row =
        query!("SELECT id, name, age, ratio, score, created, ttl, tags, nickname, best FROM user;")
            .fetch_one(&db)
            .await
            .unwrap();

    assert_eq!(row.id, RecordId::new("user", "ada"));
    assert_eq!(row.name, "ada");
    assert_eq!(row.age, 42);
    assert!((row.ratio - 1.5).abs() < f64::EPSILON);
    assert_eq!(row.score, Decimal::new(999, 2));
    assert_eq!(
        row.created,
        Datetime::from(
            "2024-01-01T00:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
        )
    );
    assert_eq!(row.ttl, Duration::from_secs(3600));
    assert_eq!(row.tags, vec!["x".to_string()]);
    assert_eq!(row.nickname, None);
    assert_eq!(row.best, None);
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn a_bound_parameter_reaches_the_server() {
    let db = connect("sg_params").await;
    seed_ada(&db).await;

    let matched = query!("SELECT name FROM user WHERE age > $min;", min = 18)
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(matched.len(), 1);

    let excluded = query!("SELECT name FROM user WHERE age > $min;", min = 99)
        .fetch_all(&db)
        .await
        .unwrap();
    assert!(excluded.is_empty());

    let by_name = query!("SELECT age FROM user WHERE name = $who;", who = "ada")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(by_name.age, 42);
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn fetch_returns_the_analyzed_shape_for_a_scalar() {
    let db = connect("sg_scalar").await;

    let value = query!("RETURN 1 + 1;").fetch(&db).await.unwrap();
    let value: i64 = value;
    assert_eq!(value, 2);
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn select_from_only_fetches_an_option() {
    let db = connect("sg_only").await;
    seed_ada(&db).await;

    let row = query!("SELECT name FROM ONLY user:ada;")
        .fetch(&db)
        .await
        .unwrap();
    assert_eq!(row.unwrap().name, "ada");

    let missing = query!("SELECT name FROM ONLY user:nobody;")
        .fetch(&db)
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn multiple_statements_fetch_as_a_tuple_skipping_the_let() {
    let db = connect("sg_multi").await;
    seed_ada(&db).await;
    db.query("CREATE post:hello SET title = 'hello', author = user:ada;")
        .await
        .unwrap()
        .check()
        .unwrap();

    let (users, posts) = query!("LET $n = 1; SELECT name FROM user; SELECT title FROM post;")
        .fetch(&db)
        .await
        .unwrap();

    assert_eq!(users[0].name, "ada");
    assert_eq!(posts[0].title, "hello");
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn execute_runs_a_statement_and_discards_its_result() {
    let db = connect("sg_execute").await;

    query!("CREATE user:bob SET name = 'bob', age = 7, ratio = 0.5f, score = <decimal> 1, created = d'2024-01-01T00:00:00Z', ttl = 1s, tags = [];")
        .execute(&db)
        .await
        .unwrap();

    let row = query!("SELECT name FROM ONLY user:bob;")
        .fetch(&db)
        .await
        .unwrap();
    assert_eq!(row.unwrap().name, "bob");
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn a_database_error_carries_the_query_text() {
    let db = connect("sg_db_error").await;
    seed_ada(&db).await;

    // Valid to the analyzer — every required field is set — but the record id
    // is already taken, which only the server can know.
    // `<decimal> 9.99` rather than `9.99dec`: the `dec` suffix does not
    // currently parse inside a SET clause (see this crate's README).
    const CREATE: &str = "CREATE user:ada SET name = 'ada', age = 42, ratio = 1.5f, score = <decimal> 9.99, created = d'2024-01-01T00:00:00Z', ttl = 1h, tags = [];";
    let error = query!("CREATE user:ada SET name = 'ada', age = 42, ratio = 1.5f, score = <decimal> 9.99, created = d'2024-01-01T00:00:00Z', ttl = 1h, tags = [];")
        .execute(&db)
        .await
        .unwrap_err();

    assert!(matches!(error.kind(), ErrorKind::Database(_)));
    assert_eq!(error.query(), CREATE);
    // The whole point of the crate's own error type: the failure names the
    // query that caused it, which the SDK's error does not.
    assert!(error.to_string().contains("CREATE user:ada"));
}

#[tokio::test]
#[ignore = "needs a live SurrealDB server; see this file's docs"]
async fn query_file_executes_the_same_way() {
    let db = connect("sg_query_file").await;
    seed_ada(&db).await;

    let rows = query_file!("tests/queries/adults.surql", min = 18)
        .fetch_all(&db)
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "ada");
    assert_eq!(rows[0].age, 42);
}
