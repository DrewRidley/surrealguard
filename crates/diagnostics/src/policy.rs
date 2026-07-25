//! Severity policy, applied at consumption edges (rustc model): findings
//! carry their intrinsic class; [`PolicyConfig`] maps that to what a
//! surface reports — promoting warnings, adjusting lint levels, or
//! suppressing them — without ever rewriting the finding itself.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{FindingCode, Severity};

/// How a surface treats one lint code: silenced, reported as a warning,
/// or promoted to an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LintLevel {
    /// Silence the lint entirely — it is never reported.
    Allow,
    /// Report the lint as a warning (the default).
    Warn,
    /// Promote the lint to an error.
    Deny,
}

/// A consumer's severity policy: warnings-as-errors plus per-code level
/// overrides, resolved against each finding's intrinsic class at the edge.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyConfig {
    warnings_as_errors: bool,
    lint_levels: HashMap<FindingCode, LintLevel>,
}

impl PolicyConfig {
    /// Enable promotion of every non-lint warning to an error.
    pub fn set_warnings_as_errors(&mut self, enabled: bool) {
        self.warnings_as_errors = enabled;
    }

    /// Override the level for one code (any family, not only lints),
    /// replacing whatever this surface would otherwise report.
    pub fn set_lint_level(&mut self, code: FindingCode, level: LintLevel) {
        self.lint_levels.insert(code, level);
    }

    /// The configured level for `code`: an explicit `[lints]` override if one
    /// exists, otherwise the catalog's rustc-style `default_level`. Codes with
    /// no catalog entry fall back to `Warn`.
    pub fn resolve_lint_level(&self, code: FindingCode) -> LintLevel {
        self.lint_levels
            .get(&code)
            .copied()
            .or_else(|| crate::catalog::entry(code.number()).map(|entry| entry.default_level))
            .unwrap_or(LintLevel::Warn)
    }

    /// Resolves a finding's effective severity for this surface, or `None`
    /// when policy silences it.
    ///
    /// An explicit per-code override (from `[lints]`) wins for *any* family:
    /// `allow` silences, `warn` reports as a warning, `deny`/`error` promotes
    /// to an error, and an explicit level is never touched by
    /// `warnings_as_errors`. With no override, the code's rustc-style
    /// `default_level` from the catalog decides: `allow` silences, `deny`
    /// errors, and `deny`/`allow` are both unaffected by `warnings_as_errors`
    /// (a `warnings_as_errors` run never un-allows an allow-by-default lint).
    /// A `warn`-by-default code reports a warning, promoted to an error only
    /// when `warnings_as_errors` is set.
    pub fn resolve_severity(
        &self,
        code: FindingCode,
        default_severity: Severity,
    ) -> Option<Severity> {
        if let Some(level) = self.lint_levels.get(&code).copied() {
            return match level {
                LintLevel::Allow => None,
                LintLevel::Warn => Some(Severity::Warning),
                LintLevel::Deny => Some(Severity::Error),
            };
        }

        // No explicit override: the catalog's default level decides. Codes
        // absent from the catalog fall back to their intrinsic severity.
        let Some(entry) = crate::catalog::entry(code.number()) else {
            if self.warnings_as_errors && default_severity == Severity::Warning {
                return Some(Severity::Error);
            }
            return Some(default_severity);
        };

        match entry.default_level {
            LintLevel::Allow => None,
            LintLevel::Deny => Some(Severity::Error),
            LintLevel::Warn => {
                if self.warnings_as_errors {
                    Some(Severity::Error)
                } else {
                    Some(Severity::Warning)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FindingCode, Severity};

    #[test]
    fn lint_policy_resolves_allow_warn_and_deny() {
        let mut config = PolicyConfig::default();
        // 7004 (control flow decided by a constant) is a Warn-by-default lint;
        // 7001 is intentionally Allow-by-default (opt-in), so it is unfit here.
        let code = FindingCode::lint(7004);

        assert_eq!(config.resolve_lint_level(code), LintLevel::Warn);
        assert_eq!(
            config.resolve_severity(code, Severity::Hint),
            Some(Severity::Warning)
        );
        config.set_lint_level(code, LintLevel::Allow);
        assert_eq!(config.resolve_severity(code, Severity::Hint), None);

        config.set_lint_level(code, LintLevel::Warn);
        assert_eq!(
            config.resolve_severity(code, Severity::Hint),
            Some(Severity::Warning)
        );

        config.set_lint_level(code, LintLevel::Deny);
        assert_eq!(
            config.resolve_severity(code, Severity::Hint),
            Some(Severity::Error)
        );
    }

    #[test]
    fn per_code_override_applies_to_non_lint_families() {
        // `E1002 = "allow"` must silence a schema-family error, and a `deny`
        // override must be honored — per-code overrides are not lint-only.
        let mut config = PolicyConfig::default();
        let schema_code = FindingCode::schema(1002);

        assert_eq!(
            config.resolve_severity(schema_code, Severity::Error),
            Some(Severity::Error)
        );
        config.set_lint_level(schema_code, LintLevel::Allow);
        assert_eq!(config.resolve_severity(schema_code, Severity::Error), None);

        config.set_lint_level(FindingCode::type_error(2005), LintLevel::Deny);
        assert_eq!(
            config.resolve_severity(FindingCode::type_error(2005), Severity::Warning),
            Some(Severity::Error)
        );
    }

    #[test]
    fn warnings_as_errors_promotes_non_lint_warnings() {
        let mut config = PolicyConfig::default();
        config.set_warnings_as_errors(true);

        assert_eq!(
            config.resolve_severity(FindingCode::param(6001), Severity::Warning),
            Some(Severity::Error)
        );
    }
}
