//! Repository discovery. Walking touches the filesystem, but the traversal
//! rules — depth limit, ignore list, and `.git` detection — are small and are
//! covered by tests that run against temporary directories.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::thread;

use crate::config::{Config, ROOT_DEPTH, UNCATEGORIZED};
use crate::logger::Logger;
use crate::path::expand_home_dir;

/// A discovered repository, together with where it was found and who owns it.
pub struct FoundRepo {
    /// The category its owner falls in, or [`UNCATEGORIZED`].
    pub category: String,
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
/// expanding `~` against `home` and tagging each with its owner and category.
///
/// Each search runs on its own thread. A missing path is a hard error — a typo
/// in the root should say so rather than quietly finding nothing; unreadable
/// subdirectories are logged and skipped.
pub fn find_all_repos(cfg: &Config, home: &Path, log: &Logger) -> Result<Vec<FoundRepo>> {
    if cfg.root.is_none() && cfg.extra.is_empty() {
        anyhow::bail!("no `root` set in config file");
    }

    // The root is searched at the owner/repo depth; each `extra` path is a
    // repository in its own right, so it is searched at depth 0.
    let jobs: Vec<(&str, usize, bool)> = cfg
        .root
        .iter()
        .map(|r| (r.as_str(), ROOT_DEPTH, true))
        .chain(cfg.extra.iter().map(|p| (p.as_str(), 0, false)))
        .collect();

    let results: Vec<Result<Vec<FoundRepo>>> = thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(path, depth, is_root)| {
                scope.spawn(move || {
                    let root = expand_home_dir(path, home);
                    log.info(&format!("searching {path} (depth {depth})"));
                    let repos = find_repos(&root, depth, &cfg.ignore, log)?;
                    Ok(repos
                        .into_iter()
                        .map(|path| {
                            let owner = if is_root {
                                owner_of(&root, &path)
                            } else {
                                None
                            };
                            let category = owner
                                .as_deref()
                                .and_then(|o| cfg.category_of(o))
                                .unwrap_or(UNCATEGORIZED)
                                .to_string();
                            FoundRepo {
                                category,
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
    Ok(repos)
}

/// Find directories containing a `.git` entry under `root`, descending at most
/// `max_depth` levels. Directories whose basename is in `ignore` are skipped,
/// and a discovered repository is not descended into.
fn find_repos(
    root: &Path,
    max_depth: usize,
    ignore: &[String],
    log: &Logger,
) -> Result<Vec<PathBuf>> {
    std::fs::metadata(root).with_context(|| format!("root path {}", root.display()))?;
    let mut repos = Vec::new();
    walk(root, 0, max_depth, ignore, &mut repos, log);
    Ok(repos)
}

fn walk(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    ignore: &[String],
    repos: &mut Vec<PathBuf>,
    log: &Logger,
) {
    if dir.join(".git").exists() {
        repos.push(dir.to_path_buf());
        return;
    }
    if depth >= max_depth {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            log.warn(&format!(
                "skipping unreadable path {}: {err}",
                dir.display()
            ));
            return;
        }
    };

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
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if ignore.iter().any(|i| i == name) {
            log.debug(&format!("skipping excluded dir {}", path.display()));
            continue;
        }
        walk(&path, depth + 1, max_depth, ignore, repos, log);
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

    #[test]
    fn missing_root_errors() {
        let root = TempDir::new().unwrap();
        let missing = root.path().join("nope");
        assert!(find_repos(&missing, 1, &[], &quiet()).is_err());
    }
}
