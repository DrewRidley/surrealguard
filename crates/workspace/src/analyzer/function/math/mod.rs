//! `math` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

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

/// Every `math::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "math::abs",
        "The absolute value of a number.",
        abs::signature,
        abs::analyze_math_abs,
    ),
    BuiltinEntry::new(
        "math::acos",
        "The arccosine of a number, in radians.",
        acos::signature,
        acos::analyze_math_acos,
    ),
    BuiltinEntry::new(
        "math::acot",
        "The arccotangent of a number, in radians.",
        acot::signature,
        acot::analyze_math_acot,
    ),
    BuiltinEntry::new(
        "math::asin",
        "The arcsine of a number, in radians.",
        asin::signature,
        asin::analyze_math_asin,
    ),
    BuiltinEntry::new(
        "math::atan",
        "The arctangent of a number, in radians.",
        atan::signature,
        atan::analyze_math_atan,
    ),
    BuiltinEntry::new(
        "math::bottom",
        "The smallest N numbers of an array.",
        bottom::signature,
        bottom::analyze_math_bottom,
    ),
    BuiltinEntry::new(
        "math::ceil",
        "The number rounded up to the nearest integer.",
        ceil::signature,
        ceil::analyze_math_ceil,
    ),
    BuiltinEntry::new(
        "math::clamp",
        "The number constrained between a minimum and a maximum.",
        clamp::signature,
        clamp::analyze_math_clamp,
    ),
    BuiltinEntry::new(
        "math::cos",
        "The cosine of an angle in radians.",
        cos::signature,
        cos::analyze_math_cos,
    ),
    BuiltinEntry::new(
        "math::cot",
        "The cotangent of an angle in radians.",
        cot::signature,
        cot::analyze_math_cot,
    ),
    BuiltinEntry::new(
        "math::deg2rad",
        "Converts degrees to radians.",
        deg2rad::signature,
        deg2rad::analyze_math_deg2rad,
    ),
    BuiltinEntry::new(
        "math::fixed",
        "The number rounded to a fixed number of decimal places.",
        fixed::signature,
        fixed::analyze_math_fixed,
    ),
    BuiltinEntry::new(
        "math::floor",
        "The number rounded down to the nearest integer.",
        floor::signature,
        floor::analyze_math_floor,
    ),
    BuiltinEntry::new(
        "math::interquartile",
        "The interquartile range of an array of numbers.",
        interquartile::signature,
        interquartile::analyze_math_interquartile,
    ),
    BuiltinEntry::new(
        "math::lerp",
        "Linear interpolation between two numbers by a fraction.",
        lerp::signature,
        lerp::analyze_math_lerp,
    ),
    BuiltinEntry::new(
        "math::lerpangle",
        "Linear interpolation between two angles in degrees by a fraction, taking the shortest path.",
        lerpangle::signature,
        lerpangle::analyze_math_lerpangle,
    ),
    BuiltinEntry::new(
        "math::ln",
        "The natural logarithm of a number.",
        ln::signature,
        ln::analyze_math_ln,
    ),
    BuiltinEntry::new(
        "math::log",
        "The logarithm of a number in the given base.",
        log::signature,
        log::analyze_math_log,
    ),
    BuiltinEntry::new(
        "math::log10",
        "The base-10 logarithm of a number.",
        log10::signature,
        log10::analyze_math_log10,
    ),
    BuiltinEntry::new(
        "math::log2",
        "The base-2 logarithm of a number.",
        log2::signature,
        log2::analyze_math_log2,
    ),
    BuiltinEntry::new(
        "math::max",
        "The greatest number in an array.",
        max::signature,
        max::analyze_math_max,
    ),
    BuiltinEntry::new(
        "math::mean",
        "The arithmetic mean of an array of numbers.",
        mean::signature,
        mean::analyze_math_mean,
    ),
    BuiltinEntry::new(
        "math::median",
        "The median of an array of numbers.",
        median::signature,
        median::analyze_math_median,
    ),
    BuiltinEntry::new(
        "math::midhinge",
        "The midhinge of an array of numbers.",
        midhinge::signature,
        midhinge::analyze_math_midhinge,
    ),
    BuiltinEntry::new(
        "math::min",
        "The smallest number in an array.",
        min::signature,
        min::analyze_math_min,
    ),
    BuiltinEntry::new(
        "math::mode",
        "The most frequent value in an array of numbers.",
        mode::signature,
        mode::analyze_math_mode,
    ),
    BuiltinEntry::new(
        "math::nearestrank",
        "The value at the given percentile of an array, by nearest rank.",
        nearestrank::signature,
        nearestrank::analyze_math_nearestrank,
    ),
    BuiltinEntry::new(
        "math::percentile",
        "The value at the given percentile of an array of numbers.",
        percentile::signature,
        percentile::analyze_math_percentile,
    ),
    BuiltinEntry::new(
        "math::pow",
        "A number raised to a power.",
        pow::signature,
        pow::analyze_math_pow,
    ),
    BuiltinEntry::new(
        "math::product",
        "The product of an array of numbers.",
        product::signature,
        product::analyze_math_product,
    ),
    BuiltinEntry::new(
        "math::rad2deg",
        "Converts radians to degrees.",
        rad2deg::signature,
        rad2deg::analyze_math_rad2deg,
    ),
    BuiltinEntry::new(
        "math::round",
        "The number rounded to the nearest integer.",
        round::signature,
        round::analyze_math_round,
    ),
    BuiltinEntry::new(
        "math::sign",
        "The sign of a number: -1, 0, or 1.",
        sign::signature,
        sign::analyze_math_sign,
    ),
    BuiltinEntry::new(
        "math::sin",
        "The sine of an angle in radians.",
        sin::signature,
        sin::analyze_math_sin,
    ),
    BuiltinEntry::new(
        "math::spread",
        "The difference between the greatest and smallest numbers of an array.",
        spread::signature,
        spread::analyze_math_spread,
    ),
    BuiltinEntry::new(
        "math::sqrt",
        "The square root of a number.",
        sqrt::signature,
        sqrt::analyze_math_sqrt,
    ),
    BuiltinEntry::new(
        "math::stddev",
        "The population standard deviation of an array of numbers.",
        stddev::signature,
        stddev::analyze_math_stddev,
    ),
    BuiltinEntry::new(
        "math::sum",
        "The sum of an array of numbers.",
        sum::signature,
        sum::analyze_math_sum,
    ),
    BuiltinEntry::new(
        "math::tan",
        "The tangent of an angle in radians.",
        tan::signature,
        tan::analyze_math_tan,
    ),
    BuiltinEntry::new(
        "math::top",
        "The greatest N numbers of an array.",
        top::signature,
        top::analyze_math_top,
    ),
    BuiltinEntry::new(
        "math::trimean",
        "The trimean of an array of numbers.",
        trimean::signature,
        trimean::analyze_math_trimean,
    ),
    BuiltinEntry::new(
        "math::variance",
        "The population variance of an array of numbers.",
        variance::signature,
        variance::analyze_math_variance,
    ),
];
