//! Expression and idiom lowering.
//!
//! CST shapes this encodes (verified against the grammar, see
//! `examples/dump_cst.rs` for the inspection tool):
//!
//! - `Number` wraps an `Int`/`Float`/`Decimal` child; a sign directly on a
//!   literal is part of the `Number` text (`-5`), while a sign on anything
//!   else is a `PrefixExpression` (`-$x`). Suffixes (`1.5f`, `1dec`) and `_`
//!   separators are part of the token text.
//! - `String` has no children; `d'…'`/`u'…'`/`r'…'`/`b'…'`/`f'…'` prefixes
//!   select datetime/uuid/regex/bytes/file literals, normalized here.
//! - `Constant` wraps a `FunctionName` with no argument list (`math::pi`).
//! - `Range` is `[start?, RangeOp, end?]`; `RangeRecordId` wraps a
//!   `RecordId` in pipes (`|t:1..10|`).
//! - `None` covers both `NONE` and `null`, distinguished by text.
//! - `Path` is `[start, subscript/lookup/filter...]` where `start` is an
//!   `Ident` (row field) or a value node like `VariableName` (`$user.name`).
//! - `Subscript` carries `.field`, `.{destructure}`, or `.method()`.
//! - `Filter` carries `[index-expr]` or `[WHERE …]` — one node, two meanings.
//! - `Lookup` is one graph step: direction token plus a bare edge `Ident`,
//!   `Any` (`->?`), or a `LookupSelection` (`(edge WHERE …)`, whose
//!   `GraphPredicate` children carry the targets). A `LookupSelection` may
//!   also hold a `GraphFieldSelection` (`(SELECT a FROM edge)`), which
//!   reshapes the rows the step reached — so one `Lookup` can lower to a
//!   graph part *plus* the path parts that reshape it.

use tree_sitter::Node;

use super::{is_broken, node_range, partial};
use crate::ast::{
    BinaryOp, Block, Call, Closure, Expr, GraphDir, GraphStep, Idiom, IdiomPart, Knn, Literal,
    PrefixOp, Range, Spanned, TypeExpr,
};
use crate::span::ByteRange;

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
        if is_broken(node) {
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
            // `|t:10|` / `|t:1..10|` — a record range in pipes denotes many
            // records of the table, so it lowers to the record id it wraps
            // with the range flag forced on.
            "RangeRecordId" => match first_child_of_kind(node, "RecordId") {
                Some(inner) => match self.record_id(inner) {
                    Expr::RecordId { table, id, .. } => Expr::RecordId {
                        table,
                        id,
                        range: true,
                    },
                    other => other,
                },
                None => Expr::Partial(partial(node)),
            },
            "Constant" => {
                Expr::Constant(self.spanned(node, self.node_text(node).trim().to_ascii_lowercase()))
            }
            "Range" => self.range(node),
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
        // carries the sign. The kind suffix (`1.5f`, `1dec`) and `_`
        // separators are part of the token and must go before parsing — a
        // parse failure is not a zero.
        let text: String = self
            .node_text(node)
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '_')
            .collect();
        let literal = match named_children(node).first().map(tree_sitter::Node::kind) {
            Some("Float") => {
                let digits = text.strip_suffix(['f', 'F']).unwrap_or(&text);
                match digits.parse::<f64>() {
                    Ok(value) => Literal::Float(value),
                    Err(_) => return Expr::Partial(partial(node)),
                }
            }
            Some("Decimal") => Literal::Decimal,
            _ => match text.parse::<i64>() {
                Ok(value) => Literal::Int(value),
                // An integer literal past `i64` is a real value the engine
                // accepts (it widens); it just has no payload we can keep.
                Err(_)
                    if text
                        .trim_start_matches(['-', '+'])
                        .bytes()
                        .all(|b| b.is_ascii_digit()) =>
                {
                    Literal::Decimal
                }
                Err(_) => return Expr::Partial(partial(node)),
            },
        };
        Expr::Literal(literal)
    }

    /// `a..b` / `..=b` / `a>..` — either bound may be missing; the `RangeOp`
    /// token spells the inclusivity.
    fn range(&self, node: Node<'_>) -> Expr {
        let children = named_children(node);
        let Some(op_index) = children.iter().position(|c| c.kind() == "RangeOp") else {
            return Expr::Partial(partial(node));
        };
        let op_text = self.node_text(children[op_index]);
        let start = children[..op_index]
            .last()
            .map(|child| Box::new(self.expr(*child)));
        let end = children
            .get(op_index + 1)
            .map(|child| Box::new(self.expr(*child)));
        Expr::Range(Range {
            start,
            end,
            start_exclusive: op_text.starts_with('>'),
            end_inclusive: op_text.ends_with('='),
        })
    }

    fn string_literal(&self, node: Node<'_>) -> Expr {
        let text = self.node_text(node);
        let bytes = text.as_bytes();
        let prefixed = bytes.len() > 2 && matches!(bytes.get(1), Some(b'\'' | b'"'));
        let inner = || text[1..].trim_matches(['\'', '"']).to_string();
        let literal = match bytes.first().map(u8::to_ascii_lowercase) {
            Some(b'd') if prefixed => Literal::Datetime(inner()),
            Some(b'u') if prefixed => Literal::Uuid(inner()),
            // `r'table:id'` is a record id, not a regex — regex literals are
            // written `/…/`.
            Some(b'r') if prefixed => return self.record_id_string(node),
            Some(b'b') if prefixed => Literal::Bytes(inner()),
            Some(b'f') if prefixed => Literal::File(inner()),
            // `s'…'` is an explicitly plain string.
            Some(b's') if prefixed => Literal::String(inner()),
            _ => Literal::String(text.trim_matches(['\'', '"']).to_string()),
        };
        Expr::Literal(literal)
    }

    /// `r'account:ada'` — a record id written as a prefixed string. The
    /// engine parses the inner text as a record id (`type::of(r'a:b')` is
    /// `record` and `r'a:1' = a:1` holds on 3.2.3) and rejects anything
    /// without the `table:id` shape (`r'notarecord'`, `r'a:'`, `r':b'` are
    /// parse errors), so the same shape lowers to [`Expr::RecordId`] with
    /// both halves spanned inside the quotes, and anything else stays an
    /// explicit `Partial` rather than a mistyped string.
    fn record_id_string(&self, node: Node<'_>) -> Expr {
        let text = self.node_text(node);
        let range = node_range(node);
        // Past the prefix letter and the opening quote, up to the closing one.
        let inner = text.get(2..).unwrap_or("");
        let inner = inner.strip_suffix(['\'', '"']).unwrap_or(inner);
        let Some(colon) = inner.find(':') else {
            return Expr::Partial(partial(node));
        };
        let (table, id) = (&inner[..colon], &inner[colon + 1..]);
        if table.is_empty() || id.is_empty() {
            return Expr::Partial(partial(node));
        }
        let inner_start = range.start() + 2;
        let (Ok(colon_at), Ok(inner_len)) = (u32::try_from(colon), u32::try_from(inner.len()))
        else {
            return Expr::Partial(partial(node));
        };
        let (Ok(table_range), Ok(id_range)) = (
            ByteRange::new(inner_start, inner_start + colon_at),
            ByteRange::new(inner_start + colon_at + 1, inner_start + inner_len),
        ) else {
            return Expr::Partial(partial(node));
        };
        Expr::RecordId {
            table: Spanned::new(table.to_string(), table_range),
            id: id_range,
            range: false,
        }
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
                range: id.kind() == "RecordIdRange",
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
            op: self.spanned(op_node, self.binary_op(op_node)),
            rhs: Box::new(self.expr(rhs)),
        }
    }

    /// The operator an `Operator` node spells. Keyword operators may be
    /// written in any case; `IS NOT` / `NOT IN` are two tokens with
    /// arbitrary whitespace between them; `@n@` and `<|k, …|>` carry their
    /// parameters as child nodes.
    fn binary_op(&self, node: Node<'_>) -> BinaryOp {
        let text = self.node_text(node);
        let words: Vec<String> = text
            .split_whitespace()
            .map(str::to_ascii_uppercase)
            .collect();
        let joined = words.join(" ");
        if let Some(op) = simple_binary_op(&joined) {
            return op;
        }
        if text.starts_with('@') {
            let reference = first_child_of_kind(node, "Number")
                .and_then(|number| self.node_text(number).trim().parse::<i64>().ok());
            return BinaryOp::Matches(reference);
        }
        if text.starts_with("<|") {
            let mut knn = Knn::default();
            let mut numbers = named_children(node)
                .into_iter()
                .filter(|child| child.kind() == "Number")
                .filter_map(|number| self.node_text(number).trim().parse::<i64>().ok());
            knn.k = numbers.next();
            knn.ef = numbers.next();
            knn.distance = first_child_of_kind(node, "Distance")
                .map(|distance| self.node_text(distance).trim().to_ascii_uppercase());
            return BinaryOp::Knn(knn);
        }
        BinaryOp::Other(text.to_string())
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

    /// `<T> value` — the target is any type expression the grammar admits
    /// (`<set<int>>`, `<option<int>>`, `<record<t>>`, `<int | string>`,
    /// `<geometry<point>>`), lowered by the same [`Self::type_expr`] a
    /// `DEFINE FIELD … TYPE` clause uses; the operand is whatever value
    /// follows it.
    fn cast(&self, node: Node<'_>) -> Expr {
        let children = named_children(node);
        let ty = children.iter().find(|c| is_type_node(c.kind()));
        let value = children.iter().find(|c| !is_type_node(c.kind()));
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
                // `array<T, 3>` / `set<T, 3>` — the trailing `Number` is a
                // length bound, not a type argument; the element type is the
                // structure analysis reads.
                let args: Vec<_> = args
                    .iter()
                    .filter(|arg| arg.kind() != "Number")
                    .map(|arg| self.type_expr(*arg))
                    .collect();
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

    /// `( … )` — either grouping parentheses around a value, or a genuine
    /// subquery around a statement.
    ///
    /// Parentheses are a semantic no-op in SurrealQL: `(email + 1)` **is**
    /// `email + 1`, so it must lower to the inner expression and be inferred,
    /// checked and narrowed identically. Wrapping it in an `Expr::Subquery`
    /// instead put an opaque node in front of every consumer that matches on
    /// expression shape, silently disabling narrowing, field validation and
    /// every expression-level diagnostic behind a pair of parentheses.
    /// Grouping still holds: the CST already nests `(a + b) * c` as
    /// `Binary(Binary(a,+,b), *, c)`, and the outer span (applied by
    /// [`Self::expr`]) keeps covering the parentheses for diagnostics.
    ///
    /// Only a *statement* inside the parentheses (`(SELECT …)`, `({ … })`,
    /// `(THROW …)`) is a real subquery value.
    fn subquery(&self, node: Node<'_>) -> Expr {
        let Some(inner) = subquery_content(node) else {
            return Expr::Partial(partial(node));
        };
        if !is_statement_kind(inner.kind()) {
            return self.expr(inner).node;
        }
        Expr::Subquery(Box::new(super::statement::lower_statement(
            inner, self.text,
        )))
    }

    fn block(&self, node: Node<'_>) -> Block {
        let mut statements = Vec::new();
        for child in named_children(node) {
            if matches!(child.kind(), "BraceOpen" | "BraceClose") {
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
            if is_broken(child) {
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
                "Optional" => parts.push(self.spanned(child, IdiomPart::Optional)),
                "Flatten" => parts.push(self.spanned(child, IdiomPart::Flatten)),
                "Subscript" => self.subscript_parts(child, &mut parts),
                // One `Lookup` can lower to more than one part: a `SELECT …
                // FROM` inside it reshapes what the step reached, and that is
                // a path part of its own.
                "Lookup" => self.graph_parts(child, &mut parts),
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
                "Optional" => IdiomPart::Optional,
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

    fn graph_parts(&self, node: Node<'_>, parts: &mut Vec<Spanned<IdiomPart>>) {
        let mut dir = None;
        let mut step = GraphStep {
            targets: Vec::new(),
            where_clause: None,
            limit: None,
            start: None,
            reference: false,
            wildcard: false,
            alias: None,
            unmodeled: Vec::new(),
        };
        let mut selection = None;

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
                // Unparenthesized `->?` — the same wildcard the parenthesized
                // `->(?)` spells.
                "Any" => step.wildcard = true,
                "LookupSelection" => selection = self.lookup_selection(child, &mut step),
                _ => {}
            }
        }

        let Some(dir) = dir else {
            parts.push(self.spanned(node, IdiomPart::Partial(partial(node))));
            return;
        };
        // The selection reshapes what the step *reached*, so it lowers to the
        // path parts that follow the step — and it may add to `unmodeled`,
        // which is why the step is built before it is pushed.
        let selected = selection
            .map(|fields| self.graph_selection_parts(fields, &mut step))
            .unwrap_or_default();
        parts.push(self.spanned(node, IdiomPart::Graph { dir, step }));
        parts.extend(selected);
    }

    /// `->(SELECT a, b FROM t)` projects fields off the rows the step reached,
    /// which is what `->t.{a, b}` does — the same result, key for key
    /// (SurrealDB 3.2.3). `SELECT *` is `.*` and `SELECT VALUE a` is `.a`, on
    /// the same evidence. Lowering the three to the path parts they are
    /// equivalent to routes them through the resolvers and checks the
    /// unparenthesized spellings already go through, instead of teaching those
    /// a second spelling. Without it the projection kept the *link* type the
    /// step would have had without a selection, which is simply the wrong type.
    ///
    /// A projection with no path equivalent is left unmodeled rather than
    /// guessed at: an alias (`a AS b`) or a computed value has no destructure
    /// form, and a dotted projection is not the flat key a destructure would
    /// make of it (`SELECT author.name` nests under `author`; `.{author.name}`
    /// is not even valid SurrealQL).
    fn graph_selection_parts(
        &self,
        fields: Node<'_>,
        step: &mut GraphStep,
    ) -> Vec<Spanned<IdiomPart>> {
        let mut value = false;
        let mut wildcard = None;
        let mut selected: Vec<Spanned<Idiom>> = Vec::new();
        let mut modeled = true;

        for child in named_children(fields) {
            match child.kind() {
                "Keyword" if self.node_text(child).eq_ignore_ascii_case("value") => value = true,
                "Keyword" => {}
                "Any" => wildcard = Some(child),
                "Predicate" => match single_named_child(child) {
                    Some(inner) if inner.kind() == "Ident" => selected.push(self.spanned(
                        inner,
                        Idiom {
                            parts: vec![self.spanned(
                                inner,
                                IdiomPart::Field(self.node_text(inner).to_string()),
                            )],
                        },
                    )),
                    // A multi-part path only survives under `VALUE`, which
                    // hands the value back whole rather than keying it.
                    Some(inner) if value && matches!(inner.kind(), "Path" | "Idiom") => {
                        selected.push(self.spanned(inner, self.idiom(inner)));
                    }
                    _ => modeled = false,
                },
                _ => modeled = false,
            }
        }

        let parts = match (modeled, value, wildcard, selected.as_slice()) {
            (true, false, Some(star), []) => vec![self.spanned(star, IdiomPart::All)],
            (true, true, None, [only]) => only.node.parts.clone(),
            (true, false, None, [_, ..]) => {
                vec![self.spanned(fields, IdiomPart::Destructure(selected.clone()))]
            }
            _ => {
                step.unmodeled.push(partial(fields));
                Vec::new()
            }
        };
        parts
    }

    fn lookup_selection<'tree>(
        &self,
        node: Node<'tree>,
        step: &mut GraphStep,
    ) -> Option<Node<'tree>> {
        let mut selection = None;
        // The keyword that owns the next bare identifier: `AS alias` names
        // the step's result key, `FIELD f` restricts a reference traversal to
        // one referencing field.
        let mut ident_owner: Option<String> = None;
        for child in named_children(node) {
            match child.kind() {
                "Keyword" => {
                    ident_owner = Some(self.node_text(child).to_ascii_uppercase());
                }
                "Ident" if ident_owner.as_deref() == Some("AS") => {
                    step.alias = Some(self.spanned(child, self.node_text(child).to_string()));
                    ident_owner = None;
                }
                "Ident" => {}
                "GraphPredicate" => {
                    // The grammar admits any value in target position; what
                    // SurrealDB accepts is a table name, `?`, or a record
                    // range. Keeping only the first of those left everything
                    // else looking like a step that named nothing.
                    match single_named_child(child) {
                        Some(inner) if inner.kind() == "Ident" => step
                            .targets
                            .push(self.spanned(inner, self.node_text(inner).to_string())),
                        Some(inner) if inner.kind() == "Any" => step.wildcard = true,
                        // `->(post:1..9)` walks to `post` records, exactly as
                        // `->post` does — only the ids are narrowed.
                        Some(inner) if inner.kind() == "RecordId" => {
                            match record_range_table(inner) {
                                Some(table) => step
                                    .targets
                                    .push(self.spanned(table, self.node_text(table).to_string())),
                                None => step.unmodeled.push(partial(inner)),
                            }
                        }
                        Some(inner) => step.unmodeled.push(partial(inner)),
                        None => step.unmodeled.push(partial(child)),
                    }
                }
                "WhereClause" => {
                    if let Some(expr_node) = clause_value(child) {
                        step.where_clause = Some(Box::new(self.expr(expr_node)));
                    }
                }
                // `(SELECT a, b FROM t …)` — handed back so the caller can
                // append the path parts it is equivalent to *after* the step.
                "GraphFieldSelection" => {
                    selection = first_child_of_kind(child, "Fields");
                }
                // `(likes LIMIT 3 START 1)` — the same two clauses a SELECT
                // writes, under the same contract, so they are kept for the
                // same check rather than dropped as decoration.
                "GraphLimitStartComboClause" => {
                    for clause in named_children(child) {
                        let target = match clause.kind() {
                            "LimitClause" => &mut step.limit,
                            "StartClause" => &mut step.start,
                            _ => continue,
                        };
                        if let Some(value) = clause_value(clause) {
                            *target = Some(Box::new(self.expr(value)));
                        }
                    }
                }
                _ => {}
            }
        }
        selection
    }

    fn filter_part(&self, node: Node<'_>) -> IdiomPart {
        let children = named_children(node);
        match children.as_slice() {
            [child] if child.kind() == "WhereClause" => match clause_value(*child) {
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

/// The table half of a record *range* id (`post:1..9` → `post`). `None` for a
/// single record id (`post:one`), which SurrealDB rejects in graph-target
/// position and which therefore names no traversal target.
fn record_range_table<'tree>(record_id: Node<'tree>) -> Option<Node<'tree>> {
    let children = named_children(record_id);
    let is_range = children.iter().any(|child| child.kind() == "RecordIdRange");
    is_range
        .then(|| children.into_iter().find(|c| c.kind() == "RecordTbIdent"))
        .flatten()
}

/// The value of a keyword-led clause — `LIMIT 3` → `3`, `WHERE a = b` → the
/// comparison: the last named child that is not the keyword itself.
fn clause_value<'tree>(clause: Node<'tree>) -> Option<Node<'tree>> {
    named_children(clause)
        .into_iter()
        .rfind(|child| child.kind() != "Keyword")
}

/// The parameterless operators, keyed by their whitespace-normalized,
/// uppercased spelling.
fn simple_binary_op(text: &str) -> Option<BinaryOp> {
    Some(match text {
        "+" => BinaryOp::Add,
        "-" => BinaryOp::Sub,
        "*" | "×" => BinaryOp::Mul,
        "/" | "÷" => BinaryOp::Div,
        "%" => BinaryOp::Rem,
        "**" => BinaryOp::Pow,
        "=" => BinaryOp::Eq,
        "==" => BinaryOp::Exact,
        "!=" => BinaryOp::NotEq,
        "<" => BinaryOp::Lt,
        "<=" => BinaryOp::LtEq,
        ">" => BinaryOp::Gt,
        ">=" => BinaryOp::GtEq,
        "AND" | "&&" => BinaryOp::And,
        "OR" | "||" => BinaryOp::Or,
        "??" => BinaryOp::NullCoalesce,
        "?:" => BinaryOp::TruthyCoalesce,
        "IS" => BinaryOp::Is,
        "IS NOT" => BinaryOp::IsNot,
        "IN" => BinaryOp::In,
        "NOT IN" => BinaryOp::NotIn,
        "CONTAINS" | "∋" => BinaryOp::Contains,
        "CONTAINSNOT" | "∌" => BinaryOp::ContainsNot,
        "CONTAINSALL" | "⊇" => BinaryOp::ContainsAll,
        "CONTAINSANY" | "⊃" => BinaryOp::ContainsAny,
        "CONTAINSNONE" | "⊅" => BinaryOp::ContainsNone,
        "INSIDE" | "∈" => BinaryOp::Inside,
        "NOTINSIDE" | "∉" => BinaryOp::NotInside,
        "ALLINSIDE" | "⊆" => BinaryOp::AllInside,
        "ANYINSIDE" | "⊂" => BinaryOp::AnyInside,
        "NONEINSIDE" | "⊄" => BinaryOp::NoneInside,
        "OUTSIDE" => BinaryOp::Outside,
        "INTERSECTS" => BinaryOp::Intersects,
        "~" => BinaryOp::Match,
        "!~" => BinaryOp::NotMatch,
        "*~" => BinaryOp::AllMatch,
        "?~" => BinaryOp::AnyMatch,
        "?=" => BinaryOp::AnyEq,
        "*=" => BinaryOp::AllEq,
        "@@" => BinaryOp::Matches(None),
        _ => return None,
    })
}

fn prefix_op(text: &str) -> PrefixOp {
    match text.to_ascii_uppercase().as_str() {
        "!" | "NOT" => PrefixOp::Not,
        "-" => PrefixOp::Neg,
        "+" => PrefixOp::Pos,
        _ => PrefixOp::Other(text.to_string()),
    }
}

/// `type::is::record` → `type::is_record` (matches the function analyzers'
/// canonical paths).
///
/// Builtin names are case-insensitive on the engine (`STRING::LEN('a')`,
/// `Count()` and `NOT(true)` all resolve on 3.2.3), so they fold to lowercase
/// — which is also what makes `NOT (x)`, lexed as a call to a function named
/// `NOT`, reach the `not` builtin. A user function keeps its case: `fn::Foo`
/// and `fn::foo` are different functions there.
fn normalize_function_path(path: &str) -> String {
    let path = path.trim();
    let is_custom = path
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("fn::"));
    if is_custom {
        path.to_string()
    } else {
        path.to_ascii_lowercase().replace("::is::", "::is_")
    }
}

/// The CST kinds a type expression can take — the target of a cast, as
/// opposed to the value being cast.
fn is_type_node(kind: &str) -> bool {
    matches!(
        kind,
        "TypeName"
            | "Type"
            | "ParameterizedType"
            | "UnionType"
            | "LiteralType"
            | "ObjectType"
            | "ArrayType"
    )
}

/// The named children of `node`, minus comments.
///
/// `Comment`/`BlockComment` are grammar *extras*, so tree-sitter may insert
/// one between any two tokens of any rule — including between an operand and
/// its operator (`n\n-- why\n= 1` puts `Comment` between `Ident` and
/// `Operator`). Every scan below picks operands positionally ("the child
/// before the operator", "the last non-keyword child"), so a comment left in
/// the list is silently selected as an operand and the real one is discarded.
/// Dropping extras here makes all of them immune by construction; no lowering
/// site ever wants a comment node.
fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    let children = node
        .children(&mut cursor)
        .filter(|child| child.is_named() && !is_comment(*child))
        .collect();
    children
}

/// Whether `node` is a comment extra. Not to be confused with `CommentClause`
/// (`DEFINE … COMMENT "…"`), which is real syntax.
fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "Comment" | "BlockComment")
}

fn single_named_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let children = named_children(node);
    match children.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The single value/statement a `SubQuery`'s parentheses wrap, ignoring
/// comments (`( /* why */ 1 )`). `None` for an empty or multi-child
/// `SubQuery`, which has no inner expression to be transparent about.
fn subquery_content<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut children = named_children(node).into_iter();
    let first = children.next()?;
    children.next().is_none().then_some(first)
}

/// The expression a `SubQuery`'s parentheses merely *group* — `Some` for
/// `(email + 1)` / `(user)`, `None` for `(SELECT …)` and for a malformed
/// `SubQuery`. Statement-position callers (a SELECT `FROM` source) use this to
/// see through grouping parentheses exactly as expression lowering does.
pub(crate) fn paren_group_inner<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let inner = subquery_content(node)?;
    (!is_statement_kind(inner.kind())).then_some(inner)
}

/// Whether a CST node kind sits in statement position. Inside a `SubQuery`,
/// these are the contents that make a genuine subquery value; everything else
/// is a grouped expression.
fn is_statement_kind(kind: &str) -> bool {
    kind.ends_with("Statement") || kind == "Block"
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
            "RETURN [1, -2, 2.5, 1dec, 'hi', \"there\", d'2024-01-01T00:00:00Z', u'0189-aa', /ab+/, true, false, NONE, null, 1h];",
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

    /// `.{}` is valid SurrealQL — `SELECT VALUE id.{} FROM ONLY user:ada`
    /// returns `{}` on 3.0.5. The grammar used to require at least one selected
    /// entry, so every such expression was a *parse* error, and a parse error
    /// is fatal to the whole source rather than to one expression.
    #[test]
    fn lowers_an_empty_destructure_without_a_parse_error() {
        let query = "SELECT id.{} FROM user;";
        let parsed = parse(query);
        assert!(
            !parsed.has_error(),
            "an empty destructure must parse: {:?}",
            parsed.syntax_diagnostics()
        );

        let path = lower_first(&parsed, "Path");
        let parts = idiom_parts(&path);

        let IdiomPart::Destructure(fields) = &parts[1].node else {
            panic!("expected destructure, got {:?}", parts[1].node);
        };
        assert!(fields.is_empty(), "nothing was selected");
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

    /// Everything a `LookupSelection`'s target position can hold, and what
    /// each of them names. Only an identifier, `?`, and a record range are
    /// legal SurrealQL (3.2.3 rejects the rest with a parse error), but the
    /// vendored grammar admits any value here — so lowering has to say which
    /// it saw. Dropping the ones it did not model left the step looking like
    /// it named nothing, which is indistinguishable from `->()`.
    #[test]
    fn lowers_every_parenthesized_graph_target_the_grammar_admits() {
        fn step_of(query: &str, index: usize) -> GraphStep {
            let parsed = parse(query);
            let path = lower_first(&parsed, "Path");
            let parts = idiom_parts(&path);
            let IdiomPart::Graph { step, .. } = &parts[index].node else {
                panic!("expected graph part, got {:?}", parts[index].node);
            };
            step.clone()
        }

        // A bare name in parentheses is the bare name.
        let step = step_of("SELECT ->(likes) FROM person;", 0);
        assert_eq!(step.targets[0].node, "likes");
        assert!(!step.wildcard && step.unmodeled.is_empty());

        // Several names stay several names.
        let step = step_of("SELECT ->(likes, follows) FROM person;", 0);
        let names: Vec<_> = step.targets.iter().map(|t| t.node.clone()).collect();
        assert_eq!(names, vec!["likes", "follows"]);

        // `?` is a wildcard, parenthesized or not — not a missing target.
        assert!(step_of("SELECT ->(?) FROM person;", 0).wildcard);
        assert!(step_of("SELECT ->? FROM person;", 0).wildcard);

        // A record *range* walks one table's records, so it names that table.
        let step = step_of("SELECT ->likes->(post:1..9) FROM person;", 1);
        assert_eq!(step.targets[0].node, "post");
        assert!(step.unmodeled.is_empty());

        // A single record id is not a range and names no table.
        let step = step_of("SELECT ->likes->(post:one) FROM person;", 1);
        assert!(step.targets.is_empty());
        assert_eq!(step.unmodeled[0].cst_kind, "RecordId");

        // A path in target position: the destructure and the splat belong
        // after the closing paren, and a nested traversal is not a target.
        for (query, cst) in [
            ("SELECT ->likes->(post.{title}) FROM person;", "Path"),
            ("SELECT ->likes->(post.*) FROM person;", "Path"),
            ("SELECT ->likes->(post->likes) FROM person;", "Path"),
        ] {
            let step = step_of(query, 1);
            assert!(step.targets.is_empty(), "for {query}");
            assert_eq!(step.unmodeled.len(), 1, "for {query}");
            assert_eq!(step.unmodeled[0].cst_kind, cst, "for {query}");
        }
    }

    /// A `SELECT … FROM` inside a graph step projects the rows the step
    /// reached, so it lowers to the path parts that do the same thing —
    /// verified against SurrealDB 3.2.3, where `->wrote->(SELECT title FROM
    /// post)` and `->wrote->post.{title}` return the identical value.
    #[test]
    fn lowers_a_graph_field_selection_to_the_path_it_is_equivalent_to() {
        fn parts_of(query: &str) -> Vec<IdiomPart> {
            let parsed = parse(query);
            let path = lower_first(&parsed, "Path");
            idiom_parts(&path)
                .iter()
                .map(|part| part.node.clone())
                .collect()
        }

        // `SELECT a, b` is `.{a, b}` — one step, then the shape it projects.
        let parts = parts_of("SELECT ->likes->(SELECT title, id FROM post) FROM person;");
        assert!(matches!(parts[0], IdiomPart::Graph { .. }));
        assert!(matches!(parts[1], IdiomPart::Graph { .. }));
        let IdiomPart::Destructure(selected) = &parts[2] else {
            panic!("expected destructure, got {:?}", parts[2]);
        };
        let names: Vec<_> = selected
            .iter()
            .map(|idiom| match &idiom.node.parts[0].node {
                IdiomPart::Field(name) => name.clone(),
                other => panic!("expected field, got {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["title", "id"]);

        // `SELECT *` is `.*`; `SELECT VALUE a.b` is `.a.b`.
        let parts = parts_of("SELECT ->likes->(SELECT * FROM post) FROM person;");
        assert!(matches!(parts[2], IdiomPart::All));
        let parts = parts_of("SELECT ->likes->(SELECT VALUE author.name FROM post) FROM person;");
        assert!(matches!(&parts[2], IdiomPart::Field(f) if f == "author"));
        assert!(matches!(&parts[3], IdiomPart::Field(f) if f == "name"));

        // An alias, a computed projection, and a dotted key have no path
        // equivalent, so the step says it did not model them rather than
        // leaving behind the link type it would have had with no selection.
        for query in [
            "SELECT ->likes->(SELECT title AS t FROM post) FROM person;",
            "SELECT ->likes->(SELECT string::len(title) FROM post) FROM person;",
            "SELECT ->likes->(SELECT author.name FROM post) FROM person;",
        ] {
            let parts = parts_of(query);
            assert_eq!(parts.len(), 2, "for {query}");
            let IdiomPart::Graph { step, .. } = &parts[1] else {
                panic!("expected graph part for {query}");
            };
            assert_eq!(step.targets[0].node, "post", "for {query}");
            assert_eq!(step.unmodeled.len(), 1, "for {query}");
            assert_eq!(step.unmodeled[0].cst_kind, "Fields", "for {query}");
        }
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
        let parsed = parse("DEFINE FIELD owner ON thing TYPE record<team | user | organization>;");
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

    #[test]
    fn grouping_parentheses_lower_to_the_inner_expression() {
        // Parentheses are a semantic no-op: `(a + b)` IS `a + b`. Lowering it
        // to an opaque `Expr::Subquery` put a wall in front of every consumer
        // that matches on expression shape, silently turning off narrowing,
        // field validation and every expression-level diagnostic.
        let parsed = parse("RETURN (1 + 2);");
        let lowered = lower_first(&parsed, "SubQuery");
        assert!(
            matches!(lowered.node, Expr::Binary { .. }),
            "a grouped expression must lower to itself, got {:?}",
            lowered.node
        );
        // The span still covers the parentheses, so diagnostics point at the
        // expression exactly as written.
        let span = lowered.span.start() as usize..lowered.span.end() as usize;
        assert_eq!(&parsed.text()[span], "(1 + 2)");

        // Nested parentheses collapse all the way down.
        let parsed = parse("RETURN (((1 + 2)));");
        let lowered = lower_first(&parsed, "SubQuery");
        assert!(matches!(lowered.node, Expr::Binary { .. }));

        // Grouping is still structural: `(a + b) * c` keeps its shape.
        let parsed = parse("RETURN (1 + 2) * 3;");
        let lowered = lower_first(&parsed, "BinaryExpression");
        let Expr::Binary { lhs, op, .. } = &lowered.node else {
            panic!("expected a binary, got {:?}", lowered.node);
        };
        assert!(matches!(op.node, BinaryOp::Mul));
        let Expr::Binary { op: inner_op, .. } = &lhs.node else {
            panic!(
                "expected the grouped binary on the left, got {:?}",
                lhs.node
            );
        };
        assert!(matches!(inner_op.node, BinaryOp::Add));
    }

    #[test]
    fn parenthesized_statements_stay_subqueries() {
        // A *statement* in parentheses is a genuine subquery value, and must
        // keep its `Expr::Subquery` wrapper — only grouping is transparent.
        let parsed = parse("RETURN (SELECT * FROM person);");
        let lowered = lower_first(&parsed, "SubQuery");
        let Expr::Subquery(inner) = &lowered.node else {
            panic!("expected a subquery, got {:?}", lowered.node);
        };
        assert!(matches!(inner.node, crate::ast::Statement::Select(_)));

        let parsed = parse("RETURN ({ RETURN 1; });");
        let lowered = lower_first(&parsed, "SubQuery");
        assert!(matches!(lowered.node, Expr::Subquery(_)));
    }

    #[test]
    fn a_comment_inside_parentheses_does_not_recurse_forever() {
        // `( /* why */ 1 )` gives the `SubQuery` two named children. Falling
        // back to the `SubQuery` node itself made lowering re-enter through
        // the bare-expression statement arm and overflow the stack.
        let parsed = parse("RETURN (/* why */ 1);");
        let lowered = lower_first(&parsed, "SubQuery");
        assert!(
            matches!(lowered.node, Expr::Literal(Literal::Int(1))),
            "a commented group is still its inner expression, got {:?}",
            lowered.node
        );
    }

    #[test]
    fn a_comment_between_operands_does_not_swallow_an_operand() {
        // Comments are grammar extras, so tree-sitter puts a `Comment` node
        // *between* an operand and its operator. Selecting operands by
        // position then picked the comment and discarded the real operand —
        // dropping an arbitrarily large side of the expression, and with it
        // every check that would have run on it.
        for query in [
            // before the operator
            "SELECT * FROM t WHERE a = 1\n  -- why\n  AND b = 2;",
            // after the operator
            "SELECT * FROM t WHERE a = 1 AND\n  -- why\n  b = 2;",
            // on both sides, both comment syntaxes
            "SELECT * FROM t WHERE a = 1 /* one */ AND -- two\n b = 2;",
            // inside grouping parentheses
            "SELECT * FROM t WHERE (a = 1 -- why\n) AND b = 2;",
            // a multi-line WHERE with a comment on every line
            "SELECT * FROM t\nWHERE -- head\n  a = 1 -- first\n  AND -- mid\n  b = 2 -- tail\n;",
        ] {
            let parsed = parse(query);
            let lowered = lower_first(&parsed, "BinaryExpression");
            let Expr::Binary { lhs, op, rhs } = &lowered.node else {
                panic!("expected a binary for {query:?}, got {:?}", lowered.node);
            };
            assert!(matches!(op.node, BinaryOp::And), "operator of {query:?}");
            assert!(
                matches!(lhs.node, Expr::Binary { .. }),
                "left operand of {query:?} was discarded: {:?}",
                lhs.node
            );
            assert!(
                matches!(rhs.node, Expr::Binary { .. }),
                "right operand of {query:?} was discarded: {:?}",
                rhs.node
            );
        }

        // The minimal shape: a comment between a bare field and its operator.
        let parsed = parse("SELECT * FROM t WHERE n\n  -- why\n  = \"x\";");
        let lowered = lower_first(&parsed, "BinaryExpression");
        let Expr::Binary { lhs, .. } = &lowered.node else {
            panic!("expected a binary, got {:?}", lowered.node);
        };
        assert!(
            matches!(&lhs.node, Expr::Idiom(idiom) if idiom.parts.len() == 1),
            "left operand should still be the field `n`, got {:?}",
            lhs.node
        );
    }

    #[test]
    fn a_comment_after_a_prefix_operator_is_not_the_operand() {
        let parsed = parse("SELECT * FROM t WHERE ! -- why\n active;");
        let lowered = lower_first(&parsed, "PrefixExpression");
        let Expr::Prefix { op, expr } = &lowered.node else {
            panic!("expected a prefix, got {:?}", lowered.node);
        };
        assert!(matches!(op.node, PrefixOp::Not));
        assert!(
            matches!(expr.node, Expr::Idiom(_)),
            "operand should be `active`, got {:?}",
            expr.node
        );
    }

    #[test]
    fn comments_do_not_displace_operands_elsewhere() {
        // Every other positional operand scan has the same exposure, so they
        // are covered by the same filter: a cast's value, an array's
        // elements, and a record id's id part.
        let parsed = parse("SELECT * FROM t WHERE <int> /* why */ a;");
        let lowered = lower_first(&parsed, "TypeCast");
        let Expr::Cast { expr, .. } = &lowered.node else {
            panic!("expected a cast, got {:?}", lowered.node);
        };
        assert!(
            matches!(expr.node, Expr::Idiom(_)),
            "cast value should be `a`, got {:?}",
            expr.node
        );

        let parsed = parse("RETURN [1, /* why */ 2];");
        let lowered = lower_first(&parsed, "Array");
        let Expr::Array(items) = &lowered.node else {
            panic!("expected an array, got {:?}", lowered.node);
        };
        assert_eq!(items.len(), 2, "a comment is not an element: {items:?}");

        let parsed = parse("RETURN { k /* why */ : 1 };");
        let lowered = lower_first(&parsed, "RecordId");
        let Expr::RecordId { table, id, .. } = &lowered.node else {
            panic!("expected a record id, got {:?}", lowered.node);
        };
        assert_eq!(table.node, "k");
        assert_eq!(&parsed.text()[id.start() as usize..id.end() as usize], "1");
    }
}
