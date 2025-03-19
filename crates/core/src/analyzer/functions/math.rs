use super::AnalyzerContext;
use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};

pub(super) fn analyze_math(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Get the full function name – e.g. "math::abs" or "math::clamp"
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // There must be at least two segments: "math" and the function name.
    if segments.len() < 2 {
        return Err(AnalyzerError::UnexpectedSyntax);
    }

    match segments[1] {
        // -------------------------------------------------------------------
        // FUNCTIONS THAT DO NOT EXPECT ARGUMENTS (constants)
        "e" | "pi" | "inf" | "neg_inf" | "tau" | "ln_10" | "ln_2" | "log10_2" | "log10_e"
        | "log2_10" | "log2_e" | "frac_1_pi" | "frac_1_sqrt_2" | "frac_2_pi" | "frac_2_sqrt_pi"
        | "frac_pi_2" | "frac_pi_3" | "frac_pi_4" | "frac_pi_6" | "frac_pi_8" | "sqrt_2" => {
            if !func.args().is_empty() {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        }

        // -------------------------------------------------------------------
        // FUNCTIONS THAT TAKE A SINGLE NUMBER (e.g. abs, acos, asin, atan, etc.)
        "abs" | "acos" | "acot" | "asin" | "atan" | "ceil" | "floor" | "ln" | "log10" | "log2"
        | "rad2deg" | "round" | "sign" | "sin" | "sqrt" | "tan" | "deg2rad" => {
            let args = func.args();
            if args.len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            match ctx.resolve(&args[0])? {
                Kind::Number => Ok(Kind::Number),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        }

        // "fixed" requires two number arguments: fixed(number, decimal_places)
        "fixed" => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            match (ctx.resolve(&args[0])?, ctx.resolve(&args[1])?) {
                (Kind::Number, Kind::Number) => Ok(Kind::Number),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        }

        // -------------------------------------------------------------------
        // FUNCTIONS THAT TAKE MULTIPLE NUMBER ARGUMENTS
        // clamp(number, min, max)
        "clamp" => {
            let args = func.args();
            if args.len() != 3 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            for arg in args {
                if let Kind::Number = ctx.resolve(arg)? {
                    continue;
                } else {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::Number)
        }
        // log(number, base)
        "log" => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            for arg in args {
                if let Kind::Number = ctx.resolve(arg)? {
                    continue;
                } else {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::Number)
        }
        // lerp and lerpangle both require three numbers
        "lerp" | "lerpangle" => {
            let args = func.args();
            if args.len() != 3 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            for arg in args {
                if let Kind::Number = ctx.resolve(arg)? {
                    continue;
                } else {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::Number)
        }
        // pow(number, exponent)
        "pow" => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            for arg in args {
                if let Kind::Number = ctx.resolve(arg)? {
                    continue;
                } else {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::Number)
        }

        // -------------------------------------------------------------------
        // FUNCTIONS THAT TAKE ARRAY ARGUMENTS (or an array plus a number)
        // These functions are for aggregations. Most of them accept an array of numbers and return a number.
        "mean" | "median" | "midhinge" | "min" | "mode" | "nearestrank" | "percentile"
        | "product" | "stddev" | "sum" | "spread" | "trimean" | "variance" => {
            let args = func.args();
            if args.len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            match ctx.resolve(&args[0])? {
                // Assuming your array type is Kind::Array(inner, _)
                Kind::Array(_, _) => {
                    // You might want to ensure that the inner type is a Number.
                    Ok(Kind::Number)
                }
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        }
        // "bottom" and "top" expect an array of numbers and a numeric argument (the count)
        "bottom" | "top" => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            match ctx.resolve(&args[0])? {
                Kind::Array(inner, _) => {
                    // Ensure the second argument is a Number.
                    if let Kind::Number = ctx.resolve(&args[1])? {
                        // Preserve the inner type of the array on output.
                        Ok(Kind::Array(inner, None))
                    } else {
                        Err(AnalyzerError::UnexpectedSyntax)
                    }
                }
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        }

        // -------------------------------------------------------------------
        // If the function name is not found, report an error.
        other => Err(AnalyzerError::FunctionNotFound(format!("math::{}", other))),
    }
}
