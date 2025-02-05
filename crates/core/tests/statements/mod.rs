#[cfg(test)]
mod select; // existing select tests

#[cfg(test)]
mod tests {
    use surrealguard_core::analyzer::{analyze, context::AnalyzerContext};
    use surrealguard_macros::kind;

    #[test]
    fn update_table() {
        let mut ctx = AnalyzerContext::new();

        // Build schema for table "user"
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#).expect("Schema construction should succeed");

        // Analyze UPDATE statement
        let stmt = "UPDATE user SET name = 'John';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // UPDATE returns an array wrapping the full table type.
        let expected_kind = kind!("array<{ name: string, age: number }>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn create_table() {
        let mut ctx = AnalyzerContext::new();

        // Build schema for table "user"
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#).expect("Schema construction should succeed");

        // Analyze CREATE statement
        let stmt = "CREATE user CONTENT { name: 'John', age: 42 };";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // CREATE returns an array wrapping the full table type.
        let expected_kind = kind!("array<{ name: string, age: number }>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn upsert_table() {
        let mut ctx = AnalyzerContext::new();

        // Build schema for table "user"
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#).expect("Schema construction should succeed");

        // Analyze UPSERT statement
        let stmt = "UPSERT user SET name = 'Jane';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // UPSERT returns an array wrapping the full table type.
        let expected_kind = kind!("array<{ name: string, age: number }>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn insert_table() {
        let mut ctx = AnalyzerContext::new();

        // Build schema for table "user"
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#).expect("Schema construction should succeed");

        // Analyze INSERT statement
        let stmt = "INSERT INTO user { name: 'Jane', age: 30 };";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // INSERT returns an array wrapping the full table type.
        let expected_kind = kind!("array<{ name: string, age: number }>");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn delete_table() {
        let mut ctx = AnalyzerContext::new();

        // Build schema for table "user"
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
        "#).expect("Schema construction should succeed");

        // Analyze DELETE statement
        let stmt = "DELETE user WHERE name = 'Jane';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // DELETE always returns an empty array.
        let expected_kind = kind!("[]");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn relate_statement() {
        let mut ctx = AnalyzerContext::new();

        // Build schema for tables "user", "org", and the relation "memberOf"
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
                DEFINE FIELD role ON memberOf TYPE string;
                DEFINE FIELD since ON memberOf TYPE datetime;
        "#).expect("Schema construction should succeed");

        // Analyze RELATE statement
        let stmt = "RELATE user:alice->memberOf->org:google";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // RELATE returns an array wrapping the full type of the relation table ("memberOf")
        let expected_kind = kind!("array<{ role: string, since: datetime }>");
        assert_eq!(analyzed_kind, expected_kind);
    }
}
