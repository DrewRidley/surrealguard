mod config;
mod error;
mod typescript;

pub use config::{Config, Language};
pub use error::{CodegenError, Result};

use notify::Event;
use typescript::Generator as TypeScriptGenerator;

pub fn check(config: &Config) -> Result<()> {
    match config.language {
        Language::TypeScript => {
            let mut generator = TypeScriptGenerator::new();
            generator.check(config)
        }
        Language::Rust => {
            Err(CodegenError::LanguageNotImplemented("Rust".to_string()))
        }
    }
}

pub fn generate(config: &Config) -> Result<()> {
    match config.language {
        Language::TypeScript => {
            let mut generator = TypeScriptGenerator::new();
            generator.generate(config)
        }
        Language::Rust => {
            Err(CodegenError::LanguageNotImplemented("Rust".to_string()))
        }
    }
}

pub fn watch(config: &Config) -> Result<()> {
    use notify::{RecommendedWatcher, RecursiveMode, Watcher};

    println!("Initial generation...");
    generate(config)?;

    println!("Watching for changes...");
    let (tx, rx) = std::sync::mpsc::channel();

    // Convert notify errors to CodegenError
    let mut watcher = RecommendedWatcher::new(
        tx,
        notify::Config::default(),
    ).map_err(|e| CodegenError::Watch(e))?;

    // Watch schema directory
    watcher.watch(&config.schema.path, RecursiveMode::Recursive)
        .map_err(|e| CodegenError::Watch(e))?;

    // Watch query directory if specified
    if let Some(queries_path) = &config.queries.path {
        watcher.watch(queries_path, RecursiveMode::Recursive)
            .map_err(|e| CodegenError::Watch(e))?;
    }

    // Watch source directories if specified
    if let Some(src_dirs) = &config.queries.src {
        for dir in src_dirs {
            watcher.watch(dir, RecursiveMode::Recursive)
                .map_err(|e| CodegenError::Watch(e))?;
        }
    }


    let output_path = config.output.path.canonicalize().unwrap_or(config.output.path.clone());

    for res in rx {
        match res {
            Ok(event) => {
                if event.paths.contains(&output_path) {
                    continue;
                }

                println!("Change detected: {:?}", event);
                if let Err(e) = generate(config) {
                    eprintln!("Generation failed: {}", e);
                }
            }
            Err(e) => eprintln!("Watch error: {:?}", e),
        }
    }

    Ok(())
}
