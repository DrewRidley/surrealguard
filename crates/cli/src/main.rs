use clap::{Parser, Subcommand};
use std::env;
use std::fs;
use surrealguard_codegen::{self, Config, CodegenError};

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new surrealguard.toml config file
    Init,

    /// Check schema and queries without generating output
    Check,

    /// Generate code once and exit
    Run,

    /// Generate code and watch for changes
    Watch,
}

const EXAMPLE_CONFIG: &str = r#"version = "1.0"
language = "typescript"

[schema]
path = "schema/surrealql/"

[queries]
path = "queries/surrealql/"
src = ["src/"]

[output]
path = "src/queries.ts"
format = true
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            let config_path = env::current_dir()?.join("surrealguard.toml");
            if config_path.exists() {
                println!("Config file already exists at {}", config_path.display());
                return Ok(());
            }

            fs::write(&config_path, EXAMPLE_CONFIG)?;
            println!("Created surrealguard.toml");
            Ok(())
        }
        cmd => {
            match Config::find_and_load(&env::current_dir()?) {
                Ok((config, config_dir)) => {
                    env::set_current_dir(&config_dir)?;
                    println!("Using configuration from: {}", config_dir.display());

                    match cmd {
                        Commands::Check => {
                            println!("Checking schema and queries...");
                            surrealguard_codegen::check(&config)?;
                            println!("All checks passed!");
                        }
                        Commands::Run => {
                            println!("Generating code...");
                            surrealguard_codegen::generate(&config)?;
                            println!("Done!");
                        }
                        Commands::Watch => {
                            println!("Starting watch mode...");
                            surrealguard_codegen::watch(&config)?;
                        }
                        Commands::Init => unreachable!(),
                    }
                    Ok(())
                }
                Err(CodegenError::ConfigNotFound(_)) => {
                    eprintln!("Error: No surrealguard.toml found in current directory or parent directories");
                    eprintln!("Run 'surrealguard init' to create a new config file");
                    std::process::exit(1);
                }
                Err(e) => Err(e.into()),
            }
        }
    }
}
