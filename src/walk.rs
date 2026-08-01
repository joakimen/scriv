//! Walking a directory tree for files to offer in a selector.
//!
//! Built on the [`ignore`] crate — the same walker [fd] is — so `.gitignore`,
//! `.ignore` and `.fdignore` are honoured in-process, with no `fd` subprocess
//! to find on `PATH`.
//!
//! [fd]: https://github.com/sharkdp/fd

use std::path::Path;
use std::sync::mpsc::sync_channel;

use ignore::{WalkBuilder, WalkState};

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

/// How many discovered paths may sit between the walk and whatever is reading
/// it.
///
/// The walk runs on its own threads, so results the selector has not taken yet
/// need somewhere to wait. Bounded, not unbounded: a walk of `$HOME` finds
/// files faster than a selector consumes them, and an unbounded queue would hold
/// the whole tree in memory to no purpose. A full queue blocks the walker
/// threads, and that backpressure is what keeps the footprint flat however
/// large the tree is.
const QUEUE_BOUND: usize = 1024;

/// Files under `root`, as paths relative to `root`.
///
/// Streamed: rows are handed over as they are found, so a selector fed from this
/// opens on the first filename rather than the last, and memory stays bounded
/// by [`QUEUE_BOUND`] however large the tree. Dotfiles are included — config
/// files are common targets — while ignore files and [`WALKER_SKIP`] are
/// respected.
///
/// **The order is not specified.** The walk runs on several threads, which is
/// worth roughly a threefold speedup on a large tree and costs the
/// directory-at-a-time ordering a single-threaded walk gave for free. Nothing
/// downstream depends on it: skim ranks rows by the query the moment one is
/// typed, and the untyped order is arrival order either way.
///
/// **Unreadable paths are skipped, not reported.** This is `fzf`'s
/// `find … 2>/dev/null`: a walk of `$HOME` on macOS crosses directories that
/// only an app with Full Disk Access can read (`~/Library/CallHistory…` and
/// friends), and failing the whole selector over one of them — or printing at a
/// terminal skim is drawing on — is worse than leaving them out. There is
/// nothing to grant from in here either: Full Disk Access is given to the
/// terminal application, not to a process that asks for it.
pub fn files(root: &Path) -> impl Iterator<Item = String> + Send + 'static {
    let root = root.to_path_buf();
    let (tx, rx) = sync_channel::<String>(QUEUE_BOUND);

    let walker = WalkBuilder::new(&root)
        .hidden(false) // include dotfiles; config files are common targets
        .require_git(false) // honour .gitignore/.ignore even outside a git repo
        .add_custom_ignore_filename(".fdignore") // shared with fd, if the user has one
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !WALKER_SKIP.contains(&name))
        })
        .build_parallel();

    // Detached: `run` blocks until the tree is exhausted, and the consumer has
    // no reason to wait on it — dropping the receiver is how it is called off.
    std::thread::spawn(move || {
        walker.run(|| {
            let tx = tx.clone();
            let root = root.clone();
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    return WalkState::Continue;
                }
                let path = entry.path();
                let rel = path.strip_prefix(&root).unwrap_or(path);
                match tx.send(rel.to_string_lossy().into_owned()) {
                    Ok(()) => WalkState::Continue,
                    // The receiver is gone: the user has selected or cancelled,
                    // so stop walking rather than spend the rest of the tree on
                    // a selector that is no longer there.
                    Err(_) => WalkState::Quit,
                }
            })
        });
    });

    rx.into_iter()
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
    /// expects the same paths to stay out of scriv's selectors.
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
    /// selector to one is what this replaced.
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

    /// The walk hands rows over as it finds them, through a queue bounded by
    /// [`QUEUE_BOUND`] — so a tree several times that size only finishes if the
    /// walker threads block when the queue fills and resume as the consumer
    /// drains it.
    ///
    /// This is the test that earns the bounded queue. An unbounded one would
    /// hold the whole tree in memory and still pass every other test here; a
    /// mis-wired bounded one deadlocks, and deadlocks in front of a selector that
    /// has already taken over the terminal.
    #[test]
    fn files_streams_a_tree_larger_than_its_queue() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let count = QUEUE_BOUND * 3;
        for i in 0..count {
            fs::write(root.join(format!("f{i}")), "").unwrap();
        }

        let mut walk = files(root);
        // A row is there for the taking well before the tree is exhausted.
        assert!(walk.next().is_some());
        assert_eq!(walk.count(), count - 1, "the walk lost or repeated rows");
    }
}
