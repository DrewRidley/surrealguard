//! `math` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod abs;
pub mod acos;
pub mod acot;
pub mod asin;
pub mod atan;
pub mod bottom;
pub mod ceil;
pub mod clamp;
pub mod cos;
pub mod cot;
pub mod deg2rad;
pub mod fixed;
pub mod floor;
pub mod interquartile;
pub mod lerp;
pub mod lerpangle;
pub mod ln;
pub mod log;
pub mod log10;
pub mod log2;
pub mod max;
pub mod mean;
pub mod median;
pub mod midhinge;
pub mod min;
pub mod mode;
pub mod nearestrank;
pub mod percentile;
pub mod pow;
pub mod product;
pub mod rad2deg;
pub mod round;
pub mod sign;
pub mod sin;
pub mod spread;
pub mod sqrt;
pub mod stddev;
pub mod sum;
pub mod tan;
pub mod top;
pub mod trimean;
pub mod variance;

pub fn analyze_math_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "math::abs" => abs::analyze_math_abs(ctx, call, args),
        "math::acos" => acos::analyze_math_acos(ctx, call, args),
        "math::acot" => acot::analyze_math_acot(ctx, call, args),
        "math::asin" => asin::analyze_math_asin(ctx, call, args),
        "math::atan" => atan::analyze_math_atan(ctx, call, args),
        "math::bottom" => bottom::analyze_math_bottom(ctx, call, args),
        "math::ceil" => ceil::analyze_math_ceil(ctx, call, args),
        "math::clamp" => clamp::analyze_math_clamp(ctx, call, args),
        "math::cos" => cos::analyze_math_cos(ctx, call, args),
        "math::cot" => cot::analyze_math_cot(ctx, call, args),
        "math::deg2rad" => deg2rad::analyze_math_deg2rad(ctx, call, args),
        "math::fixed" => fixed::analyze_math_fixed(ctx, call, args),
        "math::floor" => floor::analyze_math_floor(ctx, call, args),
        "math::interquartile" => interquartile::analyze_math_interquartile(ctx, call, args),
        "math::lerp" => lerp::analyze_math_lerp(ctx, call, args),
        "math::lerpangle" => lerpangle::analyze_math_lerpangle(ctx, call, args),
        "math::ln" => ln::analyze_math_ln(ctx, call, args),
        "math::log" => log::analyze_math_log(ctx, call, args),
        "math::log10" => log10::analyze_math_log10(ctx, call, args),
        "math::log2" => log2::analyze_math_log2(ctx, call, args),
        "math::max" => max::analyze_math_max(ctx, call, args),
        "math::mean" => mean::analyze_math_mean(ctx, call, args),
        "math::median" => median::analyze_math_median(ctx, call, args),
        "math::midhinge" => midhinge::analyze_math_midhinge(ctx, call, args),
        "math::min" => min::analyze_math_min(ctx, call, args),
        "math::mode" => mode::analyze_math_mode(ctx, call, args),
        "math::nearestrank" => nearestrank::analyze_math_nearestrank(ctx, call, args),
        "math::percentile" => percentile::analyze_math_percentile(ctx, call, args),
        "math::pow" => pow::analyze_math_pow(ctx, call, args),
        "math::product" => product::analyze_math_product(ctx, call, args),
        "math::rad2deg" => rad2deg::analyze_math_rad2deg(ctx, call, args),
        "math::round" => round::analyze_math_round(ctx, call, args),
        "math::sign" => sign::analyze_math_sign(ctx, call, args),
        "math::sin" => sin::analyze_math_sin(ctx, call, args),
        "math::spread" => spread::analyze_math_spread(ctx, call, args),
        "math::sqrt" => sqrt::analyze_math_sqrt(ctx, call, args),
        "math::stddev" => stddev::analyze_math_stddev(ctx, call, args),
        "math::sum" => sum::analyze_math_sum(ctx, call, args),
        "math::tan" => tan::analyze_math_tan(ctx, call, args),
        "math::top" => top::analyze_math_top(ctx, call, args),
        "math::trimean" => trimean::analyze_math_trimean(ctx, call, args),
        "math::variance" => variance::analyze_math_variance(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
