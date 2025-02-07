use serde::Deserialize;
use std::path::{Path, PathBuf};
use crate::error::{CodegenError, Result};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub version: String,
    pub language: Language,
    pub schema: SchemaConfig,
    pub queries: QueriesConfig,
    pub output: OutputConfig,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    TypeScript,
    Rust,
}

#[derive(Debug, Deserialize)]
pub struct SchemaConfig {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct QueriesConfig {
    pub path: Option<PathBuf>,
    pub src: Option<Vec<PathBuf>>,
}

#[derive(Debug, Deserialize)]
pub struct OutputConfig {
    pub path: PathBuf,
    pub format: bool,
}

impl Config {
    pub fn find_and_load(start_dir: &Path) -> Result<(Self, PathBuf)> {
        const CONFIG_FILE: &str = "surrealguard.toml";

        let mut current_dir = start_dir.to_path_buf();

        loop {
            let config_path = current_dir.join(CONFIG_FILE);
            if config_path.exists() {
                let config = Self::load(&config_path)?;
                return Ok((config, current_dir));
            }

            if !current_dir.pop() {
                return Err(CodegenError::ConfigNotFound(start_dir.to_path_buf()));
            }
        }
    }

    fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if !self.schema.path.exists() {
            return Err(CodegenError::InvalidPath(self.schema.path.clone()));
        }

        if let Some(path) = &self.queries.path {
            if !path.exists() {
                return Err(CodegenError::InvalidPath(path.clone()));
            }
        }

        if let Some(src_paths) = &self.queries.src {
            for path in src_paths {
                if !path.exists() {
                    return Err(CodegenError::InvalidPath(path.clone()));
                }
            }
        }

        if let Some(parent) = self.output.path.parent() {
            if !parent.exists() {
                return Err(CodegenError::InvalidPath(parent.to_path_buf()));
            }
        }

        Ok(())
    }
}
