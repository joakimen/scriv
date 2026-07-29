//! Terminal capability checks shared by the printing commands.
//!
//! Colour is opt-out per the `NO_COLOR` convention and is never emitted when
//! stdout is redirected, so `scriv … ls` stays pipe-safe by default. The same
//! rule governs [`Spinner`] and [`ScratchRow`]: they touch the display only
//! when there is one to touch.

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

/// A row of the terminal to draw on that is not the shell's.
///
/// Everything scriv draws inline — the spinner, the picker's viewport — starts
/// on the row the cursor is on, and when scriv is invoked from a key binding
/// that is the last row of the shell's prompt. Drawing there overwrites it, and
/// erasing there leaves it blank. A one-line prompt survives either, because
/// the shell redraws the whole thing afterwards; a two-line prompt does not —
/// its first row is still on screen, so what is left is a prompt cut in half.
///
/// Taking a fresh row instead keeps any prompt intact. Stepping back up on the
/// way out is what makes that safe: it leaves the cursor on the row the shell
/// left it on, which is where its repaint expects to find it. Without that the
/// shell redraws one row lower and strands a copy of the prompt above the new
/// one.
///
/// Bind it for as long as the drawing lasts — `let _row = ScratchRow::take()`.
/// A bare statement gives the row straight back.
#[must_use]
pub struct ScratchRow {
    /// `false` when there was no terminal to take a row on, which makes the
    /// whole type a no-op rather than a check at each call site.
    taken: bool,
}

impl ScratchRow {
    /// Move to a fresh row below the cursor, giving it back when dropped.
    pub fn take() -> Self {
        let taken = std::io::stderr().is_terminal();
        if taken {
            let mut err = std::io::stderr().lock();
            // An explicit carriage return: the cursor sits part-way along the
            // prompt row, and whether a bare newline also returns to column 0
            // depends on a terminal mode the shell owns, not scriv.
            let _ = err.write_all(b"\r\n");
            let _ = err.flush();
        }
        Self { taken }
    }

    /// No row at all, for a caller that has nothing to protect.
    pub fn none() -> Self {
        Self { taken: false }
    }

    /// Whether a row was actually taken — false without a terminal to take one
    /// on. For tests and for callers deciding what to draw.
    pub fn is_taken(&self) -> bool {
        self.taken
    }
}

impl Drop for ScratchRow {
    fn drop(&mut self) {
        if !self.taken {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(CURSOR_UP);
        let _ = err.flush();
    }
}

/// Move the cursor up one row, staying in its column.
const CURSOR_UP: &[u8] = b"\x1b[A";

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
/// It takes a [`ScratchRow`] to turn on, because each frame erases the whole
/// row it is drawn on: on the prompt's row that would wipe the prompt, and
/// unlike the picker that follows it, a spinner is not big enough to hide what
/// it destroyed.
///
/// Nothing is drawn at all when stderr is not a terminal, so a redirected or
/// piped run stays clean, and the animation is never what a script has to
/// parse around.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    /// `None` when there was no terminal to draw on, which makes every method
    /// here a no-op rather than a special case at each call site.
    thread: Option<JoinHandle<()>>,
    /// Dropped after the animation is erased, handing the row back.
    _row: ScratchRow,
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
        return Spinner {
            stop,
            thread: None,
            _row: ScratchRow::none(),
        };
    }
    let row = ScratchRow::take();

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
        _row: row,
    }
}

impl Drop for Spinner {
    /// Stop the animation and erase its line before the row it was drawn on
    /// goes back, leaving the terminal exactly as it was found — the picker
    /// that opens next draws over nothing.
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

    /// The spinner has to draw on a row of its own: every frame erases the
    /// whole line, so on the prompt's row it would wipe the prompt and leave
    /// nothing but a blank line behind once it stopped.
    #[test]
    fn the_spinner_draws_on_a_row_of_its_own() {
        let spinner = spinner("fetching");
        assert_eq!(
            spinner._row.is_taken(),
            spinner.thread.is_some(),
            "the spinner drew somewhere it did not own",
        );
    }

    /// Taking a row writes to the terminal, so a redirected run must take
    /// none — the newline and the cursor-up would otherwise end up in
    /// whatever is reading stderr.
    #[test]
    fn no_terminal_means_no_row_taken() {
        assert_eq!(
            ScratchRow::take().is_taken(),
            std::io::stderr().is_terminal()
        );
        assert!(!ScratchRow::none().is_taken());
    }

    /// Stepping back up is what leaves the cursor where the shell left it. A
    /// sequence that also moved the column, or one that scrolled, would put
    /// the shell's repaint somewhere else entirely.
    #[test]
    fn the_row_is_given_back_by_moving_up_one() {
        assert_eq!(CURSOR_UP, b"\x1b[A");
    }
}
