//! Management of query parameters and local variables

use std::collections::HashMap;
use crate::analyzer::model::Type;

/// Tracks parameters and variables available during analysis
#[derive(Debug, Default, Clone)]
pub struct ParamsContext {
    /// Map of parameter names to their types
    parameters: HashMap<String, Type>,
    /// Map of local variables to their types
    variables: HashMap<String, Type>,
}

impl ParamsContext {
    /// Creates a new empty parameters context
    pub fn new() -> Self {
        Self {
            parameters: HashMap::new(),
            variables: HashMap::new(),
        }
    }

    /// Adds a parameter definition to the context
    pub fn add_parameter(&mut self, name: String, param_type: Type) {
        self.parameters.insert(name, param_type);
    }

    /// Retrieves the type of a parameter if defined
    pub fn get_parameter_type(&self, name: &str) -> Option<&Type> {
        self.parameters.get(name)
    }
}
