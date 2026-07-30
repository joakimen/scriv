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
/// the preview say when it was last used.
pub fn recent_first(entries: Vec<Entry>) -> Vec<Entry> {
    let mut seen: HashSet<String> = HashSet::with_capacity(entries.len());
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries.into_iter().rev() {
        if seen.insert(entry.cmd.clone()) {
            out.push(entry);
        }
    }
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

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// How long ago `then` was, seen from `now` — both Unix seconds.
///
/// Coarse on purpose: a history entry answers "was this today or last spring",
/// and the second it happened is never the question. A `then` in the future is
/// a clock that has been put back, not a command from tomorrow, so it reads as
/// having just happened rather than as a negative age.
pub fn relative_time(now: i64, then: i64) -> String {
    let secs = now.saturating_sub(then).max(0);
    match secs {
        s if s < MINUTE => "just now".to_string(),
        s if s < HOUR => format!("{}m ago", s / MINUTE),
        s if s < DAY => format!("{}h ago", s / HOUR),
        s if s < MONTH => format!("{}d ago", s / DAY),
        s if s < YEAR => format!("{}mo ago", s / MONTH),
        s => format!("{}y ago", s / YEAR),
    }
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
        // The surviving `ls` is the one from the most recent run, so the
        // preview dates it by when it was last used rather than first.
        assert_eq!(recent[0].when, Some(3));
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

    #[test]
    fn ages_read_from_seconds_to_years() {
        let now = 10 * YEAR;
        assert_eq!(relative_time(now, now - 5), "just now");
        assert_eq!(relative_time(now, now - 5 * MINUTE), "5m ago");
        assert_eq!(relative_time(now, now - 3 * HOUR), "3h ago");
        assert_eq!(relative_time(now, now - 2 * DAY), "2d ago");
        assert_eq!(relative_time(now, now - 4 * MONTH), "4mo ago");
        assert_eq!(relative_time(now, now - 3 * YEAR), "3y ago");
    }

    /// A history file written before the clock was corrected has entries dated
    /// in the future; "in -3h" is not a thing a listing should ever say.
    #[test]
    fn a_future_timestamp_reads_as_just_now() {
        assert_eq!(relative_time(1000, 9999), "just now");
    }
}
