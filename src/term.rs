//! Terminal capability checks shared by the printing commands.
//!
//! Colour is opt-out per the `NO_COLOR` convention and is never emitted when
//! stdout is redirected, so `scriv … ls` stays pipe-safe by default. The same
//! rule governs [`Spinner`]: it is drawn only when there is a terminal to draw
//! it on.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// Whether stdout should carry ANSI colour: a terminal, and `NO_COLOR` unset.
pub fn stdout_color() -> bool {
    std::io::stdout().is_terminal() && !no_color()
}

/// Honour the `NO_COLOR` convention: colour is disabled when the variable is
/// present and non-empty.
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Wrap `text` in an ANSI 256-colour sequence when `on`, so the same colour
/// indices the picker uses also drive plain listings.
pub fn paint(text: &str, color: u8, on: bool) -> String {
    if on {
        format!("\x1b[38;5;{color}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Paint `text`, then return to `back` rather than to the terminal default —
/// for a cell that carries its own colour inside a row that already has one.
///
/// [`paint`] ends with a reset, so using it for a cell mid-row would leave
/// everything after that cell uncoloured. Here the row's own colour is
/// re-opened instead, and the row's trailing reset still closes it.
pub fn paint_within(text: &str, color: u8, back: u8, on: bool) -> String {
    if on {
        format!("\x1b[38;5;{color}m{text}\x1b[38;5;{back}m")
    } else {
        text.to_string()
    }
}

/// Frames of the spinner, in order. Braille dots: one cell wide in every
/// terminal, so the line never changes width as it turns.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How long each frame is held. Ten frames at this rate is a full turn per
/// second — visible movement, and nowhere near enough redraws to matter.
const FRAME_TIME: Duration = Duration::from_millis(100);

/// Erase the line the cursor is on and return to its start, so the next frame
/// overwrites the previous one instead of stacking up.
const CLEAR_LINE: &str = "\r\x1b[2K";

/// A one-line "working on it" animation on stderr, erased when dropped.
///
/// For the waits scriv cannot make shorter — a `git fetch`, a `gh` round trip —
/// where the alternative is a frozen terminal that looks like a hang. It draws
/// on stderr because stdout is a result: `scriv branch pick` writes a branch
/// name there for a shell to read, and an animation in the middle of it would
/// be read as part of the name.
///
/// Nothing is drawn at all when stderr is not a terminal, so a redirected or
/// piped run stays clean, and the animation is never what a script has to
/// parse around.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    /// `None` when there was no terminal to draw on, which makes every method
    /// here a no-op rather than a special case at each call site.
    thread: Option<JoinHandle<()>>,
}

/// Start a spinner labelled `label` (e.g. `fetching`), running until it is
/// dropped.
///
/// Bind it to a name — `let _spinner = term::spinner(…)` — for as long as the
/// wait lasts. A bare `term::spinner(…)` statement drops it immediately and
/// spins for nothing.
#[must_use]
pub fn spinner(label: &str) -> Spinner {
    let stop = Arc::new(AtomicBool::new(false));
    if !std::io::stderr().is_terminal() {
        return Spinner { stop, thread: None };
    }

    let label = label.to_string();
    let color = !no_color();
    let flag = Arc::clone(&stop);
    let thread = std::thread::spawn(move || {
        let mut frames = FRAMES.iter().cycle();
        while !flag.load(Ordering::Relaxed) {
            let frame = frames.next().unwrap_or(&FRAMES[0]);
            let mut err = std::io::stderr().lock();
            // Cyan matches the "not here yet" hue the pickers already use for
            // things that come from a remote.
            let _ = write!(err, "{CLEAR_LINE}{} {label}", paint(frame, 6, color));
            let _ = err.flush();
            // Sleeping the whole frame would delay the erase by up to a frame
            // after the work finishes; waking often keeps the exit prompt.
            for _ in 0..10 {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(FRAME_TIME / 10);
            }
        }
    });
    Spinner {
        stop,
        thread: Some(thread),
    }
}

impl Drop for Spinner {
    /// Stop the animation and erase its line, leaving the terminal exactly as
    /// it was found — the picker that opens next draws over nothing.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let Some(thread) = self.thread.take() else {
            return;
        };
        // Join before erasing: a frame still in flight would otherwise be
        // written after the erase and left on screen.
        let _ = thread.join();
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "{CLEAR_LINE}");
        let _ = err.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_is_identity_when_off() {
        assert_eq!(paint("main", 2, false), "main");
    }

    #[test]
    fn paint_wraps_when_on() {
        assert_eq!(paint("main", 2, true), "\x1b[38;5;2mmain\x1b[0m");
    }

    /// A cell painted mid-row has to hand the row's colour back, or everything
    /// after it renders in the terminal default.
    #[test]
    fn paint_within_returns_to_the_row_colour() {
        assert_eq!(
            paint_within("✓", 2, 5, true),
            "\x1b[38;5;2m✓\x1b[38;5;5m",
            "the cell must not end in a reset",
        );
        assert_eq!(paint_within("✓", 2, 5, false), "✓");
    }

    /// Every frame has to be one column wide, or the line jitters as it turns
    /// and the erase leaves a tail behind.
    #[test]
    fn spinner_frames_are_a_single_character() {
        for frame in FRAMES {
            assert_eq!(frame.chars().count(), 1, "{frame:?} is not one character");
        }
    }

    /// The erase has to clear the whole line, not just return to its start:
    /// a shorter label drawn over a longer one would otherwise leave the tail
    /// of the old text on screen.
    #[test]
    fn clearing_erases_the_line_it_returns_to() {
        assert!(CLEAR_LINE.starts_with('\r'));
        assert!(CLEAR_LINE.contains("\x1b[2K"), "no erase-line sequence");
    }

    /// With no terminal to draw on — a piped or redirected run — the spinner
    /// starts no thread and writes nothing at all.
    #[test]
    fn no_terminal_means_no_animation() {
        // The test harness captures stderr, so this is the redirected case.
        let spinner = spinner("fetching");
        assert!(spinner.thread.is_none(), "spun without a terminal");
    }
}
