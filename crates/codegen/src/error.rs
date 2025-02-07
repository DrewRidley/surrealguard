use thiserror::Error;
use std::path::PathBuf;

#[derive(Error, Debug)]
pub enum CodegenError {
    #[error("Config file not found in {0}")]
    ConfigNotFound(PathBuf),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Invalid path: {0}")]
    InvalidPath(PathBuf),

    #[error("Analysis error: {0}")]
    Analyzer(#[from] surrealguard_core::analyzer::error::AnalyzerError),

    #[error("Format error: {0}")]
    Format(String),

    #[error("Language {0} not implemented")]
    LanguageNotImplemented(String),

    #[error("Watch error: {0}")]
    Watch(#[from] notify::Error),
}

pub type Result<T> = std::result::Result<T, CodegenError>;
