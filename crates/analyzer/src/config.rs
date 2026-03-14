/// Project configuration for SurrealGuard.
///
/// Loaded from `.surrealguard.toml` in the project root.
/// Specifies which files contain schema definitions vs queries,
/// and controls analysis behavior.
use std::fs;
use std::path::{Path, PathBuf};

/// Project configuration parsed from `.surrealguard.toml`.
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    /// Glob patterns for files containing schema definitions (DEFINE TABLE, DEFINE FIELD, etc.).
    /// These are parsed first to build the type context.
    pub schema: Vec<String>,
    /// Glob patterns for files containing queries to analyze.
    /// These are analyzed against the schema context.
    pub queries: Vec<String>,
    /// Whether to enable strict mode (errors on undefined tables/fields).
    pub strict: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            schema: vec!["schema/**/*.surql".to_string()],
            queries: vec!["queries/**/*.surql".to_string(), "**/*.surql".to_string()],
            strict: false,
        }
    }
}

/// Errors that can occur when loading config.
#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "failed to read config: {e}"),
            ConfigError::Parse(e) => write!(f, "failed to parse config: {e}"),
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl ProjectConfig {
    /// The default config file name.
    pub const FILE_NAME: &'static str = ".surrealguard.toml";

    /// Search for `.surrealguard.toml` starting from `dir` and walking up.
    /// Returns the config and the directory it was found in.
    pub fn discover(dir: &Path) -> Option<(Self, PathBuf)> {
        let mut current = dir.to_path_buf();
        loop {
            let config_path = current.join(Self::FILE_NAME);
            if config_path.is_file() {
                match Self::load(&config_path) {
                    Ok(config) => return Some((config, current)),
                    Err(_) => return None,
                }
            }
            if !current.pop() {
                break;
            }
        }
        None
    }

    /// Load config from a specific file path.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Parse config from TOML string.
    pub fn parse(content: &str) -> Result<Self, ConfigError> {
        let mut config = Self::default();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                match key {
                    "strict" => {
                        config.strict = value.trim_matches('"') == "true";
                    }
                    "schema" => {
                        config.schema = parse_string_array(value)?;
                    }
                    "queries" => {
                        config.queries = parse_string_array(value)?;
                    }
                    _ => {}
                }
            }
        }

        Ok(config)
    }
}

/// Parse a TOML-style string array: `["a", "b", "c"]`
fn parse_string_array(value: &str) -> Result<Vec<String>, ConfigError> {
    let trimmed = value.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(ConfigError::Parse(format!(
            "expected array value like [\"a\", \"b\"], got: {trimmed}"
        )));
    }

    let inner = &trimmed[1..trimmed.len() - 1];
    Ok(inner
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_default_config() {
        let config = ProjectConfig::default();
        assert_eq!(config.schema, vec!["schema/**/*.surql"]);
        assert!(!config.strict);
    }

    #[test]
    fn parse_config_toml() {
        let toml = r#"
# SurrealGuard config
schema = ["migrations/*.surql", "schema/*.surql"]
queries = ["src/**/*.surql"]
strict = "true"
"#;
        let config = ProjectConfig::parse(toml).unwrap();
        assert_eq!(config.schema, vec!["migrations/*.surql", "schema/*.surql"]);
        assert_eq!(config.queries, vec!["src/**/*.surql"]);
        assert!(config.strict);
    }

    #[test]
    fn parse_empty_config() {
        let config = ProjectConfig::parse("").unwrap();
        // Should use defaults
        assert_eq!(config.schema, vec!["schema/**/*.surql"]);
    }
}
