//! `surrealguard.toml` workspace configuration.

use std::collections::HashMap;

use serde::Deserialize;
use surrealguard_diagnostics::{catalog, FindingCode, LintLevel, PolicyConfig};

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
///
/// The derived default is the whole default: not strict, and no target
/// version — "the latest release", so no version-gated check fires until a
/// workspace states which engine it deploys against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnalysisConfig {
    /// Run in strict mode, tightening otherwise-advisory checks.
    pub strict: bool,
    /// Target SurrealDB version (`"2"`, `"2.2"`, `"3.0.2"`), or empty for
    /// "the latest". Selects version-gated behavior: a call to a function
    /// the target lacks is 8001, syntax it lacks is 8003, syntax it removed
    /// is 8002. Parsed by [`AnalysisConfig::target_version`].
    pub surrealdb_version: String,
}

impl AnalysisConfig {
    /// The configured target as a comparable version, or `None` when the
    /// key is unset (or unparsable), which means "the latest release": no
    /// version-gated finding fires.
    pub fn target_version(&self) -> Option<TargetVersion> {
        TargetVersion::parse(&self.surrealdb_version)
    }
}

/// A SurrealDB release a fact is pinned to: the version a function or a
/// piece of syntax was added in, or removed in. Always fully specified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Major release line (`2` in `2.2.1`).
    pub major: u16,
    /// Minor release within the line.
    pub minor: u16,
    /// Patch release.
    pub patch: u16,
}

impl Version {
    /// A version literal, usable in `const` tables.
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for Version {
    /// `3.0` for a `.0` patch, `3.0.2` otherwise — the way release notes
    /// write them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

/// The `analysis.surrealdb_version` a workspace targets, as written: a
/// major alone (`"2"`), a major and minor (`"2.2"`), or all three.
///
/// An omitted component means "the latest release with this prefix", so a
/// target of `2` is every 2.x: it *predates* everything added in 3.0 and
/// nothing added in 2.3, and every 2.x removal applies to it. That is the
/// reading with no false positives — a workspace that only says "2" is not
/// told that `rand::duration` (2.3) is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetVersion {
    /// Major release line.
    pub major: u16,
    /// Minor release, when the config names one.
    pub minor: Option<u16>,
    /// Patch release, when the config names one.
    pub patch: Option<u16>,
}

impl TargetVersion {
    /// Parses `"2"`, `"2.2"`, `"v3.0.2"`; `None` for an empty or malformed
    /// value (malformed is treated as unset rather than rejected: the
    /// analyzer stays silent instead of inventing a target).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        if text.is_empty() {
            return None;
        }
        let mut parts = text.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = match parts.next() {
            Some(part) => Some(part.parse().ok()?),
            None => None,
        };
        let patch = match parts.next() {
            Some(part) => Some(part.parse().ok()?),
            None => None,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    /// Whether this target is older than `version` — i.e. lacks something
    /// `version` introduced. An omitted component is read as "latest", so
    /// `2` does not predate `2.3`, but `2.2` does.
    pub fn predates(&self, version: Version) -> bool {
        if self.major != version.major {
            return self.major < version.major;
        }
        let Some(minor) = self.minor else {
            return false;
        };
        if minor != version.minor {
            return minor < version.minor;
        }
        let Some(patch) = self.patch else {
            return false;
        };
        patch < version.patch
    }

    /// Whether this target is `version` or newer — i.e. a removal made in
    /// `version` applies to it.
    pub fn includes(&self, version: Version) -> bool {
        !self.predates(version)
    }
}

impl std::fmt::Display for TargetVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.major)?;
        if let Some(minor) = self.minor {
            write!(f, ".{minor}")?;
        }
        if let Some(patch) = self.patch {
            write!(f, ".{patch}")?;
        }
        Ok(())
    }
}

/// How findings are surfaced: escalation policy and suppression rules.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticConfig {
    /// Promote every warning-level finding to an error.
    pub warnings_as_errors: bool,
    /// Require a written reason on every inline suppression.
    pub require_suppression_reasons: bool,
}

/// Per-code level overrides read from `[lints]`.
///
/// `[lints]` accepts arbitrary diagnostic codes (`E1002`, `W7002`, or the
/// bare number `7002`), whole-family wildcards (`"7xxx"` / `"7*"`), and the
/// three legacy named lints. Codes and families resolve into [`levels`], the
/// per-[`FindingCode`] map consumed when building a [`PolicyConfig`]; a
/// specific code always overrides a family wildcard that also covers it.
///
/// [`levels`]: LintConfig::levels
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintConfig {
    /// Level override for `SELECT *` (legacy named lint).
    pub select_star: Option<LintLevel>,
    /// Level override for dynamically-built queries (legacy named lint).
    pub dynamic_query: Option<LintLevel>,
    /// Level override for selecting a permission-gated field (legacy named
    /// lint).
    pub permission_gated_field: Option<LintLevel>,
    /// Per-code overrides, with family wildcards already expanded to their
    /// concrete catalog codes.
    pub levels: HashMap<FindingCode, LintLevel>,
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

    /// Builds the [`PolicyConfig`] both surfaces (CLI, LSP) resolve findings
    /// through: applies `warnings_as_errors` and every per-code/family level
    /// override from `[lints]`. This is the single place config becomes
    /// policy, so a `surrealguard.toml` behaves identically everywhere.
    pub fn policy(&self) -> PolicyConfig {
        let mut policy = PolicyConfig::default();
        policy.set_warnings_as_errors(self.diagnostics.warnings_as_errors);
        for (code, level) in &self.lints.levels {
            policy.set_lint_level(*code, *level);
        }
        policy
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

/// The raw `[lints]` table: every key is a string, classified during
/// [`RawWorkspaceConfig::into_config`] into a named lint, a family wildcard,
/// or a specific catalog code.
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
struct RawLintConfig {
    entries: HashMap<String, String>,
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
            lints: parse_lint_config(self.lints.entries)?,
        })
    }
}

/// Classifies every `[lints]` key into a named lint, a family wildcard, or a
/// specific code, resolving levels and expanding families into concrete
/// codes. Specific codes are applied after families so a `W7002 = "allow"`
/// always overrides a `"7xxx" = "warn"` that also covers it.
fn parse_lint_config(entries: HashMap<String, String>) -> Result<LintConfig, ConfigError> {
    let mut config = LintConfig::default();
    let mut families: Vec<(u16, LintLevel)> = Vec::new();
    let mut codes: Vec<(FindingCode, LintLevel)> = Vec::new();

    for (key, value) in entries {
        let level = parse_lint_level(&value, &format!("lints.{key}"))?;
        match key.as_str() {
            "select_star" => {
                // The legacy `select_star` alias configures the bare-`SELECT *`
                // lint, 7015 (a specific code, so it wins over a family wildcard).
                config.select_star = Some(level);
                codes.push((FindingCode::from_number(7015), level));
            }
            "dynamic_query" => config.dynamic_query = Some(level),
            "permission_gated_field" => config.permission_gated_field = Some(level),
            _ => {
                if let Some(family) = parse_family_wildcard(&key) {
                    families.push((family, level));
                } else if let Some(number) = parse_code_number(&key) {
                    if catalog::entry(number).is_none() {
                        return Err(ConfigError {
                            message: format!(
                                "unknown diagnostic code in [lints]: {key:?} is not a catalog code"
                            ),
                        });
                    }
                    codes.push((FindingCode::from_number(number), level));
                } else {
                    return Err(ConfigError {
                        message: format!(
                            "unknown lint key in [lints]: {key:?}; expected a named lint, a \
                             diagnostic code like \"E1002\", or a family wildcard like \"7xxx\""
                        ),
                    });
                }
            }
        }
    }

    // Families first; specific codes override the family they fall under.
    for (family, level) in families {
        for entry in catalog::all().filter(|entry| entry.number / 1000 == family) {
            config
                .levels
                .insert(FindingCode::from_number(entry.number), level);
        }
    }
    for (code, level) in codes {
        config.levels.insert(code, level);
    }

    Ok(config)
}

fn parse_lint_level(value: &str, key: &str) -> Result<LintLevel, ConfigError> {
    match value {
        "allow" => Ok(LintLevel::Allow),
        "warn" => Ok(LintLevel::Warn),
        // `error` is an alias for `deny`: both promote the finding to an error.
        "deny" | "error" => Ok(LintLevel::Deny),
        _ => Err(ConfigError {
            message: format!(
                "invalid lint level for {key}: expected allow, warn, deny, or error; got {value:?}"
            ),
        }),
    }
}

/// A whole-family wildcard: a single family digit (`0`–`8`) followed by
/// `xxx` (any case) or `*`, e.g. `"7xxx"`, `"7XXX"`, `"7*"`. Returns the
/// family digit.
fn parse_family_wildcard(key: &str) -> Option<u16> {
    let mut chars = key.chars();
    let digit = chars.next()?.to_digit(10)?;
    if digit > 8 {
        return None;
    }
    match chars.as_str() {
        "xxx" | "XXX" | "*" => Some(digit as u16),
        _ => None,
    }
}

/// A specific code key: an optional leading severity/family letter (`E`,
/// `W`, `I`, `S`, `L`, any case) followed by the code number, e.g.
/// `"E1002"`, `"W7002"`, or the bare `"7002"`. The letter is display-only —
/// the number determines the code — so it is not validated here. Returns the
/// code number; `None` when the key is not code-shaped.
fn parse_code_number(key: &str) -> Option<u16> {
    let digits = key
        .strip_prefix(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(key);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u16>().ok()
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
    fn lints_table_accepts_specific_codes_and_family_wildcards() {
        let config = WorkspaceConfig::from_toml_str(
            r#"
[lints]
E1002 = "allow"
"7xxx" = "warn"
W7002 = "deny"
"#,
        )
        .expect("config parses");

        // A specific non-lint code resolves to its FindingCode.
        assert_eq!(
            config.lints.levels.get(&FindingCode::from_number(1002)),
            Some(&LintLevel::Allow)
        );
        // The `7xxx` family expands to every catalog code in the 7-block...
        assert_eq!(
            config.lints.levels.get(&FindingCode::from_number(7001)),
            Some(&LintLevel::Warn)
        );
        // ...but the specific `W7002` override wins over the family default.
        assert_eq!(
            config.lints.levels.get(&FindingCode::from_number(7002)),
            Some(&LintLevel::Deny)
        );
    }

    #[test]
    fn lints_table_accepts_error_as_a_deny_alias() {
        let config =
            WorkspaceConfig::from_toml_str("[lints]\nE1002 = \"error\"\n").expect("config parses");
        assert_eq!(
            config.lints.levels.get(&FindingCode::from_number(1002)),
            Some(&LintLevel::Deny)
        );
    }

    #[test]
    fn lints_table_rejects_an_unknown_code() {
        let error = WorkspaceConfig::from_toml_str("[lints]\nE9999 = \"allow\"\n")
            .expect_err("an unknown catalog code must be rejected");
        assert!(
            error.message().contains("E9999"),
            "error names the bad code: {}",
            error.message()
        );
    }

    #[test]
    fn lints_table_rejects_an_unknown_key() {
        let error = WorkspaceConfig::from_toml_str("[lints]\nnot_a_lint = \"allow\"\n")
            .expect_err("an unrecognized key must be rejected");
        assert!(error.message().contains("not_a_lint"));
    }

    #[test]
    fn lints_table_rejects_an_invalid_level() {
        let error = WorkspaceConfig::from_toml_str("[lints]\nE1002 = \"loud\"\n")
            .expect_err("an invalid level must be rejected");
        assert!(error
            .message()
            .contains("expected allow, warn, deny, or error"));
    }

    #[test]
    fn config_policy_applies_per_code_overrides_and_warnings_as_errors() {
        use surrealguard_diagnostics::Severity;

        let config = WorkspaceConfig::from_toml_str(
            r#"
[diagnostics]
warnings_as_errors = true

[lints]
E1002 = "allow"
W7002 = "deny"
"#,
        )
        .expect("config parses");
        let policy = config.policy();

        // Allowed schema code is silenced.
        assert_eq!(
            policy.resolve_severity(FindingCode::from_number(1002), Severity::Error),
            None
        );
        // Denied lint is promoted to an error.
        assert_eq!(
            policy.resolve_severity(FindingCode::from_number(7002), Severity::Hint),
            Some(Severity::Error)
        );
        // warnings_as_errors still promotes an un-overridden warning.
        assert_eq!(
            policy.resolve_severity(FindingCode::from_number(6002), Severity::Warning),
            Some(Severity::Error)
        );
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
        assert_eq!(config.analysis.surrealdb_version, "");
        assert_eq!(config.analysis.target_version(), None);
    }

    #[test]
    fn target_version_reads_an_omitted_component_as_latest() {
        let v = |major, minor, patch| Version::new(major, minor, patch);

        let two = TargetVersion::parse("2").expect("parses");
        assert!(two.predates(v(3, 0, 0)));
        assert!(!two.predates(v(2, 3, 0)), "`2` is every 2.x");
        assert!(two.includes(v(2, 0, 0)));
        assert!(!two.includes(v(3, 0, 0)));

        let two_two = TargetVersion::parse("2.2").expect("parses");
        assert!(two_two.predates(v(2, 3, 0)));
        assert!(!two_two.predates(v(2, 2, 0)));
        assert!(!two_two.predates(v(2, 2, 8)), "`2.2` is every 2.2.x");
        assert!(two_two.includes(v(2, 2, 0)));

        let three = TargetVersion::parse("v3.0.1").expect("parses");
        assert!(three.predates(v(3, 0, 2)));
        assert!(!three.predates(v(3, 0, 0)));
        assert_eq!(three.to_string(), "3.0.1");
        assert_eq!(v(3, 0, 0).to_string(), "3.0");
        assert_eq!(v(3, 0, 2).to_string(), "3.0.2");

        assert_eq!(TargetVersion::parse(""), None);
        assert_eq!(TargetVersion::parse("latest"), None);
        assert_eq!(TargetVersion::parse("2.x"), None);
        assert_eq!(TargetVersion::parse("1.2.3.4"), None);
    }
}
