use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FindingCategory {
    Syntax,
    Schema,
    Type,
    Param,
    Graph,
    Permission,
    Dynamic,
    Lint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FindingCode {
    category: FindingCategory,
    number: u16,
}

impl FindingCode {
    pub fn syntax(number: u16) -> Self {
        Self::new(FindingCategory::Syntax, number)
    }

    pub fn schema(number: u16) -> Self {
        Self::new(FindingCategory::Schema, number)
    }

    pub fn type_error(number: u16) -> Self {
        Self::new(FindingCategory::Type, number)
    }

    pub fn param(number: u16) -> Self {
        Self::new(FindingCategory::Param, number)
    }

    pub fn graph(number: u16) -> Self {
        Self::new(FindingCategory::Graph, number)
    }

    pub fn permission(number: u16) -> Self {
        Self::new(FindingCategory::Permission, number)
    }

    pub fn dynamic(number: u16) -> Self {
        Self::new(FindingCategory::Dynamic, number)
    }

    pub fn lint(number: u16) -> Self {
        Self::new(FindingCategory::Lint, number)
    }

    pub fn category(self) -> FindingCategory {
        self.category
    }

    pub fn number(self) -> u16 {
        self.number
    }

    pub fn prefix(self) -> char {
        match self.category {
            FindingCategory::Syntax => 'S',
            FindingCategory::Schema
            | FindingCategory::Type
            | FindingCategory::Param
            | FindingCategory::Graph => 'E',
            FindingCategory::Permission | FindingCategory::Dynamic => 'W',
            FindingCategory::Lint => 'L',
        }
    }

    const fn new(category: FindingCategory, number: u16) -> Self {
        Self { category, number }
    }
}

impl fmt::Display for FindingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:04}", self.prefix(), self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_strings_are_stable_public_contract() {
        assert_eq!(FindingCode::syntax(1).to_string(), "S0001");
        assert_eq!(FindingCode::schema(1001).to_string(), "E1001");
        assert_eq!(FindingCode::type_error(2001).to_string(), "E2001");
        assert_eq!(FindingCode::lint(7001).to_string(), "L7001");
    }

    #[test]
    fn codes_report_their_category() {
        assert_eq!(
            FindingCode::dynamic(6001).category(),
            FindingCategory::Dynamic
        );
        assert_eq!(
            FindingCode::permission(5001).category(),
            FindingCategory::Permission
        );
    }
}
