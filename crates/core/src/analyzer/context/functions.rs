//! Management of function signatures and behavior

use std::collections::HashMap;
use crate::analyzer::model::Type;

/// Contains information about available functions
#[derive(Debug, Default, Clone)]
pub struct FunctionsContext {
    /// Map of function names to their signatures
    pub functions: HashMap<String, FunctionSignature>,
}

/// Signature of a function including parameter types and return type
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    /// List of parameter types expected by the function
    pub parameters: Vec<Type>,
    /// Return type of the function
    pub return_type: Type,
    /// Whether the function has side effects (database writes, network calls etc)
    pub is_volatile: bool,
}

impl FunctionsContext {
    /// Adds a new function signature to the context
    pub fn add_function(&mut self, name: impl Into<String>, signature: FunctionSignature) {
        self.functions.insert(name.into(), signature);
    }

    /// Gets the signature for a function if it exists
    pub fn get_function(&self, name: &str) -> Option<&FunctionSignature> {
        self.functions.get(name)
    }

    /// Registers built-in SurrealQL functions
    pub fn register_builtins(&mut self) {
        // Example registration
        self.add_function("type::is::email", FunctionSignature {
            parameters: vec![Type::string()],
            return_type: Type::bool(),
            is_volatile: false,
        });
    }
}
