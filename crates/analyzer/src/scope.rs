/// Variable scope system.
///
/// Tracks variable bindings at different levels with a scope stack.
/// Inner scopes shadow outer bindings. Each binding tracks:
/// - The variable name
/// - Its resolved type
/// - The span where it was defined (for "defined here" diagnostics)
/// - Whether it's mutable
use std::collections::HashMap;

use crate::span::Span;
use crate::types::Kind;

/// A single variable binding.
#[derive(Debug, Clone)]
pub struct Binding {
    pub name: String,
    pub typ: Kind,
    pub span: Span,
    pub mutable: bool,
    pub kind: BindingKind,
}

/// How the variable was introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// `LET $x = ...`
    Let,
    /// `DEFINE PARAM $x VALUE ...`
    DefineParam,
    /// `FOR $x IN ...`
    ForLoop,
    /// Closure parameter `|$x|`
    ClosureParam,
    /// Function parameter `fn::name($x: type)`
    FunctionParam,
    /// Implicit: $auth, $this, $parent, $session, $before, $after, $event, $value, $input
    Implicit,
}

/// A single scope level.
#[derive(Debug, Clone)]
struct ScopeLevel {
    bindings: HashMap<String, Binding>,
    /// What kind of scope this is (for error messages).
    #[allow(dead_code)]
    kind: ScopeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Global,
    Block,
    Function,
    Closure,
    ForLoop,
    Event,
    Permissions,
}

/// The scope stack.
#[derive(Debug, Clone)]
pub struct Scope {
    levels: Vec<ScopeLevel>,
}

impl Scope {
    pub fn new() -> Self {
        let mut scope = Self {
            levels: vec![ScopeLevel {
                bindings: HashMap::new(),
                kind: ScopeKind::Global,
            }],
        };

        // Register implicit variables that are always available.
        let implicit_span = Span::new(0, 0);
        scope.bind(Binding {
            name: "$session".into(),
            typ: Kind::Object,
            span: implicit_span,
            mutable: false,
            kind: BindingKind::Implicit,
        });

        scope
    }

    /// Push a new scope level.
    pub fn push(&mut self, kind: ScopeKind) {
        self.levels.push(ScopeLevel {
            bindings: HashMap::new(),
            kind,
        });
    }

    /// Pop the current scope level.
    pub fn pop(&mut self) {
        if self.levels.len() > 1 {
            self.levels.pop();
        }
    }

    /// Bind a variable in the current scope.
    pub fn bind(&mut self, binding: Binding) {
        if let Some(level) = self.levels.last_mut() {
            level.bindings.insert(binding.name.clone(), binding);
        }
    }

    /// Return all bindings visible in the current scope (all levels).
    pub fn all_bindings(&self) -> Vec<&Binding> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for level in self.levels.iter().rev() {
            for (name, binding) in &level.bindings {
                if seen.insert(name.clone()) {
                    result.push(binding);
                }
            }
        }
        result
    }

    /// Look up a variable, searching from innermost to outermost scope.
    pub fn lookup(&self, name: &str) -> Option<&Binding> {
        for level in self.levels.iter().rev() {
            if let Some(binding) = level.bindings.get(name) {
                return Some(binding);
            }
        }
        None
    }

    /// Check if a variable exists in the current (innermost) scope only.
    pub fn exists_in_current(&self, name: &str) -> bool {
        self.levels
            .last()
            .map(|l| l.bindings.contains_key(name))
            .unwrap_or(false)
    }

    /// Check if a variable would shadow an outer binding.
    pub fn would_shadow(&self, name: &str) -> Option<&Binding> {
        // Skip the current scope, check outer scopes.
        for level in self.levels.iter().rev().skip(1) {
            if let Some(binding) = level.bindings.get(name) {
                return Some(binding);
            }
        }
        None
    }

    /// Set implicit `$this` for the current scope (e.g., in permission clauses).
    pub fn set_this(&mut self, typ: Kind, span: Span) {
        self.bind(Binding {
            name: "$this".into(),
            typ,
            span,
            mutable: false,
            kind: BindingKind::Implicit,
        });
    }

    /// Set implicit `$auth` scope.
    pub fn set_auth(&mut self, typ: Kind, span: Span) {
        self.bind(Binding {
            name: "$auth".into(),
            typ,
            span,
            mutable: false,
            kind: BindingKind::Implicit,
        });
    }

    /// Set event-scope implicit variables.
    pub fn set_event_scope(&mut self, table_type: Kind, span: Span) {
        for name in &["$event", "$before", "$after", "$value", "$input"] {
            self.bind(Binding {
                name: name.to_string(),
                typ: table_type.clone(),
                span,
                mutable: false,
                kind: BindingKind::Implicit,
            });
        }
        self.bind(Binding {
            name: "$event".into(),
            typ: Kind::String,
            span,
            mutable: false,
            kind: BindingKind::Implicit,
        });
    }

    /// Check whether the current scope is nested inside a loop.
    pub fn is_in_loop(&self) -> bool {
        self.levels.iter().any(|l| l.kind == ScopeKind::ForLoop)
    }

    /// Current scope depth (for debugging).
    pub fn depth(&self) -> usize {
        self.levels.len()
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_lookup() {
        let mut scope = Scope::new();
        scope.bind(Binding {
            name: "$x".into(),
            typ: Kind::Int,
            span: Span::new(0, 5),
            mutable: true,
            kind: BindingKind::Let,
        });
        let b = scope.lookup("$x").unwrap();
        assert_eq!(b.typ, Kind::Int);
    }

    #[test]
    fn shadowing() {
        let mut scope = Scope::new();
        scope.bind(Binding {
            name: "$x".into(),
            typ: Kind::Int,
            span: Span::new(0, 5),
            mutable: true,
            kind: BindingKind::Let,
        });

        scope.push(ScopeKind::Block);

        // Should detect shadow
        assert!(scope.would_shadow("$x").is_some());

        scope.bind(Binding {
            name: "$x".into(),
            typ: Kind::String,
            span: Span::new(10, 15),
            mutable: true,
            kind: BindingKind::Let,
        });

        // Inner scope wins
        assert_eq!(scope.lookup("$x").unwrap().typ, Kind::String);

        scope.pop();

        // Outer scope restored
        assert_eq!(scope.lookup("$x").unwrap().typ, Kind::Int);
    }

    #[test]
    fn implicit_session() {
        let scope = Scope::new();
        assert!(scope.lookup("$session").is_some());
    }
}
