mod generate;

use std::path::PathBuf;
use std::process;

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {}", msg);
            eprintln!();
            print_usage();
            process::exit(1);
        }
    };

    if args.help {
        print_usage();
        return;
    }

    let schema_dir = PathBuf::from(&args.schema);
    let queries_dir = PathBuf::from(&args.queries);
    let out_path = PathBuf::from(&args.out);

    match generate::generate(&schema_dir, &queries_dir) {
        Ok(output) => {
            // Ensure parent directory exists.
            if let Some(parent) = out_path.parent() {
                if !parent.exists() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        eprintln!("error: failed to create output directory: {}", e);
                        process::exit(1);
                    }
                }
            }

            if let Err(e) = std::fs::write(&out_path, &output) {
                eprintln!("error: failed to write output file: {}", e);
                process::exit(1);
            }

            eprintln!(
                "Generated {} bytes → {}",
                output.len(),
                out_path.display()
            );
        }
        Err(msg) => {
            eprintln!("error: {}", msg);
            process::exit(1);
        }
    }
}

struct Args {
    schema: String,
    queries: String,
    out: String,
    help: bool,
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        return Err("no arguments provided".to_string());
    }

    let mut schema = None;
    let mut queries = None;
    let mut out = None;
    let mut help = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--schema" => {
                i += 1;
                schema = Some(
                    args.get(i)
                        .ok_or("--schema requires a value")?
                        .to_string(),
                );
            }
            "--queries" => {
                i += 1;
                queries = Some(
                    args.get(i)
                        .ok_or("--queries requires a value")?
                        .to_string(),
                );
            }
            "--out" | "-o" => {
                i += 1;
                out = Some(
                    args.get(i)
                        .ok_or("--out requires a value")?
                        .to_string(),
                );
            }
            "--help" | "-h" => {
                help = true;
            }
            other => {
                return Err(format!("unknown argument: {}", other));
            }
        }
        i += 1;
    }

    if help {
        return Ok(Args {
            schema: String::new(),
            queries: String::new(),
            out: String::new(),
            help: true,
        });
    }

    Ok(Args {
        schema: schema.ok_or("--schema is required")?,
        queries: queries.ok_or("--queries is required")?,
        out: out.ok_or("--out is required")?,
        help: false,
    })
}

fn print_usage() {
    eprintln!("surrealguard-typegen — generate typed TypeScript from .surql files");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("  surrealguard-typegen --schema <dir> --queries <dir> --out <file>");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("  --schema <dir>    Directory containing schema .surql files");
    eprintln!("  --queries <dir>   Directory containing query .surql files");
    eprintln!("  --out, -o <file>  Output TypeScript file path");
    eprintln!("  --help, -h        Print this help message");
    eprintln!();
    eprintln!("EXAMPLE:");
    eprintln!("  surrealguard-typegen --schema schema/ --queries queries/ --out src/generated/surql.ts");
}
