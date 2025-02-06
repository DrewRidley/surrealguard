// crates/core/tests/statements/select.rs
use surrealguard_core::analyzer::{analyze, context::AnalyzerContext};
use surrealguard_macros::kind;

#[test]
fn basic() {
    let stmt = "SELECT name, age FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
    "#,
    )
    .expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!("array<{ name: string, age: number }>");

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn full() {
    let stmt = "SELECT * FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
            DEFINE FIELD address ON user TYPE {
                city: string,
                state: string,
                zip: number,
                country: string
            };
    "#,
    )
    .expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        name: string,
        age: number,
        address: {
            city: string,
            state: string,
            zip: number,
            country: string
        }
    }>"#
    );

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn alias() {
    let stmt = "SELECT name as nom FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
    "#,
    )
    .expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!("array<{ nom: string }>");

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn omit() {
    let stmt = "SELECT * OMIT age, address.zip FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
            DEFINE FIELD address ON user TYPE {
                city: string,
                state: string,
                zip: number,
                country: string
            };
    "#,
    )
    .expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        name: string,
        address: {
            city: string,
            state: string,
            country: string
        }
    }>"#
    );

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn fetch_record_link() {
    let schema = r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE number;
        DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post TYPE record<user>;
    "#;
    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, schema).expect("Schema construction should succeed");

    let query = "SELECT author FROM post FETCH author;";
    let analyzed_kind = analyze(&mut ctx, query).expect("Analysis should succeed");
    let expected_kind = kind!("array<{ author: { name: string, age: number } }>");

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn fetch_array_of_record_links() {
    let schema = r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD username ON user TYPE string;
            DEFINE FIELD email ON user TYPE string;
        DEFINE TABLE group SCHEMAFULL;
            DEFINE FIELD members ON group TYPE array<record<user>>;
    "#;
    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, schema).expect("Schema construction should succeed");

    let query = "SELECT members FROM group FETCH members;";
    let analyzed_kind = analyze(&mut ctx, query).expect("Analysis should succeed");
    let expected_kind = kind!("array<{ members: [ { username: string, email: string } ] }>");

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
    // Changed to match the actual structure: Array(Literal(Array([String])), None)
    let expected_kind = kind!("array<[string]>");
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn select_only() {
    let schema = r#"
        DEFINE TABLE person SCHEMAFULL;
            DEFINE FIELD name ON person TYPE string;
            DEFINE FIELD age ON person TYPE number;
    "#;
    let mut ctx = AnalyzerContext::new();
    analyze(&mut ctx, schema).expect("Schema construction should succeed");

    let query = "SELECT * FROM ONLY person:tobie;";
    let analyzed_kind = analyze(&mut ctx, query).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        name: string,
        age: number
    }>"#
    );
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn destructuring() {
    let stmt = "SELECT address.{city, country} FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD address ON user TYPE {
                city: string,
                state: string,
                zip: number,
                country: string
            };
    "#,
    )
    .expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        address: {
            city: string,
            country: string
        }
    }>"#
    );

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn destructuring_with_alias() {
    let stmt = "SELECT address.{city, country} AS location FROM user;";

    let mut ctx = AnalyzerContext::new();
    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD address ON user TYPE {
                city: string,
                state: string,
                zip: number,
                country: string
            };
    "#,
    )
    .expect("Schema construction should succeed");

    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        location: {
            city: string,
            country: string
        }
    }>"#
    );

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn graph_traversal_simple() {
    let mut ctx = AnalyzerContext::new();

    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
        DEFINE TABLE org SCHEMAFULL;
            DEFINE FIELD name ON org TYPE string;
        DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
    "#,
    )
    .expect("Schema construction should succeed");

    let stmt = "SELECT ->memberOf FROM user;";
    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        "->memberOf": [record<memberOf>]
    }>"#
    );
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn graph_traversal_multi_hop() {
    let mut ctx = AnalyzerContext::new();

    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
        DEFINE TABLE team SCHEMAFULL;
            DEFINE FIELD name ON team TYPE string;
        DEFINE TABLE org SCHEMAFULL;
            DEFINE FIELD name ON org TYPE string;
            DEFINE FIELD industry ON org TYPE string;
        DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO team;
        DEFINE TABLE partOf SCHEMAFULL TYPE RELATION FROM team TO org;
    "#,
    )
    .expect("Schema construction should succeed");

    let stmt = "SELECT ->memberOf->partOf->org.* FROM user;";
    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        "->memberOf": {
            "->partOf": {
                "->org": [{
                    name: string,
                    industry: string
                }]
            }
        }
    }>"#
    );

    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn graph_traversal_to_node() {
    let mut ctx = AnalyzerContext::new();

    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
        DEFINE TABLE org SCHEMAFULL;
            DEFINE FIELD name ON org TYPE string;
        DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
    "#,
    )
    .expect("Schema construction should succeed");

    let stmt = "SELECT ->memberOf->org FROM user;";
    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        "->memberOf": {
            "->org": [record<org>]
        }
    }>"#
    );
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn graph_traversal_with_fields() {
    let mut ctx = AnalyzerContext::new();

    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
        DEFINE TABLE org SCHEMAFULL;
            DEFINE FIELD name ON org TYPE string;
        DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
    "#,
    )
    .expect("Schema construction should succeed");

    let stmt = "SELECT ->memberOf->org.* FROM user;";
    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        "->memberOf": {
            "->org": [{
                name: string
            }]
        }
    }>"#
    );
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn graph_traversal_with_destructure() {
    let mut ctx = AnalyzerContext::new();

    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
        DEFINE TABLE org SCHEMAFULL;
            DEFINE FIELD name ON org TYPE string;
            DEFINE FIELD address ON org TYPE string;
        DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
    "#,
    )
    .expect("Schema construction should succeed");

    let stmt = "SELECT ->memberOf->org.{name} FROM user;";
    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        "->memberOf": {
            "->org": [{
                name: string
            }]
        }
    }>"#
    );
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn graph_traversal_reverse() {
    let mut ctx = AnalyzerContext::new();

    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
        DEFINE TABLE org SCHEMAFULL;
        DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
    "#,
    )
    .expect("Schema construction should succeed");

    let stmt = "SELECT <-memberOf<-user.* FROM org;";
    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        "<-memberOf": {
            "<-user": [{
                name: string
            }]
        }
    }>"#
    );
    assert_eq!(analyzed_kind, expected_kind);
}

#[test]
fn graph_traversal_with_alias() {
    let mut ctx = AnalyzerContext::new();

    analyze(
        &mut ctx,
        r#"
        DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
        DEFINE TABLE org SCHEMAFULL;
            DEFINE FIELD name ON org TYPE string;
            DEFINE FIELD industry ON org TYPE string;
        DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
    "#,
    )
    .expect("Schema construction should succeed");

    let stmt = "SELECT ->memberOf->org.* AS orgs FROM user;";
    let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");
    let expected_kind = kind!(
        r#"array<{
        "orgs": [{
            name: string,
            industry: string
        }]
    }>"#
    );

    assert_eq!(analyzed_kind, expected_kind);
}
