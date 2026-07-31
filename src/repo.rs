//! Repository discovery. Walking touches the filesystem, but the traversal
//! rules — depth limit, ignore list, and `.git` detection — are small and are
//! covered by tests that run against temporary directories.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::thread;

use crate::config::{Config, ROOT_DEPTH, UNLABELLED};
use crate::logger::Logger;
use crate::path::expand_home_dir;

/// A discovered repository, together with where it was found and who owns it.
pub struct FoundRepo {
    /// The label its owner carries, or [`UNLABELLED`].
    pub label: String,
    /// The GitHub owner, taken from the directory under the root. `None` for a
    /// repository from `extra`, which sits outside the `<owner>/<repo>` layout.
    pub owner: Option<String>,
    /// The search root it was found under, for rendering a relative label.
    pub root: PathBuf,
    pub path: PathBuf,
}

impl FoundRepo {
    /// `owner/repo` when known, otherwise the directory name — what identifies
    /// the repository in a list.
    pub fn slug(&self) -> String {
        let name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        match &self.owner {
            Some(owner) => format!("{owner}/{name}"),
            None => name.to_string(),
        }
    }
}

/// The owner of a repository at `path` under `root`: the single directory
/// between them, per the `<root>/<owner>/<repo>` layout.
///
/// `None` when `path` is not exactly that shape, which is what keeps a stray
/// checkout at the wrong depth from being labelled with a nonsense owner.
pub fn owner_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<_> = rel.components().collect();
    if parts.len() != ROOT_DEPTH {
        return None;
    }
    Some(parts[0].as_os_str().to_str()?.to_string())
}

/// Discover repositories under the configured root and any `extra` paths,
/// expanding `~` against `home` and tagging each with its owner and label.
///
/// Each search runs on its own thread. A missing path is a hard error — a typo
/// in the root should say so rather than quietly finding nothing; unreadable
/// subdirectories are logged and skipped.
pub fn find_all_repos(cfg: &Config, home: &Path, log: &Logger) -> Result<Vec<FoundRepo>> {
    if cfg.repo.root.is_none() && cfg.repo.extra.is_empty() {
        anyhow::bail!("no `root` set in config file");
    }

    // The root is searched at the owner/repo depth; each `extra` path is a
    // repository in its own right, so it is searched at depth 0.
    let jobs: Vec<(&str, usize, bool)> = cfg
        .repo
        .root
        .iter()
        .map(|r| (r.as_str(), ROOT_DEPTH, true))
        .chain(cfg.repo.extra.iter().map(|p| (p.as_str(), 0, false)))
        .collect();

    let results: Vec<Result<Vec<FoundRepo>>> = thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(path, depth, is_root)| {
                scope.spawn(move || {
                    let root = expand_home_dir(path, home);
                    log.info(&format!("searching {path} (depth {depth})"));
                    let repos = find_repos(&root, depth, &cfg.repo.ignore, log)?;
                    Ok(repos
                        .into_iter()
                        .map(|path| {
                            let owner = if is_root {
                                owner_of(&root, &path)
                            } else {
                                None
                            };
                            let label = owner
                                .as_deref()
                                .and_then(|o| cfg.repo.label_of(o))
                                .unwrap_or(UNLABELLED)
                                .to_string();
                            FoundRepo {
                                label,
                                owner,
                                root: root.clone(),
                                path,
                            }
                        })
                        .collect())
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("discovery worker panicked")))
            })
            .collect()
    });

    let mut repos = Vec::new();
    for result in results {
        repos.extend(result?);
    }
    Ok(dedup_by_path(repos))
}

/// Drop repositories found more than once, keeping the first.
///
/// The searches are independent, so nothing stops two of them from reaching the
/// same checkout: an `extra` path that also sits under the root, the same path
/// listed twice, or two spellings of one directory. Every one of those used to
/// put the repository in the list twice, which in a picker is two identical
/// rows where selecting either does the same thing.
///
/// The first occurrence wins because the jobs are ordered root-first, and only
/// the root search knows a repository's owner and therefore its label. Keeping
/// the later one would leave a labelled repository showing up unlabelled.
fn dedup_by_path(repos: Vec<FoundRepo>) -> Vec<FoundRepo> {
    let mut seen = HashSet::with_capacity(repos.len());
    repos
        .into_iter()
        .filter(|repo| seen.insert(repo.path.clone()))
        .collect()
}

/// Find directories containing a `.git` entry under `root`, descending at most
/// `max_depth` levels. Directories whose basename is in `ignore` are skipped,
/// and a discovered repository is not descended into.
///
/// The search runs one depth level at a time, with the directories at each
/// level divided between worker threads. The work is almost entirely waiting on
/// `stat` and `readdir` — on a cold cache a root of a hundred repositories
/// spends most of its half-second latency-bound, one directory at a time — so
/// overlapping the levels is what makes it quick. The number of *directories*
/// visited is unchanged; they are simply not queued behind each other.
fn find_repos(
    root: &Path,
    max_depth: usize,
    ignore: &[String],
    log: &Logger,
) -> Result<Vec<PathBuf>> {
    std::fs::metadata(root).with_context(|| format!("root path {}", root.display()))?;

    let mut repos = Vec::new();
    let mut frontier = vec![root.to_path_buf()];

    for depth in 0..=max_depth {
        if frontier.is_empty() {
            break;
        }
        let last = depth == max_depth;
        let mut next = Vec::new();
        for (found, children) in visit_level(&frontier, last, ignore, log) {
            repos.extend(found);
            next.extend(children);
        }
        frontier = next;
    }

    Ok(repos)
}

/// How many directories to look at concurrently. The work is I/O latency, not
/// computation, so this is a concurrency figure rather than a CPU count — but
/// the core count is a sane scale for it, and a machine that reports none gets
/// a modest default rather than no parallelism at all.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// Visit every directory in `level`, returning each one's discovered
/// repositories and the subdirectories to look at next.
///
/// Results come back in `level` order regardless of which thread produced
/// them, so a discovery run is reproducible.
fn visit_level(
    level: &[PathBuf],
    last: bool,
    ignore: &[String],
    log: &Logger,
) -> Vec<(Vec<PathBuf>, Vec<PathBuf>)> {
    let chunks: Vec<&[PathBuf]> = level
        .chunks(level.len().div_ceil(workers()).max(1))
        .collect();
    if chunks.len() < 2 {
        return level
            .iter()
            .map(|dir| visit(dir, last, ignore, log))
            .collect();
    }

    thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|dir| visit(dir, last, ignore, log))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            // Re-raise rather than dropping a chunk: silently returning fewer
            // repositories than exist is the one failure a picker cannot show.
            // `find_all_repos` turns this into "discovery worker panicked".
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    })
}

/// Classify one directory: either it is a repository, or it contributes the
/// subdirectories below it. `last` suppresses descending past the depth limit.
fn visit(dir: &Path, last: bool, ignore: &[String], log: &Logger) -> (Vec<PathBuf>, Vec<PathBuf>) {
    if dir.join(".git").exists() {
        return (vec![dir.to_path_buf()], Vec::new());
    }
    if last {
        return (Vec::new(), Vec::new());
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            log.warn(&format!(
                "skipping unreadable path {}: {err}",
                dir.display()
            ));
            return (Vec::new(), Vec::new());
        }
    };

    let mut children = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                log.warn(&format!(
                    "skipping unreadable entry in {}: {err}",
                    dir.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        if !is_dir(&entry, &path) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if ignore.iter().any(|i| i == name) {
            log.debug(&format!("skipping excluded dir {}", path.display()));
            continue;
        }
        children.push(path);
    }
    (Vec::new(), children)
}

/// Whether `entry` is a directory, **following symbolic links**.
///
/// [`DirEntry::file_type`](std::fs::DirEntry::file_type) reports on the link
/// itself, so a symlinked checkout — or a symlinked owner directory, which
/// takes every repository under it down too — reads as "not a directory" and
/// vanishes from the listing without so much as a warning. Symlinking a
/// repository into the root is an ordinary thing to do, and discovery is
/// depth-capped, so the link is followed and the extra `stat` is paid only for
/// the entries that are actually links.
fn is_dir(entry: &std::fs::DirEntry, path: &Path) -> bool {
    match entry.file_type() {
        Ok(file_type) if file_type.is_symlink() => {
            std::fs::metadata(path).is_ok_and(|meta| meta.is_dir())
        }
        Ok(file_type) => file_type.is_dir(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn quiet() -> Logger {
        Logger::new(false)
    }

    fn mk_repo(root: &Path, rel: &str) -> PathBuf {
        let full = root.join(rel);
        fs::create_dir_all(full.join(".git")).unwrap();
        full
    }

    fn sorted(mut v: Vec<PathBuf>) -> Vec<PathBuf> {
        v.sort();
        v
    }

    #[test]
    fn finds_git_dirs() {
        let root = TempDir::new().unwrap();
        let a = mk_repo(root.path(), "a");
        let b = mk_repo(root.path(), "nested/b");
        fs::create_dir_all(root.path().join("nested/not-a-repo")).unwrap();

        let got = find_repos(root.path(), 5, &[], &quiet()).unwrap();
        assert_eq!(sorted(got), sorted(vec![a, b]));
    }

    #[test]
    fn respects_depth() {
        let root = TempDir::new().unwrap();
        mk_repo(root.path(), "top");
        let deep = mk_repo(root.path(), "a/b/c/deep");

        let got = find_repos(root.path(), 1, &[], &quiet()).unwrap();
        assert!(!got.contains(&deep));
    }

    #[test]
    fn skips_ignored() {
        let root = TempDir::new().unwrap();
        mk_repo(root.path(), "node_modules/hidden");
        let visible = mk_repo(root.path(), "visible");

        let got = find_repos(root.path(), 5, &["node_modules".to_string()], &quiet()).unwrap();
        assert_eq!(got, vec![visible]);
    }

    #[test]
    fn root_repo_returned_at_depth_zero() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join(".git")).unwrap();

        let got = find_repos(root.path(), 0, &[], &quiet()).unwrap();
        assert_eq!(got, vec![root.path().to_path_buf()]);
    }

    /// A checkout symlinked into the root is a repository like any other.
    /// `DirEntry::file_type` reports on the link, not its target, so both of
    /// these used to read as "not a directory" and disappear — the symlinked
    /// owner taking every repository beneath it along with it, and no warning
    /// printed either way.
    #[cfg(unix)]
    #[test]
    fn follows_symlinked_directories() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("root");
        let away = tmp.path().join("away");
        fs::create_dir_all(&root).unwrap();

        // A symlinked repository, directly under the root.
        let repo = mk_repo(&away, "linked-repo");
        std::os::unix::fs::symlink(&repo, root.join("linked-repo")).unwrap();
        // A symlinked owner directory, with a repository inside it.
        let owner = away.join("owner");
        let nested = mk_repo(&owner, "nested-repo");
        std::os::unix::fs::symlink(&owner, root.join("owner")).unwrap();

        let got = find_repos(&root, 2, &[], &quiet()).unwrap();

        assert!(
            got.contains(&root.join("linked-repo")),
            "symlinked repository was skipped: {got:?}"
        );
        assert!(
            got.contains(&root.join("owner").join("nested-repo")),
            "repository under a symlinked owner was skipped: {got:?}"
        );
        // The targets exist where the links say they do.
        assert!(repo.join(".git").exists() && nested.join(".git").exists());
    }

    /// Every level is walked concurrently, so a root wide enough to be split
    /// across threads must still find everything exactly once.
    #[test]
    fn parallel_levels_find_every_repo() {
        let root = TempDir::new().unwrap();
        let mut expected: Vec<PathBuf> = (0..64)
            .map(|i| mk_repo(root.path(), &format!("owner{}/repo{i}", i % 7)))
            .collect();
        expected.sort();

        let got = find_repos(root.path(), 2, &[], &quiet()).unwrap();
        assert_eq!(sorted(got), expected);
    }

    fn found(path: &str, label: &str, owner: Option<&str>) -> FoundRepo {
        FoundRepo {
            label: label.to_string(),
            owner: owner.map(str::to_string),
            root: PathBuf::from("/root"),
            path: PathBuf::from(path),
        }
    }

    /// The root search and each `extra` path run independently, so nothing
    /// stops two of them reaching the same checkout — an `extra` entry that is
    /// also under the root is the ordinary way it happens. Two identical picker
    /// rows, where selecting either does the same thing, is not a list.
    #[test]
    fn a_repository_found_twice_is_listed_once() {
        let got = dedup_by_path(vec![
            found("/root/me/foo", "personal", Some("me")),
            found("/root/me/bar", "personal", Some("me")),
            found("/root/me/foo", "-", None),
        ]);
        assert_eq!(
            got.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            vec![PathBuf::from("/root/me/foo"), PathBuf::from("/root/me/bar")]
        );
    }

    /// The first occurrence is the one kept, and it has to be: the jobs run
    /// root-first, and only the root search knows a repository's owner and
    /// therefore its label. Keep the later copy and a labelled repository turns
    /// up unlabelled.
    #[test]
    fn deduping_keeps_the_labelled_copy() {
        let got = dedup_by_path(vec![
            found("/root/me/foo", "personal", Some("me")),
            found("/root/me/foo", "-", None),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "personal");
        assert_eq!(got[0].owner.as_deref(), Some("me"));
    }

    #[test]
    fn missing_root_errors() {
        let root = TempDir::new().unwrap();
        let missing = root.path().join("nope");
        assert!(find_repos(&missing, 1, &[], &quiet()).is_err());
    }
}
