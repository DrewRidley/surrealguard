//! The watch loop behind `surrealql-analyzer watch`, `check --watch` and
//! `generate --watch`.
//!
//! Generated types go stale silently: you edit a `.surql` file or a host file
//! and nothing re-runs. Watching closes that loop — run once, then re-run on
//! every change to an input the analysis actually consumes.
//!
//! # Output
//!
//! See [`report`]. The short version: a watch is a display of the workspace's
//! current state, not a log of its history, so each run repaints the screen
//! rather than scrolling the last one away, and the verdict is a coloured band
//! that reads as PASS or FAIL without being read.
//!
//! # What is watched
//!
//! Exactly the inputs `check`/`generate` read:
//!
//! * every `.surql` / `.surrealql` file under the workspace root,
//! * every host file ([`crate::is_host_source`]: `.ts`/`.tsx`/`.js`/`.jsx`/
//!   `.svelte`/`.vue`/`.astro`) — the files
//!   [`crate::discover_host_sources`] scans for embedded queries,
//! * `surrealql-analyzer.toml` itself, because a config change alters which files
//!   matter and how findings are graded.
//!
//! Everything the config's `[sources] ignore` covers is dropped — twice over:
//! ignored top-level directories are never handed to the watcher at all (so
//! `target/`, `node_modules/` and `.git/` cost nothing), and every delivered
//! event is re-checked against the ignore patterns before it can trigger a run.
//! The generated registry is excluded too: `generate` writes it, so watching it
//! would make the process re-trigger itself forever.
//!
//! # Debounce
//!
//! One editor save is several filesystem events, and an atomic-save editor adds
//! a create/rename/remove burst on top. Two layers collapse that into one run:
//! `notify-debouncer-full` merges raw OS events over a [`DEBOUNCE`] window and
//! tracks renames by file id, and [`coalesce`] then keeps draining the
//! debouncer's own output while batches keep arriving inside the same window —
//! a save whose events straddle two debouncer ticks still yields exactly one
//! re-run. [`coalesce`] is a pure function over a channel, which is what makes
//! the coalescing testable without sleeping on real filesystem events.
//!
//! # Failure
//!
//! A run that fails reports and the loop keeps going. Typing a syntax error
//! must not kill the watcher — the next save is the fix.

use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use std::collections::BTreeSet;
use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use surrealql_analyzer_workspace::config::WorkspaceConfig;

use crate::style::{Outcome, Styles};

/// How long the loop waits for the change stream to go quiet before re-running.
/// Editors emit several events per save; 150ms is comfortably longer than the
/// gap between them and still below the threshold where a human notices lag.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(150);

/// Upper bound on how long [`coalesce`] will keep extending its quiet window.
/// Without it, an editor that writes on every keystroke could postpone the
/// re-run indefinitely; with it, the loop always makes progress.
pub(crate) const MAX_COALESCE: Duration = Duration::from_secs(2);

/// What happened to a watched path. Purely presentational — every verb
/// triggers the same re-run — but "created"/"deleted" is the difference between
/// a log line that explains itself and one that doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Verb {
    /// The file appeared.
    Created,
    /// The file's contents changed.
    Changed,
    /// The file went away.
    Deleted,
}

impl Verb {
    fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Changed => "changed",
            Self::Deleted => "deleted",
        }
    }
}

/// Which verb to print for a changed path, decided against the world rather
/// than against the event kind.
///
/// The event kind is not trustworthy: macOS `FSEvents` reports the *cumulative*
/// flags for a path, so a plain edit of a file that was created earlier in the
/// session still arrives carrying `Create`, and a delete can arrive carrying
/// `Create | Remove`. Comparing "does it exist now" against "was it an input at
/// the last run" gets it right on every platform.
pub(crate) fn verb_for(path: &Path, known: &BTreeSet<PathBuf>) -> Verb {
    if !path.exists() {
        Verb::Deleted
    } else if known.contains(path) {
        Verb::Changed
    } else {
        Verb::Created
    }
}

/// The paths that changed in one debounced burst, deduplicated. Sorted so a
/// repeating log line stays readable.
pub(crate) type ChangeSet = BTreeSet<PathBuf>;

/// Why a path matters to the analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Watched {
    /// `surrealql-analyzer.toml` — changes which files are inputs at all.
    Config,
    /// A `.surql` / `.surrealql` source.
    Surql,
    /// A host file that may carry embedded queries.
    Host,
}

/// Classifies a filesystem path against the workspace, or `None` when a change
/// to it cannot affect the analysis.
///
/// This is deliberately path-only: a deleted file cannot be stat'd, and a
/// deletion must be as watchable as a write.
///
/// `exclude` is the generated registry — `generate` writes it, so treating it
/// as an input would make every run trigger the next one.
pub(crate) fn classify(
    root: &Path,
    path: &Path,
    ignore: &[String],
    exclude: Option<&Path>,
) -> Option<Watched> {
    if exclude.is_some_and(|excluded| excluded == path) {
        return None;
    }
    // A path the watcher reported from outside the root cannot be an input.
    let relative = path.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    if ignore
        .iter()
        .any(|pattern| crate::matches_simple_ignore(relative, pattern))
    {
        return None;
    }
    if relative == Path::new("surrealql-analyzer.toml") {
        return Some(Watched::Config);
    }
    if crate::is_surrealql_source(path) {
        return Some(Watched::Surql);
    }
    if crate::is_host_source(path) {
        return Some(Watched::Host);
    }
    None
}

/// The absolute, symlink-resolved form of a path the command is going to
/// write, so it can be compared against the paths the watcher reports.
///
/// Getting this wrong is a feedback loop, not a cosmetic bug: `generate` writes
/// the registry, the watcher sees the write, `generate` runs again. So every
/// way the path can differ from the watcher's spelling is handled here —
/// `--out` may be relative (it is resolved against the working directory the
/// command writes through), may contain `..`, and on macOS the watcher reports
/// `/private/tmp/...` for a working directory spelled `/tmp/...`.
///
/// The file usually does not exist yet on the first run, so the *parent* is
/// canonicalized and the file name re-attached.
pub(crate) fn resolve_output(root: &Path, out: &Path) -> PathBuf {
    let absolute = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| root.to_path_buf())
            .join(out)
    };
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map_or_else(|_| absolute.clone(), |resolved| resolved.join(name)),
        // A path with no parent or no file name is not something `fs::write`
        // will succeed on either; leave it alone and let the run report it.
        _ => absolute,
    }
}

/// The immediate subdirectories of `root` that the watcher should follow
/// recursively — every one the ignore patterns don't cover.
///
/// Watching the root recursively would be one line shorter and would drag
/// `node_modules/` and `target/` into the watcher, which on Linux means an
/// inotify watch per directory in them. Splitting at the top level keeps the
/// named ignores off the watcher entirely; the per-event [`classify`] check
/// catches anything ignored deeper down.
pub(crate) fn watch_dirs(root: &Path, ignore: &[String]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        // `file_type` does not follow symlinks, so a symlinked directory is
        // skipped — a link back up the tree would otherwise loop.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            let relative = path.strip_prefix(root).unwrap_or(path);
            !ignore
                .iter()
                .any(|pattern| crate::matches_simple_ignore(relative, pattern))
        })
        .collect();
    dirs.sort();
    dirs
}

/// Blocks for the first batch on `rx`, then keeps merging batches for as long
/// as they keep arriving within `window`, up to `max_wait` overall.
///
/// This is the second debounce layer, and the reason one save is one run: the
/// OS-level debouncer emits on a fixed tick, so a single save's events can
/// straddle two ticks and arrive as two batches milliseconds apart. Merging
/// them here means the analysis runs once, on the settled state.
///
/// Returns `None` only when the channel is disconnected before any batch
/// arrives — i.e. the watcher is gone and the loop should end.
pub(crate) fn coalesce(
    rx: &Receiver<ChangeSet>,
    window: Duration,
    max_wait: Duration,
) -> Option<ChangeSet> {
    let mut merged = rx.recv().ok()?;
    let deadline = Instant::now() + max_wait;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(window.min(remaining)) {
            Ok(batch) => merged.extend(batch),
            Err(RecvTimeoutError::Timeout) => break,
            // The watcher died mid-burst; report what we have, and the next
            // `recv` ends the loop.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Some(merged)
}

/// The result of one watched run, in the shape the log line needs.
pub(crate) struct RunOutcome {
    /// Clean, warned, or failed — what the status band shows.
    pub(crate) outcome: Outcome,
    /// One line: `12 sources · no diagnostics · wrote
    /// surrealql-analyzer.generated.ts (3 queries)`.
    pub(crate) summary: String,
    /// Rendered diagnostic blocks (or an error message) printed beneath.
    pub(crate) detail: String,
}

/// Renders the `what changed` half of a run's log line.
fn describe(root: &Path, changes: &ChangeSet, known: &BTreeSet<PathBuf>) -> String {
    let mut names: Vec<String> = changes
        .iter()
        .map(|path| {
            let shown = path.strip_prefix(root).unwrap_or(path);
            format!("{} {}", shown.display(), verb_for(path, known).as_str())
        })
        .collect();
    match names.len() {
        0 => "no changes".into(),
        1 => names.remove(0),
        2 => format!("{}, {}", names[0], names[1]),
        n => format!("{}, {}, +{}", names[0], names[1], n - 2),
    }
}

/// Prints one run, replacing the previous one.
///
/// A watch is not a log — it is a *display*, and the only thing that matters is
/// the current state of the workspace. So on a terminal the screen is cleared
/// first and each run repaints it, which is why the answer is always at the
/// same place on the screen instead of scrolling away under the last twelve
/// runs. Piped output cannot be cleared, so it gets a rule between runs
/// instead; either way one run is one visually bounded block.
///
/// The scrollback (`\x1b[3J`) is deliberately *not* cleared — a run whose
/// diagnostics were longer than the window still has to be scrollable.
fn report(
    root: &Path,
    run: usize,
    reason: &str,
    outcome: &RunOutcome,
    elapsed: Duration,
    styles: Styles,
) {
    if styles.is_colored() {
        // Erase the display and home the cursor.
        print!("\x1b[2J\x1b[H");
    } else if run > 1 {
        println!("{}", "-".repeat(72));
    }

    println!(
        "{} {}",
        styles.message("surrealql-analyzer watch"),
        styles.dim(&root.display().to_string())
    );
    println!(
        "{}",
        styles.dim(&format!("run {run} · {reason} · {}ms", elapsed.as_millis()))
    );
    println!();

    if !outcome.detail.is_empty() {
        // Both streams go to stdout in watch mode: a terminal reading a live
        // log needs the blocks interleaved with their run line, and split
        // streams reorder the moment the output is piped.
        print!("{}", outcome.detail);
        if !outcome.detail.ends_with('\n') {
            println!();
        }
        println!();
    }

    println!(
        "{} {}",
        styles.badge(outcome.outcome.word(), outcome.outcome),
        crate::style::tint(styles, outcome.outcome, &outcome.summary)
    );
    println!();
    println!(
        "{}",
        styles.dim("watching .surql, host files and surrealql-analyzer.toml — Ctrl-C to stop")
    );
    let _ = std::io::stdout().flush();
}

/// Adds a watch for every directory that should have one and drops the ones
/// that shouldn't, returning the newly added paths.
///
/// Re-synced on every burst so a directory created after startup (a fresh
/// `queries/`, a `git checkout` that lands a whole tree) becomes watched
/// without restarting the process.
fn sync_watches(
    debouncer: &mut Debouncer<RecommendedWatcher, RecommendedCache>,
    root: &Path,
    ignore: &[String],
    watched: &mut BTreeSet<PathBuf>,
) -> Vec<PathBuf> {
    let wanted: BTreeSet<PathBuf> = watch_dirs(root, ignore).into_iter().collect();
    for stale in watched.difference(&wanted) {
        let _ = debouncer.unwatch(stale);
    }
    let mut added = Vec::new();
    for fresh in wanted.difference(watched) {
        if debouncer.watch(fresh, RecursiveMode::Recursive).is_ok() {
            added.push(fresh.clone());
        }
    }
    *watched = wanted;
    added
}

/// Runs `run` once, then again on every change to a watched input, until the
/// process is interrupted.
///
/// `exclude` is the file the command writes (the generated registry), kept out
/// of the input set so a run cannot trigger itself.
///
/// The config is re-read on every burst: it decides the ignore patterns and
/// therefore the watch set, so editing `surrealql-analyzer.toml` re-targets the
/// watcher on the next tick. A config that stops parsing keeps the previous
/// patterns — `run` is what reports the parse error, and it keeps reporting it
/// until the file is fixed.
pub(crate) fn watch_loop(
    root: &Path,
    exclude: Option<&Path>,
    styles: Styles,
    mut run: impl FnMut() -> RunOutcome,
) -> Result<(), Box<dyn Error>> {
    let mut config = workspace_config(root);

    let (tx, rx) = std::sync::mpsc::channel::<ChangeSet>();
    let mut debouncer = new_debouncer(DEBOUNCE, None, forward_to(tx))?;

    // The root itself is watched non-recursively: it carries
    // `surrealql-analyzer.toml` and any top-level source, and it is where a new
    // top-level directory shows up.
    debouncer.watch(root, RecursiveMode::NonRecursive)?;
    let mut watched = BTreeSet::new();
    sync_watches(&mut debouncer, root, &config.sources.ignore, &mut watched);

    // The inputs that existed at the last run — what makes "created" mean
    // created rather than "the event kind said so".
    let mut known = input_paths(root, &config);
    let mut run_index = 1usize;
    let started = Instant::now();
    let outcome = run();
    report(
        root,
        run_index,
        "initial run",
        &outcome,
        started.elapsed(),
        styles,
    );

    while let Some(batch) = coalesce(&rx, DEBOUNCE, MAX_COALESCE) {
        // A config edit can change the ignore patterns and the source globs;
        // re-read before deciding what the batch means and what to watch.
        config = workspace_config(root);
        let ignore = &config.sources.ignore;
        let appeared = sync_watches(&mut debouncer, root, ignore, &mut watched);

        let mut changes: ChangeSet = batch
            .into_iter()
            .filter(|path| classify(root, path, ignore, exclude).is_some())
            .collect();
        // A directory that appeared complete (a checkout, a `mv`) emits no
        // per-file events once it is watched, so treat its arrival as a change.
        changes.extend(appeared);
        if changes.is_empty() {
            continue;
        }

        let reason = describe(root, &changes, &known);
        run_index += 1;
        let started = Instant::now();
        let outcome = run();
        report(
            root,
            run_index,
            &reason,
            &outcome,
            started.elapsed(),
            styles,
        );
        known = input_paths(root, &config);
    }

    Ok(())
}

/// The debouncer callback: reduces a debounced batch to the touched paths and
/// forwards it as one message. The event *kind* is dropped on purpose — see
/// [`verb_for`] for why it cannot be trusted.
///
/// Watcher errors are reported and swallowed — a watch that drops one directory
/// must not take down the process, and the next run still reads from disk.
fn forward_to(tx: Sender<ChangeSet>) -> impl FnMut(DebounceEventResult) + Send + 'static {
    move |result: DebounceEventResult| match result {
        Ok(events) => {
            let batch: ChangeSet = events
                .into_iter()
                .flat_map(|event| event.paths.clone())
                .collect();
            if !batch.is_empty() {
                let _ = tx.send(batch);
            }
        }
        Err(errors) => {
            for error in errors {
                eprintln!("watch error: {error}");
            }
        }
    }
}

/// The workspace config, falling back to the defaults when it is missing or
/// currently unparseable. A half-typed `surrealql-analyzer.toml` must not stop the
/// watcher — `run` is what reports the parse error, on every run, until it is
/// fixed.
fn workspace_config(root: &Path) -> WorkspaceConfig {
    crate::load_workspace_config(root).unwrap_or_default()
}

/// Every input the analysis currently reads: the `.surql` sources, the host
/// files, and the config. Recorded after each run so the next one can tell a
/// new file from an edited one.
fn input_paths(root: &Path, config: &WorkspaceConfig) -> BTreeSet<PathBuf> {
    let mut paths: BTreeSet<PathBuf> = crate::discover_surrealql_sources(root, config)
        .into_iter()
        .chain(crate::discover_host_sources(root, config))
        .collect();
    // Neither discovery function returns the config — it is an input all the
    // same, and without it every `surrealql-analyzer.toml` edit would read "created".
    let config_path = root.join("surrealql-analyzer.toml");
    if config_path.exists() {
        paths.insert(config_path);
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    const IGNORE: [&str; 3] = ["target/**", "node_modules/**", ".git/**"];

    fn ignore() -> Vec<String> {
        IGNORE
            .iter()
            .map(|pattern| (*pattern).to_string())
            .collect()
    }

    #[test]
    fn surql_host_and_config_paths_are_watched_inputs() {
        let root = Path::new("/w");
        let ignore = ignore();
        for (path, expected) in [
            ("/w/schema/user.surql", Watched::Surql),
            ("/w/queries/a.surrealql", Watched::Surql),
            ("/w/src/probe.ts", Watched::Host),
            ("/w/src/App.svelte", Watched::Host),
            ("/w/src/page.astro", Watched::Host),
            ("/w/surrealql-analyzer.toml", Watched::Config),
        ] {
            assert_eq!(
                classify(root, Path::new(path), &ignore, None),
                Some(expected),
                "{path} should classify as {expected:?}"
            );
        }
    }

    #[test]
    fn unrelated_files_never_trigger_a_run() {
        let root = Path::new("/w");
        let ignore = ignore();
        for path in ["/w/README.md", "/w/Cargo.toml", "/w/src/styles.css", "/w"] {
            assert_eq!(
                classify(root, Path::new(path), &ignore, None),
                None,
                "{path} is not an analysis input"
            );
        }
    }

    #[test]
    fn ignored_directories_are_filtered_out_of_events() {
        // The watcher never subscribes to these at the top level, but a nested
        // `packages/app/node_modules` sits inside a directory that IS watched —
        // its events must still be dropped before they can trigger a run.
        let root = Path::new("/w");
        let ignore = ignore();
        for path in [
            "/w/node_modules/pkg/index.ts",
            "/w/packages/app/node_modules/dep/q.surql",
            "/w/target/debug/build/x.ts",
            "/w/.git/COMMIT_EDITMSG.surql",
        ] {
            assert_eq!(
                classify(root, Path::new(path), &ignore, None),
                None,
                "{path} is ignored and must not trigger a run"
            );
        }
        // ...while the same extension outside them still counts.
        assert_eq!(
            classify(
                root,
                Path::new("/w/packages/app/src/q.surql"),
                &ignore,
                None
            ),
            Some(Watched::Surql)
        );
    }

    #[test]
    fn a_path_outside_the_workspace_root_is_not_an_input() {
        assert_eq!(
            classify(
                Path::new("/w"),
                Path::new("/elsewhere/schema.surql"),
                &ignore(),
                None
            ),
            None
        );
    }

    #[test]
    fn the_generated_registry_is_not_an_input() {
        // `generate` writes this file. If a write to it counted as a change the
        // watcher would re-trigger itself forever.
        let root = Path::new("/w");
        let out = PathBuf::from("/w/surrealql-analyzer.generated.ts");
        assert_eq!(classify(root, &out, &ignore(), Some(&out)), None);
        // It is a plain host file to any other command.
        assert_eq!(
            classify(root, &out, &ignore(), None),
            Some(Watched::Host),
            "excluding it is the watch loop's job, not the extension check's"
        );
    }

    #[test]
    fn a_relative_output_path_still_resolves_to_the_path_the_watcher_reports() {
        // The loop that this prevents: `generate` writes the registry, the
        // watcher reports the write under its absolute, symlink-resolved name,
        // an exclusion spelled `src/registry.ts` fails to match, `generate`
        // runs again — forever. Exercised with `--out` *inside* a watched
        // directory, which is the common case.
        let root = crate::tests::temp_project_dir("watch-out-relative");
        std::fs::create_dir_all(root.join("src")).expect("dir");
        let canonical_root = root.canonicalize().expect("canonical root");

        let resolved = {
            // `resolve_output` reads the working directory for a relative path,
            // exactly as `generate` does when it writes.
            let previous = std::env::current_dir().expect("cwd");
            std::env::set_current_dir(&root).expect("enter project");
            let resolved = resolve_output(&root, Path::new("src/registry.ts"));
            std::env::set_current_dir(previous).expect("restore cwd");
            resolved
        };

        assert!(resolved.is_absolute(), "must be absolute: {resolved:?}");
        assert_eq!(resolved, canonical_root.join("src/registry.ts"));
        // And with that spelling, the write is no longer an input.
        assert_eq!(
            classify(
                &canonical_root,
                &canonical_root.join("src/registry.ts"),
                &ignore(),
                Some(&resolved),
            ),
            None,
            "the registry `generate` writes must never trigger the next run"
        );
    }

    #[test]
    fn an_output_path_with_parent_traversal_resolves_to_the_same_file() {
        let root = crate::tests::temp_project_dir("watch-out-traversal");
        std::fs::create_dir_all(root.join("src")).expect("dir");
        let canonical_root = root.canonicalize().expect("canonical root");

        let direct = resolve_output(&root, &canonical_root.join("src/registry.ts"));
        let traversed = resolve_output(&root, &canonical_root.join("src/../src/registry.ts"));
        assert_eq!(direct, traversed);
    }

    #[test]
    fn ignored_top_level_directories_are_never_handed_to_the_watcher() {
        let root = crate::tests::temp_project_dir("watch-dirs");
        for name in ["schema", "src", "target", "node_modules", ".git"] {
            std::fs::create_dir_all(root.join(name)).expect("create dir");
        }
        std::fs::write(root.join("surrealql-analyzer.toml"), "").expect("write config");

        let dirs = watch_dirs(&root, &ignore());
        let names: Vec<String> = dirs
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["schema".to_string(), "src".to_string()],
            "target/, node_modules/ and .git/ must not be watched at all"
        );
    }

    #[test]
    fn a_burst_of_events_coalesces_into_one_change_set() {
        // The determinism here is deliberate: the channel is driven directly,
        // so the test asserts the coalescing contract without racing a real
        // filesystem. Five events (one save, as an editor emits it) must
        // produce exactly one run's worth of changes.
        let (tx, rx) = channel::<ChangeSet>();
        for _ in 0..5 {
            tx.send(ChangeSet::from([PathBuf::from("/w/schema/user.surql")]))
                .expect("send");
        }
        drop(tx);

        let first = coalesce(&rx, Duration::from_millis(50), MAX_COALESCE).expect("a change set");
        assert_eq!(first.len(), 1, "one file changed, so one entry: {first:?}");
        assert!(first.contains(Path::new("/w/schema/user.surql")));
        assert!(
            coalesce(&rx, Duration::from_millis(50), MAX_COALESCE).is_none(),
            "the five events were one run, not five"
        );
    }

    #[test]
    fn a_burst_touching_several_files_reports_all_of_them_once() {
        let (tx, rx) = channel::<ChangeSet>();
        for path in ["/w/a.surql", "/w/b.surql", "/w/a.surql", "/w/src/c.ts"] {
            tx.send(ChangeSet::from([PathBuf::from(path)]))
                .expect("send");
        }
        drop(tx);

        let merged = coalesce(&rx, Duration::from_millis(50), MAX_COALESCE).expect("a change set");
        assert_eq!(merged.len(), 3, "a.surql is one entry, not two: {merged:?}");
    }

    #[test]
    fn separate_bursts_are_separate_runs() {
        // The window closes between bursts: a second save after the quiet
        // period must not be swallowed into the first run.
        let (tx, rx) = channel::<ChangeSet>();
        tx.send(ChangeSet::from([PathBuf::from("/w/a.surql")]))
            .expect("send");
        let first =
            coalesce(&rx, Duration::from_millis(20), MAX_COALESCE).expect("the first burst");
        assert_eq!(first, ChangeSet::from([PathBuf::from("/w/a.surql")]));

        tx.send(ChangeSet::from([PathBuf::from("/w/b.surql")]))
            .expect("send");
        let second =
            coalesce(&rx, Duration::from_millis(20), MAX_COALESCE).expect("the second burst");
        assert_eq!(second, ChangeSet::from([PathBuf::from("/w/b.surql")]));
    }

    #[test]
    fn the_verb_comes_from_the_filesystem_not_the_event_kind() {
        // macOS replays cumulative FSEvents flags, so an edit arrives carrying
        // `Create` and a delete arrives carrying `Create | Remove`. The verb is
        // decided by what is on disk against what was an input last run — which
        // is also the only way "created" can be right the first time a file
        // appears.
        let root = crate::tests::temp_project_dir("watch-verbs");
        let edited = root.join("edited.surql");
        let fresh = root.join("fresh.surql");
        let gone = root.join("gone.surql");
        std::fs::write(&edited, "DEFINE TABLE t;").expect("write");
        std::fs::write(&fresh, "DEFINE TABLE u;").expect("write");

        let known = BTreeSet::from([edited.clone(), gone.clone()]);
        assert_eq!(verb_for(&edited, &known), Verb::Changed);
        assert_eq!(verb_for(&fresh, &known), Verb::Created);
        assert_eq!(verb_for(&gone, &known), Verb::Deleted);
    }

    #[test]
    fn a_disconnected_channel_ends_the_loop() {
        let (tx, rx) = channel::<ChangeSet>();
        drop(tx);
        assert!(coalesce(&rx, Duration::from_millis(10), MAX_COALESCE).is_none());
    }

    #[test]
    fn coalescing_stops_extending_at_the_maximum_wait() {
        // A sender that never pauses must not postpone the run forever.
        let (tx, rx) = channel::<ChangeSet>();
        std::thread::spawn(move || {
            for index in 0..1000 {
                if tx
                    .send(ChangeSet::from([PathBuf::from(format!(
                        "/w/{index}.surql"
                    ))]))
                    .is_err()
                {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let started = Instant::now();
        let merged = coalesce(&rx, Duration::from_millis(50), Duration::from_millis(120))
            .expect("a change set");
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "the cap must bound the wait, took {:?}",
            started.elapsed()
        );
        assert!(!merged.is_empty());
    }

    #[test]
    fn the_change_line_names_files_and_summarizes_the_rest() {
        // None of these paths exist, so every verb resolves to "deleted" —
        // which is exactly what the loop should print for files that are gone.
        let root = Path::new("/w");
        let known = BTreeSet::new();
        let one = ChangeSet::from([PathBuf::from("/w/schema/user.surql")]);
        assert_eq!(describe(root, &one, &known), "schema/user.surql deleted");

        let many = ChangeSet::from([
            PathBuf::from("/w/a.surql"),
            PathBuf::from("/w/b.surql"),
            PathBuf::from("/w/c.surql"),
            PathBuf::from("/w/d.surql"),
        ]);
        assert_eq!(
            describe(root, &many, &known),
            "a.surql deleted, b.surql deleted, +2"
        );
    }

    #[test]
    fn the_recorded_input_set_is_what_the_run_reads() {
        // `known` must be the same set `check`/`generate` discover, or the
        // verbs drift from what actually happened.
        let root = crate::tests::temp_project_dir("watch-inputs");
        std::fs::create_dir_all(root.join("schema")).expect("dir");
        std::fs::create_dir_all(root.join("src")).expect("dir");
        std::fs::create_dir_all(root.join("node_modules/dep")).expect("dir");
        std::fs::write(root.join("surrealql-analyzer.toml"), "").expect("config");
        std::fs::write(root.join("schema/t.surql"), "DEFINE TABLE t;").expect("schema");
        std::fs::write(root.join("src/app.ts"), "// host").expect("host");
        std::fs::write(root.join("README.md"), "# docs").expect("doc");
        std::fs::write(root.join("node_modules/dep/i.ts"), "// dep").expect("dep");

        let config = workspace_config(&root);
        let inputs = input_paths(&root, &config);
        assert!(inputs.contains(&root.join("schema/t.surql")));
        assert!(inputs.contains(&root.join("src/app.ts")));
        assert!(!inputs.contains(&root.join("README.md")));
        assert!(
            !inputs.contains(&root.join("node_modules/dep/i.ts")),
            "ignored paths are not inputs: {inputs:?}"
        );
    }
}
