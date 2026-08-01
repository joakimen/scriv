//! Terminal capability checks shared by the printing commands.
//!
//! Colour is decided once, at startup, by [`ColorChoice::resolve`] and carried
//! on [`Ctx`](crate::Ctx) — `--color` if the user said, else `SCRIV_NO_COLOR`,
//! else whether stdout is a terminal, so `scriv … ls` stays pipe-safe by
//! default. [`Spinner`] and [`ScratchRow`] follow the same rule from the other
//! side: they touch the display only when there is one to touch.

use std::io::{IsTerminal, Write};

use rustix::fd::AsFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// When to colour printed output, as `--color` states it.
///
/// The three names every other tool of this kind uses — ripgrep, fd, `ls`,
/// `git` — so the flag needs no explaining to anyone who has met one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[clap(rename_all = "lower")]
pub enum ColorChoice {
    /// Colour when stdout is a terminal and `SCRIV_NO_COLOR` is unset.
    #[default]
    Auto,
    /// Always colour, terminal or not — for a pager (`less -R`) or a recording.
    Always,
    /// Never colour.
    Never,
}

impl ColorChoice {
    /// Whether printed output should carry ANSI colour.
    ///
    /// `is_tty` is passed in rather than looked up so the rule is a pure
    /// function with a test; [`ColorChoice::for_stdout`] is what callers use.
    ///
    /// An explicit `always`/`never` outranks `SCRIV_NO_COLOR`: the environment states
    /// a default, and a flag on the command line is the user overriding their
    /// own default for this one run. `auto` is where `SCRIV_NO_COLOR` applies.
    pub fn resolve(self, is_tty: bool, no_color: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => is_tty && !no_color,
        }
    }

    /// [`ColorChoice::resolve`] against this process's stdout and environment.
    pub fn for_stdout(self) -> bool {
        self.resolve(std::io::stdout().is_terminal(), no_color())
    }
}

/// The variable that turns scriv's colour off: set and non-empty means no
/// colour.
///
/// Deliberately scriv's own rather than the cross-tool `NO_COLOR`
/// (<https://no-color.org>), which scriv does not read. The convention is a
/// single switch for every tool at once, and this is a switch for this one;
/// having them be the same name would mean scriv could not be made colourful in
/// an environment that had turned everything else plain. `--color always` is
/// still the per-run override in either direction.
pub fn no_color() -> bool {
    std::env::var_os(NO_COLOR_ENV_VAR).is_some_and(|v| !v.is_empty())
}

/// The environment variable [`no_color`] reads.
pub const NO_COLOR_ENV_VAR: &str = "SCRIV_NO_COLOR";

/// Stdout for a listing, which ends quietly when the reader stops reading.
///
/// `println!` panics on a closed pipe, so `scriv history ls | head` — five
/// thousand rows produced for a reader that wanted three — ends in a Rust
/// stack trace where every other command-line tool would simply stop. Listings
/// write through here instead.
///
/// Only the long ones ever noticed: a few hundred rows fit in the pipe buffer,
/// so the write that fails never happens and the panic stayed hidden until a
/// listing got big enough to outrun `head`.
pub struct Listing<W: Write> {
    out: W,
    /// Cleared once the far end has gone, so the caller is told once and no
    /// further write is attempted.
    open: bool,
}

impl Listing<std::io::Stdout> {
    /// The listing every `ls` command writes: stdout.
    pub fn stdout() -> Self {
        Self::new(std::io::stdout())
    }
}

impl<W: Write> Listing<W> {
    /// Wrap a writer. Taking one rather than reaching for stdout is what lets
    /// the closed-pipe behaviour be tested against a writer that really fails.
    pub fn new(out: W) -> Self {
        Self { out, open: true }
    }

    /// Write one line, reporting whether there is still anyone reading.
    ///
    /// `Ok(false)` means the reader has closed the pipe: stop producing rows
    /// that have nowhere to go. Any other write failure is a real error and is
    /// returned as one — a full disk must not look like a `head`.
    pub fn line(&mut self, text: &str) -> std::io::Result<bool> {
        if !self.open {
            return Ok(false);
        }
        match writeln!(self.out, "{text}") {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                self.open = false;
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }
}

/// Whether an answer read from a prompt means yes.
///
/// Only an explicit yes counts: anything else — a bare return, a stray
/// keystroke, end of input — is no. A command that deletes something has to be
/// harder to trigger by accident than by intent.
pub fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Ask `question` on stderr and read the answer from stdin.
///
/// The question goes to stderr because stdout is a result: `scriv file prune`
/// prints the entries it removed there, and a prompt mixed into that would be
/// something a script had to parse around.
///
/// Whether there is anyone to answer is the caller's problem, not this
/// function's — see [`Confirm`] for why that distinction matters.
pub fn confirm(question: &str) -> std::io::Result<bool> {
    let mut err = std::io::stderr().lock();
    write!(err, "{question} [y/N] ")?;
    err.flush()?;
    drop(err);

    let mut answer = String::new();
    // End of input is not a yes: a closed stdin has said nothing, and the
    // safe reading of nothing is no.
    if std::io::stdin().read_line(&mut answer)? == 0 {
        return Ok(false);
    }
    Ok(is_yes(&answer))
}

/// Whether a question can be asked at all.
///
/// A prompt written to a stdin nobody is typing at is not a safety check — it
/// is a hang, or an instant "no" that looks like a refusal to work. A command
/// that needs confirmation and cannot ask for it should say so and name the
/// flag that skips the question, rather than guessing which answer was meant.
pub enum Confirm {
    /// There is a terminal on stdin: ask.
    Ask,
    /// `--yes` was given: do not ask.
    Assumed,
    /// stdin is a pipe or a file, and no `--yes`: refuse rather than guess.
    Impossible,
}

impl Confirm {
    /// Decide from the `--yes` flag and whether stdin is a terminal.
    ///
    /// Split from the check itself so the rule is a pure function with a test;
    /// [`Confirm::resolve`] is what callers use.
    pub fn decide(yes: bool, stdin_is_tty: bool) -> Self {
        match (yes, stdin_is_tty) {
            (true, _) => Self::Assumed,
            (false, true) => Self::Ask,
            (false, false) => Self::Impossible,
        }
    }

    /// [`Confirm::decide`] against this process's stdin.
    pub fn resolve(yes: bool) -> Self {
        Self::decide(yes, std::io::stdin().is_terminal())
    }
}

/// The status scriv exits with when its terminal disappears underneath it.
///
/// 128 + SIGHUP, which is what the shell would have reported had the signal
/// been delivered and taken its default action. Nothing is waiting to read it —
/// the terminal is gone — but a status that means something is still cheaper
/// than one that does not.
pub const EXIT_HANGUP: u8 = 129;

/// How often the terminal is probed while a picker is open. Twice a second is
/// far below anything measurable and still notices a hangup promptly.
const HANGUP_POLL: Duration = Duration::from_millis(500);

/// How many probes in a row must fail before the terminal is called gone.
///
/// More than one because the consequence is ending the process: a single odd
/// answer from a terminal that is still there would take down a picker somebody
/// was reading.
const HANGUP_STRIKES: u32 = 2;

/// Whether a failed probe means the terminal has gone, rather than that the
/// probe itself was interrupted.
///
/// `EIO` is what a pty whose other end has closed answers with, and `ENXIO` is
/// the same answer from a device that is no longer attached. `EBADF` means the
/// descriptor is not there at all. `EINTR` and `EAGAIN` are ordinary and say
/// nothing about the terminal.
pub fn is_hangup(err: rustix::io::Errno) -> bool {
    use rustix::io::Errno;
    matches!(err, Errno::IO | Errno::NXIO | Errno::BADF | Errno::PIPE)
}

/// Ask the terminal whether it is still there, without writing to it.
///
/// A zero-length write runs the driver's checks and then writes no bytes, so it
/// cannot disturb what the picker has drawn — and unlike a read, it cannot take
/// a keystroke that skim was going to act on.
fn still_attached(fd: rustix::fd::BorrowedFd<'_>) -> bool {
    match rustix::io::write(fd, &[]) {
        Ok(_) => true,
        Err(e) => !is_hangup(e),
    }
}

/// Ends the process if the terminal goes away while a picker is open.
///
/// The picker is skim, and skim's input loop does not stop when its event
/// stream ends: on a pty whose other end has closed, `crossterm`'s
/// `EventStream` yields end-of-stream immediately and forever, and skim's
/// `select!` treats that as nothing to do and asks again. The result is a
/// process pinning a core for as long as it is left alone — one was found here
/// having done so for over a day.
///
/// Normally the kernel prevents this: closing the other end of a pty sends
/// `SIGHUP` to the foreground process group and the default action ends scriv
/// before skim can spin. This is for when that does not happen — an orphaned
/// process group, a terminal that never hung up the group scriv was in — which
/// is the state the spinning process had reached.
///
/// The real fix belongs in skim, whose loop should stop when its input ends.
/// Until it does, scriv declines to be the process left behind.
#[must_use]
pub struct HangupWatch {
    stop: Arc<AtomicBool>,
    /// `None` when there was no terminal to watch, which makes this a no-op
    /// rather than a check at the call site.
    thread: Option<JoinHandle<()>>,
}

/// Watch the terminal for as long as the returned guard is held.
///
/// Bind it — `let _watch = term::watch_for_hangup()`. A bare statement drops it
/// immediately and watches nothing.
pub fn watch_for_hangup() -> HangupWatch {
    let stop = Arc::new(AtomicBool::new(false));
    // stderr is what the picker draws on, so it is the terminal whose loss
    // matters. Without one there is nothing to watch and nothing to protect.
    if !std::io::stderr().is_terminal() {
        return HangupWatch { stop, thread: None };
    }

    let flag = Arc::clone(&stop);
    let thread = std::thread::spawn(move || {
        let mut strikes = 0;
        while !flag.load(Ordering::Relaxed) {
            std::thread::sleep(HANGUP_POLL);
            if flag.load(Ordering::Relaxed) {
                return;
            }
            if still_attached(std::io::stderr().as_fd()) {
                strikes = 0;
                continue;
            }
            strikes += 1;
            if strikes >= HANGUP_STRIKES {
                // Nothing to unwind: the terminal that skim's state describes is
                // gone, and every destructor between here and `main` would be
                // writing to it. Leaving by the shortest route is the point.
                std::process::exit(EXIT_HANGUP as i32);
            }
        }
    });
    HangupWatch {
        stop,
        thread: Some(thread),
    }
}

impl Drop for HangupWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Not joined: the thread sleeps in half-second steps, and making every
        // picker's exit wait for one would be paying for the watch on the path
        // where nothing went wrong.
        drop(self.thread.take());
    }
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
/// `color` is the resolved [`ColorChoice`], so `--color never` gives a plain
/// spinner rather than a cyan one — the flag means the same thing everywhere
/// scriv draws.
///
/// Bind it to a name — `let _spinner = term::spinner(…)` — for as long as the
/// wait lasts. A bare `term::spinner(…)` statement drops it immediately and
/// spins for nothing.
#[must_use]
pub fn spinner(label: &str, color: bool) -> Spinner {
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
    /// A writer that takes `ok` complete lines and then fails with `kind`,
    /// standing in for a `head` that has read its rows and gone.
    ///
    /// Counting newlines rather than calls is what makes it a stand-in at all:
    /// `writeln!` reaches the writer more than once per line, so a stub that
    /// counted calls would fail partway through a row instead of between them.
    struct FailsAfter {
        ok: usize,
        kind: std::io::ErrorKind,
        buf: String,
    }

    impl FailsAfter {
        fn new(ok: usize, kind: std::io::ErrorKind) -> Self {
            Self {
                ok,
                kind,
                buf: String::new(),
            }
        }

        fn lines(&self) -> Vec<&str> {
            self.buf.lines().collect()
        }
    }

    impl std::io::Write for FailsAfter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.buf.matches('\n').count() >= self.ok {
                return Err(std::io::Error::from(self.kind));
            }
            self.buf.push_str(&String::from_utf8_lossy(buf));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// `scriv history ls | head` produces five thousand rows for a reader that
    /// wants three, and `println!` answers the closed pipe with a panic. Short
    /// listings hid it — a few hundred rows fit in the pipe buffer, so the
    /// failing write never happened — which is why it only surfaced once a
    /// listing got long enough to outrun `head`.
    #[test]
    fn a_reader_that_stops_reading_ends_the_listing_rather_than_failing_it() {
        let mut listing = super::Listing::new(FailsAfter::new(2, std::io::ErrorKind::BrokenPipe));
        assert!(listing.line("one").unwrap());
        assert!(listing.line("two").unwrap());
        assert!(
            !listing.line("three").unwrap(),
            "kept writing past the reader"
        );
        // Told once, then silent: no further write is even attempted.
        assert!(!listing.line("four").unwrap());
        assert_eq!(listing.out.lines(), vec!["one", "two"]);
    }

    /// A full disk must not be mistaken for a `head`. Everything other than the
    /// reader leaving is a real failure, and a listing that swallowed it would
    /// report success having printed half of what was asked for.
    #[test]
    fn other_write_failures_still_fail_the_listing() {
        for kind in [
            std::io::ErrorKind::StorageFull,
            std::io::ErrorKind::PermissionDenied,
        ] {
            let mut listing = super::Listing::new(FailsAfter::new(1, kind));
            assert!(listing.line("one").unwrap());
            assert_eq!(
                listing.line("two").unwrap_err().kind(),
                kind,
                "{kind:?} was treated as the reader going away"
            );
        }
    }

    use super::*;

    /// `auto` is the rule that was there before there was a flag: a terminal,
    /// and `SCRIV_NO_COLOR` unset.
    #[test]
    fn auto_colours_only_a_terminal_without_no_color() {
        assert!(ColorChoice::Auto.resolve(true, false));
        assert!(!ColorChoice::Auto.resolve(false, false), "coloured a pipe");
        assert!(
            !ColorChoice::Auto.resolve(true, true),
            "ignored SCRIV_NO_COLOR"
        );
    }

    /// The point of `always` is a destination that is not a terminal — a pager
    /// reading through `less -R`, a recording, a file to be replayed. A rule
    /// that still checked for a tty would make the flag do nothing at all in
    /// exactly the case it exists for.
    #[test]
    fn always_colours_a_pipe_too() {
        assert!(ColorChoice::Always.resolve(false, false));
        assert!(!ColorChoice::Never.resolve(false, false));
    }

    /// `EIO` is what a pty answers once the other end has closed, and it is the
    /// answer the spinning picker was getting. `ENXIO` is a device that is no
    /// longer attached, `EBADF` a descriptor that is not there at all.
    #[test]
    fn the_errors_that_mean_the_terminal_has_gone() {
        use rustix::io::Errno;
        for err in [Errno::IO, Errno::NXIO, Errno::BADF, Errno::PIPE] {
            assert!(is_hangup(err), "{err:?} was not read as a hangup");
        }
    }

    /// Ending the process is the consequence, so anything that merely means
    /// "ask again" must not reach it. An interrupted or would-block write says
    /// nothing at all about whether the terminal is still there.
    #[test]
    fn an_interrupted_probe_is_not_a_hangup() {
        use rustix::io::Errno;
        for err in [Errno::INTR, Errno::AGAIN, Errno::NOSPC, Errno::PERM] {
            assert!(!is_hangup(err), "{err:?} would have killed a live picker");
        }
    }

    /// Without a terminal there is nothing to watch and nothing that could go
    /// wrong, so no thread is started — and nothing can call `exit` on a run
    /// that was only ever a pipe.
    #[test]
    fn no_terminal_means_nothing_is_watched() {
        // The test harness captures stderr, so this is the redirected case.
        let watch = watch_for_hangup();
        assert!(
            watch.thread.is_none(),
            "watched a terminal that was not there"
        );
    }

    /// 128 + SIGHUP: the status the shell would have reported had the signal
    /// been delivered and taken its default action.
    #[test]
    fn the_hangup_exit_status_is_the_one_a_signal_would_have_given() {
        assert_eq!(EXIT_HANGUP, 128 + 1);
    }

    /// scriv reads its own variable, not the cross-tool `NO_COLOR`
    /// (<https://no-color.org>). The convention is one switch for every tool at
    /// once; this is a switch for this one, and sharing the name would leave no
    /// way to keep scriv coloured in an environment that had turned everything
    /// else plain.
    ///
    /// Pinned here because nothing else can catch it: `no_color` reads the
    /// environment, and whether it read the right name is invisible to a test
    /// that cannot set one — which is every test in this crate, since mutating
    /// the environment is unsound once another thread exists.
    #[test]
    fn the_variable_is_scrivs_own_and_not_the_shared_convention() {
        assert_eq!(NO_COLOR_ENV_VAR, "SCRIV_NO_COLOR");
    }

    /// `SCRIV_NO_COLOR` states a default for the environment; a flag on the command
    /// line is the user overriding their own default for this one run, so it
    /// has to win in both directions.
    #[test]
    fn an_explicit_choice_outranks_no_color() {
        assert!(
            ColorChoice::Always.resolve(true, true),
            "SCRIV_NO_COLOR beat --color always"
        );
        assert!(!ColorChoice::Never.resolve(true, false));
    }

    /// Only an explicit yes deletes anything. A bare return is the answer
    /// someone gives when they were not reading, and it has to mean no.
    #[test]
    fn only_an_explicit_yes_is_a_yes() {
        for answer in ["y", "Y", "yes", "YES", " yes \n"] {
            assert!(is_yes(answer), "{answer:?} was not read as yes");
        }
        for answer in ["", "\n", "n", "no", "ye", "yep", "sure", "1"] {
            assert!(!is_yes(answer), "{answer:?} was read as yes");
        }
    }

    /// `--yes` is the whole point of the flag: it answers the question so a
    /// script never reaches the prompt.
    #[test]
    fn the_yes_flag_skips_the_question() {
        assert!(matches!(Confirm::decide(true, true), Confirm::Assumed));
        assert!(matches!(Confirm::decide(true, false), Confirm::Assumed));
    }

    /// Prompting a stdin nobody is typing at is not caution — it is a hang or
    /// an instant no that reads as a refusal to work. Saying so, and naming the
    /// flag, is the only useful answer.
    #[test]
    fn a_question_that_cannot_be_asked_is_not_answered_for_the_user() {
        assert!(matches!(Confirm::decide(false, false), Confirm::Impossible));
        assert!(matches!(Confirm::decide(false, true), Confirm::Ask));
    }

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
        let spinner = spinner("fetching", true);
        assert!(spinner.thread.is_none(), "spun without a terminal");
    }

    /// The spinner has to draw on a row of its own: every frame erases the
    /// whole line, so on the prompt's row it would wipe the prompt and leave
    /// nothing but a blank line behind once it stopped.
    #[test]
    fn the_spinner_draws_on_a_row_of_its_own() {
        let spinner = spinner("fetching", true);
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
