//! Path helpers with no I/O. Pure functions over path values, so they are
//! trivially testable in isolation.
//!
//! Two representations coexist here. Repository discovery works in
//! [`PathBuf`] because it walks the filesystem; the known-files list is a
//! line-oriented text file and stays in `String` end to end. The string
//! variants exist for the latter.

use std::path::{Path, PathBuf};

/// Expand a leading `~` in `dir` to `home`: a bare `~` becomes `home`, and
/// `~/rest` becomes `home/rest`. Any path without the prefix is returned
/// unchanged.
pub fn expand_home_dir(dir: &str, home: &Path) -> PathBuf {
    if dir == "~" {
        return home.to_path_buf();
    }
    match dir.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(dir),
    }
}

/// Expand a leading `~` to `home`, operating on strings.
///
/// A path that does not begin with `~` is returned unchanged. `~` alone expands
/// to `home`; `~/rest` expands to `home` followed by `/rest`.
pub fn expand_tilde(path: &str, home: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    format!("{}{}", home, &path[1..])
}

/// Render a repository path relative to the search `root` it was found under,
/// so a shared base is not repeated on every row.
///
/// When the repository *is* the root (a root that is itself a repo, at depth 0),
/// the relative part is empty; the basename is used instead so the row is not
/// blank.
pub fn relative_label(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
        _ => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
    }
}

/// Render `path` for display. Paths are returned verbatim when `absolute` is
/// set; otherwise a `home` prefix is collapsed to `~`.
///
/// The prefix is only collapsed at a path boundary, so `/home/user` and
/// `/home/user/x` collapse but `/home/username` does not.
pub fn display_path(path: &str, home: &str, absolute: bool) -> String {
    if absolute {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix(home)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// Normalise a user-supplied path into the canonical form stored in the
/// known-files list.
///
/// Precedence of the input shape:
/// - Absolute paths under `home` are shortened to `~/…`.
/// - Absolute paths outside `home` are kept as-is.
/// - Tilde paths are kept as-is.
/// - Everything else is treated as relative to `pwd`.
///
/// The result is always lexically cleaned (`.`/`..`/redundant separators removed).
pub fn sanitize_file_path(input: &str, home: &str, pwd: &str) -> String {
    let path = if input.starts_with('/') {
        display_path(input, home, false)
    } else if input.starts_with('~') {
        input.to_string()
    } else {
        format!("{}/{}", display_path(pwd, home, false), input)
    };

    clean(&path)
}

/// Lexically clean a path, mirroring Go's `filepath.Clean`.
///
/// Collapses redundant separators, drops `.` elements, and resolves inner `..`
/// elements without touching the filesystem. A leading `~` is treated as an
/// ordinary path element, not a root.
fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }

    let bytes = path.as_bytes();
    let rooted = bytes[0] == b'/';
    let n = bytes.len();
    let mut buf = vec![0u8; n];
    let mut w = 0usize;
    let mut r = 0usize;
    let mut dotdot = 0usize;

    if rooted {
        buf[w] = b'/';
        w += 1;
        r = 1;
        dotdot = 1;
    }

    while r < n {
        if bytes[r] == b'/' || (bytes[r] == b'.' && (r + 1 == n || bytes[r + 1] == b'/')) {
            r += 1;
        } else if bytes[r] == b'.'
            && r + 1 < n
            && bytes[r + 1] == b'.'
            && (r + 2 == n || bytes[r + 2] == b'/')
        {
            r += 2;
            if w > dotdot {
                w -= 1;
                while w > dotdot && buf[w] != b'/' {
                    w -= 1;
                }
            } else if !rooted {
                if w > 0 {
                    buf[w] = b'/';
                    w += 1;
                }
                buf[w] = b'.';
                w += 1;
                buf[w] = b'.';
                w += 1;
                dotdot = w;
            }
        } else {
            if (rooted && w != 1) || (!rooted && w != 0) {
                buf[w] = b'/';
                w += 1;
            }
            while r < n && bytes[r] != b'/' {
                buf[w] = bytes[r];
                w += 1;
                r += 1;
            }
        }
    }

    if w == 0 {
        return ".".to_string();
    }
    String::from_utf8_lossy(&buf[..w]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/kevin";
    const PWD: &str = "/Users/kevin/fake/dir";

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
    fn expands_bare_tilde_to_home() {
        let home = Path::new("/home/user");
        assert_eq!(expand_home_dir("~", home), home.to_path_buf());
    }

    #[test]
    fn leaves_non_tilde_paths_unchanged() {
        let home = Path::new("/home/user");
        for input in ["/etc/hosts", "foo/bar", "/opt/~/foo", ""] {
            assert_eq!(expand_home_dir(input, home), PathBuf::from(input));
        }
    }

    #[test]
    fn expand_tilde_expands_to_absolute() {
        assert_eq!(
            expand_tilde("~/mydir/file.txt", HOME),
            "/Users/kevin/mydir/file.txt"
        );
    }

    #[test]
    fn expand_tilde_leaves_absolute_alone() {
        assert_eq!(expand_tilde("/etc/passwd", HOME), "/etc/passwd");
    }

    #[test]
    fn expand_tilde_bare_expands_to_home() {
        assert_eq!(expand_tilde("~", HOME), HOME);
    }

    #[test]
    fn expand_tilde_only_rewrites_a_leading_tilde() {
        assert_eq!(expand_tilde("/etc/~/passwd", HOME), "/etc/~/passwd");
    }

    #[test]
    fn formats_home_relative_by_default() {
        assert_eq!(
            display_path("/home/user/dev/repo", "/home/user", false),
            "~/dev/repo"
        );
    }

    #[test]
    fn formats_absolute_when_requested() {
        assert_eq!(
            display_path("/home/user/dev/repo", "/home/user", true),
            "/home/user/dev/repo"
        );
    }

    #[test]
    fn leaves_paths_outside_home_unchanged() {
        assert_eq!(
            display_path("/opt/tools/repo", "/home/user", false),
            "/opt/tools/repo"
        );
    }

    #[test]
    fn relative_label_strips_the_root() {
        assert_eq!(
            relative_label(
                Path::new("/Users/kevin/dev/github.com/kkc/scriv"),
                Path::new("/Users/kevin/dev/github.com")
            ),
            "kkc/scriv"
        );
    }

    #[test]
    fn relative_label_falls_back_to_basename_when_repo_is_the_root() {
        assert_eq!(
            relative_label(Path::new("/Users/kevin/bin"), Path::new("/Users/kevin/bin")),
            "bin"
        );
    }

    #[test]
    fn collapses_only_at_a_path_boundary() {
        // Bare home collapses to `~`; a sibling that merely shares the prefix
        // as a substring is left alone.
        assert_eq!(display_path("/home/user", "/home/user", false), "~");
        assert_eq!(
            display_path("/home/username/x", "/home/user", false),
            "/home/username/x"
        );
    }

    #[test]
    fn sanitize_leaves_abspath_outside_home_alone() {
        assert_eq!(sanitize_file_path("/etc/passwd", HOME, PWD), "/etc/passwd");
    }

    #[test]
    fn sanitize_cleans_abspath() {
        assert_eq!(
            sanitize_file_path("/relative/path/../path/file.txt", HOME, PWD),
            "/relative/path/file.txt"
        );
    }

    #[test]
    fn sanitize_shrinks_home_path_with_tilde() {
        assert_eq!(
            sanitize_file_path("/Users/kevin/file.txt", HOME, PWD),
            "~/file.txt"
        );
    }

    #[test]
    fn sanitize_leaves_tilde_alone() {
        assert_eq!(
            sanitize_file_path("~/mydir/file.txt", HOME, PWD),
            "~/mydir/file.txt"
        );
    }

    #[test]
    fn sanitize_joins_relative_to_pwd() {
        assert_eq!(
            sanitize_file_path("file.txt", HOME, PWD),
            "~/fake/dir/file.txt"
        );
    }

    #[test]
    fn sanitize_relative_with_parent_segments() {
        assert_eq!(
            sanitize_file_path("../file.txt", HOME, PWD),
            "~/fake/file.txt"
        );
    }

    #[test]
    fn clean_collapses_redundant_separators() {
        assert_eq!(clean("/a//b/./c"), "/a/b/c");
    }

    #[test]
    fn clean_empty_is_dot() {
        assert_eq!(clean(""), ".");
    }
}
