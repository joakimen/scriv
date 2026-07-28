//! Walking a directory tree for files to offer in a picker.
//!
//! Built on the [`ignore`] crate — the same walker [fd] is — so `.gitignore`,
//! `.ignore` and `.fdignore` are honoured in-process, with no `fd` subprocess
//! to find on `PATH`.
//!
//! [fd]: https://github.com/sharkdp/fd

use std::path::Path;

use anyhow::{Context, Result};
use ignore::WalkBuilder;

/// Directory names never worth walking into, matching the `--walker-skip` list
/// this replaced. `.gitignore` covers most build output; these are the
/// directories that are commonly *not* ignored yet never worth offering.
const WALKER_SKIP: &[&str] = &[
    ".git",
    "node_modules",
    ".clj-kondo",
    ".cpcache",
    ".venv",
    "lib",
];

/// List files under `root`, as paths relative to `root`, sorted.
///
/// Dotfiles are included — config files are common targets — while ignore
/// files and [`WALKER_SKIP`] are respected.
pub fn list_files(root: &Path) -> Result<Vec<String>> {
    let walker = WalkBuilder::new(root)
        .hidden(false) // include dotfiles; config files are common targets
        .require_git(false) // honour .gitignore/.ignore even outside a git repo
        .add_custom_ignore_filename(".fdignore") // shared with fd, if the user has one
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

    /// fd reads `.fdignore` in addition to `.gitignore`; a user who keeps one
    /// expects the same paths to stay out of scriv's pickers.
    #[test]
    fn list_files_honours_fdignore() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("keep.txt"), "").unwrap();
        fs::write(root.join(".fdignore"), "drop.txt\n").unwrap();
        fs::write(root.join("drop.txt"), "").unwrap();

        let got = list_files(root).unwrap();

        assert!(got.contains(&"keep.txt".to_string()));
        assert!(
            !got.contains(&"drop.txt".to_string()),
            ".fdignore not honoured: {got:?}"
        );
    }
}
