//! Expression and idiom lowering.
//!
//! CST shapes this encodes (verified against the grammar, see
//! `examples/dump_cst.rs` for the inspection tool):
//!
//! - `Number` wraps an `Int`/`Float`/`Decimal` child; a leading minus is part
//!   of the `Number` text, not a prefix expression.
//! - `String` has no children; `d'…'`/`u'…'`/`r'…'` prefixes select
//!   datetime/uuid/regex literals, normalized here.
//! - `None` covers both `NONE` and `null`, distinguished by text.
//! - `Path` is `[start, subscript/lookup/filter...]` where `start` is an
//!   `Ident` (row field) or a value node like `VariableName` (`$user.name`).
//! - `Subscript` carries `.field`, `.{destructure}`, or `.method()`.
//! - `Filter` carries `[index-expr]` or `[WHERE …]` — one node, two meanings.
//! - `Lookup` is one graph step: direction token plus a bare edge `Ident` or
//!   a `LookupSelection` (`(edge WHERE …)` with `GraphPredicate` targets).

use tree_sitter::Node;

use super::{node_range, partial};
use crate::ast::{
    BinaryOp, Block, Call, Closure, Expr, GraphDir, GraphStep, Idiom, IdiomPart, Literal, PrefixOp,
    Spanned, TypeExpr,
};

/// Lowers an expression-position CST node.
pub fn lower_expr(node: Node<'_>, text: &str) -> Spanned<Expr> {
    Lowerer { text }.expr(node)
}

/// Lowers a type-position node (`Type`, `TypeName`, `ParameterizedType`,
/// `UnionType`, `LiteralType`) — used by DEFINE FIELD/cast lowering and
/// schema extraction.
pub fn lower_type_expr(node: Node<'_>, text: &str) -> Spanned<TypeExpr> {
    Lowerer { text }.type_expr(node)
}

/// Lowers a `Block` node — used by statement lowering for IF/FOR bodies and
/// statement-position blocks.
pub fn lower_block_node(node: Node<'_>, text: &str) -> Block {
    Lowerer { text }.block(node)
}

/// Lowers a path-position node (`Path`, `Idiom`, or bare `Ident`) to an
/// [`Idiom`] — used by statement lowering for clause paths.
pub fn lower_idiom_node(node: Node<'_>, text: &str) -> Idiom {
    let lowerer = Lowerer { text };
    match node.kind() {
        "Ident" => Idiom {
            parts: vec![lowerer.spanned(node, IdiomPart::Field(text[node.byte_range()].into()))],
        },
        _ => lowerer.idiom(node),
    }
}

struct Lowerer<'a> {
    text: &'a str,
}

impl Lowerer<'_> {
    fn node_text(&self, node: Node<'_>) -> &str {
        &self.text[node.byte_range()]
    }

    fn spanned<T>(&self, node: Node<'_>, value: T) -> Spanned<T> {
        Spanned::new(value, node_range(node))
    }

    fn expr(&self, node: Node<'_>) -> Spanned<Expr> {
        if node.is_error() || node.is_missing() {
            return self.spanned(node, Expr::Partial(partial(node)));
        }

        let expr = match node.kind() {
            // Wrappers the grammar puts around single expressions.
            "Predicate" | "Fields" => match single_named_child(node) {
                Some(child) => return self.expr(child),
                None => Expr::Partial(partial(node)),
            },
            "Number" => self.number_literal(node),
            "String" => self.string_literal(node),
            "Bool" => Expr::Literal(Literal::Bool(
                self.node_text(node).eq_ignore_ascii_case("true"),
            )),
            "None" => {
                if self.node_text(node).eq_ignore_ascii_case("null") {
                    Expr::Literal(Literal::Null)
                } else {
                    Expr::Literal(Literal::None)
                }
            }
            "Duration" => Expr::Literal(Literal::Duration(self.node_text(node).to_string())),
            "Regex" => Expr::Literal(Literal::Regex(
                self.node_text(node).trim_matches('/').to_string(),
            )),
            "VariableName" => Expr::Param(self.param_name(node)),
            "RecordId" => self.record_id(node),
            "Array" => Expr::Array(
                named_children(node)
                    .into_iter()
                    .map(|child| self.expr(child))
                    .collect(),
            ),
            "Object" => self.object(node),
            "BinaryExpression" => self.binary(node),
            "PrefixExpression" => self.prefix(node),
            "FunctionCall" => Expr::Call(self.call(node)),
            "TypeCast" => self.cast(node),
            "SubQuery" => self.subquery(node),
            // A bare responding statement in value position (`LET $x = SELECT
            // …`, `RETURN CREATE …`, `RETURN IF c { a } ELSE { b }`) is a
            // subquery without the parentheses: lower it to the same
            // `Expr::Subquery` so its response shape types the surrounding
            // expression (an IF-as-value unions its branch values).
            "SelectStatement" | "CreateStatement" | "UpdateStatement" | "UpsertStatement"
            | "DeleteStatement" | "InsertStatement" | "RelateStatement" | "IfElseStatement" => {
                Expr::Subquery(Box::new(super::statement::lower_statement(node, self.text)))
            }
            "Block" => Expr::Block(self.block(node)),
            "Closure" => self.closure(node),
            "Path" | "Idiom" => Expr::Idiom(self.idiom(node)),
            "Ident" => Expr::Idiom(Idiom {
                parts: vec![self.spanned(node, IdiomPart::Field(self.node_text(node).into()))],
            }),
            _ => Expr::Partial(partial(node)),
        };
        self.spanned(node, expr)
    }

    fn number_literal(&self, node: Node<'_>) -> Expr {
        // The Int/Float/Decimal child classifies; the Number node's own text
        // carries the sign.
        let text = self.node_text(node);
        let literal = match named_children(node).first().map(tree_sitter::Node::kind) {
            Some("Float") => text
                .parse::<f64>()
                .map_or(Literal::Float(0.0), Literal::Float),
            Some("Decimal") => Literal::Decimal,
            _ => text.parse::<i64>().map_or(Literal::Int(0), Literal::Int),
        };
        Expr::Literal(literal)
    }

    fn string_literal(&self, node: Node<'_>) -> Expr {
        let text = self.node_text(node);
        let bytes = text.as_bytes();
        let prefixed = bytes.len() > 2 && matches!(bytes.get(1), Some(b'\'' | b'"'));
        let inner = || text[1..].trim_matches(['\'', '"']).to_string();
        let literal = match bytes.first().map(u8::to_ascii_lowercase) {
            Some(b'd') if prefixed => Literal::Datetime(inner()),
            Some(b'u') if prefixed => Literal::Uuid(inner()),
            Some(b'r') if prefixed => Literal::Regex(inner()),
            _ => {
                let content = text
                    .trim_start_matches(['d', 'u', 'r'])
                    .trim_matches(['\'', '"']);
                Literal::String(content.to_string())
            }
        };
        Expr::Literal(literal)
    }

    fn record_id(&self, node: Node<'_>) -> Expr {
        let table = named_children(node)
            .into_iter()
            .find(|c| c.kind() == "RecordTbIdent");
        let id = named_children(node)
            .into_iter()
            .find(|c| !matches!(c.kind(), "RecordTbIdent" | "Colon"));
        match (table, id) {
            (Some(table), Some(id)) => Expr::RecordId {
                table: self.spanned(table, self.node_text(table).to_string()),
                id: node_range(id),
            },
            _ => Expr::Partial(partial(node)),
        }
    }

    fn param_name(&self, node: Node<'_>) -> String {
        self.node_text(node).trim_start_matches('$').to_string()
    }

    fn object(&self, node: Node<'_>) -> Expr {
        let mut fields = Vec::new();
        collect_object_properties(node, &mut |property| {
            let Some(key_node) = first_descendant_of_kind(property, "ObjectKey") else {
                return;
            };
            let key_leaf = single_named_child(key_node).unwrap_or(key_node);
            let key = self.spanned(
                key_leaf,
                self.node_text(key_leaf)
                    .trim_matches(['`', '"', '\''])
                    .to_string(),
            );
            let value = named_children(property)
                .into_iter()
                .rfind(|child| child.kind() != "ObjectKey");
            let value = match value {
                Some(value_node) => self.expr(value_node),
                None => self.spanned(property, Expr::Partial(partial(property))),
            };
            fields.push((key, value));
        });
        Expr::Object(fields)
    }

    fn binary(&self, node: Node<'_>) -> Expr {
        let children = named_children(node);
        let Some(op_index) = children.iter().position(|c| c.kind() == "Operator") else {
            return Expr::Partial(partial(node));
        };
        let lhs = children[..op_index]
            .iter()
            .rev()
            .find(|c| c.kind() != "Operator");
        let rhs = children[op_index + 1..]
            .iter()
            .find(|c| c.kind() != "Operator");
        let (Some(&lhs), Some(&rhs)) = (lhs, rhs) else {
            return Expr::Partial(partial(node));
        };
        let op_node = children[op_index];

        Expr::Binary {
            lhs: Box::new(self.expr(lhs)),
            op: self.spanned(op_node, binary_op(self.node_text(op_node))),
            rhs: Box::new(self.expr(rhs)),
        }
    }

    fn prefix(&self, node: Node<'_>) -> Expr {
        let children = named_children(node);
        let op_node = children.iter().find(|c| c.kind() == "Operator");
        let operand = children.iter().find(|c| c.kind() != "Operator");
        let (Some(&op_node), Some(&operand)) = (op_node, operand) else {
            return Expr::Partial(partial(node));
        };

        Expr::Prefix {
            op: self.spanned(op_node, prefix_op(self.node_text(op_node))),
            expr: Box::new(self.expr(operand)),
        }
    }

    fn call(&self, node: Node<'_>) -> Call {
        let name = first_child_of_kind(node, "FunctionName");
        let path = match name {
            Some(name) => self.spanned(name, normalize_function_path(self.node_text(name))),
            None => Spanned::new(String::new(), node_range(node)),
        };
        let args = first_child_of_kind(node, "ArgumentList")
            .map(|list| {
                named_children(list)
                    .into_iter()
                    .map(|arg| self.expr(arg))
                    .collect()
            })
            .unwrap_or_default();
        Call { path, args }
    }

    fn cast(&self, node: Node<'_>) -> Expr {
        let children = named_children(node);
        let ty = children
            .iter()
            .find(|c| matches!(c.kind(), "TypeName" | "Type"));
        let value = children
            .iter()
            .find(|c| !matches!(c.kind(), "TypeName" | "Type"));
        let (Some(&ty), Some(&value)) = (ty, value) else {
            return Expr::Partial(partial(node));
        };

        Expr::Cast {
            ty: self.type_expr(ty),
            expr: Box::new(self.expr(value)),
        }
    }

    /// Structural type lowering: names, parameterized types (`array<string>`,
    /// with `option<T>` normalized to `Optional`), unions, and literal types.
    fn type_expr(&self, node: Node<'_>) -> Spanned<TypeExpr> {
        let ty = match node.kind() {
            "TypeName" => TypeExpr::Name(self.spanned(node, self.node_text(node).to_string())),
            "Type" => match single_named_child(node) {
                Some(child) => return self.type_expr(child),
                None => TypeExpr::Partial(partial(node)),
            },
            "ParameterizedType" => {
                let children = named_children(node);
                let Some((name_node, args)) = children.split_first() else {
                    return self.spanned(node, TypeExpr::Partial(partial(node)));
                };
                let name = self.spanned(*name_node, self.node_text(*name_node).to_string());
                let args: Vec<_> = args.iter().map(|arg| self.type_expr(*arg)).collect();
                // `option<T>` is sugar for an optional type.
                if name.node.eq_ignore_ascii_case("option") && args.len() == 1 {
                    TypeExpr::Optional(Box::new(
                        args.into_iter().next().expect("one option argument"),
                    ))
                } else {
                    TypeExpr::Parameterized { name, args }
                }
            }
            "UnionType" => {
                let variants: Vec<_> = named_children(node)
                    .into_iter()
                    .filter(|child| child.kind() != "Pipe")
                    .map(|child| self.type_expr(child))
                    .collect();
                TypeExpr::Union(variants)
            }
            // `{ name: string, ... }` — an object type. The grammar nests it
            // under `LiteralType`, but the field TYPE clause can also hand it
            // to us directly, so handle both entry points.
            "ObjectType" => self.object_type(node),
            "LiteralType" => match single_named_child(node) {
                Some(value) if value.kind() == "ObjectType" => return self.type_expr(value),
                Some(value) => match self.expr(value).node {
                    Expr::Literal(literal) => TypeExpr::Literal(literal),
                    _ => TypeExpr::Partial(partial(node)),
                },
                None => TypeExpr::Partial(partial(node)),
            },
            _ => TypeExpr::Partial(partial(node)),
        };
        self.spanned(node, ty)
    }

    /// Lowers an `ObjectType` node (`{ key: T, ... }`) to a structural
    /// [`TypeExpr::Object`], recursing into each property's declared type.
    fn object_type(&self, node: Node<'_>) -> TypeExpr {
        let mut properties = Vec::new();
        collect_object_type_properties(node, &mut |property| {
            let Some(key_node) = first_descendant_of_kind(property, "ObjectKey") else {
                return;
            };
            let key_leaf = single_named_child(key_node).unwrap_or(key_node);
            let key = self.spanned(
                key_leaf,
                self.node_text(key_leaf)
                    .trim_matches(['`', '"', '\''])
                    .to_string(),
            );
            let value = named_children(property)
                .into_iter()
                .find(|child| !matches!(child.kind(), "ObjectKey" | "Colon"));
            let value = match value {
                Some(ty_node) => self.type_expr(ty_node),
                None => self.spanned(property, TypeExpr::Partial(partial(property))),
            };
            properties.push((key, value));
        });
        TypeExpr::Object(properties)
    }

    fn subquery(&self, node: Node<'_>) -> Expr {
        let inner = single_named_child(node).unwrap_or(node);
        Expr::Subquery(Box::new(super::statement::lower_statement(
            inner, self.text,
        )))
    }

    fn block(&self, node: Node<'_>) -> Block {
        let mut statements = Vec::new();
        for child in named_children(node) {
            if matches!(
                child.kind(),
                "BraceOpen" | "BraceClose" | "Comment" | "BlockComment"
            ) {
                continue;
            }
            // Recover valid statements around a broken sibling: tree-sitter may
            // nest the statement following a syntax error inside the broken
            // one's subtree, so a plain per-child lowering would drop it.
            super::statement::recover_statement(child, self.text, &mut statements);
        }
        Block { statements }
    }

    fn closure(&self, node: Node<'_>) -> Expr {
        let mut params = Vec::new();
        let mut return_ty = None;
        let mut body = None;
        let mut saw_arrow = false;

        for child in named_children(node) {
            match child.kind() {
                "Pipe" => {}
                "LookupRight" => saw_arrow = true,
                "ParamDefinition" => {
                    let mut name = None;
                    let mut ty = None;
                    for part in named_children(child) {
                        match part.kind() {
                            "VariableName" => {
                                name = Some(self.spanned(
                                    part,
                                    self.node_text(part).trim_start_matches('$').to_string(),
                                ));
                            }
                            "Type" | "TypeName" | "ParameterizedType" | "UnionType"
                            | "LiteralType" => ty = Some(self.type_expr(part)),
                            _ => {}
                        }
                    }
                    if let Some(name) = name {
                        params.push((name, ty));
                    }
                }
                "Type" | "TypeName" | "ParameterizedType" | "UnionType" | "LiteralType"
                    if saw_arrow =>
                {
                    return_ty = Some(self.type_expr(child));
                }
                _ if body.is_none() => body = Some(self.expr(child)),
                _ => {}
            }
        }

        match body {
            Some(body) => Expr::Closure(Closure {
                params,
                return_ty,
                body: Box::new(body),
            }),
            None => Expr::Partial(partial(node)),
        }
    }

    fn idiom(&self, node: Node<'_>) -> Idiom {
        let mut parts: Vec<Spanned<IdiomPart>> = Vec::new();

        for child in named_children(node) {
            if child.is_error() || child.is_missing() {
                parts.push(self.spanned(child, IdiomPart::Partial(partial(child))));
                continue;
            }
            match child.kind() {
                // `Path` nests later fields inside `Subscript`s, but `Idiom`
                // nodes (FETCH/SPLIT/GROUP paths) list bare `Ident`s
                // sequentially — a field is a field at any position.
                "Ident" => {
                    parts.push(
                        self.spanned(child, IdiomPart::Field(self.node_text(child).to_string())),
                    );
                }
                "Subscript" => self.subscript_parts(child, &mut parts),
                "Lookup" => parts.push(self.spanned(child, self.graph_part(child))),
                "Filter" => parts.push(self.spanned(child, self.filter_part(child))),
                // `Idiom` nodes carry `[*]` as a bare `Any` child rather than
                // the `Filter`/`Subscript` wrapper a `Path` uses — this is the
                // shape a `DEFINE FIELD items[*].price` path takes. It is the
                // same element step either way.
                "Any" => parts.push(self.spanned(child, IdiomPart::All)),
                // Any leading value node (`$user.name`, `fn().field`, ...).
                _ if parts.is_empty() => {
                    parts.push(self.spanned(child, IdiomPart::Start(Box::new(self.expr(child)))));
                }
                _ => parts.push(self.spanned(child, IdiomPart::Partial(partial(child)))),
            }
        }

        Idiom { parts }
    }

    fn subscript_parts(&self, node: Node<'_>, parts: &mut Vec<Spanned<IdiomPart>>) {
        for child in named_children(node) {
            let part = match child.kind() {
                "Ident" => IdiomPart::Field(self.node_text(child).to_string()),
                "Destructure" => IdiomPart::Destructure(self.destructure_fields(child)),
                "Recurse" => {
                    // `{1..3}` bounded; `{..}` / `{1..}` unbounded above.
                    let text = self.node_text(child);
                    let bounded = text.rsplit("..").next().is_some_and(|tail| {
                        tail.trim_end_matches(['}', ' '])
                            .chars()
                            .any(|c| c.is_ascii_digit())
                    });
                    IdiomPart::Recurse { bounded }
                }
                "IdiomFunction" => self.method_part(child),
                "Any" => IdiomPart::All,
                _ if child.is_error() || child.is_missing() => IdiomPart::Partial(partial(child)),
                _ => IdiomPart::Partial(partial(child)),
            };
            parts.push(self.spanned(child, part));
        }
    }

    fn destructure_fields(&self, node: Node<'_>) -> Vec<Spanned<Idiom>> {
        named_children(node)
            .into_iter()
            .filter(|child| !matches!(child.kind(), "BraceOpen" | "BraceClose"))
            .map(|child| match child.kind() {
                "Ident" => self.spanned(
                    child,
                    Idiom {
                        parts: vec![self
                            .spanned(child, IdiomPart::Field(self.node_text(child).to_string()))],
                    },
                ),
                "Path" => self.spanned(child, self.idiom(child)),
                _ => self.spanned(
                    child,
                    Idiom {
                        parts: vec![self.spanned(child, IdiomPart::Partial(partial(child)))],
                    },
                ),
            })
            .collect()
    }

    fn method_part(&self, node: Node<'_>) -> IdiomPart {
        let name = match first_child_of_kind(node, "FunctionName") {
            Some(name) => self.spanned(name, self.node_text(name).to_string()),
            None => return IdiomPart::Partial(partial(node)),
        };
        let args = first_child_of_kind(node, "ArgumentList")
            .map(|list| {
                named_children(list)
                    .into_iter()
                    .map(|arg| self.expr(arg))
                    .collect()
            })
            .unwrap_or_default();
        IdiomPart::Method { name, args }
    }

    fn graph_part(&self, node: Node<'_>) -> IdiomPart {
        let mut dir = None;
        let mut step = GraphStep {
            targets: Vec::new(),
            where_clause: None,
            reference: false,
        };

        for child in named_children(node) {
            match child.kind() {
                "LookupRight" => dir = Some(self.spanned(child, GraphDir::Out)),
                // `<-` is a graph-edge step; `<~` is a record-reference step.
                // Both alias to `LookupLeft` in the grammar, so the `~` in the
                // operator text is what distinguishes a reference traversal.
                "LookupLeft" => {
                    step.reference = self.node_text(child).contains('~');
                    dir = Some(self.spanned(child, GraphDir::In));
                }
                "LookupBoth" => dir = Some(self.spanned(child, GraphDir::Both)),
                "Ident" => step
                    .targets
                    .push(self.spanned(child, self.node_text(child).to_string())),
                "LookupSelection" => self.lookup_selection(child, &mut step),
                _ => {}
            }
        }

        match dir {
            Some(dir) => IdiomPart::Graph { dir, step },
            None => IdiomPart::Partial(partial(node)),
        }
    }

    fn lookup_selection(&self, node: Node<'_>, step: &mut GraphStep) {
        for child in named_children(node) {
            match child.kind() {
                "GraphPredicate" => {
                    // The predicate wraps the edge-table identifier.
                    match single_named_child(child) {
                        Some(ident) if ident.kind() == "Ident" => step
                            .targets
                            .push(self.spanned(ident, self.node_text(ident).to_string())),
                        _ => {}
                    }
                }
                "WhereClause" => {
                    if let Some(expr_node) = where_clause_expr(child) {
                        step.where_clause = Some(Box::new(self.expr(expr_node)));
                    }
                }
                _ => {}
            }
        }
    }

    fn filter_part(&self, node: Node<'_>) -> IdiomPart {
        let children = named_children(node);
        match children.as_slice() {
            [child] if child.kind() == "WhereClause" => match where_clause_expr(*child) {
                Some(expr_node) => IdiomPart::Where(Box::new(self.expr(expr_node))),
                None => IdiomPart::Partial(partial(node)),
            },
            [child] if child.kind() == "Any" => IdiomPart::All,
            [child] if child.kind() == "Last" => IdiomPart::Last,
            [child] if !child.is_error() => IdiomPart::Index(Box::new(self.expr(*child))),
            _ => IdiomPart::Partial(partial(node)),
        }
    }
}

/// The expression of a `WHERE <expr>` clause (the last named non-keyword child).
fn where_clause_expr(clause: Node<'_>) -> Option<Node<'_>> {
    named_children(clause)
        .into_iter()
        .rfind(|child| child.kind() != "Keyword")
}

fn binary_op(text: &str) -> BinaryOp {
    match text.to_ascii_uppercase().as_str() {
        "+" => BinaryOp::Add,
        "-" => BinaryOp::Sub,
        "*" => BinaryOp::Mul,
        "/" => BinaryOp::Div,
        "=" | "==" => BinaryOp::Eq,
        "!=" => BinaryOp::NotEq,
        "<" => BinaryOp::Lt,
        "<=" => BinaryOp::LtEq,
        ">" => BinaryOp::Gt,
        ">=" => BinaryOp::GtEq,
        "AND" | "&&" => BinaryOp::And,
        "OR" | "||" => BinaryOp::Or,
        "??" => BinaryOp::NullCoalesce,
        _ => BinaryOp::Other(text.to_string()),
    }
}

fn prefix_op(text: &str) -> PrefixOp {
    match text {
        "!" => PrefixOp::Not,
        "-" => PrefixOp::Neg,
        "+" => PrefixOp::Pos,
        _ => PrefixOp::Other(text.to_string()),
    }
}

/// `type::is::record` → `type::is_record` (matches the function analyzers'
/// canonical paths).
fn normalize_function_path(path: &str) -> String {
    path.trim().replace("::is::", "::is_")
}

fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    let children = node
        .children(&mut cursor)
        .filter(tree_sitter::Node::is_named)
        .collect();
    children
}

fn single_named_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let children = named_children(node);
    match children.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

fn first_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    named_children(node)
        .into_iter()
        .find(|child| child.kind() == kind)
}

fn first_descendant_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    for child in named_children(node) {
        if let Some(found) = first_descendant_of_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn collect_object_properties(node: Node<'_>, visit: &mut impl FnMut(Node<'_>)) {
    for child in named_children(node) {
        match child.kind() {
            "ObjectProperty" => visit(child),
            "ObjectContent" => collect_object_properties(child, visit),
            _ => {}
        }
    }
}

fn collect_object_type_properties(node: Node<'_>, visit: &mut impl FnMut(Node<'_>)) {
    for child in named_children(node) {
        match child.kind() {
            "ObjectTypeProperty" => visit(child),
            "ObjectTypeContent" => collect_object_type_properties(child, visit),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_source, ParsedSource};
    use crate::source::SourceId;

    fn parse(query: &str) -> ParsedSource {
        parse_source(SourceId::new("lower:test"), query).expect("test query parses")
    }

    /// Finds the first named node of `kind` and lowers it.
    fn lower_first(parsed: &ParsedSource, kind: &str) -> Spanned<Expr> {
        let node = find_first(parsed.tree().root_node(), kind)
            .unwrap_or_else(|| panic!("no {kind} node in {:?}", parsed.text()));
        lower_expr(node, parsed.text())
    }

    fn find_first<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .find_map(|child| find_first(child, kind));
        found
    }

    fn idiom_parts(expr: &Spanned<Expr>) -> &[Spanned<IdiomPart>] {
        match &expr.node {
            Expr::Idiom(idiom) => &idiom.parts,
            other => panic!("expected idiom, got {other:?}"),
        }
    }

    #[test]
    fn lowers_every_literal_kind_with_prefix_normalization() {
        let parsed = parse(
            "RETURN [1, -2, 2.5, 1dec, 'hi', \"there\", d'2024-01-01T00:00:00Z', u'0189-aa', r'ab+', true, false, NONE, null, 1h];",
        );

        let array = lower_first(&parsed, "Array");
        let Expr::Array(elements) = &array.node else {
            panic!("expected array, got {:?}", array.node);
        };
        let literals: Vec<_> = elements
            .iter()
            .map(|e| match &e.node {
                Expr::Literal(lit) => lit.clone(),
                other => panic!("expected literal, got {other:?}"),
            })
            .collect();

        assert_eq!(
            literals,
            vec![
                Literal::Int(1),
                Literal::Int(-2),
                Literal::Float(2.5),
                Literal::Decimal,
                Literal::String("hi".into()),
                Literal::String("there".into()),
                Literal::Datetime("2024-01-01T00:00:00Z".into()),
                Literal::Uuid("0189-aa".into()),
                Literal::Regex("ab+".into()),
                Literal::Bool(true),
                Literal::Bool(false),
                Literal::None,
                Literal::Null,
                Literal::Duration("1h".into()),
            ]
        );
    }

    #[test]
    fn lowers_param_rooted_idiom_with_start_part() {
        let parsed = parse("RETURN $user.name;");

        let path = lower_first(&parsed, "Path");
        let parts = idiom_parts(&path);

        assert_eq!(parts.len(), 2);
        let IdiomPart::Start(start) = &parts[0].node else {
            panic!("expected Start, got {:?}", parts[0].node);
        };
        assert_eq!(start.node, Expr::Param("user".into()));
        assert_eq!(parts[1].node, IdiomPart::Field("name".into()));
    }

    #[test]
    fn lowers_graph_traversal_with_per_arrow_spans_and_destructure() {
        let query = "SELECT ->likes->post.{title, id} FROM person;";
        let parsed = parse(query);

        let path = lower_first(&parsed, "Path");
        let parts = idiom_parts(&path);
        assert_eq!(parts.len(), 3);

        let IdiomPart::Graph { dir, step } = &parts[0].node else {
            panic!("expected graph part, got {:?}", parts[0].node);
        };
        assert_eq!(dir.node, GraphDir::Out);
        // The direction span covers exactly the arrow token.
        assert_eq!(
            &query[dir.span.start() as usize..dir.span.end() as usize],
            "->"
        );
        assert_eq!(step.targets.len(), 1);
        assert_eq!(step.targets[0].node, "likes");
        assert!(step.where_clause.is_none());

        let IdiomPart::Destructure(fields) = &parts[2].node else {
            panic!("expected destructure, got {:?}", parts[2].node);
        };
        let names: Vec<_> = fields
            .iter()
            .map(|idiom| match &idiom.node.parts[0].node {
                IdiomPart::Field(name) => name.clone(),
                other => panic!("expected field, got {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["title", "id"]);
    }

    #[test]
    fn lowers_filtered_graph_step_with_inline_where() {
        let parsed = parse("SELECT ->(likes WHERE since > $x)->post FROM person;");

        let path = lower_first(&parsed, "Path");
        let parts = idiom_parts(&path);

        let IdiomPart::Graph { step, .. } = &parts[0].node else {
            panic!("expected graph part, got {:?}", parts[0].node);
        };
        assert_eq!(step.targets[0].node, "likes");
        let where_clause = step.where_clause.as_ref().expect("has inline WHERE");
        assert!(matches!(where_clause.node, Expr::Binary { .. }));
    }

    #[test]
    fn lowers_index_and_where_filters_distinctly() {
        let parsed = parse("SELECT tags[0], tags[$i], tags[WHERE active] FROM person;");

        let root = parsed.tree().root_node();
        let mut paths = Vec::new();
        collect_kind(root, "Path", &mut paths);
        let lowered: Vec<_> = paths
            .iter()
            .map(|p| lower_expr(*p, parsed.text()))
            .collect();

        let by_index = idiom_parts(&lowered[0]);
        let IdiomPart::Index(index) = &by_index[1].node else {
            panic!("expected index, got {:?}", by_index[1].node);
        };
        assert_eq!(index.node, Expr::Literal(Literal::Int(0)));

        let by_param = idiom_parts(&lowered[1]);
        let IdiomPart::Index(index) = &by_param[1].node else {
            panic!("expected index, got {:?}", by_param[1].node);
        };
        assert_eq!(index.node, Expr::Param("i".into()));

        let by_where = idiom_parts(&lowered[2]);
        assert!(matches!(by_where[1].node, IdiomPart::Where(_)));
    }

    fn collect_kind<'tree>(node: Node<'tree>, kind: &str, out: &mut Vec<Node<'tree>>) {
        if node.kind() == kind {
            out.push(node);
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_kind(child, kind, out);
        }
    }

    #[test]
    fn lowers_method_call_idiom_part() {
        let parsed = parse("RETURN foo.len();");

        let path = lower_first(&parsed, "Path");
        let parts = idiom_parts(&path);

        let IdiomPart::Method { name, args } = &parts[1].node else {
            panic!("expected method, got {:?}", parts[1].node);
        };
        assert_eq!(name.node, "len");
        assert!(args.is_empty());
    }

    #[test]
    fn lowers_calls_with_normalized_paths_and_spanned_args() {
        let query = "RETURN type::is::record($id);";
        let parsed = parse(query);

        let call = lower_first(&parsed, "FunctionCall");
        let Expr::Call(call) = &call.node else {
            panic!("expected call, got {:?}", call.node);
        };

        assert_eq!(call.path.node, "type::is_record");
        assert_eq!(call.args.len(), 1);
        assert_eq!(call.args[0].node, Expr::Param("id".into()));
        assert_eq!(
            &query[call.args[0].span.start() as usize..call.args[0].span.end() as usize],
            "$id"
        );
    }

    #[test]
    fn lowers_binary_and_prefix_operators() {
        let parsed = parse("SELECT * FROM person WHERE age > 18 AND !banned;");

        let outer = lower_first(&parsed, "BinaryExpression");
        let Expr::Binary { lhs, op, rhs } = &outer.node else {
            panic!("expected binary, got {:?}", outer.node);
        };
        assert_eq!(op.node, BinaryOp::And);

        let Expr::Binary { op: inner_op, .. } = &lhs.node else {
            panic!("expected nested binary, got {:?}", lhs.node);
        };
        assert_eq!(inner_op.node, BinaryOp::Gt);

        let Expr::Prefix { op: prefix, .. } = &rhs.node else {
            panic!("expected prefix, got {:?}", rhs.node);
        };
        assert_eq!(prefix.node, PrefixOp::Not);
    }

    #[test]
    fn lowers_object_literals_with_trimmed_keys() {
        let parsed = parse("RETURN { name: 'a', \"age\": 1 };");

        let object = lower_first(&parsed, "Object");
        let Expr::Object(fields) = &object.node else {
            panic!("expected object, got {:?}", object.node);
        };

        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].0.node, "name");
        assert_eq!(fields[0].1.node, Expr::Literal(Literal::String("a".into())));
        assert_eq!(fields[1].0.node, "age");
        assert_eq!(fields[1].1.node, Expr::Literal(Literal::Int(1)));
    }

    #[test]
    fn lowers_object_and_record_union_field_types() {
        // Object field types: `{ street: string, zip: int }` — the grammar
        // nests the ObjectType under a LiteralType.
        let parsed = parse("DEFINE FIELD address ON person TYPE { street: string, zip: int };");
        let object_type =
            find_first(parsed.tree().root_node(), "ObjectType").expect("has an ObjectType node");
        let ty = lower_type_expr(object_type, parsed.text());
        let TypeExpr::Object(properties) = &ty.node else {
            panic!("expected object type, got {:?}", ty.node);
        };
        assert_eq!(properties.len(), 2);
        assert_eq!(properties[0].0.node, "street");
        assert!(matches!(&properties[0].1.node, TypeExpr::Name(n) if n.node == "string"));
        assert_eq!(properties[1].0.node, "zip");
        assert!(matches!(&properties[1].1.node, TypeExpr::Name(n) if n.node == "int"));

        // Record unions: `record<team | user | organization>` nests the table
        // names as a single UnionType argument.
        let parsed =
            parse("DEFINE FIELD owner ON thing TYPE record<team | user | organization>;");
        let param = find_first(parsed.tree().root_node(), "ParameterizedType")
            .expect("has a ParameterizedType node");
        let ty = lower_type_expr(param, parsed.text());
        let TypeExpr::Parameterized { name, args } = &ty.node else {
            panic!("expected parameterized type, got {:?}", ty.node);
        };
        assert_eq!(name.node, "record");
        assert_eq!(args.len(), 1);
        let TypeExpr::Union(variants) = &args[0].node else {
            panic!("expected union argument, got {:?}", args[0].node);
        };
        let names: Vec<_> = variants
            .iter()
            .map(|v| match &v.node {
                TypeExpr::Name(n) => n.node.clone(),
                other => panic!("expected table name, got {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["team", "user", "organization"]);
    }

    #[test]
    fn lowers_type_cast() {
        let parsed = parse("RETURN <int> '42';");

        let cast = lower_first(&parsed, "TypeCast");
        let Expr::Cast { ty, expr } = &cast.node else {
            panic!("expected cast, got {:?}", cast.node);
        };
        let TypeExpr::Name(name) = &ty.node else {
            panic!("expected type name, got {:?}", ty.node);
        };
        assert_eq!(name.node, "int");
        assert_eq!(expr.node, Expr::Literal(Literal::String("42".into())));
    }

    #[test]
    fn error_nodes_lower_to_explicit_partials() {
        // Broken input must produce Partial, never be silently skipped.
        let parsed = parse("SELECT name, FROM person;");

        let error = find_first(parsed.tree().root_node(), "ERROR").expect("input has ERROR node");
        let lowered = lower_expr(error, parsed.text());

        assert!(
            matches!(lowered.node, Expr::Partial(_)),
            "ERROR must lower to Partial, got {:?}",
            lowered.node
        );
    }

    #[test]
    fn an_idiom_node_carries_its_wildcard_as_an_element_step() {
        // `Path` wraps `[*]` in a `Filter`, but an `Idiom` node (a DEFINE FIELD
        // path, a FETCH/SPLIT/GROUP path) lists a bare `Any` child. Both are the
        // same element step — dropping it collapses `items[*].price` onto
        // `items.price`, which is a different declaration entirely.
        let parsed = parse("DEFINE FIELD items[*].price ON t TYPE string;");
        let node = find_first(parsed.tree().root_node(), "Idiom").expect("an Idiom node");
        let idiom = lower_idiom_node(node, parsed.text());

        assert!(matches!(idiom.parts[0].node, IdiomPart::Field(ref n) if n == "items"));
        assert!(matches!(idiom.parts[1].node, IdiomPart::All));
        assert!(matches!(idiom.parts[2].node, IdiomPart::Field(ref n) if n == "price"));
    }

    #[test]
    fn wildcard_and_last_filters_lower_to_their_idiom_parts() {
        let parsed = parse("SELECT tags[*], tags[$] FROM person;");

        let path = lower_first(&parsed, "Path");
        let parts = idiom_parts(&path);
        assert!(matches!(parts[0].node, IdiomPart::Field(_)));
        assert!(matches!(parts[1].node, IdiomPart::All));

        let root = parsed.tree().root_node();
        let mut paths = Vec::new();
        collect_kind(root, "Path", &mut paths);
        let last_path = lower_expr(paths[1], parsed.text());
        let parts = idiom_parts(&last_path);
        assert!(matches!(parts[1].node, IdiomPart::Last));
    }

    #[test]
    fn closures_lower_to_explicit_non_silent_variants() {
        // Closures are deliberately unmodeled: if the grammar parses one it
        // must lower to `Expr::Closure(..)`; if the grammar ERRORs on it,
        // `Expr::Partial(..)` is the honest answer. Anything else would mean
        // the closure was silently misread as a value.
        let parsed = parse("RETURN array::map([1], |$v| $v);");

        let call = lower_first(&parsed, "FunctionCall");
        let Expr::Call(call) = &call.node else {
            panic!("expected call, got {:?}", call.node);
        };
        let closure_arg = call.args.get(1).expect("closure argument present");
        assert!(
            matches!(closure_arg.node, Expr::Closure(_) | Expr::Partial(_)),
            "closure argument must be explicitly unmodeled, got {:?}",
            closure_arg.node
        );
    }
}
