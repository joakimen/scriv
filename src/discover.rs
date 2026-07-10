//! Repository discovery. Walking touches the filesystem, but the traversal
//! rules — depth limit, ignore list, and `.git` detection — are small and are
//! covered by tests that run against temporary directories.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::thread;

use crate::config::Config;
use crate::logger::Logger;
use crate::path::expand_home_dir;

/// Discover repositories across every configured path, expanding `~` against
/// `home`. Each path is searched on its own thread. A missing root path is a
/// hard error; unreadable subdirectories are logged and skipped.
pub fn find_all_repos(cfg: &Config, home: &Path, log: &Logger) -> Result<Vec<PathBuf>> {
    if cfg.paths.is_empty() {
        anyhow::bail!("no paths found in config file");
    }

    let results: Vec<Result<Vec<PathBuf>>> = thread::scope(|scope| {
        let handles: Vec<_> = cfg
            .paths
            .iter()
            .map(|entry| {
                scope.spawn(|| {
                    let root = expand_home_dir(&entry.path, home);
                    log.info(&format!(
                        "path entry {} (depth {})",
                        entry.path, entry.depth
                    ));
                    find_repos(&root, entry.depth, &cfg.ignore, log)
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
pub fn find_repos(
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
