//! `scriv file` — manage the list of files you visit regularly (formerly `kf`).

use std::path::Path;

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;

use crate::path::{display_path, expand_tilde, sanitize_file_path};
use crate::pick::PickItem;
use crate::{Ctx, files, pick};

/// `scriv file ls` — print known files, optionally with existence status.
pub fn ls(ctx: &Ctx, status: bool, missing: bool, exists: bool) -> Result<()> {
    ctx.ensure_files_migrated()?;
    let lines = files::read_lines(&ctx.files_path)?;

    // Plain listing: expand `~` and print, nothing else.
    if !status && !missing && !exists {
        for line in &lines {
            println!("{}", expand_tilde(line, ctx.home_str()));
        }
        return Ok(());
    }

    let use_color = status && stdout_is_tty() && !no_color();

    for line in &lines {
        let expanded = expand_tilde(line, ctx.home_str());
        let present = Path::new(&expanded).exists();

        if missing && present {
            continue;
        }
        if exists && !present {
            continue;
        }

        if !status {
            println!("{expanded}");
            continue;
        }

        match (use_color, present) {
            (true, true) => println!("\x1b[32m✓ {expanded}\x1b[0m"),
            (true, false) => println!("\x1b[31m✗ {expanded}\x1b[0m"),
            (false, true) => println!("✓ {expanded}"),
            (false, false) => println!("✗ {expanded}"),
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
            let shown = display_path(&expand_tilde(line, ctx.home_str()), ctx.home_str(), false);
            PickItem::new(shown, line.clone())
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
        bail!("no known files");
    }

    let items: Vec<PickItem> = lines
        .iter()
        .map(|line| {
            let abs = expand_tilde(line, ctx.home_str());
            let shown = display_path(&abs, ctx.home_str(), false);
            PickItem::new(shown, abs)
        })
        .collect();

    let choice = pick::pick_one(items, "Pick a file", &ctx.config.picker)?;
    println!("{choice}");
    Ok(())
}

/// Interactively choose a file from the current directory tree to add.
///
/// Returns `Ok(None)` when the user cancels the picker.
fn pick_from_cwd(ctx: &Ctx) -> Result<Option<String>> {
    let candidates = list_files(Path::new("."))?;
    if candidates.is_empty() {
        bail!("no files found in the current directory");
    }
    let items = candidates.into_iter().map(PickItem::plain).collect();
    match pick::pick_one(items, "Add a file", &ctx.config.picker) {
        Ok(choice) => Ok(Some(choice)),
        Err(e) if e.is::<pick::Cancelled>() => Ok(None),
        Err(e) => Err(e),
    }
}

/// Directory names never worth walking into when picking a file to add,
/// matching the user's fzf `--walker-skip` list.
const WALKER_SKIP: &[&str] = &[
    ".git",
    "node_modules",
    ".clj-kondo",
    ".cpcache",
    ".venv",
    "lib",
];

/// List files under `root`, as paths relative to `root`.
///
/// Uses the `ignore` crate — the same directory walker fd is built on — so it
/// honours `.gitignore` and skips [`WALKER_SKIP`] directories, all in-process
/// with no `fd` subprocess.
fn list_files(root: &Path) -> Result<Vec<String>> {
    let walker = WalkBuilder::new(root)
        .hidden(false) // include dotfiles; config files are common targets
        .require_git(false) // honour .gitignore/.ignore even outside a git repo
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !WALKER_SKIP.contains(&name))
        })
        .build();

    let mut files = Vec::new();
    for entry in walker {
        let entry = entry.context("walking the directory")?;
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(path);
            files.push(rel.to_string_lossy().into_owned());
        }
    }
    files.sort();
    Ok(files)
}

fn stdout_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Honour the `NO_COLOR` convention: colour is disabled when the variable is
/// present and non-empty.
fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn list_files_walks_and_skips() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        // A skipped directory and a gitignored file must not appear.
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "").unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "").unwrap();

        let got = list_files(root).unwrap();

        assert!(got.contains(&"a.txt".to_string()));
        assert!(got.contains(&"src/main.rs".to_string()));
        assert!(got.contains(&".gitignore".to_string())); // dotfiles included
        assert!(
            !got.iter().any(|f| f.contains("node_modules")),
            "WALKER_SKIP dir leaked: {got:?}"
        );
        assert!(
            !got.contains(&"ignored.txt".to_string()),
            ".gitignore not honoured: {got:?}"
        );
    }
}
