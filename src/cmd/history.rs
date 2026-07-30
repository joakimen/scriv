//! `scriv history` — search the commands you have already run.
//!
//! The list is fish's own history file, newest first with repeats dropped, so
//! ctrl-r lands on what you ran a moment ago and a command you run daily takes
//! one row. Selecting a command prints it rather than running it: what to do
//! with it is the shell's decision, and the fish integration puts it back on
//! the command line for you to look at before pressing enter.

use std::io::Write;

use anyhow::{Context, Result};

use crate::history::{self, Entry};
use crate::pick::{PickItem, Preview};
use crate::{Ctx, pick};

/// The clock, read once per command so every row is dated against the same
/// instant.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// Every command in fish's history, newest first and deduplicated.
fn load(ctx: &Ctx) -> Result<Vec<Entry>> {
    let path = &ctx.history_path;
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "no fish history at {} — set `[history] file` if yours is somewhere else",
            path.display()
        ),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };

    // A history file is decades of accumulated sessions and holds whatever was
    // typed at the shell, which is not always valid UTF-8. One bad byte in one
    // entry from 2019 must not cost the user the list.
    let entries = history::recent_first(history::parse(&String::from_utf8_lossy(&data)));
    if entries.is_empty() {
        anyhow::bail!("no commands in {}", path.display());
    }
    ctx.log.info(&format!(
        "read {} commands from {}",
        entries.len(),
        path.display()
    ));
    Ok(entries)
}

/// The preview for a command: when it was last run, then the command in full.
///
/// The row only has one line, so this is where a command longer than the
/// terminal is wide — or one that spans several lines — is actually readable.
/// Built from the entry already in hand, so scrolling the list spawns nothing.
fn preview(entry: &Entry, now: i64) -> String {
    match entry.when {
        Some(when) => format!(
            "last run {}\n\n{}",
            history::relative_time(now, when),
            entry.cmd
        ),
        None => entry.cmd.clone(),
    }
}

/// Build picker rows: the command folded onto one line, returning the command
/// as it was really typed.
fn items(entries: &[Entry], now: i64) -> Vec<PickItem> {
    entries
        .iter()
        .map(|entry| {
            PickItem::new(history::one_line(&entry.cmd), entry.cmd.clone())
                .preview(Preview::Text(preview(entry, now)))
        })
        .collect()
}

/// Width of the widest age, for column alignment in `--status` output.
fn age_width(ages: &[String]) -> usize {
    ages.iter()
        .map(|age| age.chars().count())
        .max()
        .unwrap_or(0)
}

/// `scriv history ls` — print past commands, newest first, one per line.
///
/// A multi-line command is folded onto its one line like everywhere else, so
/// the output stays one entry per line and pipes into `wc -l` or `grep`
/// meaning what it looks like it means. `--status` prefixes how long ago each
/// was last run.
pub fn ls(ctx: &Ctx, status: bool) -> Result<()> {
    let entries = load(ctx)?;
    let now = now();

    if !status {
        for entry in &entries {
            println!("{}", history::one_line(&entry.cmd));
        }
        return Ok(());
    }

    let ages: Vec<String> = entries
        .iter()
        .map(|entry| {
            entry
                .when
                .map(|when| history::relative_time(now, when))
                .unwrap_or_default()
        })
        .collect();
    let width = age_width(&ages);
    for (entry, age) in entries.iter().zip(&ages) {
        println!("{age:<width$}  {}", history::one_line(&entry.cmd));
    }
    Ok(())
}

/// `scriv history pick` — fuzzy-select a past command and print it.
///
/// `query` seeds the search box, so ctrl-r pressed halfway through typing a
/// command starts narrowed to what is already there instead of throwing it
/// away. `print0` terminates the result with a NUL rather than a newline: a
/// command may itself contain newlines, and only a NUL tells the shell reading
/// this where one command ends.
pub fn pick(ctx: &Ctx, query: Option<&str>, print0: bool) -> Result<()> {
    let entries = load(ctx)?;
    let now = now();
    let chosen = pick::pick_one_queried(
        items(&entries, now),
        "Pick a command",
        query.unwrap_or_default(),
        &ctx.config.picker,
    )?;

    if !print0 {
        println!("{chosen}");
        return Ok(());
    }
    let mut out = std::io::stdout().lock();
    out.write_all(chosen.as_bytes())?;
    out.write_all(&[0])?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Entry {
                cmd: "git status".into(),
                when: Some(900),
            },
            Entry {
                cmd: "git commit -m 'a\nb'".into(),
                when: None,
            },
        ]
    }

    /// The row is folded onto one line; selecting it has to yield the command
    /// as it was actually typed, newlines and all, or a multi-line command
    /// comes back as something the user never ran.
    #[test]
    fn rows_fold_onto_one_line_but_return_the_real_command() {
        let items = items(&entries(), 1000);
        assert_eq!(items[1].label, "git commit -m 'a ⏎ b'");
        assert_eq!(items[1].value(), "git commit -m 'a\nb'");
    }

    /// Previews are built from data already in hand — a command preview would
    /// be spawned again on every keypress that moves the cursor.
    #[test]
    fn previews_are_text_rather_than_commands() {
        for item in items(&entries(), 1000) {
            assert!(
                matches!(item.preview, Some(Preview::Text(_))),
                "a history row spawns a process to preview itself"
            );
        }
    }

    #[test]
    fn the_preview_dates_the_command_and_shows_it_in_full() {
        let text = preview(&entries()[0], 1000);
        assert!(text.starts_with("last run 1m ago"), "{text}");
        assert!(text.ends_with("git status"), "{text}");
    }

    /// An entry fish recorded without a `when:` still previews; there is simply
    /// no date to put above it.
    #[test]
    fn an_undated_command_previews_without_a_date() {
        assert_eq!(preview(&entries()[1], 1000), "git commit -m 'a\nb'");
    }

    /// `{:<width$}` pads by character count, so a byte width would ragged the
    /// column the moment an age is rendered in a non-ASCII locale.
    #[test]
    fn the_age_column_is_as_wide_as_its_widest_entry() {
        assert_eq!(age_width(&["3d ago".into(), "just now".into()]), 8);
        assert_eq!(age_width(&[]), 0);
    }
}
