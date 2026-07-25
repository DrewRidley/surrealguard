//! The `any`/`unknown` ratchet.
//!
//! Precision is allowed to improve freely and forbidden to degrade silently.
//! This test counts every `Kind::Any` leaf (and every site that produced no
//! kind at all — what the editor surfaces spell `unknown`) the vendored corpus
//! yields, per **site**, and holds the result against a committed baseline:
//!
//! * a site that gains `any`s, or a site that was precise and now is not,
//!   fails;
//! * a site that loses `any`s passes, and the baseline is stale by exactly that
//!   improvement — the failure message and the baseline header both say to
//!   regenerate;
//! * a site that is genuinely unknowable (a third-party payload, a shapeless
//!   `object`) can be frozen with `# expected: <reason>`, so the ratchet
//!   distinguishes "we accept this" from "we owe this".
//!
//! Failures list the offending **sites**, not just a total. A bare count tells
//! you precision moved; it does not tell you where, which is the only thing
//! that lets you act on it.
//!
//! Regenerate with:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test any_ratchet
//! ```

mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use support::{analyze_corpus, sites, snapshot_path, updating, Site};

const BASELINE: &str = "any_baseline.txt";

const HEADER: &str = "\
# `any` / `unknown` ratchet baseline — every corpus site that yields an
# imprecise type, with how many `any` leaves it contains.
#
# Regenerate: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test any_ratchet
#
# Format:  <count>  <site id>  [# expected: <reason>]
#
# A site with a `# expected:` note is genuinely unknowable and is frozen WITH
# its reason — that is the record of why the imprecision is acceptable.
# Every other line is DEBT: the count may fall (regenerate to bank the win) but
# may never rise, and a site that is not listed at all may not become imprecise.
#
# Regenerating PRESERVES the `# expected:` notes on sites that still exist.
";

#[test]
fn corpus_any_count_does_not_regress() {
    let corpus = analyze_corpus();
    let observed = observed_sites(&sites(&corpus));
    let path = snapshot_path(BASELINE);

    let baseline = match std::fs::read_to_string(&path) {
        Ok(text) => parse_baseline(&text),
        Err(_) if updating() => BTreeMap::new(),
        Err(_) => panic!(
            "missing baseline {}\n\
             create it with: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test any_ratchet",
            path.display()
        ),
    };

    if updating() {
        std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("create snapshot dir");
        std::fs::write(&path, render_baseline(&observed, &baseline)).expect("write baseline");
        return;
    }

    let mut regressions: Vec<String> = Vec::new();
    for (id, count) in &observed {
        match baseline.get(id) {
            None => regressions.push(format!(
                "  NEW      {count:>3} any  {id}\n           \
                 this site was precise (or did not exist); it is not anymore"
            )),
            Some(entry) if *count > entry.count => regressions.push(format!(
                "  WORSE    {:>3} → {count} any  {id}{}",
                entry.count,
                entry
                    .reason
                    .as_ref()
                    .map_or_else(String::new, |reason| format!(
                        "\n           expected-because: {reason}"
                    ))
            )),
            Some(_) => {}
        }
    }

    assert!(
        regressions.is_empty(),
        "\n\
         PRECISION DEGRADED — {} site(s) produce more `any`/`unknown` than the baseline allows.\n\
         Every line below is a place the editor now shows a less useful type.\n\
         \n{}\n\n\
         Baseline: {}\n\
         If the imprecision is genuinely unavoidable, add the site to the baseline WITH a\n\
         `# expected: <reason>` note. Otherwise fix the inference — do not raise the number.\n\
         Regenerate with: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test any_ratchet\n",
        regressions.len(),
        regressions.join("\n"),
        path.display()
    );

    // Improvements never fail, but a stale baseline hides the *next* regression
    // at that site, so say so loudly enough to be seen in a normal test run.
    let improved: Vec<String> = baseline
        .iter()
        .filter_map(|(id, entry)| {
            let now = observed.get(id).copied().unwrap_or(0);
            (now < entry.count).then(|| format!("  {id}: {} → {now}", entry.count))
        })
        .collect();
    if !improved.is_empty() {
        eprintln!(
            "any-ratchet: precision IMPROVED at {} site(s) — regenerate the baseline to bank it:\n{}",
            improved.len(),
            improved.join("\n")
        );
    }
}

/// Every site that is imprecise, as `site id → any-leaf count`. Precise sites
/// are absent, which is what makes an unlisted site becoming imprecise a
/// detectable "NEW".
fn observed_sites(sites: &[Site]) -> BTreeMap<String, usize> {
    sites
        .iter()
        .filter_map(|site| {
            let count = site.any_count();
            (count > 0).then(|| (site.id.clone(), count))
        })
        .collect()
}

/// One baseline row.
struct Entry {
    count: usize,
    reason: Option<String>,
}

fn parse_baseline(text: &str) -> BTreeMap<String, Entry> {
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (count, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let Ok(count) = count.parse::<usize>() else {
            continue;
        };
        let rest = rest.trim();
        let (id, reason) = match rest.split_once('#') {
            Some((id, note)) => (
                id.trim(),
                Some(
                    note.trim()
                        .strip_prefix("expected:")
                        .unwrap_or(note.trim())
                        .trim()
                        .to_string(),
                ),
            ),
            None => (rest, None),
        };
        entries.insert(id.to_string(), Entry { count, reason });
    }
    entries
}

/// Renders the baseline from what was observed, carrying over the `# expected:`
/// note of any site that still exists — regenerating must never silently drop
/// the recorded justification for an accepted imprecision.
fn render_baseline(
    observed: &BTreeMap<String, usize>,
    previous: &BTreeMap<String, Entry>,
) -> String {
    let mut out = String::from(HEADER);
    let _ = writeln!(
        out,
        "#\n# total: {} any/unknown leaves across {} site(s)\n",
        observed.values().sum::<usize>(),
        observed.len()
    );
    for (id, count) in observed {
        match previous.get(id).and_then(|entry| entry.reason.as_ref()) {
            Some(reason) => {
                let _ = writeln!(out, "{count}  {id}  # expected: {reason}");
            }
            None => {
                let _ = writeln!(out, "{count}  {id}");
            }
        }
    }
    out
}
