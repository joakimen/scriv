//! `scriv file` — manage the list of files you visit regularly (formerly `kf`).

use std::path::Path;

use anyhow::{Result, bail};

use crate::path::{display_path, expand_tilde, sanitize_file_path};
use crate::pick::{PickItem, Preview, file_preview};
use crate::term;
use crate::{Ctx, files, pick, walk};

/// `scriv file ls` — print known files, optionally with existence status.
pub fn ls(ctx: &Ctx, status: bool, missing: bool, exists: bool) -> Result<()> {
    ctx.ensure_files_migrated()?;
    let lines = files::read_lines(&ctx.files_path)?;

    let mut out = term::Listing::stdout();

    // Plain listing: expand `~` and print, nothing else.
    if !status && !missing && !exists {
        for line in &lines {
            if !out.line(&expand_tilde(line, ctx.home_str()))? {
                break;
            }
        }
        return Ok(());
    }

    let use_color = status && ctx.color();

    for line in &lines {
        let expanded = expand_tilde(line, ctx.home_str());
        let present = Path::new(&expanded).exists();

        if missing && present {
            continue;
        }
        if exists && !present {
            continue;
        }

        let row = match (status, use_color, present) {
            (false, _, _) => expanded,
            (true, true, true) => format!("\x1b[32m✓ {expanded}\x1b[0m"),
            (true, true, false) => format!("\x1b[31m✗ {expanded}\x1b[0m"),
            (true, false, true) => format!("✓ {expanded}"),
            (true, false, false) => format!("✗ {expanded}"),
        };
        if !out.line(&row)? {
            break;
        }
    }
    Ok(())
}

/// `scriv file add [path]` — record a file, canonicalising the path first.
///
/// With no `file`, a file is chosen interactively from the current directory
/// tree via the configured picker.
pub fn add(ctx: &Ctx, file: Option<&str>) -> Result<()> {
    ctx.ensure_files_migrated()?;

    let file = match file {
        Some(file) => file.to_string(),
        None => match pick_from_cwd(ctx)? {
            Some(file) => file,
            None => return Ok(()),
        },
    };

    let sanitized = sanitize_file_path(&file, ctx.home_str(), ctx.pwd_str());
    let expanded = expand_tilde(&sanitized, ctx.home_str());
    if !Path::new(&expanded).exists() {
        eprintln!("warning: {expanded} does not exist");
    }

    let mut lines = files::read_lines(&ctx.files_path)?;
    if lines.iter().any(|line| line == &sanitized) {
        bail!("entry already exists in the known-files list");
    }

    lines.push(sanitized.clone());
    files::write_lines(&ctx.files_path, &lines)?;
    println!("Added {sanitized}");
    Ok(())
}

/// `scriv file remove [path]` — remove a file, by argument or interactively.
pub fn remove(ctx: &Ctx, file: Option<&str>) -> Result<()> {
    ctx.ensure_files_migrated()?;
    match file {
        Some(file) => remove_by_arg(ctx, file),
        None => remove_interactive(ctx),
    }
}

fn remove_by_arg(ctx: &Ctx, file: &str) -> Result<()> {
    let sanitized = sanitize_file_path(file, ctx.home_str(), ctx.pwd_str());

    let lines = files::read_lines(&ctx.files_path)?;
    let (kept, removed) = files::partition_remove(&lines, std::slice::from_ref(&sanitized));
    if removed.is_empty() {
        println!("No matching entry found");
        return Ok(());
    }

    files::write_lines(&ctx.files_path, &kept)?;
    println!("Removed {sanitized}");
    Ok(())
}

fn remove_interactive(ctx: &Ctx) -> Result<()> {
    let lines = files::read_lines(&ctx.files_path)?;
    if lines.is_empty() {
        println!("No known files");
        return Ok(());
    }

    // Show a `~`-collapsed label; the returned value is the stored line so it
    // matches the list for removal.
    let items: Vec<PickItem> = lines
        .iter()
        .map(|line| {
            let expanded = expand_tilde(line, ctx.home_str());
            let shown = display_path(&expanded, ctx.home_str(), false);
            PickItem::new(shown, line.clone()).preview(file_preview(&expanded))
        })
        .collect();

    let selected = match pick::pick_many(items, "Select files to remove", &ctx.config.picker) {
        Ok(selected) => selected,
        Err(e) if e.is::<pick::Cancelled>() => return Ok(()),
        Err(e) => return Err(e),
    };

    let (kept, removed) = files::partition_remove(&lines, &selected);
    if removed.is_empty() {
        return Ok(());
    }

    files::write_lines(&ctx.files_path, &kept)?;
    for file in &removed {
        println!("Removed {file}");
    }
    Ok(())
}

/// `scriv file pick` — fuzzy-select a known file and print its absolute path.
///
/// The picker shows `~`-collapsed paths; the printed path is absolute.
pub fn pick(ctx: &Ctx) -> Result<()> {
    ctx.ensure_files_migrated()?;
    let lines = files::read_lines(&ctx.files_path)?;
    if lines.is_empty() {
        bail!("no known files yet — add one with `scriv file add <path>`");
    }

    let items: Vec<PickItem> = lines
        .iter()
        .map(|line| {
            let abs = expand_tilde(line, ctx.home_str());
            let shown = display_path(&abs, ctx.home_str(), false);
            PickItem::new(shown, abs.clone()).preview(file_preview(&abs))
        })
        .collect();

    let choice = pick::pick_one(items, "Pick a file", &ctx.config.picker)?;
    println!("{choice}");
    Ok(())
}

/// Interactively choose a file from the current directory tree to add.
///
/// The walk is streamed into the picker, so it opens on the first filename
/// rather than the last — see [`crate::cmd::edit`]. Returns `Ok(None)` when the
/// user cancels the picker.
fn pick_from_cwd(ctx: &Ctx) -> Result<Option<String>> {
    let items =
        walk::files(Path::new(".")).map(|file| PickItem::plain(file).preview(Preview::File));
    match pick::pick_one_streamed(items, "Add a file", true, &ctx.config.picker) {
        Ok(choice) => Ok(Some(choice)),
        Err(e) if e.is::<pick::Cancelled>() => Ok(None),
        Err(e) => Err(e),
    }
}
