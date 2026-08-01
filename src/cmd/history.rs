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
use crate::select::SelectItem;
use crate::term;
use crate::{Ctx, select};

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

/// Build selector rows: the local date it was last run, then the command folded
/// onto one line, returning the command as it was really typed.
///
/// The date is a [`SelectItem::prefix`] rather than part of the label, so it is
/// shown without being searched. A date is digits at the front of every row;
/// matched, a query of `3` would rank thousands of timestamps above the command
/// being reached for.
///
/// No row carries a preview, so the selector keeps the full width for the
/// commands: a preview pane would only repeat the row it is beside, and the
/// selected command goes back on the command line to be read before it runs.
fn items(entries: &[Entry], offset: time::UtcOffset) -> Vec<SelectItem> {
    entries
        .iter()
        .map(|entry| {
            SelectItem::new(history::one_line(&entry.cmd), entry.cmd.clone())
                .prefix(format!("{}  ", history::stamp(entry.when, offset)))
        })
        .collect()
}

/// `scriv history ls` — print past commands, newest first, one per line.
///
/// A multi-line command is folded onto its one line like everywhere else, so
/// the output stays one entry per line and pipes into `wc -l` or `grep`
/// meaning what it looks like it means. `--status` prefixes the local date and
/// time each was last run — the same column the selector shows, in a fixed-width
/// sortable form, so `awk` and `grep` can both work on it.
pub fn ls(ctx: &Ctx, status: bool) -> Result<()> {
    let entries = load(ctx)?;
    let offset = ctx.utc_offset();

    let mut out = term::Listing::stdout();
    for entry in &entries {
        let command = history::one_line(&entry.cmd);
        let row = if status {
            format!("{}  {command}", history::stamp(entry.when, offset))
        } else {
            command
        };
        if !out.line(&row)? {
            break;
        }
    }
    Ok(())
}

/// `scriv history sel` — fuzzy-select a past command and print it.
///
/// `query` seeds the search box, so ctrl-r pressed halfway through typing a
/// command starts narrowed to what is already there instead of throwing it
/// away. `print0` terminates the result with a NUL rather than a newline: a
/// command may itself contain newlines, and only a NUL tells the shell reading
/// this where one command ends.
pub fn sel(ctx: &Ctx, query: Option<&str>, print0: bool) -> Result<()> {
    let entries = load(ctx)?;
    let chosen = select::select_one_queried(
        items(&entries, ctx.utc_offset()),
        "Select a command",
        query.unwrap_or_default(),
        &ctx.config.selector,
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

    fn utc() -> time::UtcOffset {
        time::UtcOffset::UTC
    }

    /// The row is folded onto one line; selecting it has to yield the command
    /// as it was actually typed, newlines and all, or a multi-line command
    /// comes back as something the user never ran.
    #[test]
    fn rows_fold_onto_one_line_but_return_the_real_command() {
        let items = items(&entries(), utc());
        assert_eq!(items[1].label, "git commit -m 'a ⏎ b'");
        assert_eq!(items[1].value(), "git commit -m 'a\nb'");
    }

    /// The date is shown but never searched. It lives in the prefix, so the
    /// label — which is what skim matches the query against — is the command
    /// and nothing else. Fold the date into the label instead and a query of
    /// `3` ranks four thousand timestamps above the command being reached for.
    #[test]
    fn the_date_is_shown_beside_the_command_but_not_matched() {
        let items = items(&entries(), utc());
        assert_eq!(items[0].prefix.as_deref(), Some("1970-01-01 00:15  "));
        assert_eq!(items[0].label, "git status");
        assert!(!items[0].label.contains("1970"), "the date is searchable");
    }

    /// Undated rows keep the column open, so every command starts in the same
    /// place whatever fish recorded.
    #[test]
    fn an_undated_row_holds_the_date_column_open() {
        let items = items(&entries(), utc());
        let (dated, undated) = (
            items[0].prefix.as_deref().unwrap(),
            items[1].prefix.as_deref().unwrap(),
        );
        assert_eq!(dated.len(), undated.len());
        assert!(undated.trim().is_empty(), "{undated:?}");
    }

    /// No history row previews. The pane exists only when some row asks for
    /// one, so this is what keeps the full terminal width on the commands
    /// rather than on a pane repeating the highlighted row back.
    #[test]
    fn no_row_opens_a_preview_pane() {
        for item in items(&entries(), utc()) {
            assert!(item.preview.is_none(), "a history row previews itself");
        }
    }
}
