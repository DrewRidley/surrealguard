use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

pub(super) fn analyze_vector(ctx: &mut AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Get full function name e.g. "vector::add"
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // Check that we are in the vector namespace.
    if segments.is_empty() || segments[0] != "vector" {
        return Err(AnalyzerError::FunctionNotFound(name.to_string()));
    }

    // Helper: for operations that take an array argument we expect the value to be of Kind::Array
    // We assume vector functions work on arrays of numbers.
    let expect_array = |arg: &surrealdb::sql::Value| -> AnalyzerResult<()> {
        match ctx.resolve(arg)? {
            Kind::Array(inner, _) => {
                // Optionally: check that the inner type is a Number
                if *inner != Kind::Number {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
                Ok(())
            }
            _ => Err(AnalyzerError::UnexpectedSyntax),
        }
    };

    // Now choose based on the second segment
    match segments.get(1) {
        // Element-wise operations: add, subtract, multiply, divide, cross, project
        Some(&"add") |
        Some(&"subtract") |
        Some(&"multiply") |
        Some(&"divide") |
        Some(&"cross") |
        Some(&"project") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Check both arguments are arrays of numbers.
            expect_array(&args[0])?;
            expect_array(&args[1])?;
            // For these operations we return an array of numbers.
            Ok(Kind::Array(Box::new(Kind::Number), None))
        },

        // Normalize takes one array, returns an array (normalized vector)
        Some(&"normalize") => {
            let args = func.args();
            if args.len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            expect_array(&args[0])?;
            Ok(Kind::Array(Box::new(Kind::Number), None))
        },

        // Scale multiplies an array by a number.
        Some(&"scale") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // First argument must be an array.
            expect_array(&args[0])?;
            // Second argument must be a number.
            match ctx.resolve(&args[1])? {
                Kind::Number => {},
                _ => return Err(AnalyzerError::UnexpectedSyntax),
            }
            Ok(Kind::Array(Box::new(Kind::Number), None))
        },

        // Angle: takes two arrays and returns a number.
        Some(&"angle") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            expect_array(&args[0])?;
            expect_array(&args[1])?;
            Ok(Kind::Number)
        },

        // Dot: computes dot product
        Some(&"dot") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            expect_array(&args[0])?;
            expect_array(&args[1])?;
            Ok(Kind::Number)
        },

        // Magnitude: takes one array and returns a number.
        Some(&"magnitude") => {
            let args = func.args();
            if args.len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            expect_array(&args[0])?;
            Ok(Kind::Number)
        },

        // Distance functions are under a sub-namespace
        Some(&"distance") => {
            let args = func.args();
            match segments.get(2) {
                Some(&"chebyshev") |
                Some(&"euclidean") |
                Some(&"hamming") |
                Some(&"manhattan") => {
                    if args.len() != 2 {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    expect_array(&args[0])?;
                    expect_array(&args[1])?;
                    Ok(Kind::Number)
                },
                Some(&"minkowski") => {
                    if args.len() != 3 {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    expect_array(&args[0])?;
                    expect_array(&args[1])?;
                    // Third argument must be a number (the power parameter).
                    match ctx.resolve(&args[2])? {
                        Kind::Number => {},
                        _ => return Err(AnalyzerError::UnexpectedSyntax),
                    }
                    Ok(Kind::Number)
                },
                Some(&"knn") => {
                    // knn does not take explicit arguments
                    if !args.is_empty() {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    Ok(Kind::Number)
                },
                _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
            }
        },

        // Similarity functions are under the "similarity" submodule.
        Some(&"similarity") => {
            let args = func.args();
            match segments.get(2) {
                Some(&"cosine") |
                Some(&"jaccard") |
                Some(&"pearson") => {
                    if args.len() != 2 {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    expect_array(&args[0])?;
                    expect_array(&args[1])?;
                    Ok(Kind::Number)
                },
                _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
            }
        },

        // If none of the known sub-functions match, return a function-not-found error.
        Some(other) => Err(AnalyzerError::FunctionNotFound(format!("vector::{}", other))),
        None => Err(AnalyzerError::UnexpectedSyntax),
    }
}
