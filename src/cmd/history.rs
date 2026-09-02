//! `scriv history` — search the commands you have already run.
//!
//! The list is fish's own history file, newest first with repeats dropped and
//! scriv's own key bindings left out. Selecting a command prints it rather than
//! running it; the fish integration puts it back on the command line to be read
//! before enter.

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

    // A history file holds whatever was typed at the shell, which is not
    // always valid UTF-8.
    let entries = history::recent_first(history::typed_only(history::parse(
        &String::from_utf8_lossy(&data),
    )));
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
/// shown without being searched — matched, a query of `3` would rank thousands
/// of timestamps first. No row carries a preview, so the selector keeps the
/// full width for the commands.
/// Almost every command is one line with nothing in it to strip, and folding
/// leaves it identical. Those rows keep one copy of the text rather than a
/// label and a value that happen to be equal — this runs over the whole history
/// on every ctrl-r.
fn items(entries: &[Entry], offset: time::UtcOffset) -> Vec<SelectItem> {
    entries
        .iter()
        .map(|entry| {
            let folded = history::one_line(&entry.cmd);
            let item = if folded == entry.cmd {
                SelectItem::plain(folded)
            } else {
                SelectItem::new(folded, entry.cmd.clone())
            };
            item.prefix(
                format!("{}  ", history::stamp(entry.when, offset)),
                Vec::new(),
            )
        })
        .collect()
}

/// `scriv history ls` — print past commands, newest first, one per line.
///
/// A multi-line command is folded onto one line, so the output stays one entry
/// per line. `--status` prefixes the local date and time each was last run, in
/// a fixed-width sortable form.
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
    out.finish()?;
    Ok(())
}

/// `scriv history sel` — fuzzy-select a past command and print it.
///
/// `query` seeds the search box. `print0` terminates the result with a NUL
/// rather than a newline, since a command may itself contain newlines.
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

    #[test]
    fn rows_fold_onto_one_line_but_return_the_real_command() {
        let items = items(&entries(), utc());
        assert_eq!(items[1].label, "git commit -m 'a ⏎ b'");
        assert_eq!(items[1].value(), "git commit -m 'a\nb'");
    }

    #[test]
    fn the_date_is_shown_beside_the_command_but_not_matched() {
        let items = items(&entries(), utc());
        assert_eq!(items[0].prefix.as_deref(), Some("1970-01-01 00:15  "));
        assert_eq!(items[0].label, "git status");
        assert!(!items[0].label.contains("1970"), "the date is searchable");
    }

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

    /// The one-copy path must still return the command, whichever branch of
    /// `items` built the row.
    #[test]
    fn a_folded_row_and_an_untouched_one_both_return_the_command() {
        let items = items(&entries(), utc());
        assert_eq!(items[0].value(), "git status");
        assert_eq!(items[1].value(), "git commit -m 'a\nb'");
    }

    #[test]
    fn no_row_opens_a_preview_pane() {
        for item in items(&entries(), utc()) {
            assert!(item.preview.is_none(), "a history row previews itself");
        }
    }
}
