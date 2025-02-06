use surrealguard_core::analyzer::analyze;
use surrealguard_macros::kind;

#[cfg(test)]
mod select; // existing select tests

#[cfg(test)]
mod tests {
    use surrealguard_core::analyzer::{analyze, context::AnalyzerContext};
    use surrealguard_macros::kind;

    #[test]
    fn update_table() {
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

        let stmt = "UPDATE user SET name = 'John';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<array<{ name: string, age: number }>>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn create_table() {
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

        let stmt = "CREATE user CONTENT { name: 'John', age: 42 };";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<array<{ name: string, age: number }>>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn upsert_table() {
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

        let stmt = "UPSERT user SET name = 'Jane';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<array<{ name: string, age: number }>>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn insert_table() {
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

        let stmt = "INSERT INTO user { name: 'Jane', age: 30 };";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<array<{ name: string, age: number }>>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn delete_table() {
        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "DELETE user WHERE name = 'Jane';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<[]>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn relate_statement() {
        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
                DEFINE FIELD role ON memberOf TYPE string;
                DEFINE FIELD since ON memberOf TYPE datetime;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "RELATE user:alice->memberOf->org:google";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<array<{ role: string, since: datetime }>>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn multiple_statements() {
        let mut ctx = AnalyzerContext::new();

        // Build schema
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE organization SCHEMAFULL;
                DEFINE FIELD name ON organization TYPE string;
                DEFINE FIELD desc ON organization TYPE string;
                DEFINE FIELD industry ON organization TYPE string;

            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD password ON user TYPE string;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD organization ON user TYPE record<organization>;
        "#,
        )
        .expect("Schema construction should succeed");

        // Test multi-statement query
        let stmt = r#"
            CREATE organization:applebees CONTENT {
                name: "AppleBees",
                desc: "A big restaurant",
                industry: "food"
            };

            CREATE user:jane CONTENT {
                email: "jane@doe.org",
                password: crypto::argon2::generate("password"),
                name: "Jane Doe",
                organization: organization:applebees
            };
        "#;

        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // Should be a literal array with exact types for each statement
        let expected_kind = kind!(
            r#"[
                array<{ name: string, desc: string, industry: string }>,
                array<{ email: string, password: string, name: string, organization: record<organization> }>
            ]"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }
}
