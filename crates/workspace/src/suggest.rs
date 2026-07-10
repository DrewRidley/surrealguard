//! Did-you-mean suggestions: the closest candidate by edit distance,
//! offered only when it is close enough to be a plausible typo.

/// The candidate closest to `name`, when its distance is small relative
/// to the name's length (a quarter of it, minimum one edit — the rustc
/// heuristic neighborhood).
pub(crate) fn closest<'a, I>(name: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let budget = (name.chars().count() / 4).max(1);
    candidates
        .into_iter()
        .filter(|candidate| *candidate != name)
        .map(|candidate| (edit_distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= budget)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

/// Damerau-Levenshtein (adjacent transpositions count as one edit —
/// they are the most common typo).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut rows: Vec<Vec<usize>> = vec![(0..=b.len()).collect()];
    for (i, ch_a) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, ch_b) in b.iter().enumerate() {
            let substitution = rows[i][j] + usize::from(ch_a != ch_b);
            let mut best = substitution.min(rows[i][j + 1] + 1).min(row[j] + 1);
            if i > 0 && j > 0 && *ch_a == b[j - 1] && a[i - 1] == *ch_b {
                best = best.min(rows[i - 1][j - 1] + 1);
            }
            row.push(best);
        }
        rows.push(row);
    }
    rows[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::closest;

    #[test]
    fn closest_offers_only_plausible_typos() {
        let tables = ["person", "post", "likes"];
        assert_eq!(closest("persn", tables), Some("person"));
        assert_eq!(closest("pots", tables), Some("post"));
        // Too far from everything: no suggestion beats a wrong one.
        assert_eq!(closest("zebra", tables), None);
        // Exact match is the caller's case, never a suggestion.
        assert_eq!(closest("person", tables), None);
    }
}
