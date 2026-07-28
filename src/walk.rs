//! Walking a directory tree for files to offer in a picker.
//!
//! Built on the [`ignore`] crate — the same walker [fd] is — so `.gitignore`,
//! `.ignore` and `.fdignore` are honoured in-process, with no `fd` subprocess
//! to find on `PATH`.
//!
//! [fd]: https://github.com/sharkdp/fd

use std::path::Path;

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

/// Files under `root`, as paths relative to `root`.
///
/// Lazy: the walk advances as the iterator is drained, so a picker fed from it
/// opens on the first filename rather than the last. Dotfiles are included —
/// config files are common targets — while ignore files and [`WALKER_SKIP`] are
/// respected, and entries are sorted within each directory so the order is the
/// same twice running.
///
/// **Unreadable paths are skipped, not reported.** This is `fzf`'s
/// `find … 2>/dev/null`: a walk of `$HOME` on macOS crosses directories that
/// only an app with Full Disk Access can read (`~/Library/CallHistory…` and
/// friends), and failing the whole picker over one of them — or printing at a
/// terminal skim is drawing on — is worse than leaving them out. There is
/// nothing to grant from in here either: Full Disk Access is given to the
/// terminal application, not to a process that asks for it.
pub fn files(root: &Path) -> impl Iterator<Item = String> + Send + 'static {
    let root = root.to_path_buf();
    WalkBuilder::new(&root)
        .hidden(false) // include dotfiles; config files are common targets
        .require_git(false) // honour .gitignore/.ignore even outside a git repo
        .add_custom_ignore_filename(".fdignore") // shared with fd, if the user has one
        .sort_by_file_path(Ord::cmp) // stable order, one directory at a time
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !WALKER_SKIP.contains(&name))
        })
        .build()
        .filter_map(move |entry| {
            let entry = entry.ok()?;
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return None;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&root).unwrap_or(path);
            Some(rel.to_string_lossy().into_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn list(root: &Path) -> Vec<String> {
        files(root).collect()
    }

    #[test]
    fn files_walks_and_skips() {
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

        let got = list(root);

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
    fn files_honours_fdignore() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("keep.txt"), "").unwrap();
        fs::write(root.join(".fdignore"), "drop.txt\n").unwrap();
        fs::write(root.join("drop.txt"), "").unwrap();

        let got = list(root);

        assert!(got.contains(&"keep.txt".to_string()));
        assert!(
            !got.contains(&"drop.txt".to_string()),
            ".fdignore not honoured: {got:?}"
        );
    }

    /// A directory the user cannot read must cost its own subtree and nothing
    /// else — on macOS a walk of `$HOME` hits several, and losing the whole
    /// picker to one is what this replaced.
    #[cfg(unix)]
    #[test]
    fn files_skips_unreadable_directories() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("readable.txt"), "").unwrap();
        let locked = root.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("hidden.txt"), "").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let got = list(root);

        // Restore before the TempDir tries to remove it.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            got.contains(&"readable.txt".to_string()),
            "an unreadable sibling swallowed the walk: {got:?}"
        );
        assert!(!got.iter().any(|f| f.contains("hidden.txt")));
    }

    /// The walk must not run ahead of what is asked of it — a picker fed from
    /// it shows rows long before the tree is exhausted. Observed by writing
    /// into a directory the walk has not reached yet: only a lazy walk can
    /// still find it.
    #[test]
    fn files_is_lazy() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("a")).unwrap();
        fs::write(root.join("a/early.txt"), "").unwrap();
        fs::create_dir(root.join("b")).unwrap();

        let mut walk = files(root);
        assert_eq!(walk.next().as_deref(), Some("a/early.txt"));
        fs::write(root.join("b/late.txt"), "").unwrap();

        let rest: Vec<String> = walk.collect();

        assert_eq!(rest, vec!["b/late.txt".to_string()]);
    }
}
