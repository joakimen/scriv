//! Git branch enumeration and checkout.
//!
//! Talking to git is a subprocess concern, but the interesting part — deciding
//! which refs are local, which are remote-only, and what a checkout of a given
//! name should actually do — is pure and lives here as functions over plain
//! data ([`parse_ref_lines`], [`classify`], [`resolve`]), covered by tests.
//!
//! Branches are enumerated with a single `for-each-ref` over `refs/heads` and
//! `refs/remotes`, so local and remote branches arrive in one pass, already
//! ordered by commit recency; [`by_relevance`] then groups that order into
//! current branch, local, remote-only without asking git anything more.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use crate::Reported;

/// Field separator in the `for-each-ref` format. ASCII unit separator, so
/// commit subjects containing tabs or pipes cannot split a row.
const SEP: char = '\x1f';

/// Where a branch exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    /// Only in this clone — never pushed, or its upstream is gone.
    Local,
    /// Both here and on a remote.
    Tracked,
    /// Only on a remote; checking it out creates the local branch.
    Remote,
}

impl BranchKind {
    /// ANSI 256-colour index used for this kind, in both the selector and `ls`.
    /// Standard hues so they follow the terminal theme.
    pub fn color(self) -> u8 {
        match self {
            BranchKind::Local => 3,   // yellow — here only
            BranchKind::Tracked => 2, // green  — in sync with a remote
            BranchKind::Remote => 6,  // cyan   — not here yet
        }
    }

    /// Short tag shown in `branch ls --status`, for when colour is unavailable.
    pub fn tag(self) -> &'static str {
        match self {
            BranchKind::Local => "local",
            BranchKind::Tracked => "both",
            BranchKind::Remote => "remote",
        }
    }
}

/// Which branches a listing includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Local and remote-only branches. The default.
    #[default]
    All,
    /// Branches that exist in this clone ([`BranchKind::Local`] and
    /// [`BranchKind::Tracked`]).
    Local,
    /// Branches that exist on a remote ([`BranchKind::Tracked`] and
    /// [`BranchKind::Remote`]).
    Remote,
}

impl Filter {
    /// Resolve the mutually exclusive `--local` / `--remote` flags.
    pub fn from_flags(local: bool, remote: bool) -> Self {
        match (local, remote) {
            (true, false) => Filter::Local,
            (false, true) => Filter::Remote,
            _ => Filter::All,
        }
    }

    fn accepts(self, kind: BranchKind) -> bool {
        match self {
            Filter::All => true,
            Filter::Local => kind != BranchKind::Remote,
            Filter::Remote => kind != BranchKind::Local,
        }
    }
}

/// A branch as offered to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Displayed and returned name: `main` for a local branch, `origin/main`
    /// for a remote-only one, so the remote is never ambiguous.
    pub name: String,
    pub kind: BranchKind,
    /// Whether this is the currently checked-out branch.
    pub head: bool,
    /// Relative commit date, e.g. `2 days ago`.
    pub date: String,
    /// Commit subject line.
    pub subject: String,
}

/// One row of `for-each-ref` output, before classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLine {
    pub refname: String,
    pub head: bool,
    /// Configured upstream in short form (`origin/main`), empty when unset.
    pub upstream: String,
    pub date: String,
    pub subject: String,
}

/// What checking out a given name should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checkout {
    /// Switch to an existing local branch.
    Switch(String),
    /// Create a local branch tracking `remote_ref` (`origin/main`).
    Track { remote_ref: String },
}

/// Parse `for-each-ref` output written with the [`SEP`]-joined format.
///
/// Rows with too few fields are skipped rather than failing the listing — a
/// malformed ref should not make the whole command unusable.
pub fn parse_ref_lines(output: &str) -> Vec<RefLine> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut fields = line.splitn(5, SEP);
            let refname = fields.next()?;
            let head = fields.next()?;
            let upstream = fields.next()?;
            let date = fields.next()?;
            // The subject is last and unsplit, so separators inside it survive.
            let subject = fields.next().unwrap_or_default();
            if refname.is_empty() {
                return None;
            }
            Some(RefLine {
                refname: refname.to_string(),
                head: head.trim() == "*",
                upstream: upstream.to_string(),
                date: date.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect()
}

/// Split a remote ref short name (`origin/feat/x`) into remote and branch.
/// Branch names may contain `/`; remote names may not, so the first component
/// is the remote.
fn split_remote(short: &str) -> Option<(&str, &str)> {
    short
        .split_once('/')
        .filter(|(r, b)| !r.is_empty() && !b.is_empty())
}

/// Fold ref rows into the branch list shown to the user.
///
/// A local branch whose name also exists on a remote is [`BranchKind::Tracked`]
/// rather than two rows; a remote ref with no local counterpart becomes a
/// [`BranchKind::Remote`] row. `origin/HEAD` and friends are dropped — they are
/// pointers to a default branch, not branches of their own. Input order (git's
/// `--sort`) is preserved.
pub fn classify(lines: &[RefLine]) -> Vec<Branch> {
    let locals: HashSet<&str> = lines
        .iter()
        .filter_map(|line| line.refname.strip_prefix("refs/heads/"))
        .collect();

    // Remote refs already spoken for as some local branch's upstream, so a
    // branch tracked under a different name is not also listed on its own.
    let upstreams: HashSet<&str> = lines
        .iter()
        .filter(|line| line.refname.starts_with("refs/heads/"))
        .map(|line| line.upstream.as_str())
        .filter(|upstream| !upstream.is_empty())
        .collect();

    // Branch parts of every remote ref, across all remotes: `origin/main` and
    // `fork/main` both contribute `main`.
    let on_remote: HashSet<&str> = lines
        .iter()
        .filter_map(|line| line.refname.strip_prefix("refs/remotes/"))
        .filter(|short| !short.ends_with("/HEAD"))
        .filter_map(|short| split_remote(short).map(|(_, branch)| branch))
        .collect();

    let mut branches = Vec::new();
    for line in lines {
        if let Some(name) = line.refname.strip_prefix("refs/heads/") {
            // Prefer the configured upstream when it points somewhere else, so
            // `git branch -u origin/other` still reads as tracked.
            let counterpart = match split_remote(&line.upstream) {
                Some((_, branch)) => branch,
                None => name,
            };
            let kind = if on_remote.contains(counterpart) {
                BranchKind::Tracked
            } else {
                BranchKind::Local
            };
            branches.push(Branch {
                name: name.to_string(),
                kind,
                head: line.head,
                date: line.date.clone(),
                subject: line.subject.clone(),
            });
        } else if let Some(short) = line.refname.strip_prefix("refs/remotes/") {
            if short.ends_with("/HEAD") {
                continue;
            }
            let Some((_, branch)) = split_remote(short) else {
                continue;
            };
            // Already represented by its local counterpart.
            if locals.contains(branch) || upstreams.contains(short) {
                continue;
            }
            branches.push(Branch {
                name: short.to_string(),
                kind: BranchKind::Remote,
                head: false,
                date: line.date.clone(),
                subject: line.subject.clone(),
            });
        }
    }
    branches
}

/// Which block of the listing a branch belongs in: the current branch, then
/// what is already in this clone, then what is only on a remote.
fn tier(branch: &Branch) -> u8 {
    match (branch.head, branch.kind) {
        (true, _) => 0,
        (_, BranchKind::Remote) => 2,
        _ => 1,
    }
}

/// Order branches by how likely one is to be the one wanted: the current
/// branch, then local branches, then remote-only ones, each block most
/// recently committed first.
///
/// The recency half is free — [`branches`] asks `for-each-ref` for
/// `--sort=-committerdate`, so the input already arrives newest first and this
/// is a stable sort that only regroups it. Nothing here reads a ref or parses a
/// date, which is what keeps a repository with a thousand stale remote branches
/// as cheap to list as an empty one.
///
/// Grouping before recency is deliberate: a remote branch someone else pushed
/// an hour ago is newer than anything local, but it is not what the person who
/// typed `branch checkout` in their own working tree is usually reaching for.
pub fn by_relevance(mut branches: Vec<Branch>) -> Vec<Branch> {
    // Stable, so committer-date order survives inside each block.
    branches.sort_by_key(tier);
    branches
}

/// Apply a [`Filter`] to a branch list.
pub fn filtered(branches: Vec<Branch>, filter: Filter) -> Vec<Branch> {
    branches
        .into_iter()
        .filter(|branch| filter.accepts(branch.kind))
        .collect()
}

/// Decide what checking out `input` means, given the known branches.
///
/// Rules, in order:
/// - A name matching a listed branch uses that branch's kind: local branches
///   are switched to, remote-only ones get a tracking branch created.
/// - `origin/foo` where a local `foo` already exists switches to the local
///   branch, rather than detaching HEAD at the remote ref.
/// - Anything else is handed to git verbatim, so its own resolution (and its
///   error message) still applies.
pub fn resolve(branches: &[Branch], input: &str) -> Checkout {
    if let Some(branch) = branches.iter().find(|b| b.name == input) {
        return match branch.kind {
            BranchKind::Remote => Checkout::Track {
                remote_ref: branch.name.clone(),
            },
            _ => Checkout::Switch(branch.name.clone()),
        };
    }

    // `origin/foo` when `foo` is checked out locally: the user means `foo`.
    if let Some((_, tail)) = split_remote(input)
        && branches
            .iter()
            .any(|b| b.kind != BranchKind::Remote && b.name == tail)
    {
        return Checkout::Switch(tail.to_string());
    }

    Checkout::Switch(input.to_string())
}

/// Run git with `args`, capturing stdout.
fn capture(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => anyhow!("`git` was not found on PATH"),
            _ => anyhow!(e).context("running git"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        bail!(if stderr.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            stderr.to_string()
        });
    }
    // Reuse the buffer rather than copying it: `for-each-ref` over a repository
    // with thousands of refs is not small, and it is always valid UTF-8. The
    // copy is paid only on the malformed output that would have been lossy
    // anyway.
    Ok(String::from_utf8(output.stdout)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Run git with `args`, letting it write straight to the terminal so its
/// progress and "Switched to branch …" messages reach the user unchanged.
///
/// A failure is [`Reported`]: git has already said why on stderr, in wording
/// the user knows, so scriv exits with its status and stays quiet.
fn passthrough(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .status()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => anyhow!("`git` was not found on PATH"),
            _ => anyhow!(e).context("running git"),
        })?;
    if !status.success() {
        return Err(Reported(status.code().unwrap_or(1)).into());
    }
    Ok(())
}

/// Fail early, and with a better message than git's, when the working
/// directory is not inside a repository.
pub fn ensure_repo() -> Result<()> {
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => anyhow!("`git` was not found on PATH"),
            _ => anyhow!(e).context("running git"),
        })?;
    if !inside.success() {
        bail!("not inside a git repository");
    }
    Ok(())
}

/// The root of the repository the working directory is in, if it is in one.
///
/// Absence is an answer here rather than an error: a command that adapts to
/// where you are standing needs to know whether you are standing anywhere, and
/// "not in a repository" is an ordinary case with its own behaviour.
pub fn repo_root() -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

/// Every branch in the repository, ordered by [`by_relevance`]: the current
/// branch, then local branches, then remote-only ones, newest first within
/// each.
pub fn branches() -> Result<Vec<Branch>> {
    let format = format!(
        "--format=%(refname){SEP}%(HEAD){SEP}%(upstream:short){SEP}%(committerdate:relative){SEP}%(subject)"
    );
    let out = capture(&[
        "for-each-ref",
        "--sort=-committerdate",
        &format,
        "refs/heads",
        "refs/remotes",
    ])
    .context("listing branches")?;
    Ok(by_relevance(classify(&parse_ref_lines(&out))))
}

/// Refresh remote-tracking refs, dropping ones deleted upstream.
///
/// Captured rather than passed through, which is the one place scriv silences
/// git on purpose. `git fetch --all` narrates itself — `Fetching origin`,
/// per-remote progress — and neither half of that is wanted here. The line goes
/// to stdout, where `scriv branch sel` is writing a branch name for a shell to
/// read; the progress goes to stderr, where it would scribble over the spinner
/// the caller draws and leave the terminal to be redrawn underneath the selector.
///
/// A failure still speaks: [`capture`] returns git's stderr as the error, so
/// "could not read from remote repository" arrives as scriv's error message
/// rather than as noise nobody asked for.
pub fn fetch() -> Result<()> {
    capture(&["fetch", "--all", "--prune"])
        .context("fetching from remotes")
        .map(|_| ())
}

/// Perform a resolved checkout.
///
/// `switch --track origin/foo` creates local `foo` with its upstream already
/// set, which is exactly the two-step the user would otherwise do by hand.
pub fn checkout(action: &Checkout) -> Result<()> {
    match action {
        Checkout::Switch(name) => passthrough(&["switch", "--", name]),
        Checkout::Track { remote_ref } => passthrough(&["switch", "--track", remote_ref]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(refname: &str, head: bool, upstream: &str) -> RefLine {
        RefLine {
            refname: refname.to_string(),
            head,
            upstream: upstream.to_string(),
            date: "2 days ago".to_string(),
            subject: "some commit".to_string(),
        }
    }

    fn render(rows: &[(&str, &str, &str, &str, &str)]) -> String {
        rows.iter()
            .map(|(r, h, u, d, s)| format!("{r}{SEP}{h}{SEP}{u}{SEP}{d}{SEP}{s}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_ref_rows() {
        let out = render(&[
            ("refs/heads/main", "*", "origin/main", "2 hours ago", "init"),
            ("refs/remotes/origin/main", " ", "", "2 hours ago", "init"),
        ]);
        let got = parse_ref_lines(&out);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].refname, "refs/heads/main");
        assert!(got[0].head);
        assert_eq!(got[0].upstream, "origin/main");
        assert_eq!(got[0].date, "2 hours ago");
        assert_eq!(got[1].refname, "refs/remotes/origin/main");
        assert!(!got[1].head);
    }

    /// The subject is the final field, so separators inside it must not split
    /// the row or truncate the message.
    #[test]
    fn subject_keeps_trailing_separators() {
        let out = format!("refs/heads/main{SEP} {SEP}{SEP}now{SEP}fix: a{SEP}b");
        let got = parse_ref_lines(&out);
        assert_eq!(got[0].subject, format!("fix: a{SEP}b"));
    }

    #[test]
    fn skips_malformed_rows() {
        let out = format!("refs/heads/main{SEP}*\n\nrefs/heads/ok{SEP} {SEP}{SEP}now{SEP}s");
        let got = parse_ref_lines(&out);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].refname, "refs/heads/ok");
    }

    #[test]
    fn local_with_remote_counterpart_is_tracked() {
        let got = classify(&[
            line("refs/heads/main", true, "origin/main"),
            line("refs/remotes/origin/main", false, ""),
        ]);
        assert_eq!(got.len(), 1, "the remote ref must not add a second row");
        assert_eq!(got[0].kind, BranchKind::Tracked);
        assert!(got[0].head);
    }

    #[test]
    fn local_without_remote_is_local_only() {
        let got = classify(&[line("refs/heads/scratch", false, "")]);
        assert_eq!(got[0].kind, BranchKind::Local);
    }

    /// An upstream that has been deleted from the remote leaves the branch
    /// local-only, not "both".
    #[test]
    fn stale_upstream_is_local_only() {
        let got = classify(&[line("refs/heads/gone", false, "origin/gone")]);
        assert_eq!(got[0].kind, BranchKind::Local);
    }

    #[test]
    fn upstream_under_another_name_still_counts() {
        let got = classify(&[
            line("refs/heads/local-name", false, "origin/remote-name"),
            line("refs/remotes/origin/remote-name", false, ""),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, BranchKind::Tracked);
    }

    /// The remote prefix is kept, so a slash-heavy branch name stays
    /// unambiguous and `switch --track` gets a ref it can resolve.
    #[test]
    fn remote_only_keeps_its_remote_prefix() {
        let got = classify(&[line("refs/remotes/origin/feat/x", false, "")]);
        assert_eq!(got[0].kind, BranchKind::Remote);
        assert_eq!(got[0].name, "origin/feat/x");
        assert_eq!(
            resolve(&got, "origin/feat/x"),
            Checkout::Track {
                remote_ref: "origin/feat/x".to_string()
            }
        );
    }

    #[test]
    fn drops_remote_head_pointer() {
        let got = classify(&[
            line("refs/remotes/origin/HEAD", false, ""),
            line("refs/remotes/origin/main", false, ""),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "origin/main");
    }

    /// The same branch on two remotes, with no local copy, stays two rows —
    /// the user has to say which remote to track.
    #[test]
    fn same_branch_on_two_remotes_lists_both() {
        let got = classify(&[
            line("refs/remotes/origin/shared", false, ""),
            line("refs/remotes/fork/shared", false, ""),
        ]);
        assert_eq!(
            got.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["origin/shared", "fork/shared"]
        );
    }

    #[test]
    fn preserves_input_order() {
        let got = classify(&[
            line("refs/heads/b", false, ""),
            line("refs/heads/a", false, ""),
        ]);
        assert_eq!(
            got.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    /// Current branch, then local, then remote-only — regardless of where
    /// git's date order put them.
    #[test]
    fn relevance_groups_current_then_local_then_remote() {
        let got = by_relevance(classify(&[
            line("refs/remotes/origin/hot", false, ""),
            line("refs/heads/scratch", false, ""),
            line("refs/heads/main", true, "origin/main"),
            line("refs/remotes/origin/main", false, ""),
        ]));
        assert_eq!(
            got.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["main", "scratch", "origin/hot"]
        );
    }

    /// Within a block, git's `--sort=-committerdate` order has to survive: the
    /// sort is stable, and that is the entire recency half of the ordering.
    #[test]
    fn relevance_keeps_commit_order_inside_each_group() {
        let got = by_relevance(classify(&[
            line("refs/heads/newest", false, ""),
            line("refs/remotes/origin/newest-remote", false, ""),
            line("refs/heads/older", false, ""),
            line("refs/remotes/origin/oldest-remote", false, ""),
        ]));
        assert_eq!(
            got.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec![
                "newest",
                "older",
                "origin/newest-remote",
                "origin/oldest-remote"
            ]
        );
    }

    /// A detached HEAD has no current branch row; the listing must still be
    /// grouped rather than falling back to raw date order.
    #[test]
    fn relevance_handles_no_current_branch() {
        let got = by_relevance(classify(&[
            line("refs/remotes/origin/feature", false, ""),
            line("refs/heads/scratch", false, ""),
        ]));
        assert_eq!(
            got.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["scratch", "origin/feature"]
        );
    }

    fn sample() -> Vec<Branch> {
        classify(&[
            line("refs/heads/main", true, "origin/main"),
            line("refs/remotes/origin/main", false, ""),
            line("refs/heads/scratch", false, ""),
            line("refs/remotes/origin/feature", false, ""),
        ])
    }

    #[test]
    fn filters_by_side() {
        let names = |f| {
            filtered(sample(), f)
                .into_iter()
                .map(|b| b.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(Filter::All), ["main", "scratch", "origin/feature"]);
        assert_eq!(names(Filter::Local), ["main", "scratch"]);
        assert_eq!(names(Filter::Remote), ["main", "origin/feature"]);
    }

    #[test]
    fn filter_flags_default_to_all() {
        assert_eq!(Filter::from_flags(false, false), Filter::All);
        assert_eq!(Filter::from_flags(true, false), Filter::Local);
        assert_eq!(Filter::from_flags(false, true), Filter::Remote);
    }

    #[test]
    fn local_branch_is_switched_to() {
        assert_eq!(
            resolve(&sample(), "main"),
            Checkout::Switch("main".to_string())
        );
    }

    #[test]
    fn remote_only_branch_creates_a_tracking_branch() {
        assert_eq!(
            resolve(&sample(), "origin/feature"),
            Checkout::Track {
                remote_ref: "origin/feature".to_string()
            }
        );
    }

    /// Typing the remote form of a branch that is already local must switch to
    /// the local branch, not detach HEAD at the remote ref.
    #[test]
    fn remote_form_of_a_local_branch_switches_locally() {
        assert_eq!(
            resolve(&sample(), "origin/main"),
            Checkout::Switch("main".to_string())
        );
    }

    #[test]
    fn unknown_name_is_left_to_git() {
        assert_eq!(
            resolve(&sample(), "nope"),
            Checkout::Switch("nope".to_string())
        );
    }
}
