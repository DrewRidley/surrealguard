//! Finding codes: a category (thousand-block family) plus a number.
//!
//! Rendered strings like `E2001` are a stable public contract; retired
//! numbers are never reused.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The thousand-block family a code belongs to. The family fixes the code
/// number range and the display prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FindingCategory {
    /// 0xxx — parse-level breakage.
    Syntax,
    /// 1xxx — references to schema objects that don't exist.
    Schema,
    /// 2xxx — kind mismatches and nullability.
    Type,
    /// 3xxx — relation and traversal misuse.
    Graph,
    /// 4xxx — clause and statement misuse.
    Statement,
    /// 5xxx — function and closure misuse.
    Function,
    /// 6xxx — parameter constraints and conflicts.
    Param,
    /// 7xxx — style and suspicious-but-valid constructs.
    Lint,
    /// 8xxx — SurrealDB version compatibility.
    Compat,
}

/// A catalog code: family category plus number (`E2001`). One code per
/// contract; the rendered string is stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FindingCode {
    category: FindingCategory,
    number: u16,
}

impl FindingCode {
    /// A code in the syntax family (0xxx).
    pub fn syntax(number: u16) -> Self {
        Self::new(FindingCategory::Syntax, number)
    }

    /// A code in the schema family (1xxx).
    pub fn schema(number: u16) -> Self {
        Self::new(FindingCategory::Schema, number)
    }

    /// A code in the type family (2xxx).
    pub fn type_error(number: u16) -> Self {
        Self::new(FindingCategory::Type, number)
    }

    /// A code in the parameter family (6xxx).
    pub fn param(number: u16) -> Self {
        Self::new(FindingCategory::Param, number)
    }

    /// A code in the graph family (3xxx).
    pub fn graph(number: u16) -> Self {
        Self::new(FindingCategory::Graph, number)
    }

    /// A code in the lint family (7xxx).
    pub fn lint(number: u16) -> Self {
        Self::new(FindingCategory::Lint, number)
    }

    /// A code in the statement family (4xxx).
    pub fn statement(number: u16) -> Self {
        Self::new(FindingCategory::Statement, number)
    }

    /// A code in the function family (5xxx).
    pub fn function(number: u16) -> Self {
        Self::new(FindingCategory::Function, number)
    }

    /// A code in the compatibility family (8xxx).
    pub fn compat(number: u16) -> Self {
        Self::new(FindingCategory::Compat, number)
    }

    /// The catalog family a code number belongs to, by its thousand block.
    pub fn from_number(number: u16) -> Self {
        let category = match number / 1000 {
            0 => FindingCategory::Syntax,
            1 => FindingCategory::Schema,
            2 => FindingCategory::Type,
            3 => FindingCategory::Graph,
            4 => FindingCategory::Statement,
            5 => FindingCategory::Function,
            6 => FindingCategory::Param,
            7 => FindingCategory::Lint,
            _ => FindingCategory::Compat,
        };
        Self::new(category, number)
    }

    /// The family this code belongs to.
    pub fn category(self) -> FindingCategory {
        self.category
    }

    /// The raw code number (e.g. `2001`), without prefix or family.
    pub fn number(self) -> u16 {
        self.number
    }

    /// Rendering prefix. Syntax keeps its `S`; every other family renders
    /// with the *severity's* letter — see [`crate::render_code`]. This
    /// category-only fallback exists for contexts without a severity.
    pub fn prefix(self) -> char {
        match self.category {
            FindingCategory::Syntax => 'S',
            FindingCategory::Schema
            | FindingCategory::Type
            | FindingCategory::Param
            | FindingCategory::Graph
            | FindingCategory::Statement
            | FindingCategory::Function
            | FindingCategory::Compat => 'E',
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
        assert_eq!(FindingCode::param(6001).category(), FindingCategory::Param);
        assert_eq!(
            FindingCode::function(5001).category(),
            FindingCategory::Function
        );
    }
}
