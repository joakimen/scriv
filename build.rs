//! Decide what `scriv --version` reports: the crate version when this commit
//! is a release — on a tag, nothing modified — and a development build naming
//! its commit otherwise. Git is not required.

use std::process::{Command, Stdio};

/// The paths whose contents decide what the binary *is*: the re-run triggers
/// below and the dirty check must ask about the same set, or `.dirty` comes
/// and goes.
const SOURCES: &[&str] = &["src", "build.rs", "Cargo.toml", "Cargo.lock"];

fn main() {
    // Naming any path switches off cargo's default whole-package watch, so the
    // sources have to be listed alongside the git paths.
    for path in SOURCES {
        println!("cargo:rerun-if-changed={path}");
    }
    for path in [".git/HEAD", ".git/index", ".git/refs"] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=SCRIV_RELEASE");

    println!("cargo:rustc-env=SCRIV_VERSION={}", version());
}

fn version() -> String {
    let crate_version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");

    // Set by the release workflow. The tag test below cannot stand in for it
    // there: dist builds the binaries first and creates the tag from them
    // afterwards, out of a checkout holding no tags at all.
    if std::env::var_os("SCRIV_RELEASE").is_some() {
        return crate_version;
    }

    // No repository to ask: a packaged build, which is the release it claims.
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

    let mut version = format!("{crate_version}-dev.{sha}");
    if dirty {
        version.push_str(".dirty");
    }
    version
}

/// Run `git` with `args`, returning its trimmed stdout, or `None` when git is
/// missing, this is not a repository, or the command failed.
///
/// `--no-optional-locks` keeps a build from taking the repository's index lock
/// away from whatever the user is running in it.
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
