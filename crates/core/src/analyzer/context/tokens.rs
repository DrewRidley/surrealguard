//! Management of authentication tokens and permissions

use std::collections::HashMap;
use surrealdb::sql::Permissions;

/// Contains information about authentication and permissions
#[derive(Debug, Default, Clone)]
pub struct TokensContext {
    /// Map of token names to their definitions
    tokens: HashMap<String, TokenDefinition>,
}

#[derive(Debug, Clone)]
pub struct TokenDefinition {
    pub permissions: Permissions,
    pub roles: Vec<String>,
    pub expiration: Option<i64>,
}

impl TokensContext {
    /// Adds a new token definition
    pub fn add_token(&mut self, name: String, definition: TokenDefinition) {
        self.tokens.insert(name, definition);
    }


}
