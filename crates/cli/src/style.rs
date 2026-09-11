//! Terminal styling: one decision about ANSI colour, made once per stream, and
//! the semantic paint functions the renderer and the summary lines call.
//!
//! # When colour is on
//!
//! Colour is decoration, so it is applied only when the reader is a terminal
//! that wants it. Every one of these turns it off:
//!
//! * `--no-color` on the command line,
//! * `NO_COLOR` set to anything non-empty ([no-color.org](https://no-color.org)),
//! * `TERM=dumb`,
//! * the destination stream is not a TTY — a pipe, a file, a CI log.
//!
//! The last one is what makes `surrealql-analyzer check > report.txt` produce clean
//! text: the bytes written are the same modulo the escape sequences, so a
//! diff of the two is empty once the escapes are stripped.
//!
//! `--json` never reaches this module. It is a machine contract — one document,
//! one exit code, no decoration — so its writer paints nothing at all.

use surrealql_analyzer_diagnostics::Severity;

/// ANSI SGR parameters, spelled once.
mod sgr {
    pub(super) const RESET: &str = "\x1b[0m";
    pub(super) const BOLD: &str = "1";
    pub(super) const DIM: &str = "2";
    pub(super) const RED: &str = "31";
    pub(super) const GREEN: &str = "32";
    pub(super) const YELLOW: &str = "33";
    pub(super) const BLUE: &str = "34";
    pub(super) const CYAN: &str = "36";
    /// Inverted badge text: black on a colour, used for the watch status band.
    pub(super) const BLACK: &str = "30";
    pub(super) const ON_RED: &str = "41";
    pub(super) const ON_GREEN: &str = "42";
    pub(super) const ON_YELLOW: &str = "43";
}

/// Whether a given output stream gets ANSI escapes.
///
/// Copied rather than borrowed everywhere: it is one `bool`, and threading it
/// by value keeps every rendering function pure in it — which is what lets the
/// tests render the same finding twice, coloured and plain, and compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Styles {
    color: bool,
}

impl Styles {
    /// Never emits an escape sequence. The renderer's tests, `--json`, and any
    /// destination that is not a terminal use this.
    pub(crate) const fn plain() -> Self {
        Self { color: false }
    }

    /// Always emits escape sequences. Only tests construct this directly;
    /// commands go through [`Styles::for_stream`].
    #[cfg(test)]
    pub(crate) const fn colored() -> Self {
        Self { color: true }
    }

    /// The styling a stream gets: colour only when the user has not opted out
    /// *and* the stream is a terminal.
    ///
    /// `is_terminal` is passed in rather than probed, because `std::io::IsTerminal` is
    /// a sealed trait — there is no way to hand this function a fake stream, so
    /// the decision is testable only if the TTY answer is an argument.
    pub(crate) fn for_stream(is_terminal: bool, no_color_flag: bool) -> Self {
        Self {
            color: !no_color_flag && !env_forbids_color() && is_terminal,
        }
    }

    /// Whether escapes are emitted. The spinner and the screen-clear share the
    /// answer: both are terminal decoration, and both must vanish in a pipe.
    pub(crate) const fn is_colored(self) -> bool {
        self.color
    }

    /// Wraps `text` in `codes` (semicolon-joined SGR parameters), or returns it
    /// untouched when colour is off.
    fn paint(self, text: &str, codes: &str) -> String {
        if self.color {
            format!("\x1b[{codes}m{text}{}", sgr::RESET)
        } else {
            text.to_string()
        }
    }

    /// `error` / `warning` / `hint`, in the severity's colour.
    pub(crate) fn severity(self, severity: Severity, text: &str) -> String {
        self.paint(text, &format!("{};{}", sgr::BOLD, severity_color(severity)))
    }

    /// The diagnostic message, bold so it wins the line against the code.
    pub(crate) fn message(self, text: &str) -> String {
        self.paint(text, sgr::BOLD)
    }

    /// The `-->` and `|` scaffolding around a source excerpt.
    pub(crate) fn frame(self, text: &str) -> String {
        self.paint(text, &format!("{};{}", sgr::BOLD, sgr::BLUE))
    }

    /// A file path, so the eye finds "where" without reading the whole line.
    pub(crate) fn path(self, text: &str) -> String {
        self.paint(text, sgr::CYAN)
    }

    /// The caret underline, in the colour of whatever it underlines: the
    /// severity for the primary span, blue for a related one.
    pub(crate) fn carets(self, text: &str, severity: Option<Severity>) -> String {
        let color = severity.map_or(sgr::BLUE, severity_color);
        self.paint(text, &format!("{};{color}", sgr::BOLD))
    }

    /// The `help:` / `note:` labels.
    pub(crate) fn label(self, text: &str) -> String {
        self.paint(text, &format!("{};{}", sgr::BOLD, sgr::CYAN))
    }

    /// Secondary text — counts, timings, the things you read only when you
    /// went looking for them.
    pub(crate) fn dim(self, text: &str) -> String {
        self.paint(text, sgr::DIM)
    }

    /// A clean result.
    pub(crate) fn ok(self, text: &str) -> String {
        self.paint(text, &format!("{};{}", sgr::BOLD, sgr::GREEN))
    }

    /// A result with warnings but no errors.
    pub(crate) fn warn(self, text: &str) -> String {
        self.paint(text, &format!("{};{}", sgr::BOLD, sgr::YELLOW))
    }

    /// A failing result.
    pub(crate) fn fail(self, text: &str) -> String {
        self.paint(text, &format!("{};{}", sgr::BOLD, sgr::RED))
    }

    /// An inverted badge — the watch loop's `PASS` / `WARN` / `FAIL` band. A
    /// block of colour is legible from across the room, which is the point:
    /// this is the surface someone glances at while typing in another window.
    /// Without colour it degrades to the bare word, which still reads.
    pub(crate) fn badge(self, text: &str, outcome: Outcome) -> String {
        let background = match outcome {
            Outcome::Clean => sgr::ON_GREEN,
            Outcome::Warned => sgr::ON_YELLOW,
            Outcome::Failed => sgr::ON_RED,
        };
        if self.color {
            self.paint(
                &format!(" {text} "),
                &format!("{};{};{background}", sgr::BOLD, sgr::BLACK),
            )
        } else {
            text.to_string()
        }
    }
}

/// How a run ended, reduced to the three states worth distinguishing at a
/// glance. "Clean" and "N errors" must not look alike — that is the whole job
/// of the status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// No findings survived policy.
    Clean,
    /// Findings, but none of them errors.
    Warned,
    /// At least one error.
    Failed,
}

impl Outcome {
    /// The outcome implied by a finding count and an error count.
    pub(crate) const fn of(diagnostics: usize, errors: usize) -> Self {
        if errors > 0 {
            Self::Failed
        } else if diagnostics > 0 {
            Self::Warned
        } else {
            Self::Clean
        }
    }

    /// The badge word.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Clean => "PASS",
            Self::Warned => "WARN",
            Self::Failed => "FAIL",
        }
    }
}

/// Paints `text` in the outcome's colour without the badge background — used
/// for the one-shot summary line, where a colour block would be too loud.
pub(crate) fn tint(styles: Styles, outcome: Outcome, text: &str) -> String {
    match outcome {
        Outcome::Clean => styles.ok(text),
        Outcome::Warned => styles.warn(text),
        Outcome::Failed => styles.fail(text),
    }
}

fn severity_color(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => sgr::RED,
        Severity::Warning => sgr::YELLOW,
        Severity::Hint => sgr::CYAN,
    }
}

/// The environment's veto: `NO_COLOR` (any non-empty value) or a terminal that
/// cannot render escapes at all.
fn env_forbids_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
        || std::env::var("TERM").is_ok_and(|term| term == "dumb")
}

/// `n` with the right plural, e.g. `1 error` / `3 errors` / `2 queries`.
///
/// Counts appear in every summary line this CLI prints, so `1 error(s)` — the
/// shape they all used to have — was the most-read sloppiness in the output.
pub(crate) fn count(n: usize, singular: &str) -> String {
    if n == 1 {
        return format!("{n} {singular}");
    }
    // `query` → `queries`, `source` → `sources`. Only the consonant-plus-`y`
    // rule is needed for the vocabulary this CLI actually prints.
    match singular.strip_suffix('y') {
        Some(stem) if !stem.ends_with(['a', 'e', 'i', 'o', 'u']) => format!("{n} {stem}ies"),
        _ => format!("{n} {singular}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_never_gets_escapes() {
        // The contract behind `surrealql-analyzer check > report.txt`: a
        // non-terminal destination is plain text, whatever the flags say.
        let styles = Styles::for_stream(false, false);
        assert!(!styles.is_colored());
        assert_eq!(styles.message("hello"), "hello");
        assert_eq!(styles.fail("3 errors"), "3 errors");
    }

    #[test]
    fn the_no_color_flag_beats_a_terminal() {
        let styles = Styles::for_stream(true, true);
        assert!(!styles.is_colored());
        assert_eq!(styles.severity(Severity::Error, "error"), "error");
    }

    #[test]
    fn painting_is_the_same_text_plus_escapes() {
        // Colour must never change *what* is written, only how it is dressed —
        // otherwise piped output and terminal output would say different
        // things, and only one of them could be right.
        let plain = Styles::plain();
        let colored = Styles::colored();
        for (painted, bare) in [
            (colored.message("m"), plain.message("m")),
            (colored.path("p"), plain.path("p")),
            (colored.frame("-->"), plain.frame("-->")),
            (colored.dim("d"), plain.dim("d")),
            (colored.ok("o"), plain.ok("o")),
        ] {
            assert!(
                painted.starts_with("\x1b["),
                "expected escapes: {painted:?}"
            );
            assert!(
                painted.ends_with("\x1b[0m"),
                "expected a reset: {painted:?}"
            );
            assert!(
                painted.contains(&bare),
                "the text itself must survive: {painted:?} vs {bare:?}"
            );
        }
    }

    #[test]
    fn each_severity_gets_its_own_color() {
        let styles = Styles::colored();
        let error = styles.severity(Severity::Error, "error");
        let warning = styles.severity(Severity::Warning, "warning");
        assert!(error.contains("31"), "errors are red: {error:?}");
        assert!(warning.contains("33"), "warnings are yellow: {warning:?}");
    }

    #[test]
    fn the_outcome_is_read_off_the_counts() {
        assert_eq!(Outcome::of(0, 0), Outcome::Clean);
        assert_eq!(Outcome::of(3, 0), Outcome::Warned);
        assert_eq!(Outcome::of(3, 1), Outcome::Failed);
    }

    #[test]
    fn a_badge_without_color_is_still_the_word() {
        assert_eq!(Styles::plain().badge("FAIL", Outcome::Failed), "FAIL");
        assert!(Styles::colored()
            .badge("FAIL", Outcome::Failed)
            .contains("FAIL"));
    }

    #[test]
    fn counts_are_pluralized() {
        assert_eq!(count(1, "error"), "1 error");
        assert_eq!(count(0, "error"), "0 errors");
        assert_eq!(count(2, "source"), "2 sources");
        assert_eq!(count(1, "query"), "1 query");
        assert_eq!(count(3, "query"), "3 queries");
    }
}
