//! `scriv note` — list, select and open the notes in your vault.
//!
//! A registry like `repo` and `file`: the set is every Markdown file under
//! `[note] root`, and the verbs act on what is selected from it. The imperative
//! half lives here — the walk, the file reads, the clock and the editor;
//! [`crate::note`] decides everything else.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Result, bail};

use crate::note::{self, Note, Widths};
use crate::path::expand_home_dir;
use crate::select::{Preview, SelectItem};
use crate::{Ctx, cmd, select, term};

/// How much of a note is read to find its front matter.
///
/// The block sits at the very top of the file, so this is all a listing needs
/// and a vault is spared having every byte of every note read to build one. A
/// block longer than this is not front matter anybody wrote by hand, and the
/// note is listed by its filename as though it had none.
const HEAD_BYTES: u64 = 8 * 1024;

/// How much of a note the preview pane reads. Bounded for the same reason a
/// `Preview::Command` is: skim runs one on every move through the list.
const PREVIEW_BYTES: u64 = 128 * 1024;

/// The vault, expanded — the one directory `note` looks in.
fn vault(ctx: &Ctx) -> Result<PathBuf> {
    let root = ctx.config.note.root.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "`[note] root` is not set in {} — `scriv note` has nowhere to look",
            ctx.config_path.display()
        )
    })?;
    let path = expand_home_dir(root, ctx.home());
    if !path.is_dir() {
        bail!(
            "`[note] root` is {}, which is not a directory",
            path.display()
        );
    }
    Ok(path)
}

/// Every note in the vault, most recently modified first.
///
/// The walk and the head reads run on the walker's own threads, so a vault is
/// one pass rather than a walk followed by a read per note. Dotfiles are
/// skipped, which is what leaves Obsidian's `.obsidian` and `.trash` out
/// without naming either.
fn load(ctx: &Ctx) -> Result<Vec<Note>> {
    let root = vault(ctx)?;
    let offset = ctx.utc_offset();
    let found = Mutex::new(Vec::new());

    ignore::WalkBuilder::new(&root)
        .hidden(true)
        .require_git(false)
        .add_custom_ignore_filename(".fdignore")
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                if let Ok(entry) = entry
                    && entry.file_type().is_some_and(|ft| ft.is_file())
                    && note::is_note(entry.path())
                    && let Some(found_note) = read_note(entry.path(), &root, offset)
                {
                    found
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(found_note);
                }
                ignore::WalkState::Continue
            })
        });

    let mut notes = found.into_inner().unwrap_or_else(|e| e.into_inner());
    note::newest_first(&mut notes);
    ctx.log
        .info(&format!("{} note(s) under {}", notes.len(), root.display()));
    if notes.is_empty() {
        bail!(
            "no notes under {} — `scriv note` lists Markdown files",
            root.display()
        );
    }
    Ok(notes)
}

/// One note, read: its times from the directory entry's metadata, its front
/// matter from the first [`HEAD_BYTES`].
///
/// `None` for a file whose metadata cannot be read, which is one note missing
/// from a listing rather than the listing failing — the same way the walk
/// treats a directory it may not enter.
fn read_note(path: &Path, root: &Path, offset: time::UtcOffset) -> Option<Note> {
    let meta = path.metadata().ok()?;
    let modified = unix(meta.modified().ok()?)?;
    let birth = meta.created().ok().and_then(unix);

    let head = read_head(path, HEAD_BYTES).unwrap_or_default();
    let front = match note::split_front_matter(&head) {
        (Some(block), _) => note::parse_front(block, offset),
        (None, _) => note::Front::default(),
    };

    Some(Note {
        rel: path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned(),
        path: path.to_path_buf(),
        modified,
        created: note::created(front.created, birth, modified),
        front,
    })
}

/// The first `limit` bytes of `path`, as text. A note is whatever the user's
/// editor wrote, so invalid UTF-8 — and a multi-byte character the limit cut in
/// half — is replaced rather than refused.
fn read_head(path: &Path, limit: u64) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    std::io::BufReader::new(file)
        .take(limit)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn unix(time: SystemTime) -> Option<i64> {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => Some(since.as_secs() as i64),
        // Before 1970. A file dated that way is a clock that was wrong, not a
        // note to leave out of the list.
        Err(before) => Some(-(before.duration().as_secs() as i64)),
    }
}

/// `scriv note ls` — print the notes, most recently modified first.
///
/// Plain, one path below the vault per line, which is the name `note edit`
/// takes. `--status` adds the tags and both dates; `--absolute-paths` prints
/// what a pipe can open.
pub fn ls(ctx: &Ctx, absolute_paths: bool, status: bool) -> Result<()> {
    let notes = load(ctx)?;
    let widths = Widths::of(&notes, crate::unix_now());
    let offset = ctx.utc_offset();

    let mut out = term::Listing::stdout();
    for note in &notes {
        let row = match (absolute_paths, status) {
            (true, _) => note.path.display().to_string(),
            (false, false) => note.rel.clone(),
            (false, true) => note::status_row(note, &widths, offset),
        };
        if !out.line(&row)? {
            break;
        }
    }
    out.finish()?;
    Ok(())
}

/// `scriv note sel` — fuzzy-select a note and print its absolute path.
pub fn sel(ctx: &Ctx) -> Result<()> {
    let notes = load(ctx)?;
    let choice = select::select_one(items(ctx, &notes), "Select a note", &ctx.config.selector)?;
    println!("{choice}");
    Ok(())
}

/// `scriv note edit [NAME]...` — open notes in `[note] editor`, selecting them
/// when none are named.
pub fn edit(ctx: &Ctx, names: &[String]) -> Result<()> {
    let editor = ctx.note_editor()?;

    let targets = if names.is_empty() {
        match select_notes(ctx)? {
            Some(targets) => targets,
            None => return Ok(()),
        }
    } else {
        let root = vault(ctx)?;
        names
            .iter()
            .map(|name| resolve(&root, ctx.home(), name))
            .collect()
    };

    if targets.is_empty() {
        return Ok(());
    }
    cmd::edit::launch(ctx, &editor, &targets)
}

/// A note named on the command line, as a path. A name is relative to the
/// vault, so `scriv note edit` takes back exactly what `scriv note ls` printed;
/// an absolute path, or one that begins with `~`, is left where it points.
fn resolve(root: &Path, home: &Path, name: &str) -> String {
    let path = expand_home_dir(name, home);
    if path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }
    root.join(path).to_string_lossy().into_owned()
}

/// Choose notes from the vault. `Ok(None)` when the user cancels.
fn select_notes(ctx: &Ctx) -> Result<Option<Vec<String>>> {
    let notes = load(ctx)?;
    match select::select_many(items(ctx, &notes), "Edit a note", &ctx.config.selector) {
        Ok(chosen) => Ok(Some(chosen)),
        Err(e) if e.is::<select::Cancelled>() => Ok(None),
        Err(e) => Err(e),
    }
}

/// Selector rows: the dim age columns as a prefix, the title, folder and tags
/// as the label, and the note's absolute path as the value.
///
/// Every pane is built when its row is highlighted rather than now — see
/// [`Preview::Deferred`]. A vault read up front is one file read per note for
/// panes the user scrolls past.
fn items(ctx: &Ctx, notes: &[Note]) -> Vec<SelectItem> {
    let now = crate::unix_now();
    let widths = Widths::of(notes, now);
    let offset = ctx.utc_offset();

    notes
        .iter()
        .map(|note| {
            let (label, tints) = note::row(note, &widths);
            let note = note.clone();
            SelectItem::new(label, note.path.to_string_lossy().into_owned())
                .prefix(note::prefix(&note, now, &widths))
                .tints(tints)
                .preview(Preview::Deferred(Box::new(move || {
                    let text = read_head(&note.path, PREVIEW_BYTES).unwrap_or_default();
                    note::preview(&note, &text, now, offset)
                })))
        })
        .collect()
}

/// The `config check` row for the vault: where it is, and how much is in it.
/// One row rather than two, since a root that resolves and holds nothing has
/// already been reported by the count.
pub(crate) fn vault_summary(ctx: &Ctx) -> Result<(PathBuf, usize)> {
    let root = vault(ctx)?;
    let count = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .require_git(false)
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .filter(|entry| note::is_note(entry.path()))
        .count();
    Ok((root, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn utc() -> time::UtcOffset {
        time::UtcOffset::UTC
    }

    #[test]
    fn a_note_takes_its_name_from_where_it_sits_below_the_vault() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("work/meetings")).unwrap();
        let path = root.join("work/meetings/standup.md");
        std::fs::write(&path, "# Standup\n").unwrap();

        let note = read_note(&path, root, utc()).unwrap();

        assert_eq!(note.rel, "work/meetings/standup.md");
        assert_eq!(note.title(), "standup");
        assert_eq!(note.folder(), "work/meetings");
    }

    #[test]
    fn front_matter_is_read_from_the_head_of_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(
            &path,
            "---\ntitle: Error handling\ntags: [rust, til]\n---\n\nbody\n",
        )
        .unwrap();

        let note = read_note(&path, dir.path(), utc()).unwrap();

        assert_eq!(note.title(), "Error handling");
        assert_eq!(note.tag_column(), "#rust #til");
    }

    /// The bound is what keeps a vault one pass rather than a full read per
    /// note; a note longer than it must still list, by its filename.
    #[test]
    fn a_note_larger_than_the_head_bound_still_lists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("long.md");
        let body = "x".repeat(HEAD_BYTES as usize * 2);
        std::fs::write(&path, format!("---\ntitle: Long\n---\n{body}")).unwrap();

        let note = read_note(&path, dir.path(), utc()).unwrap();

        assert_eq!(note.title(), "Long");
    }

    /// A note nothing named a creation date for falls back to the filesystem,
    /// and never reads as newer than its own last edit.
    #[test]
    fn an_undated_note_is_created_no_later_than_it_was_modified() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "body\n").unwrap();

        let note = read_note(&path, dir.path(), utc()).unwrap();

        assert!(note.created <= note.modified, "{note:?}");
    }

    #[test]
    fn a_named_note_resolves_against_the_vault_but_an_absolute_path_does_not() {
        let (root, home) = (Path::new("/vault"), Path::new("/home/me"));
        assert_eq!(
            resolve(root, home, "work/standup.md"),
            "/vault/work/standup.md"
        );
        assert_eq!(resolve(root, home, "/elsewhere/a.md"), "/elsewhere/a.md");
        assert_eq!(resolve(root, home, "~/inbox.md"), "/home/me/inbox.md");
    }
}
