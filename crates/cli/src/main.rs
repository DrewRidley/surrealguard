
use surrealdb::sql::parse;
use surrealguard_core::analyzer::{analyze, context::AnalyzerContext};
use surrealguard_macros::kind;

fn main() {
    let mut ctx = AnalyzerContext::new();

        // Set up schema
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
        "#).expect("Schema construction should succeed");

        // Parse and print AST
        let stmt = "SELECT ->memberOf FROM user;";
        let ast = surrealdb::sql::parse(stmt).unwrap();
        println!("Simple traversal AST: {:#?}", ast);

        let stmt2 = "SELECT ->memberOf->org FROM user;";
        let ast2 = surrealdb::sql::parse(stmt2).unwrap();
        println!("Multi-step traversal AST: {:#?}", ast2);
}
