//! fish shell history: where the file lives, what is in it, and how one
//! recorded command reads as a single picker row.
//!
//! fish writes each command to its history file as it is entered, so reading
//! that file *is* reading the live history — there is no shell to ask, and
//! `scriv history ls` works from anywhere. The one thing the file cannot say is
//! which session it belongs to: fish keeps that in `$fish_history`, which it
//! does not export, so a non-default session is named in the config instead.
//!
//! Everything here is pure. The file read, the clock and the picker live in
//! [`cmd::history`](crate::cmd::history).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::path::expand_home_dir;

/// Base directory for fish's data files, per the XDG spec.
pub const XDG_DATA_ENV_VAR: &str = "XDG_DATA_HOME";

/// Stands in for a line break when a multi-line command is drawn on one row.
const NEWLINE_GLYPH: &str = "⏎";

/// One command as fish recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The command line as it was typed, newlines and all.
    pub cmd: String,
    /// Unix time it was run, when the entry carried one.
    pub when: Option<i64>,
}

/// Resolve fish's history file: the configured path, else
/// `$XDG_DATA_HOME/fish/fish_history`, else `~/.local/share/fish/fish_history`
/// — the same rule fish itself applies.
pub fn history_path(configured: Option<&str>, xdg_data: Option<&str>, home: &Path) -> PathBuf {
    if let Some(file) = configured.filter(|s| !s.trim().is_empty()) {
        return expand_home_dir(file, home);
    }
    let base = match xdg_data.filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home.join(".local").join("share"),
    };
    base.join("fish").join("fish_history")
}

/// Read fish's history file into entries, oldest first — the order it stores
/// them in.
///
/// The format is one `- cmd:` line per command, with indented `when:`,
/// `added_when:` and `paths:` lines under it. It looks like YAML and is not:
/// newlines inside a command are escaped rather than quoted, which is what
/// keeps one command to one line. Anything unrecognised is skipped rather than
/// refused — a history file is decades of accumulated sessions, and one entry
/// scriv cannot read must not cost the user the other twenty thousand.
pub fn parse(data: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in data.lines() {
        if let Some(cmd) = line.strip_prefix("- cmd:") {
            // Exactly one space separates the key from the value, so a command
            // that itself begins with a space keeps it.
            entries.push(Entry {
                cmd: unescape(cmd.strip_prefix(' ').unwrap_or(cmd)),
                when: None,
            });
        } else if let Some(when) = line.strip_prefix("  when:")
            && let Some(entry) = entries.last_mut()
        {
            entry.when = when.trim().parse().ok();
        }
    }
    entries.retain(|entry| !entry.cmd.trim().is_empty());
    entries
}

/// Undo fish's history escaping.
///
/// A backslash escapes a backslash, or an `n` standing for a newline, and
/// nothing else — so a backslash before any other character is one the user
/// typed and stays as it is. That is why a shell line continuation round-trips:
/// fish stores the trailing `\` and the newline after it as `\\` then `\n`.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.as_str().chars().next() {
            Some('\\') => {
                out.push('\\');
                chars.next();
            }
            Some('n') => {
                out.push('\n');
                chars.next();
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// Newest first, with earlier runs of the same command dropped.
///
/// The stored order is chronological, so reversing it is what a history search
/// wants: the command from a moment ago is the first row offered. Dropping
/// repeats is what keeps a command run twenty times a day to one row instead of
/// a screenful of identical ones, and keeping the *newest* of each is what lets
/// the row say when it was last used.
///
/// The set holds borrowed commands rather than copies of them. A history file
/// is tens of thousands of entries and this runs on every ctrl-r, so cloning
/// each command into the set — only to throw the clone away — was a second copy
/// of the whole history built and dropped on the path where the user is waiting
/// for a picker to open.
pub fn recent_first(entries: Vec<Entry>) -> Vec<Entry> {
    // Scoped so the borrows end before the entries are consumed below.
    let keep = {
        let mut seen: HashSet<&str> = HashSet::with_capacity(entries.len());
        let mut keep = vec![false; entries.len()];
        // Backwards, so the *newest* run of each command is the one kept and
        // its row can say when it was last used.
        for (index, entry) in entries.iter().enumerate().rev() {
            keep[index] = seen.insert(entry.cmd.as_str());
        }
        keep
    };

    let mut out: Vec<Entry> = entries
        .into_iter()
        .zip(keep)
        .filter_map(|(entry, keep)| keep.then_some(entry))
        .collect();
    out.reverse();
    out
}

/// Render a command as a single line, for a picker row or a listing.
///
/// A row is one line by construction, so a multi-line command has to be folded
/// into one — and the folding has to be visible, since `git commit -m 'first`
/// run together with `second'` is not the command it would then appear to be.
/// Tabs become spaces and other control characters are dropped, so a stray
/// escape sequence in an old entry cannot repaint the terminal from a row
/// nobody has even selected.
pub fn one_line(cmd: &str) -> String {
    let joiner = format!(" {NEWLINE_GLYPH} ");
    cmd.lines()
        .map(|line| {
            line.chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .filter(|c| !c.is_control())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(&joiner)
}

/// Width of a [`stamp`], so an entry fish recorded without a `when:` can hold
/// the column open rather than shunting its command left of every other row.
pub const STAMP_WIDTH: usize = "2026-07-30 13:57".len();

/// When a command was last run, as a local date and time: `2026-07-30 13:57`.
///
/// Blank — but still [`STAMP_WIDTH`] wide — for an entry carrying no timestamp,
/// so the commands stay in one column whatever fish did or did not record.
///
/// `offset` is passed in rather than looked up: the local offset is an
/// environment fact, read once by [`Ctx`](crate::Ctx), which is also what keeps
/// this a pure function with an exactly reproducible answer.
pub fn stamp(when: Option<i64>, offset: time::UtcOffset) -> String {
    match when.and_then(|w| local(w, offset)) {
        Some(dt) => render(&dt),
        None => " ".repeat(STAMP_WIDTH),
    }
}

/// The one place the layout of a rendered timestamp is written down.
fn render(dt: &time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dt.hour(),
        dt.minute(),
    )
}

/// The stored Unix seconds as a local date and time.
///
/// `None` for a timestamp outside the range a date can represent — a corrupt or
/// absurd `when:` is one unreadable row, not a panic partway through a listing.
fn local(when: i64, offset: time::UtcOffset) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::from_unix_timestamp(when)
        .ok()
        .map(|dt| dt.to_offset(offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/u")
    }

    #[test]
    fn the_default_history_file_is_fishs_own() {
        assert_eq!(
            history_path(None, None, &home()),
            home().join(".local/share/fish/fish_history")
        );
    }

    #[test]
    fn xdg_data_home_moves_the_history_file() {
        assert_eq!(
            history_path(None, Some("/data"), &home()),
            PathBuf::from("/data/fish/fish_history")
        );
    }

    /// The config names the whole file, not a directory: `$fish_history` picks
    /// a session by name (`work_history`), and scriv cannot see that variable.
    #[test]
    fn a_configured_file_wins_and_expands_home() {
        assert_eq!(
            history_path(Some("~/hist/work_history"), Some("/data"), &home()),
            home().join("hist/work_history")
        );
    }

    /// An empty or whitespace-only key is how a config says "leave it alone",
    /// not a request to read the current directory.
    #[test]
    fn a_blank_configured_file_falls_back_to_the_default() {
        assert_eq!(
            history_path(Some("   "), None, &home()),
            home().join(".local/share/fish/fish_history")
        );
    }

    const SAMPLE: &str = "- cmd: git status\n  when: 1000\n\
                          - cmd: cargo test\n  when: 2000\n  paths:\n    - src/lib.rs\n\
                          - cmd: echo hi\n  when: 3000\n  added_when: 2999\n";

    #[test]
    fn entries_come_back_in_the_order_fish_stored_them() {
        let entries = parse(SAMPLE);
        assert_eq!(
            entries.iter().map(|e| e.cmd.as_str()).collect::<Vec<_>>(),
            vec!["git status", "cargo test", "echo hi"]
        );
        assert_eq!(entries[1].when, Some(2000));
    }

    /// `paths:` and `added_when:` are fish's business, and a `- src/lib.rs`
    /// under `paths:` must not be mistaken for a command.
    #[test]
    fn indented_keys_are_not_commands() {
        assert_eq!(parse(SAMPLE).len(), 3);
    }

    /// A command with no `when:` is still a command worth offering — only the
    /// preview's date is lost.
    #[test]
    fn an_entry_without_a_timestamp_survives() {
        let entries = parse("- cmd: ls\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].when, None);
    }

    #[test]
    fn blank_commands_are_dropped() {
        assert!(parse("- cmd: \n  when: 1\n- cmd:    \n").is_empty());
    }

    /// A shell line continuation is a backslash *and* a newline, which fish
    /// stores as `\\` followed by `\n`. Getting this wrong turns a two-line
    /// `curl` into one line with a stray backslash in the middle of it.
    #[test]
    fn a_line_continuation_round_trips() {
        let entries = parse("- cmd: curl -X POST \\\\\\n  --data @body.json\n");
        assert_eq!(entries[0].cmd, "curl -X POST \\\n  --data @body.json");
    }

    /// A backslash that escapes nothing fish knows about is a backslash the
    /// user typed — `grep '\d'` must come back as `grep '\d'`.
    #[test]
    fn an_unrecognised_escape_keeps_its_backslash() {
        assert_eq!(unescape(r"grep '\d\t'"), r"grep '\d\t'");
    }

    #[test]
    fn a_trailing_backslash_is_kept() {
        assert_eq!(unescape(r"echo \"), r"echo \");
    }

    /// A command starting with a space is a command starting with a space —
    /// only the single separator after the key belongs to the format.
    #[test]
    fn a_leading_space_in_the_command_is_preserved() {
        assert_eq!(parse("- cmd:  ls -la\n")[0].cmd, " ls -la");
    }

    #[test]
    fn the_newest_run_of_each_command_comes_first() {
        let entries =
            parse("- cmd: ls\n  when: 1\n- cmd: git status\n  when: 2\n- cmd: ls\n  when: 3\n");
        let recent = recent_first(entries);
        assert_eq!(
            recent.iter().map(|e| e.cmd.as_str()).collect::<Vec<_>>(),
            vec!["ls", "git status"]
        );
        // The surviving `ls` is the one from the most recent run, so its row
        // is dated by when it was last used rather than first.
        assert_eq!(recent[0].when, Some(3));
    }

    /// Order and dedup have to survive the rewrite that stopped cloning every
    /// command into the set: newest run first, one row per distinct command,
    /// and every distinct command still present.
    #[test]
    fn dedup_keeps_every_distinct_command_in_recency_order() {
        let entries: Vec<Entry> = ["a", "b", "a", "c", "b", "a"]
            .iter()
            .enumerate()
            .map(|(i, cmd)| Entry {
                cmd: (*cmd).to_string(),
                when: Some(i as i64),
            })
            .collect();

        let got = recent_first(entries);
        assert_eq!(
            got.iter().map(|e| e.cmd.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "not ordered by the most recent run of each command"
        );
        // Each surviving row is dated by the *last* time it was run.
        assert_eq!(
            got.iter().map(|e| e.when).collect::<Vec<_>>(),
            vec![Some(5), Some(4), Some(3)]
        );
    }

    #[test]
    fn an_empty_history_dedups_to_nothing() {
        assert!(recent_first(Vec::new()).is_empty());
    }

    #[test]
    fn a_multiline_command_folds_onto_one_row() {
        assert_eq!(one_line("git commit -m 'a\nb'"), "git commit -m 'a ⏎ b'");
    }

    /// A row that silently joined the lines would read as a command the user
    /// never ran, and selecting it still yields the real multi-line text.
    #[test]
    fn folding_a_command_leaves_a_visible_mark() {
        assert!(one_line("a\nb").contains(NEWLINE_GLYPH));
    }

    /// An escape sequence pasted into a shell years ago is still sitting in the
    /// history file; drawing it verbatim would let a row scrolled past recolour
    /// or reposition the terminal.
    #[test]
    fn control_characters_never_reach_a_row() {
        assert_eq!(one_line("echo \x1b[31mred\x07"), "echo [31mred");
        assert_eq!(one_line("a\tb"), "a b");
    }

    /// Seven hours east of UTC, the reference moment is 1785394626.
    fn bangkok() -> time::UtcOffset {
        time::UtcOffset::from_hms(7, 0, 0).unwrap()
    }

    #[test]
    fn a_stamp_is_the_local_date_and_time() {
        assert_eq!(stamp(Some(1785394626), bangkok()), "2026-07-30 13:57");
    }

    /// The offset is the whole point of passing one: the same instant is a
    /// different wall clock — here a different *day* — depending on where you
    /// are, and a history listing that showed UTC would be quietly wrong for
    /// everyone not on it.
    #[test]
    fn the_offset_moves_the_wall_clock() {
        let instant = Some(1785394626);
        assert_eq!(stamp(instant, time::UtcOffset::UTC), "2026-07-30 06:57");
        let west = time::UtcOffset::from_hms(-8, 0, 0).unwrap();
        assert_eq!(stamp(instant, west), "2026-07-29 22:57");
    }

    /// Rows line up in one column, so an entry fish recorded without a `when:`
    /// holds the space open rather than sliding its command left of every
    /// other row.
    #[test]
    fn an_undated_entry_still_fills_the_column() {
        let blank = stamp(None, bangkok());
        assert_eq!(blank.len(), STAMP_WIDTH);
        assert!(blank.trim().is_empty(), "{blank:?}");
        assert_eq!(stamp(Some(1785394626), bangkok()).len(), STAMP_WIDTH);
    }

    /// A `when:` far outside the range a date can hold is one unreadable row,
    /// not a panic partway through five thousand of them.
    #[test]
    fn an_absurd_timestamp_renders_blank_rather_than_panicking() {
        assert!(stamp(Some(i64::MAX), bangkok()).trim().is_empty());
        assert!(stamp(Some(i64::MIN), bangkok()).trim().is_empty());
    }
}
