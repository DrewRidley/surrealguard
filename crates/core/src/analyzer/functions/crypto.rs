use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

pub(super) fn analyze_crypto(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Retrieve the full function name, e.g. "crypto::blake3" or "crypto::argon2::compare"
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // We need at least two segments: the "crypto" namespace and the function name or sub-namespace.
    if segments.len() < 2 {
        return Err(AnalyzerError::UnexpectedSyntax);
    }

    // If there are exactly two segments, they are simple crypto functions like blake3, md5, etc.
    if segments.len() == 2 {
        match segments[1] {
            "blake3" | "md5" | "sha1" | "sha256" | "sha512" => {
                // Each of these functions expects a single string argument and returns a string.
                if let Some(arg) = func.args().first() {
                    // Ensure the argument is a string.
                    match ctx.resolve(arg)? {
                        Kind::String => Ok(Kind::String),
                        _ => Err(AnalyzerError::UnexpectedSyntax),
                    }
                } else {
                    Err(AnalyzerError::UnexpectedSyntax)
                }
            },
            _ => return Err(AnalyzerError::FunctionNotFound(name.to_string())),
        }
    } else if segments.len() >= 3 {
        // We now expect functions in sub-modules like "argon2", "bcrypt", "pbkdf2" or "scrypt".
        match segments[1] {
            "argon2" | "bcrypt" | "pbkdf2" | "scrypt" => {
                let subcmd = segments[2];
                match subcmd {
                    "compare" => {
                        // The compare functions expect two arguments (usually both strings).
                        let args = func.args();
                        if args.len() < 2 {
                            return Err(AnalyzerError::UnexpectedSyntax);
                        }
                        // For compare we require both arguments to be strings.
                        let first_arg = ctx.resolve(&args[0])?;
                        let second_arg = ctx.resolve(&args[1])?;
                        match (first_arg, second_arg) {
                            (Kind::String, Kind::String) => Ok(Kind::Bool),
                            // For some compare functions (e.g. bcrypt::compare) the second argument
                            // might be more loosely typed. Adjust the checks here if needed.
                            _ => Err(AnalyzerError::UnexpectedSyntax)
                        }
                    },
                    "generate" => {
                        // The generate functions expect a single string argument and return a string.
                        if let Some(arg) = func.args().first() {
                            match ctx.resolve(arg)? {
                                Kind::String => Ok(Kind::String),
                                _ => Err(AnalyzerError::UnexpectedSyntax),
                            }
                        } else {
                            Err(AnalyzerError::UnexpectedSyntax)
                        }
                    },
                    _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
                }
            },
            _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
        }
    } else {
        Err(AnalyzerError::UnexpectedSyntax)
    }
}
