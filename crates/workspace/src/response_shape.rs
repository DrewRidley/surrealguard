use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_syntax::span::SourceSpan;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseShape {
    Unknown {
        reason: PartialReason,
    },
    Value {
        kind: Kind,
    },
    Object {
        fields: BTreeMap<String, FieldShape>,
        open: bool,
    },
    Array {
        element: Box<ResponseShape>,
        max_len: Option<u64>,
    },
    Set {
        element: Box<ResponseShape>,
        max_len: Option<u64>,
    },
    Union {
        variants: Vec<ResponseShape>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldShape {
    pub shape: ResponseShape,
    pub kind: Option<Kind>,
    pub span: SourceSpan,
    pub materialized_by_fetch: bool,
    pub partial: Vec<PartialReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartialReason {
    Unresolved,
    DynamicExpression,
    UnsupportedSyntax(String),
}
