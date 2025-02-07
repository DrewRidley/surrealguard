//! SurrealGuard is a static analysis and type checking system for SurrealQL queries.
//!
//! # Overview
//!
//! This crate provides tools to validate SurrealQL queries against schema definitions
//! before execution, helping catch errors early and ensure type safety.
//!
//! # Key Features
//!
//! - Schema validation
//! - Type inference for query parameters
//! - Complex query analysis (SELECT, CREATE, UPDATE, etc.)
//! - Graph traversal validation
//!
//! # Quick Start
//!
//! ```rust
//! use surrealguard_core::prelude::*;
//!
//! let mut ctx = AnalyzerContext::new();
//!
//! // Define schema
//! analyze(&mut ctx, r#"
//!     DEFINE TABLE user SCHEMAFULL;
//!         DEFINE FIELD name ON user TYPE string;
//!         DEFINE FIELD age ON user TYPE number;
//! "#).expect("Schema definition failed");
//!
//! // Analyze a query
//! let query = "SELECT * FROM user WHERE age > $min_age;";
//! let result = analyze(&mut ctx, query).expect("Query analysis failed");
//! ```

pub mod analyzer;
pub mod prelude;
