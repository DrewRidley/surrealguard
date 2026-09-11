//! A progress spinner that is honest about how long the work took.
//!
//! # Why it waits before drawing anything
//!
//! `check` on a small workspace finishes in about five milliseconds. A spinner
//! that draws a frame on start would add a frame's worth of latency and a
//! flash of animation to a command that was already done — it would *report*
//! progress that never existed and make an instant command feel slower. Two
//! things follow, and they are the whole design:
//!
//! * **Nothing is drawn for the first [`DELAY`].** The thread sleeps; if the
//!   run finishes inside that window — the common case — not one byte is
//!   written and the spinner cost is a thread spawn. A spinner is only honest
//!   once the wait is long enough that a human would otherwise wonder whether
//!   the process is alive, which is where [`DELAY`] is set.
//! * **The animation runs on its own thread**, so a slow run animates while
//!   the analysis holds the main thread.
//!
//! # Why it cannot corrupt the output
//!
//! The spinner writes to stderr, never stdout, so the diagnostics and the
//! `--json` document are untouched even if the two streams are separated. It
//! only ever draws when *both* streams are terminals (a redirected stderr must
//! not collect animation frames), and [`Spinner::finish`] joins the thread
//! after erasing the line — so by the time the caller prints, the drawing is
//! provably over. There is no window in which a frame can land mid-diagnostic.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a run must take before the spinner appears at all. Below this a
/// human reads the command as instantaneous, and animation would be a lie
/// about the work done.
pub(crate) const DELAY: Duration = Duration::from_millis(120);

/// How often the frame advances once it is visible.
const TICK: Duration = Duration::from_millis(80);

/// The braille cycle every modern toolchain uses. Only ever written to a real
/// terminal, so the encoding is safe.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A running spinner, or the no-op one that a pipe gets.
pub(crate) struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Starts a spinner for `label`, or a no-op when `enabled` is false.
    ///
    /// `enabled` is the caller's decision — it folds in `--no-color`,
    /// `NO_COLOR` and `--json`. This function adds the one condition it owns:
    /// both stdout and stderr must be terminals.
    pub(crate) fn start(label: &'static str, enabled: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        if !enabled || !std::io::stdout().is_terminal() || !std::io::stderr().is_terminal() {
            return Self { stop, handle: None };
        }
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("surrealql-analyzer-spinner".into())
            .spawn(move || animate(label, &flag))
            .ok();
        Self { stop, handle }
    }

    /// Stops the animation and erases the line, blocking until the drawing
    /// thread has actually finished. Consuming `self` means a caller cannot
    /// print over a spinner that is still running.
    pub(crate) fn finish(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Sleeps out [`DELAY`], then draws until told to stop, then erases whatever it
/// drew. Returns without writing anything at all if the stop flag is set during
/// the initial wait — the fast path.
fn animate(label: &'static str, stop: &AtomicBool) {
    let deadline = Instant::now() + DELAY;
    // Polled rather than parked so a run that finishes in 5ms is not kept
    // waiting on a 120ms sleep before the process can exit.
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let mut frame = 0usize;
    let mut stderr = std::io::stderr();
    while !stop.load(Ordering::Acquire) {
        // `\r` returns to column zero and `\x1b[K` clears to end of line, so a
        // shrinking label never leaves a tail behind.
        let _ = write!(
            stderr,
            "\r\x1b[K\x1b[36m{}\x1b[0m {label}...",
            FRAMES[frame % FRAMES.len()]
        );
        let _ = stderr.flush();
        frame += 1;
        std::thread::sleep(TICK);
    }
    let _ = write!(stderr, "\r\x1b[K");
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_spinner_spawns_nothing_and_writes_nothing() {
        // The pipe case, and the `--json` case: no thread, no bytes, no
        // measurable cost on a command that already finished.
        let spinner = Spinner::start("Checking", false);
        assert!(spinner.handle.is_none());
        spinner.finish();
    }

    #[test]
    fn a_fast_run_never_reaches_the_first_frame() {
        // The honesty check: the delay is long enough that the ~5ms common
        // case cannot draw. Asserted against the constant rather than a real
        // terminal, because a test harness has no TTY to draw on.
        assert!(
            DELAY > Duration::from_millis(50),
            "a 5ms check must finish inside the silent window"
        );
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        animate("Checking", &stop);
        assert!(
            started.elapsed() < DELAY,
            "an already-stopped spinner must return immediately, not sleep out the delay"
        );
    }
}
