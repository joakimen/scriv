//! `scriv edit` — fuzzy-find a file and open it in your editor.
//!
//! Unlike the other commands, this one is a verb rather than a registry: it
//! searches whatever directory you are in rather than a list scriv maintains.
//! `--tracked` points the same selector at the known-files list instead.
//!
//! Nothing here needs shell integration — a child process cannot change its
//! parent's directory, which is why `repo sel` is wrapped in a fish function,
//! but it can perfectly well inherit the terminal and run an editor in it.

use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, anyhow, bail};

use crate::path::{display_path, expand_tilde};
use crate::select::{Preview, SelectItem, file_preview};
use crate::{Ctx, Reported, files, select, walk};

/// `scriv edit [FILE]...` — open `paths`, or select interactively when empty.
///
/// With `tracked`, selection comes from the known-files list; otherwise from
/// the current directory tree. Multiple selections open together, which is what
/// an editor's buffer list is for.
pub fn run(ctx: &Ctx, paths: &[String], tracked: bool) -> Result<()> {
    let targets = if !paths.is_empty() {
        paths.to_vec()
    } else {
        let selected = if tracked {
            select_tracked(ctx)?
        } else {
            select_from_cwd(ctx)?
        };
        match selected {
            Some(targets) => targets,
            // Cancelled: a conventional silent exit, nothing to open.
            None => return Ok(()),
        }
    };

    // Selecting nothing is not an error either — the selector was simply left
    // with no rows marked.
    if targets.is_empty() {
        return Ok(());
    }

    open(ctx, &targets)
}

/// Choose files from the current directory tree.
///
/// The walk is streamed into the selector rather than collected first: a home
/// directory can hold a million files, and waiting out the last one before
/// showing the first is the difference between a selector that opens instantly
/// and one that appears to hang. An empty tree simply gives an empty selector,
/// as it would in `fzf`.
fn select_from_cwd(ctx: &Ctx) -> Result<Option<Vec<String>>> {
    // Paths stay relative to the working directory: the editor is launched from
    // it, and relative paths are what shows up in its buffer list.
    let items =
        walk::files(Path::new(".")).map(|file| SelectItem::plain(file).preview(Preview::File));

    cancellable(select::select_many_streamed(
        items,
        "Edit",
        true,
        &ctx.config.selector,
    ))
}

/// Choose files from the known-files list.
fn select_tracked(ctx: &Ctx) -> Result<Option<Vec<String>>> {
    ctx.ensure_files_migrated()?;
    let lines = files::read_lines(&ctx.files_path)?;
    if lines.is_empty() {
        bail!("no known files yet — add one with `scriv file add <path>`");
    }

    // Show a `~`-collapsed label; hand the editor the absolute path, since the
    // list is global and rarely relative to where the user is standing.
    let items = lines
        .iter()
        .map(|line| {
            let abs = expand_tilde(line, ctx.home_str());
            let shown = display_path(&abs, ctx.home_str(), false);
            SelectItem::new(shown, abs.clone()).preview(file_preview(&abs))
        })
        .collect();

    cancellable(select::select_many(items, "Edit", &ctx.config.selector))
}

/// Map a cancelled selector to `None`, leaving real errors alone.
fn cancellable(result: Result<Vec<String>>) -> Result<Option<Vec<String>>> {
    match result {
        Ok(selected) => Ok(Some(selected)),
        Err(e) if e.is::<select::Cancelled>() => Ok(None),
        Err(e) => Err(e),
    }
}

/// Launch the editor on `targets`, inheriting the terminal.
///
/// A non-zero exit is [`Reported`]: the editor has already had the terminal and
/// said whatever it wanted to say, so scriv passes the status through without
/// adding a line of its own.
fn open(ctx: &Ctx, targets: &[String]) -> Result<()> {
    let editor = ctx.editor()?;
    let (program, args) = editor
        .split_first()
        .expect("Ctx::editor rejects an empty command");

    ctx.log
        .info(&format!("opening {} file(s) with {program}", targets.len()));

    let status = Command::new(program)
        .args(args)
        .args(targets)
        .status()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => anyhow!("`{program}` was not found on PATH"),
            _ => anyhow!(e).context(format!("running {program}")),
        })?;

    if !status.success() {
        return Err(Reported(status.code().unwrap_or(1)).into());
    }
    Ok(())
}
