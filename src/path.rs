//! Path helpers with no I/O. Pure functions over path values, so they are
//! trivially testable in isolation.

use std::path::{Path, PathBuf};

/// Expand a leading `~/` in `dir` to `home`. A bare `~` prefix that is not
/// followed by a separator, and any path without the prefix, is returned
/// unchanged.
pub fn expand_home_dir(dir: &str, home: &Path) -> PathBuf {
    match dir.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(dir),
    }
}

/// Render `repo` for display. Absolute paths are returned verbatim when
/// `absolute` is set; otherwise a `home` prefix is collapsed to `~`.
pub fn format_repo_path(repo: &str, home: &str, absolute: bool) -> String {
    if absolute {
        return repo.to_string();
    }
    match repo.strip_prefix(home) {
        Some(rest) => format!("~{rest}"),
        None => repo.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tilde_prefix() {
        let home = Path::new("/home/user");
        assert_eq!(expand_home_dir("~/foo/bar", home), home.join("foo/bar"));
    }

    #[test]
    fn expands_bare_tilde_slash_to_home() {
        let home = Path::new("/home/user");
        assert_eq!(expand_home_dir("~/", home), home.to_path_buf());
    }

    #[test]
    fn leaves_non_tilde_paths_unchanged() {
        let home = Path::new("/home/user");
        for input in ["/etc/hosts", "foo/bar", "/opt/~/foo", ""] {
            assert_eq!(expand_home_dir(input, home), PathBuf::from(input));
        }
    }

    #[test]
    fn formats_home_relative_by_default() {
        assert_eq!(
            format_repo_path("/home/user/dev/repo", "/home/user", false),
            "~/dev/repo"
        );
    }

    #[test]
    fn formats_absolute_when_requested() {
        assert_eq!(
            format_repo_path("/home/user/dev/repo", "/home/user", true),
            "/home/user/dev/repo"
        );
    }

    #[test]
    fn leaves_paths_outside_home_unchanged() {
        assert_eq!(
            format_repo_path("/opt/tools/repo", "/home/user", false),
            "/opt/tools/repo"
        );
    }
}
