//! `surrealguard.toml` workspace configuration.

use serde::Deserialize;
use surrealguard_diagnostics::LintLevel;

/// Resolved `surrealguard.toml`: the source globs, analysis settings,
/// diagnostic policy, and per-lint overrides, with every unset key already
/// filled from the defaults.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceConfig {
    /// Which files are schema, which are queries, and which to ignore.
    pub sources: SourceConfig,
    /// Strictness and the target SurrealDB version.
    pub analysis: AnalysisConfig,
    /// How findings are escalated and what suppressions must carry.
    pub diagnostics: DiagnosticConfig,
    /// Per-lint level overrides; `None` leaves the lint at its default.
    pub lints: LintConfig,
}

/// The glob sets that classify a workspace's `.surql` files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceConfig {
    /// Globs whose matches contribute `DEFINE`/`REMOVE` catalog effects.
    pub schema: Vec<String>,
    /// Globs whose matches are analyzed as queries against the schema.
    pub queries: Vec<String>,
    /// Globs excluded from both sets — dependency and build directories.
    pub ignore: Vec<String>,
}

/// Settings that steer inference and checking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisConfig {
    /// Run in strict mode, tightening otherwise-advisory checks.
    pub strict: bool,
    /// Target SurrealDB version (e.g. `"2"`, `"2.1"`); selects
    /// version-gated behavior.
    pub surrealdb_version: String,
}

/// How findings are surfaced: escalation policy and suppression rules.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticConfig {
    /// Promote every warning-level finding to an error.
    pub warnings_as_errors: bool,
    /// Require a written reason on every inline suppression.
    pub require_suppression_reasons: bool,
}

/// Per-lint level overrides read from `[lints]`; `None` keeps the lint's
/// built-in default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintConfig {
    /// Level override for `SELECT *`.
    pub select_star: Option<LintLevel>,
    /// Level override for dynamically-built queries.
    pub dynamic_query: Option<LintLevel>,
    /// Level override for selecting a permission-gated field.
    pub permission_gated_field: Option<LintLevel>,
}

/// A `surrealguard.toml` that failed to parse or carried an invalid value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    /// The human-readable failure description.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

impl WorkspaceConfig {
    /// Parses a `surrealguard.toml` document, filling every unset key from
    /// [`WorkspaceConfig::default`].
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let raw: RawWorkspaceConfig = toml::from_str(input).map_err(|error| ConfigError {
            message: error.to_string(),
        })?;

        raw.into_config()
    }
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            schema: vec!["**/*.surql".into(), "**/*.surrealql".into()],
            queries: vec!["**/*.surql".into(), "**/*.surrealql".into()],
            ignore: vec![
                "target/**".into(),
                "node_modules/**".into(),
                ".git/**".into(),
            ],
        }
    }
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            strict: false,
            surrealdb_version: "2".into(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawWorkspaceConfig {
    #[serde(default)]
    sources: RawSourceConfig,
    #[serde(default)]
    analysis: RawAnalysisConfig,
    #[serde(default)]
    diagnostics: RawDiagnosticConfig,
    #[serde(default)]
    lints: RawLintConfig,
}

#[derive(Debug, Default, Deserialize)]
struct RawSourceConfig {
    schema: Option<Vec<String>>,
    queries: Option<Vec<String>>,
    ignore: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct RawAnalysisConfig {
    strict: Option<bool>,
    surrealdb_version: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawDiagnosticConfig {
    warnings_as_errors: Option<bool>,
    require_suppression_reasons: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct RawLintConfig {
    select_star: Option<String>,
    dynamic_query: Option<String>,
    permission_gated_field: Option<String>,
}

impl RawWorkspaceConfig {
    fn into_config(self) -> Result<WorkspaceConfig, ConfigError> {
        let defaults = WorkspaceConfig::default();

        Ok(WorkspaceConfig {
            sources: SourceConfig {
                schema: self.sources.schema.unwrap_or(defaults.sources.schema),
                queries: self.sources.queries.unwrap_or(defaults.sources.queries),
                ignore: self.sources.ignore.unwrap_or(defaults.sources.ignore),
            },
            analysis: AnalysisConfig {
                strict: self.analysis.strict.unwrap_or(defaults.analysis.strict),
                surrealdb_version: self
                    .analysis
                    .surrealdb_version
                    .unwrap_or(defaults.analysis.surrealdb_version),
            },
            diagnostics: DiagnosticConfig {
                warnings_as_errors: self
                    .diagnostics
                    .warnings_as_errors
                    .unwrap_or(defaults.diagnostics.warnings_as_errors),
                require_suppression_reasons: self
                    .diagnostics
                    .require_suppression_reasons
                    .unwrap_or(defaults.diagnostics.require_suppression_reasons),
            },
            lints: LintConfig {
                select_star: parse_lint_level(self.lints.select_star, "lints.select_star")?,
                dynamic_query: parse_lint_level(self.lints.dynamic_query, "lints.dynamic_query")?,
                permission_gated_field: parse_lint_level(
                    self.lints.permission_gated_field,
                    "lints.permission_gated_field",
                )?,
            },
        })
    }
}

fn parse_lint_level(value: Option<String>, key: &str) -> Result<Option<LintLevel>, ConfigError> {
    value
        .map(|value| match value.as_str() {
            "allow" => Ok(LintLevel::Allow),
            "warn" => Ok(LintLevel::Warn),
            "deny" => Ok(LintLevel::Deny),
            _ => Err(ConfigError {
                message: format!(
                    "invalid lint level for {key}: expected allow, warn, or deny; got {value:?}"
                ),
            }),
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_workspace_config() {
        let config = WorkspaceConfig::from_toml_str(
            r#"
[sources]
schema = ["schema/**/*.surql", "migrations/**/*.surql"]
queries = ["queries/**/*.surql"]

[analysis]
strict = true
surrealdb_version = "2.1"

[diagnostics]
warnings_as_errors = true
require_suppression_reasons = true

[lints]
select_star = "warn"
dynamic_query = "deny"
permission_gated_field = "warn"
"#,
        )
        .expect("config parses");

        assert_eq!(
            config.sources.schema,
            vec!["schema/**/*.surql", "migrations/**/*.surql"]
        );
        assert_eq!(config.sources.queries, vec!["queries/**/*.surql"]);
        assert!(config.analysis.strict);
        assert_eq!(config.analysis.surrealdb_version, "2.1");
        assert!(config.diagnostics.warnings_as_errors);
        assert!(config.diagnostics.require_suppression_reasons);
        assert_eq!(config.lints.select_star, Some(LintLevel::Warn));
        assert_eq!(config.lints.dynamic_query, Some(LintLevel::Deny));
        assert_eq!(config.lints.permission_gated_field, Some(LintLevel::Warn));
    }

    #[test]
    fn default_config_includes_surrealql_sources_and_ignores_dependency_dirs() {
        let config = WorkspaceConfig::default();

        assert_eq!(config.sources.schema, vec!["**/*.surql", "**/*.surrealql"]);
        assert_eq!(config.sources.queries, vec!["**/*.surql", "**/*.surrealql"]);
        assert!(config.sources.ignore.contains(&"target/**".into()));
        assert!(config.sources.ignore.contains(&"node_modules/**".into()));
        assert!(config.sources.ignore.contains(&".git/**".into()));
        assert!(!config.analysis.strict);
        assert_eq!(config.analysis.surrealdb_version, "2");
    }
}
