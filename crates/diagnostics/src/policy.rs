//! Severity policy, applied at consumption edges (rustc model): findings
//! carry their intrinsic class; [`PolicyConfig`] maps that to what a
//! surface reports — promoting warnings, adjusting lint levels, or
//! suppressing them — without ever rewriting the finding itself.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{FindingCategory, FindingCode, Severity};

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

/// A consumer's severity policy: warnings-as-errors plus per-code lint
/// levels, resolved against each finding's intrinsic class at the edge.
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

    /// Override the level for one lint code, replacing its default `Warn`.
    pub fn set_lint_level(&mut self, code: FindingCode, level: LintLevel) {
        self.lint_levels.insert(code, level);
    }

    /// Lints default to `Warn` — visible until individually allowed.
    pub fn resolve_lint_level(&self, code: FindingCode) -> LintLevel {
        self.lint_levels
            .get(&code)
            .copied()
            .unwrap_or(LintLevel::Warn)
    }

    /// Resolves a finding's effective severity for this surface, or `None`
    /// when policy silences it. Lint codes follow their [`LintLevel`];
    /// other codes are promoted per `warnings_as_errors` and otherwise keep
    /// their intrinsic `default_severity`.
    pub fn resolve_severity(
        &self,
        code: FindingCode,
        default_severity: Severity,
    ) -> Option<Severity> {
        if code.category() == FindingCategory::Lint {
            return match self.resolve_lint_level(code) {
                LintLevel::Allow => None,
                LintLevel::Warn => Some(Severity::Warning),
                LintLevel::Deny => Some(Severity::Error),
            };
        }

        if self.warnings_as_errors && default_severity == Severity::Warning {
            Some(Severity::Error)
        } else {
            Some(default_severity)
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
        let code = FindingCode::lint(7001);

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
    fn warnings_as_errors_promotes_non_lint_warnings() {
        let mut config = PolicyConfig::default();
        config.set_warnings_as_errors(true);

        assert_eq!(
            config.resolve_severity(FindingCode::param(6001), Severity::Warning),
            Some(Severity::Error)
        );
    }
}
