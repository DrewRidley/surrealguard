use surrealguard_core::analyzer::{analyze, context::AnalyzerContext};
use surrealguard_macros::kind;

#[test]
fn basic() {
    let stmt = "SELECT name, age FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
    "#).expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!("{ name: string, age: number }");

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn full() {
    let stmt = "SELECT * FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
            DEFINE FIELD address ON user TYPE {
                city: string,
                state: string,
                zip: number,
                country: string
            };
    "#).expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(r#"
        {
            name: string,
            age: number,
            address: {
                city: string,
                state: string,
                zip: number,
                country: string
            }
        }
    "#);

    assert_eq!(analyzed_kind, expected_kind);
}


#[test]
fn alias() {
    let stmt = "SELECT name as nom FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
    "#).expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!("{ nom: string }");

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn omit() {
    let stmt = "SELECT * OMIT age, address.zip FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
            DEFINE FIELD address ON user TYPE {
                city: string,
                state: string,
                zip: number,
                country: string
            };
    "#).expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(r#"
        {
            name: string,
            address: {
                city: string,
                state: string,
                country: string
            }
        }
    "#);

    assert_eq!(analyzed_kind, expected_kind);
}


#[test]
fn fetch_record_link() {
    // Schema definition: a table "user" with two fields.
    let schema = r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
        DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post TYPE record<user>;
    "#;
    // First, build the schema.
    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, schema).expect("Schema construction should succeed");

    // Query: Select the "author" field (which is a record link) from "post"
    // and FETCH it.
    let query = "SELECT author FROM post FETCH user;";
    let analyzed_kind = analyze(&mut ctx, query).expect("Analysis should succeed");

    // Expected type:
    // The field "author" should have been replaced with the full "user" type,
    // i.e. { name: string, age: number }
    let expected_kind = kind!("{ author: { name: string, age: number } }");

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn fetch_array_of_record_links() {
    // Schema definition: a table "user" and a table "group" where groups have an array of user links.
    let schema = r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD username ON user TYPE string;
            DEFINE FIELD email ON user TYPE string;
        DEFINE TABLE group SCHEMAFULL;
            DEFINE FIELD members ON group TYPE array<record<user>>;
    "#;
    // Build the schema.
    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, schema).expect("Schema construction should succeed");

    // Query: Select the "members" field (an array of record links) from "group"
    // and FETCH it.
    let query = "SELECT members FROM group FETCH user;";
    let analyzed_kind = analyze(&mut ctx, query).expect("Analysis should succeed");

    // Expected type:
    // "members" should be an array of full "user" types,
    // i.e. [ { username: string, email: string } ]
    let expected_kind = kind!("{ members: [ { username: string, email: string } ] }");

    assert_eq!(analyzed_kind, expected_kind);
}


#[test]
fn select_value() {
    let query = "SELECT VALUE email FROM user;";
    let schema = r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD email ON user TYPE string;
    "#;
    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, schema).expect("Schema construction should succeed");
    let analyzed_kind = analyze(&mut ctx, query).expect("Analysis should succeed");
    // Expected type: an array of string values, represented as Literal(Array([String]))
    let expected_kind = kind!("[ string ]");
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn select_only() {
    // Schema: define a person with two fields.
    let schema = r#"
        DEFINE TABLE person SCHEMAFULL;
            DEFINE FIELD name ON person TYPE string;
            DEFINE FIELD age ON person TYPE number;
    "#;
    // Build the schema.
    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, schema).expect("Schema construction should succeed");

    // Query: Use the ONLY keyword to select a single record.
    let query = "SELECT * FROM ONLY person:tobie;";
    let analyzed_kind = analyze(&mut ctx, query).expect("Analysis should succeed");

    // Expected type: just an object (not wrapped in an array)
    let expected_kind = kind!(r#"
        {
            name: string,
            age: number
        }
    "#);
    assert_eq!(analyzed_kind, expected_kind);
}
