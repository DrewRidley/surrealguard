#[cfg(test)]
mod tests {
    use crate::analyzer::{analyze, context::AnalyzerContext};
    use surrealguard_macros::kind;

    #[test]
    fn test_define_table() {
        let mut ctx = AnalyzerContext::new();
        
        // Test DEFINE TABLE statement
        let result = analyze(
            &mut ctx,
            "DEFINE TABLE user SCHEMAFULL;"
        );
        
        assert!(result.is_ok(), "DEFINE TABLE analysis failed: {:?}", result.err());
    }

    #[test]
    fn test_define_field() {
        let mut ctx = AnalyzerContext::new();
        
        // Define a table first
        analyze(&mut ctx, "DEFINE TABLE user SCHEMAFULL;").unwrap();
        
        // Test DEFINE FIELD statement
        let result = analyze(
            &mut ctx,
            "DEFINE FIELD name ON user TYPE string;"
        );
        
        assert!(result.is_ok(), "DEFINE FIELD analysis failed: {:?}", result.err());
    }

    #[test]
    fn test_define_index() {
        let mut ctx = AnalyzerContext::new();
        
        // Define a table first
        analyze(&mut ctx, "DEFINE TABLE user SCHEMAFULL;").unwrap();
        
        // Test DEFINE INDEX statement
        let result = analyze(
            &mut ctx,
            "DEFINE INDEX idx_name ON user FIELDS name;"
        );
        
        assert!(result.is_ok(), "DEFINE INDEX analysis failed: {:?}", result.err());
    }

    #[test]
    fn test_complex_schema() {
        let mut ctx = AnalyzerContext::new();
        
        // Test a complex schema with multiple DEFINE statements
        let result = analyze(
            &mut ctx,
            r#"
            DEFINE TABLE organization SCHEMAFULL;
                DEFINE FIELD name ON organization TYPE string;
                DEFINE FIELD desc ON organization TYPE string;
                DEFINE FIELD industry ON organization TYPE string;
                DEFINE INDEX idx_org_name ON organization FIELDS name;

            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD password ON user TYPE string;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD organization ON user TYPE record<organization>;
                DEFINE INDEX idx_user_email ON user FIELDS email UNIQUE;

            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO organization;
                DEFINE FIELD role ON memberOf TYPE string;
                DEFINE FIELD since ON memberOf TYPE datetime;
            "#
        );
        
        assert!(result.is_ok(), "Complex schema analysis failed: {:?}", result.err());
    }
}