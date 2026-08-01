//! Decide what `scriv --version` reports.
//!
//! A binary built from a checkout is not the binary the release of the same
//! name contains — it may be a commit ahead, or carry edits that were never
//! pushed — and both used to say `0.2.1`. A bug report, or a `mise` install
//! sitting beside a `make install`, had no way to tell them apart.
//!
//! So the version is the crate version only when this commit *is* a release:
//! sitting exactly on a tag, with nothing modified. Anything else is a
//! development build and says so, naming the commit it came from.
//!
//! Git is not required. A build from a packaged tarball has no repository to
//! ask — `cargo package` excludes `.git` — and that is not a development build
//! either, so it falls back to the crate version rather than failing.

use std::process::{Command, Stdio};

/// The paths whose contents decide what the binary *is*.
///
/// This list does two jobs, and it is the same list for both on purpose: cargo
/// re-runs this script when one of them changes, and the dirty check asks git
/// about exactly these. Let the two drift apart and the answer becomes a coin
/// toss — a modified file that does not trigger a re-run leaves a stale
/// `.dirty` behind, and one that triggers a re-run but is never asked about
/// produces a `.dirty` that comes and goes.
///
/// It is deliberately not the whole worktree. An edited README does not make a
/// different binary, and a version that changed because of one would be noise.
const SOURCES: &[&str] = &["src", "build.rs", "Cargo.toml", "Cargo.lock"];

fn main() {
    // Without these the version freezes at whatever the first build saw: cargo
    // does not watch `.git` on its own, so a commit changes nothing it can see.
    // Naming any path at all also switches off the default whole-package watch,
    // which is why the source paths are listed here too.
    for path in SOURCES {
        println!("cargo:rerun-if-changed={path}");
    }
    // A commit moves HEAD or the ref it points at, a checkout rewrites HEAD,
    // and staging touches the index. Watching `refs` as a directory rather than
    // resolving the current branch keeps this correct across a branch switch.
    for path in [".git/HEAD", ".git/index", ".git/refs"] {
        println!("cargo:rerun-if-changed={path}");
    }

    println!("cargo:rustc-env=SCRIV_VERSION={}", version());
}

fn version() -> String {
    let crate_version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");

    // No repository to ask: a packaged or vendored build, which is the release
    // it claims to be.
    let Some(sha) = git(&["rev-parse", "--short=7", "HEAD"]) else {
        return crate_version;
    };

    let mut status = vec!["status", "--porcelain", "--untracked-files=no", "--"];
    status.extend(SOURCES);
    let dirty = git(&status).is_some_and(|out| !out.is_empty());
    let tagged = git(&["describe", "--tags", "--exact-match", "HEAD"]).is_some();

    if tagged && !dirty {
        return crate_version;
    }

    // Dot-separated rather than `+sha`, so the whole thing stays one token for
    // anything splitting `scriv --version` on whitespace.
    let mut version = format!("{crate_version}-dev.{sha}");
    if dirty {
        version.push_str(".dirty");
    }
    version
}

/// Run `git` with `args`, returning its trimmed stdout, or `None` when git is
/// missing, this is not a repository, or the command failed.
///
/// `--no-optional-locks` for the same reason the preview panes use it: a plain
/// `git status` rewrites the index, and a build should not take the
/// repository's index lock away from whatever the user is running in it.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
