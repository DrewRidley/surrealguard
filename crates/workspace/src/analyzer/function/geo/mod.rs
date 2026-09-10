//! `geo` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod area;
pub mod bearing;
pub mod centroid;
pub mod distance;
pub mod hash_decode;
pub mod hash_encode;
pub mod is_valid;

/// Every `geo::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "geo::area",
        "The area of a geometry.",
        area::signature,
        area::analyze_geo_area,
    ),
    BuiltinEntry::new(
        "geo::bearing",
        "The compass bearing from one point to another.",
        bearing::signature,
        bearing::analyze_geo_bearing,
    ),
    BuiltinEntry::new(
        "geo::centroid",
        "The centroid of a geometry.",
        centroid::signature,
        centroid::analyze_geo_centroid,
    ),
    BuiltinEntry::new(
        "geo::distance",
        "The haversine distance between two points, in metres.",
        distance::signature,
        distance::analyze_geo_distance,
    ),
    BuiltinEntry::new(
        "geo::is_valid",
        "Whether the geometry is valid.",
        is_valid::signature,
        is_valid::analyze_geo_is_valid,
    ),
    BuiltinEntry::new(
        "geo::hash::decode",
        "The point encoded by a geohash string.",
        hash_decode::signature,
        hash_decode::analyze_geo_hash_decode,
    ),
    BuiltinEntry::new(
        "geo::hash::encode",
        "The geohash of a point, to an optional precision.",
        hash_encode::signature,
        hash_encode::analyze_geo_hash_encode,
    ),
];
