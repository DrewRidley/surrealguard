mod config;
mod error;
mod typescript;

pub use config::{Config, Language};
pub use error::{CodegenError, Result};

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
    use notify::{RecommendedWatcher, RecursiveMode, Watcher, Config as NotifyConfig, event::EventKind};
    use console::style;


    // Styled initial messages
    println!("{}", style("SurrealGuard").green().bold());
    println!("  {} Initial generation...", style("➜").green());
    generate(config)?;

    println!("  {} Watching for changes...", style("➜").cyan());
    let (tx, rx) = std::sync::mpsc::channel();

    // Rest of the watcher setup...
    let mut watcher = RecommendedWatcher::new(
        tx,
        NotifyConfig::default()
    ).map_err(|e| CodegenError::Watch(e))?;

    // Watch directories setup...
    watcher.watch(&config.schema.path, RecursiveMode::Recursive)
        .map_err(|e| CodegenError::Watch(e))?;

    if let Some(queries_path) = &config.queries.path {
        watcher.watch(queries_path, RecursiveMode::Recursive)
            .map_err(|e| CodegenError::Watch(e))?;
    }

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

                if !matches!(event.kind, EventKind::Modify(notify::event::ModifyKind::Data(_))) {
                    continue;
                }

                // Get the first changed file path for display
                if let Some(changed_path) = event.paths.first() {
                    let relative_path = changed_path
                        .strip_prefix(std::env::current_dir().unwrap())
                        .unwrap_or(changed_path)
                        .display();

                    println!("\n{} Changed: {}",
                        style("[⚡️GEN]").yellow().bold(),
                        style(relative_path).cyan()
                    );
                }

                match generate(config) {
                    Ok(_) => println!("  {} Types regenerated successfully",
                        style("➜").green()
                    ),
                    Err(e) => println!("  {} Generation failed: {}",
                        style("✖").red(),
                        style(e).red()
                    ),
                }
            }
            Err(e) => println!("  {} Watch error: {}",
                style("✖").red(),
                style(e).red()
            ),
        }
    }

    Ok(())
}
