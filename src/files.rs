//! The known-files list: storage and the pure helpers that shape its contents.
//!
//! The list is a line-oriented text file (one path per line) written
//! programmatically by the `file` commands. Writes are normalised and atomic:
//! the file is only ever replaced wholesale via a temp file and rename, so a
//! crash mid-write never leaves it truncated.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result};

/// Read the list into lines with their trailing newline stripped.
///
/// A missing file is not an error: it yields an empty list, matching the
/// "no known files yet" state.
pub fn read_lines(path: &Path) -> Result<Vec<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents.lines().map(|l| l.to_string()).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading known-files list {}", path.display())),
    }
}

/// Write `lines` to `path` after normalising them (trim, dedupe, sort).
///
/// The parent directory is created with `0700` if absent, and the write is
/// atomic via a temp file in the same directory followed by a rename.
pub fn write_lines(path: &Path, lines: &[String]) -> Result<()> {
    let normalized = normalize_entries(lines);

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating config directory {}", dir.display()))?;

    let tmp = dir.join(temp_name());
    let write_result = (|| -> Result<()> {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("creating temp file {}", tmp.display()))?;
        for line in &normalized {
            writeln!(file, "{line}").with_context(|| format!("writing to {}", tmp.display()))?;
        }
        file.sync_all()
            .with_context(|| format!("flushing {}", tmp.display()))?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("replacing known-files list {}", path.display())
    })
}

fn temp_name() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".scriv-files-{}-{}.tmp", std::process::id(), seq)
}

/// Normalise list entries for persistence: trim each line, drop blanks, remove
/// duplicates, and sort so the written file is deterministic.
///
/// Deduped by sorting rather than through a set: the output is sorted either
/// way, and a `HashSet` would need its own copy of every entry to hold the keys
/// it compares against.
pub fn normalize_entries(lines: &[String]) -> Vec<String> {
    let mut result: Vec<String> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    result.sort();
    result.dedup();
    result
}

/// Split `lines` into the entries to keep and the entries removed, preserving
/// the original order of the kept entries. Matching is exact against the raw
/// stored line.
pub fn partition_remove(lines: &[String], to_remove: &[String]) -> (Vec<String>, Vec<String>) {
    let remove: HashSet<&String> = to_remove.iter().collect();
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for line in lines {
        if remove.contains(line) {
            removed.push(line.clone());
        } else {
            kept.push(line.clone());
        }
    }
    (kept, removed)
}

/// Split `lines` into the entries whose file is still there and the entries
/// whose file has gone, preserving the original order of both.
///
/// `present` is passed in rather than reaching for the filesystem, which is
/// what keeps this a decision with a test rather than something only observable
/// against a real directory. It is given the raw stored line — expanding `~` is
/// the caller's job, since only the caller knows the home directory.
pub fn partition_missing(
    lines: &[String],
    present: impl Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut missing = Vec::new();
    for line in lines {
        if present(line) {
            kept.push(line.clone());
        } else {
            missing.push(line.clone());
        }
    }
    (kept, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn owned(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normalize_sorts_alphabetically() {
        assert_eq!(
            normalize_entries(&owned(&["~/z/file.txt", "~/a/file.txt", "~/m/file.txt"])),
            owned(&["~/a/file.txt", "~/m/file.txt", "~/z/file.txt"])
        );
    }

    #[test]
    fn normalize_removes_duplicates() {
        assert_eq!(
            normalize_entries(&owned(&["~/b.txt", "~/a.txt", "~/b.txt"])),
            owned(&["~/a.txt", "~/b.txt"])
        );
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_entries(&owned(&[" ~/b.txt ", " ~/a.txt "])),
            owned(&["~/a.txt", "~/b.txt"])
        );
    }

    #[test]
    fn normalize_drops_blank_lines() {
        assert_eq!(
            normalize_entries(&owned(&["~/a.txt", "   ", "", "~/b.txt"])),
            owned(&["~/a.txt", "~/b.txt"])
        );
    }

    #[test]
    fn partition_splits_kept_and_removed() {
        let lines = owned(&["a", "b", "c", "d"]);
        let (kept, removed) = partition_remove(&lines, &owned(&["b", "d"]));
        assert_eq!(kept, owned(&["a", "c"]));
        assert_eq!(removed, owned(&["b", "d"]));
    }

    #[test]
    fn partition_ignores_unmatched_targets() {
        let lines = owned(&["a", "b"]);
        let (kept, removed) = partition_remove(&lines, &owned(&["x"]));
        assert_eq!(kept, owned(&["a", "b"]));
        assert!(removed.is_empty());
    }

    /// Both halves keep the order the list was in, so what `prune` prints
    /// reads in the same order as `file ls` — the list the user knows.
    #[test]
    fn partition_missing_splits_on_what_is_still_there() {
        let lines = owned(&["a", "b", "c", "d"]);
        let (kept, missing) = partition_missing(&lines, |line| line == "a" || line == "c");
        assert_eq!(kept, owned(&["a", "c"]));
        assert_eq!(missing, owned(&["b", "d"]));
    }

    /// A list where nothing is missing must come back untouched, so `prune` can
    /// tell there is nothing to ask about.
    #[test]
    fn partition_missing_finds_nothing_when_every_file_is_there() {
        let lines = owned(&["a", "b"]);
        let (kept, missing) = partition_missing(&lines, |_| true);
        assert_eq!(kept, lines);
        assert!(missing.is_empty());
    }

    #[test]
    fn read_missing_file_is_empty() {
        let dir = TempDir::new().unwrap();
        let got = read_lines(&dir.path().join("nope")).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn write_then_read_roundtrips_normalized() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/files");
        write_lines(&path, &owned(&["~/b.txt", "~/a.txt", "~/b.txt", "  "])).unwrap();
        assert_eq!(read_lines(&path).unwrap(), owned(&["~/a.txt", "~/b.txt"]));
    }
}
