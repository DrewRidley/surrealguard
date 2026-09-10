//! `vector` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod add;
pub mod angle;
pub mod cross;
pub mod distance_chebyshev;
pub mod distance_euclidean;
pub mod distance_hamming;
pub mod distance_knn;
pub mod distance_mahalanobis;
pub mod distance_manhattan;
pub mod distance_minkowski;
pub mod divide;
pub mod dot;
pub mod magnitude;
pub mod multiply;
pub mod normalize;
pub mod project;
pub mod scale;
pub mod similarity_cosine;
pub mod similarity_jaccard;
pub mod similarity_pearson;
pub mod similarity_spearman;
pub mod subtract;
pub mod sum;

/// Every `vector::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "vector::add",
        "Element-wise sum of two vectors.",
        add::signature,
        add::analyze_vector_add,
    ),
    BuiltinEntry::new(
        "vector::angle",
        "The angle between two vectors, in radians.",
        angle::signature,
        angle::analyze_vector_angle,
    ),
    BuiltinEntry::new(
        "vector::cross",
        "The cross product of two 3-dimensional vectors.",
        cross::signature,
        cross::analyze_vector_cross,
    ),
    BuiltinEntry::new(
        "vector::divide",
        "Element-wise quotient of two vectors.",
        divide::signature,
        divide::analyze_vector_divide,
    ),
    BuiltinEntry::new(
        "vector::dot",
        "The dot product of two vectors.",
        dot::signature,
        dot::analyze_vector_dot,
    ),
    BuiltinEntry::new(
        "vector::magnitude",
        "The Euclidean length of a vector.",
        magnitude::signature,
        magnitude::analyze_vector_magnitude,
    ),
    BuiltinEntry::new(
        "vector::multiply",
        "Element-wise product of two vectors.",
        multiply::signature,
        multiply::analyze_vector_multiply,
    ),
    BuiltinEntry::new(
        "vector::normalize",
        "The vector scaled to unit length.",
        normalize::signature,
        normalize::analyze_vector_normalize,
    ),
    BuiltinEntry::new(
        "vector::project",
        "The projection of one vector onto another.",
        project::signature,
        project::analyze_vector_project,
    ),
    BuiltinEntry::new(
        "vector::scale",
        "The vector multiplied by a scalar.",
        scale::signature,
        scale::analyze_vector_scale,
    ),
    BuiltinEntry::new(
        "vector::subtract",
        "Element-wise difference of two vectors.",
        subtract::signature,
        subtract::analyze_vector_subtract,
    ),
    BuiltinEntry::new(
        "vector::sum",
        "The sum of a vector's components.",
        sum::signature,
        sum::analyze_vector_sum,
    ),
    BuiltinEntry::new(
        "vector::distance::chebyshev",
        "The Chebyshev distance between two vectors.",
        distance_chebyshev::signature,
        distance_chebyshev::analyze_vector_distance_chebyshev,
    ),
    BuiltinEntry::new(
        "vector::distance::euclidean",
        "The Euclidean distance between two vectors.",
        distance_euclidean::signature,
        distance_euclidean::analyze_vector_distance_euclidean,
    ),
    BuiltinEntry::new(
        "vector::distance::hamming",
        "The Hamming distance between two vectors.",
        distance_hamming::signature,
        distance_hamming::analyze_vector_distance_hamming,
    ),
    BuiltinEntry::new(
        "vector::distance::knn",
        "The distance computed by the enclosing KNN `<||>` operator.",
        distance_knn::signature,
        distance_knn::analyze_vector_distance_knn,
    ),
    BuiltinEntry::new(
        "vector::distance::mahalanobis",
        "The Mahalanobis distance between two vectors.",
        distance_mahalanobis::signature,
        distance_mahalanobis::analyze_vector_distance_mahalanobis,
    ),
    BuiltinEntry::new(
        "vector::distance::manhattan",
        "The Manhattan distance between two vectors.",
        distance_manhattan::signature,
        distance_manhattan::analyze_vector_distance_manhattan,
    ),
    BuiltinEntry::new(
        "vector::distance::minkowski",
        "The Minkowski distance between two vectors, of the given order.",
        distance_minkowski::signature,
        distance_minkowski::analyze_vector_distance_minkowski,
    ),
    BuiltinEntry::new(
        "vector::similarity::cosine",
        "The cosine similarity of two vectors.",
        similarity_cosine::signature,
        similarity_cosine::analyze_vector_similarity_cosine,
    ),
    BuiltinEntry::new(
        "vector::similarity::jaccard",
        "The Jaccard similarity of two vectors.",
        similarity_jaccard::signature,
        similarity_jaccard::analyze_vector_similarity_jaccard,
    ),
    BuiltinEntry::new(
        "vector::similarity::pearson",
        "The Pearson correlation of two vectors.",
        similarity_pearson::signature,
        similarity_pearson::analyze_vector_similarity_pearson,
    ),
    BuiltinEntry::new(
        "vector::similarity::spearman",
        "The Spearman correlation of two vectors.",
        similarity_spearman::signature,
        similarity_spearman::analyze_vector_similarity_spearman,
    ),
];
