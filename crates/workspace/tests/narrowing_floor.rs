//! The recognizer floor: narrowing may tighten, never widen.
//!
//! Stage 3 of the expression-fact migration ran both narrowing paths in one
//! process and asserted, site by site, that the fact layer's kind is **equal to
//! or a subtype of** the recognizers'. *Never wider, sometimes narrower.* That
//! comparison — not the snapshots — was the safety argument, because a snapshot
//! pins the answers while this pins the **relation** between them.
//!
//! Stage 6 deletes the recognizers, so the live comparison has nothing left to
//! compare against. What survives it is this file: the recognizers' last
//! answers, recorded once as a **floor**, and the same subtype assertion run
//! against the floor instead of against a second live run.
//!
//! The distinction from `precision_snapshot.rs` is the whole reason this file
//! exists, and it is not redundancy:
//!
//! * `precision.snap` is an **equality** on current behaviour, and it is
//!   regenerated whenever behaviour legitimately changes. Regenerating it
//!   accepts *any* diff — including a widening — on the strength of a human
//!   reading it. That is by design, and it is why it cannot be the floor.
//! * This file is an **inequality**, and it does not move when precision
//!   improves. A future change that narrows further keeps passing untouched; a
//!   change that widens a site back toward the recognizers' answer — or past it
//!   — fails here even after `precision.snap` has been regenerated to bless it.
//!
//! So the floor outlives every regeneration of the snapshot, which is exactly
//! the property the deleted both-ways test had and a golden file does not.
//!
//! Two directions stay deliberately asymmetric, as they were in Stage 3:
//!
//! * a site the recognizers could not type at all (`unknown` in the floor) may
//!   become typed — that is the improvement the migration exists for;
//! * a site the recognizers typed may **not** become `unknown`, and a kind may
//!   not grow a variant the floor does not have.
//!
//! Each line carries the type twice: as the editor renders it, for a human
//! reading the file, and as the serialized `Kind`, which is what the comparison
//! uses. The rendering alone would not do — `render_kind` emits sized arrays
//! (`array<int, 3>`) that the type parser cannot read back, so a floor stored
//! only as text would be silently uncomparable at exactly the sites whose types
//! are most structured.
//!
//! Regenerate with:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test narrowing_floor
//! ```
//!
//! Regenerating is **not** a routine operation. The floor records a path that
//! no longer exists; rewriting it from the current path turns the inequality
//! into a tautology. It exists so a site that is genuinely *removed* from the
//! corpus can be dropped from the file.

mod support;

use std::collections::BTreeMap;

use surrealdb_types::Kind;
use surrealguard_workspace::kinds::kind_is_assignable_to;
use surrealguard_workspace::render_kind;

use support::{analyze_corpus, sites, snapshot_path, updating};

const FLOOR: &str = "narrowing_floor.txt";

/// How an absent kind is spelled in the floor file — the same word the editor
/// surfaces use, and not a type, so it is never deserialized.
const UNKNOWN: &str = "unknown";

const HEADER: &str = "\
# The recognizer floor — what `flow/narrow.rs`'s hand-written recognizers
# inferred at every corpus site, on the last commit before they were deleted.
#
# THIS FILE IS AN UPPER BOUND, NOT AN EXPECTATION. Inference may infer anything
# narrower than the type recorded here, and does at many sites. It may not infer
# anything WIDER, and may not lose a site that has a type here.
#
# That is the property the deleted both-ways test (`tests/fact_layer.rs`) proved
# by running both paths at once. With one path left, the other path's answers
# are pinned here instead. Unlike `precision.snap` this file does not move when
# precision improves, so it still catches a widening that a regenerated snapshot
# would bless.
#
# Format:  <site id> TAB <rendered type> TAB <serialized Kind>
#
# The rendered type is for reading; the serialized Kind is what is compared.
# Both are written from the same value, so they cannot disagree.
";

#[test]
fn narrowing_is_never_wider_than_the_recognizers_were() {
    let corpus = analyze_corpus();
    let observed: Vec<(String, Option<Kind>)> = sites(&corpus)
        .into_iter()
        .map(|site| (site.id, site.kind))
        .collect();
    let path = snapshot_path(FLOOR);

    if updating() {
        std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("create snapshot dir");
        std::fs::write(&path, render_floor(&observed)).expect("write floor");
        return;
    }

    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing floor {}\n\
             it records a deleted code path and cannot be re-derived; restore it from history",
            path.display()
        )
    });
    let floor = parse_floor(&text);
    assert!(
        !floor.is_empty(),
        "the floor at {} lists no site — the comparison below would be vacuous",
        path.display()
    );

    let mut regressions = Vec::new();
    let mut improvements = 0usize;
    let mut compared = 0usize;
    for (id, kind) in &observed {
        // A site the floor does not know is new corpus, not a regression: the
        // recognizers never saw it, so they bound nothing.
        let Some(was) = floor.get(id) else { continue };
        compared += 1;
        let Some(was) = was else {
            // The recognizers typed nothing here; anything is within bounds.
            improvements += usize::from(kind.is_some());
            continue;
        };
        match kind {
            None => regressions.push(format!(
                "{id}\n      floor: {}\n      now:   unknown  (LOST — inference types nothing here)",
                render_kind(was)
            )),
            Some(kind) if kind == was => {}
            Some(kind) if kind_is_assignable_to(kind, was) => improvements += 1,
            Some(kind) => regressions.push(format!(
                "{id}\n      floor: {}\n      now:   {}  (WIDER — not a subtype of the floor)",
                render_kind(was),
                render_kind(kind)
            )),
        }
    }

    assert!(
        regressions.is_empty(),
        "\nPRECISION LOST — {} site(s) where inference is not a subtype of the recognizer floor:\n\n    {}\n\n\
         A refinement may narrow or stay put. Widening one is the failure this file exists to rule out.\n\
         The floor is `{}`; it records a deleted path and must not be regenerated to make this pass.\n",
        regressions.len(),
        regressions.join("\n    "),
        path.display()
    );

    // The floor must actually bind something. If the floor stopped naming the
    // sites the corpus still produces, or if every site were trivially equal,
    // the inequality above would hold for reasons that have nothing to do with
    // narrowing, and this file would prove nothing.
    assert!(
        compared > floor.len() / 2,
        "the floor names {} site(s) but only {compared} still exist — the site ids have drifted, \
         and the comparison is mostly not happening",
        floor.len()
    );
    assert!(
        improvements > 0,
        "inference matched the recognizer floor at every one of the {compared} compared site(s). \
         The floor is a strict upper bound the fact layer is supposed to beat; matching it exactly \
         means narrowing stopped happening."
    );
}

/// The floor file's text for a run's sites.
fn render_floor(sites: &[(String, Option<Kind>)]) -> String {
    let mut out = String::from(HEADER);
    for (id, kind) in sites {
        let (rendered, encoded) = match kind {
            Some(kind) => (
                render_kind(kind),
                serde_json::to_string(kind).expect("a Kind serializes"),
            ),
            None => (UNKNOWN.to_string(), UNKNOWN.to_string()),
        };
        out.push_str(&format!("{id}\t{rendered}\t{encoded}\n"));
    }
    out
}

/// The floor file as `site id → recorded kind`, where `None` records a site the
/// recognizers could not type at all.
fn parse_floor(text: &str) -> BTreeMap<String, Option<Kind>> {
    let mut floor = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let id = columns.next().expect("split yields at least one column");
        let _rendered = columns
            .next()
            .unwrap_or_else(|| panic!("floor line has no rendered column: {line}"));
        let encoded = columns
            .next()
            .unwrap_or_else(|| panic!("floor line has no serialized column: {line}"));
        let kind = if encoded == UNKNOWN {
            None
        } else {
            Some(
                serde_json::from_str(encoded)
                    .unwrap_or_else(|error| panic!("floor line {id} does not decode: {error}")),
            )
        };
        floor.insert(id.to_string(), kind);
    }
    floor
}
