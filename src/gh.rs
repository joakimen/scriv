//! GitHub pull requests, via the `gh` CLI.
//!
//! scriv does no GitHub authentication of its own: it shells out to `gh`, which
//! already holds the user's token (`gh auth login`) and knows which repository
//! the working directory belongs to. That keeps tokens out of scriv's config
//! and makes enterprise hosts, SSO, and keyring storage work for free.
//!
//! As in [`crate::git`], the decisions — parsing `gh`'s JSON, colouring a PR by
//! its state, rolling checks up into a verdict — are pure functions with tests;
//! only [`list`] and the process helpers touch the outside world.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::Reported;

/// JSON fields requested from `gh pr list`.
///
/// `body`, `statusCheckRollup` and `mergeable` are fetched here, with everything
/// else, so the preview pane can be rendered from memory. They cost no extra
/// request — only a larger response, and about 200ms on the one call — and save
/// a `gh pr view` or `gh pr checks` round trip per highlighted row.
const FIELDS: &str =
    "number,title,author,headRefName,isDraft,state,updatedAt,body,statusCheckRollup,mergeable";

/// A pull request, as much of it as the picker needs.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// Absent when the author's account is gone.
    #[serde(default)]
    pub author: Option<Author>,
    /// The PR's source branch.
    pub head_ref_name: String,
    #[serde(default)]
    pub is_draft: bool,
    /// `OPEN`, `CLOSED`, or `MERGED`.
    #[serde(default)]
    pub state: String,
    /// ISO-8601 timestamp, e.g. `2026-07-27T09:12:33Z`.
    #[serde(default)]
    pub updated_at: String,
    /// The description, as markdown. Empty when none was written.
    #[serde(default)]
    pub body: String,
    /// Every check reported against the head commit. Empty when the repository
    /// runs no CI at all.
    #[serde(default)]
    pub status_check_rollup: Vec<Check>,
    /// `MERGEABLE`, `CONFLICTING`, or `UNKNOWN`. See [`Mergeable`].
    #[serde(default)]
    pub mergeable: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Author {
    #[serde(default)]
    pub login: String,
}

/// One entry in a pull request's status check rollup.
///
/// GitHub returns two shapes in the same array: `CheckRun` (GitHub Actions and
/// other check apps, which report a `status` and, once finished, a
/// `conclusion`) and `StatusContext` (the older commit-status API, which
/// reports a single `state`). Both are accepted, so a repository using either —
/// or both at once — reports correctly.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// A `CheckRun`'s name.
    #[serde(default)]
    pub name: String,
    /// A `StatusContext`'s name.
    #[serde(default)]
    pub context: String,
    /// The workflow a `CheckRun` belongs to, which is what tells two jobs named
    /// `build` apart.
    #[serde(default)]
    pub workflow_name: String,
    /// `CheckRun`: `QUEUED`, `IN_PROGRESS`, or `COMPLETED`.
    #[serde(default)]
    pub status: String,
    /// `CheckRun`: `SUCCESS`, `FAILURE`, `SKIPPED`, …; empty until completed.
    #[serde(default)]
    pub conclusion: String,
    /// `StatusContext`: `SUCCESS`, `PENDING`, `FAILURE`, `ERROR`, `EXPECTED`.
    #[serde(default)]
    pub state: String,
}

/// What one check says right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    Fail,
    Pending,
}

impl CheckResult {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Pending => "pending",
        }
    }

    /// The glyph for a list row. See [`GLYPH_WIDTH`]; the colour is
    /// [`Self::color`].
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pass => "✓",
            Self::Fail => "✗",
            Self::Pending => "⧗",
        }
    }

    /// ANSI 256-colour index matching [`Self::tag`].
    pub fn color(self) -> u8 {
        match self {
            Self::Pass => 2,    // green
            Self::Fail => 1,    // red
            Self::Pending => 3, // yellow
        }
    }
}

/// Display width of every status glyph, and of the blank that stands in for
/// one.
///
/// Every glyph is drawn from the East Asian *Narrow* class — `✓` U+2713, `✗`
/// U+2717, `⧗` U+29D7, `⊘` U+2298 — which is what makes a column of them line
/// up. Two things were ruled out on the way:
///
/// - *Ambiguous*-width glyphs (`●`, `≠`, `◆`) are one column in a Western
///   terminal and two under a CJK locale, so a list would align for some users
///   and not others.
/// - Glyphs the emoji tables also claim (`✔` U+2714, `✖` U+2716, `⚠` U+26A0,
///   and every `✅`-style emoji) get emoji presentation and double width in many
///   fonts, and bring their own colour, which cannot then be themed.
///
/// None of the four above is in the emoji data at all, so all four stay text,
/// stay one column, and take the colour they are painted.
pub const GLYPH_WIDTH: usize = 1;

/// The blank that keeps a column aligned when there is nothing to say.
pub const NO_GLYPH: &str = " ";

impl Check {
    /// The name to show: a `CheckRun`'s job name qualified by its workflow, or
    /// a `StatusContext`'s context.
    pub fn label(&self) -> String {
        let name = if self.name.is_empty() {
            &self.context
        } else {
            &self.name
        };
        if self.workflow_name.is_empty() {
            name.clone()
        } else {
            format!("{name} ({})", self.workflow_name)
        }
    }

    pub fn result(&self) -> CheckResult {
        result_for(&self.status, &self.conclusion, &self.state)
    }
}

/// Reduce one check's fields to a verdict, whichever shape it arrived in.
///
/// `SKIPPED` and `NEUTRAL` count as passing: they are conclusions GitHub itself
/// treats as non-blocking, and a matrix job that skips half its legs is the
/// normal case, not a failure.
fn result_for(status: &str, conclusion: &str, state: &str) -> CheckResult {
    // A `StatusContext` says everything in one field.
    if !state.is_empty() {
        return match state.to_ascii_uppercase().as_str() {
            "SUCCESS" => CheckResult::Pass,
            "PENDING" | "EXPECTED" => CheckResult::Pending,
            _ => CheckResult::Fail, // FAILURE, ERROR
        };
    }
    // A `CheckRun` that has not finished is pending whatever else it says.
    if !status.is_empty() && !status.eq_ignore_ascii_case("COMPLETED") {
        return CheckResult::Pending;
    }
    match conclusion.to_ascii_uppercase().as_str() {
        "SUCCESS" | "SKIPPED" | "NEUTRAL" => CheckResult::Pass,
        // Finished with nothing to say: not yet reported, so not yet a verdict.
        "" => CheckResult::Pending,
        // FAILURE, TIMED_OUT, CANCELLED, ACTION_REQUIRED, STARTUP_FAILURE.
        _ => CheckResult::Fail,
    }
}

/// How a pull request's checks add up: the counts, and the one-word verdict
/// derived from them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Checks {
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
}

impl Checks {
    /// Tally `checks` into passed/failed/pending.
    pub fn of(checks: &[Check]) -> Self {
        let mut out = Self::default();
        for check in checks {
            match check.result() {
                CheckResult::Pass => out.passed += 1,
                CheckResult::Fail => out.failed += 1,
                CheckResult::Pending => out.pending += 1,
            }
        }
        out
    }

    pub fn total(&self) -> usize {
        self.passed + self.failed + self.pending
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// The verdict, worst-first: one failure makes the set failing however many
    /// others are green, and anything still running makes it pending.
    ///
    /// Empty when there are no checks — a repository without CI should not be
    /// made to look like one whose checks have not started.
    pub fn tag(&self) -> &'static str {
        if self.is_empty() {
            ""
        } else if self.failed > 0 {
            "fail"
        } else if self.pending > 0 {
            "pending"
        } else {
            "pass"
        }
    }

    /// The verdict as a single [`CheckResult`], or `None` when there are no
    /// checks to roll up.
    pub fn result(&self) -> Option<CheckResult> {
        if self.is_empty() {
            None
        } else if self.failed > 0 {
            Some(CheckResult::Fail)
        } else if self.pending > 0 {
            Some(CheckResult::Pending)
        } else {
            Some(CheckResult::Pass)
        }
    }

    /// The glyph for a list row: one column of [`GLYPH_WIDTH`], blank when the
    /// repository reports no checks at all.
    pub fn glyph(&self) -> &'static str {
        self.result().map_or(NO_GLYPH, CheckResult::glyph)
    }

    /// ANSI 256-colour index matching [`Self::tag`], grey when there is nothing
    /// to report.
    pub fn color(&self) -> u8 {
        self.result().map_or(8, CheckResult::color)
    }

    /// A human count, e.g. `2 passed, 1 failed`. Empty when there are no checks.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        if self.pending > 0 {
            parts.push(format!("{} pending", self.pending));
        }
        if self.passed > 0 {
            parts.push(format!("{} passed", self.passed));
        }
        parts.join(", ")
    }
}

/// Whether GitHub thinks a pull request can be merged into its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mergeable {
    Clean,
    Conflicting,
    /// GitHub computes mergeability lazily, in the background, and reports
    /// `UNKNOWN` until that job has run — and always for a merged or closed
    /// pull request, where the question no longer means anything. Rendered as
    /// nothing at all rather than guessed at.
    Unknown,
}

impl Mergeable {
    pub fn parse(raw: &str) -> Self {
        match raw.to_ascii_uppercase().as_str() {
            "MERGEABLE" => Self::Clean,
            "CONFLICTING" => Self::Conflicting,
            _ => Self::Unknown,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Conflicting => "conflict",
            Self::Unknown => "",
        }
    }

    /// The glyph for a list row — one column of [`GLYPH_WIDTH`], and only for a
    /// conflict.
    ///
    /// Merging cleanly is the case that needs no announcing: it is what you
    /// already assume, it is true of nearly every row, and GitHub only reports
    /// it once a background job has run, so a green mark would be as much a
    /// statement about the API's timing as about the branch. A conflict is the
    /// one answer that changes what you do next, so it is the only one that
    /// takes up space. The preview names both.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Conflicting => "⊘",
            _ => NO_GLYPH,
        }
    }

    /// ANSI 256-colour index matching [`Self::tag`].
    pub fn color(self) -> u8 {
        match self {
            Self::Clean => 2,       // green
            Self::Conflicting => 1, // red
            Self::Unknown => 8,     // bright black
        }
    }
}

/// Whether a pull request looks mergeable right now, from what a single
/// `gh pr list` already knows.
///
/// This is the axis a merge list is sorted by in the reader's head, and it is
/// not the same as [`PullRequest::state`]: a list of open pull requests is one
/// shade of green under a state colouring, which is to say no colouring at all
/// at the moment the colour would be most useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Nothing in the way: open, not a draft, no failing check, no conflict.
    Ready,
    /// Checks are still running.
    Waiting,
    /// A failing check or a conflict with the base branch.
    Blocked,
    /// Not a candidate at all — a draft, or already merged or closed.
    Unavailable,
}

impl Readiness {
    /// ANSI 256-colour index: the traffic light this describes.
    pub fn color(self) -> u8 {
        match self {
            Self::Ready => 2,       // green
            Self::Waiting => 3,     // yellow
            Self::Blocked => 1,     // red
            Self::Unavailable => 8, // bright black
        }
    }
}

impl PullRequest {
    pub fn author_login(&self) -> &str {
        self.author.as_ref().map_or("unknown", |a| a.login.as_str())
    }

    pub fn checks(&self) -> Checks {
        Checks::of(&self.status_check_rollup)
    }

    pub fn mergeable(&self) -> Mergeable {
        Mergeable::parse(&self.mergeable)
    }

    /// Whether this looks mergeable — see [`Readiness`].
    ///
    /// A draft counts as unavailable rather than blocked: nothing is wrong with
    /// it, it is simply not being offered yet.
    pub fn readiness(&self) -> Readiness {
        if self.is_draft || !self.state.eq_ignore_ascii_case("OPEN") {
            return Readiness::Unavailable;
        }
        let checks = self.checks();
        if checks.failed > 0 || self.mergeable() == Mergeable::Conflicting {
            Readiness::Blocked
        } else if checks.pending > 0 {
            Readiness::Waiting
        } else {
            Readiness::Ready
        }
    }

    /// The checks that are not passing, failures first — the ones worth naming
    /// in a preview, since a list of thirty green jobs says nothing.
    pub fn failing_checks(&self) -> Vec<&Check> {
        let mut out: Vec<&Check> = self
            .status_check_rollup
            .iter()
            .filter(|c| c.result() != CheckResult::Pass)
            .collect();
        out.sort_by_key(|c| c.result() != CheckResult::Fail);
        out
    }

    /// Just the date part of [`Self::updated_at`]; `gh` reports no relative
    /// time and a date needs no clock of our own to render.
    pub fn updated_date(&self) -> &str {
        self.updated_at.split('T').next().unwrap_or_default()
    }

    /// ANSI 256-colour index for this PR, used in both the picker and `ls`.
    pub fn color(&self) -> u8 {
        color_for(self.is_draft, &self.state)
    }

    pub fn tag(&self) -> &'static str {
        tag_for(self.is_draft, &self.state)
    }
}

/// Colour a PR by what can be done with it: a draft is not ready to review, a
/// merged or closed one is history.
fn color_for(is_draft: bool, state: &str) -> u8 {
    match state.to_ascii_uppercase().as_str() {
        "MERGED" => 5,      // magenta
        "CLOSED" => 1,      // red
        _ if is_draft => 8, // bright black — open but draft
        _ => 2,             // green — open and ready
    }
}

fn tag_for(is_draft: bool, state: &str) -> &'static str {
    match state.to_ascii_uppercase().as_str() {
        "MERGED" => "merged",
        "CLOSED" => "closed",
        _ if is_draft => "draft",
        _ => "open",
    }
}

/// Parse the array `gh pr list --json …` prints.
pub fn parse_prs(data: &str) -> Result<Vec<PullRequest>> {
    // `gh` prints nothing at all in some error paths; treat that as empty.
    if data.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(data).context("parsing `gh pr list` output")
}

/// Pull requests for the repository the working directory belongs to.
///
/// `state` is passed through to `gh` (`open`, `closed`, `merged`, `all`).
pub fn list(state: &str, limit: usize) -> Result<Vec<PullRequest>> {
    let limit = limit.to_string();
    let out = capture(&[
        "pr", "list", "--json", FIELDS, "--state", state, "--limit", &limit,
    ])?;
    parse_prs(&out)
}

/// Check out a pull request's branch. `gh` creates the local branch and sets
/// its upstream, including for PRs from forks.
pub fn checkout(number: u64) -> Result<()> {
    run(&["pr", "checkout", &number.to_string()])
}

/// Open a pull request in the browser.
///
/// `gh pr view --web` is what does the opening: it already knows the host — so
/// GitHub Enterprise works — and defers to `$BROWSER` and the platform's opener,
/// which is more than scriv would get right on its own.
pub fn view_web(number: u64) -> Result<()> {
    run(&["pr", "view", "--web", &number.to_string()])
}

/// How to merge a pull request. `None` at the call site leaves the choice to
/// `gh`, which asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }
}

/// Merge a pull request.
///
/// With no `method`, `gh` prompts for one interactively; that is why this
/// inherits stdio rather than capturing it. skim has restored the terminal by
/// the time we get here, so the prompt behaves as it would when `gh` is run by
/// hand.
pub fn merge(
    number: u64,
    method: Option<MergeMethod>,
    delete_branch: bool,
    auto: bool,
) -> Result<()> {
    let number = number.to_string();
    let mut args = vec!["pr", "merge", &number];
    if let Some(method) = method {
        args.push(method.flag());
    }
    if delete_branch {
        args.push("--delete-branch");
    }
    if auto {
        args.push("--auto");
    }
    run(&args)
}

/// JSON fields requested from `gh repo list`.
///
/// Only what a picker row and its preview can use. `gh` will happily return
/// license, topics, and fork parentage; none of it helps choose between two
/// repositories, and all of it costs response size on an org with hundreds.
const REPO_FIELDS: &str =
    "nameWithOwner,description,isPrivate,isArchived,isFork,primaryLanguage,pushedAt";

/// A repository on GitHub, as much of it as the clone picker needs.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// `owner/name`.
    pub name_with_owner: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub is_private: bool,
    #[serde(default)]
    pub is_archived: bool,
    #[serde(default)]
    pub is_fork: bool,
    #[serde(default)]
    pub primary_language: Option<Language>,
    /// ISO-8601 timestamp of the last push.
    #[serde(default)]
    pub pushed_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Language {
    #[serde(default)]
    pub name: String,
}

impl Repo {
    /// The repository name without its owner — the directory it clones into.
    pub fn name(&self) -> &str {
        self.name_with_owner
            .split_once('/')
            .map_or(self.name_with_owner.as_str(), |(_, name)| name)
    }

    pub fn owner(&self) -> &str {
        self.name_with_owner
            .split_once('/')
            .map_or("", |(owner, _)| owner)
    }

    pub fn language(&self) -> &str {
        self.primary_language
            .as_ref()
            .map_or("", |l| l.name.as_str())
    }

    /// Just the date part of [`Self::pushed_at`].
    pub fn pushed_date(&self) -> &str {
        self.pushed_at.split('T').next().unwrap_or_default()
    }

    /// The one-word tags worth showing: what makes this repository unusual.
    /// A public, unarchived, non-fork repository is the default and says
    /// nothing.
    pub fn tags(&self) -> Vec<&'static str> {
        let mut tags = Vec::new();
        if self.is_archived {
            tags.push("archived");
        }
        if self.is_private {
            tags.push("private");
        }
        if self.is_fork {
            tags.push("fork");
        }
        tags
    }
}

/// Parse the array `gh repo list --json …` prints.
pub fn parse_repos(data: &str) -> Result<Vec<Repo>> {
    if data.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(data).context("parsing `gh repo list` output")
}

/// Every repository belonging to `owner`.
///
/// `--limit` is what drives pagination: `gh` pages through the API until it has
/// that many, so one generous number stands in for a page loop. It is exposed
/// rather than hard-coded because an org with more repositories than the limit
/// would otherwise silently show a truncated list.
pub fn list_repos(owner: &str, limit: usize) -> Result<Vec<Repo>> {
    let limit = limit.to_string();
    let out = capture(&[
        "repo",
        "list",
        owner,
        "--json",
        REPO_FIELDS,
        "--limit",
        &limit,
    ])?;
    parse_repos(&out)
}

/// Clone `owner/repo` into `dest`.
///
/// Output is captured rather than inherited: clones run concurrently, and
/// several `git` progress meters writing to one terminal at once is illegible.
/// The child's own diagnostics are returned on failure so the caller can
/// attribute them to the right repository.
pub fn clone(name_with_owner: &str, dest: &Path) -> Result<()> {
    let output = Command::new("gh")
        .args(["repo", "clone", name_with_owner, &dest.to_string_lossy()])
        .stdin(Stdio::null())
        .output()
        .map_err(spawn_error)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        bail!(if stderr.is_empty() {
            format!("cloning {name_with_owner} failed")
        } else {
            stderr.to_string()
        });
    }
    Ok(())
}

/// The authenticated user's login, and the organisations they belong to.
///
/// This is the answer to "which owners do you actually clone from" on a machine
/// with nothing cloned yet, where looking at the filesystem would find nothing.
pub fn owners() -> Result<Vec<String>> {
    let mut out = Vec::new();
    if let Ok(login) = capture(&["api", "user", "--jq", ".login"]) {
        out.extend(login.split_whitespace().map(str::to_string));
    }
    if let Ok(orgs) = capture(&["api", "user/orgs", "--jq", ".[].login"]) {
        out.extend(orgs.split_whitespace().map(str::to_string));
    }
    Ok(out)
}

/// Run `gh` with the terminal attached, passing its exit status through.
///
/// `gh` writes its own diagnostics, so a failure is [`Reported`] rather than
/// restated in vaguer words on top.
fn run(args: &[&str]) -> Result<()> {
    let status = Command::new("gh")
        .args(args)
        .status()
        .map_err(spawn_error)?;
    if !status.success() {
        return Err(Reported(status.code().unwrap_or(1)).into());
    }
    Ok(())
}

fn capture(args: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(spawn_error)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        bail!(if stderr.is_empty() {
            format!("gh {} failed", args.join(" "))
        } else {
            stderr.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A missing `gh` is the one failure worth explaining, since PR support is the
/// only thing that depends on it.
fn spawn_error(err: std::io::Error) -> anyhow::Error {
    match err.kind() {
        ErrorKind::NotFound => anyhow!(
            "`gh` was not found on PATH — pull request commands need the GitHub CLI (https://cli.github.com), authenticated with `gh auth login`"
        ),
        _ => anyhow!(err).context("running gh"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
        {"number":12,"title":"Add branch picker","author":{"login":"joakimen"},
         "headRefName":"feat/branches","isDraft":false,"state":"OPEN",
         "updatedAt":"2026-07-27T09:12:33Z"},
        {"number":9,"title":"WIP","author":null,"headRefName":"wip",
         "isDraft":true,"state":"OPEN","updatedAt":"2026-07-20T11:00:00Z"}
    ]"#;

    #[test]
    fn parses_gh_output() {
        let prs = parse_prs(SAMPLE).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 12);
        assert_eq!(prs[0].head_ref_name, "feat/branches");
        assert_eq!(prs[0].author_login(), "joakimen");
        assert_eq!(prs[0].updated_date(), "2026-07-27");
        assert!(prs[1].is_draft);
    }

    /// A deleted account leaves `author: null`; the listing must still render.
    #[test]
    fn missing_author_falls_back() {
        let prs = parse_prs(SAMPLE).unwrap();
        assert_eq!(prs[1].author_login(), "unknown");
    }

    #[test]
    fn empty_output_is_no_prs() {
        assert!(parse_prs("").unwrap().is_empty());
        assert!(parse_prs("[]").unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_prs("{oops").is_err());
    }

    #[test]
    fn colours_by_state() {
        assert_eq!(color_for(false, "OPEN"), 2);
        assert_eq!(color_for(true, "OPEN"), 8);
        assert_eq!(color_for(false, "MERGED"), 5);
        assert_eq!(color_for(false, "CLOSED"), 1);
        // A draft that was merged or closed is history first.
        assert_eq!(color_for(true, "MERGED"), 5);
    }

    #[test]
    fn tags_match_colours() {
        assert_eq!(tag_for(false, "OPEN"), "open");
        assert_eq!(tag_for(true, "OPEN"), "draft");
        assert_eq!(tag_for(false, "MERGED"), "merged");
        assert_eq!(tag_for(false, "CLOSED"), "closed");
    }

    /// The rollup arrives as a mix of `CheckRun` and `StatusContext` objects,
    /// which report their outcome in entirely different fields.
    const CHECKS: &str = r#"[
        {"number":1,"title":"Mixed","headRefName":"m","state":"OPEN",
         "mergeable":"MERGEABLE","statusCheckRollup":[
            {"__typename":"CheckRun","name":"build","workflowName":"ci",
             "status":"COMPLETED","conclusion":"SUCCESS"},
            {"__typename":"CheckRun","name":"lint","workflowName":"ci",
             "status":"COMPLETED","conclusion":"FAILURE"},
            {"__typename":"CheckRun","name":"e2e","workflowName":"ci",
             "status":"IN_PROGRESS","conclusion":""},
            {"__typename":"CheckRun","name":"docs","workflowName":"ci",
             "status":"COMPLETED","conclusion":"SKIPPED"},
            {"__typename":"StatusContext","context":"ci/legacy","state":"SUCCESS"},
            {"__typename":"StatusContext","context":"ci/flaky","state":"ERROR"}
         ]}
    ]"#;

    fn mixed() -> PullRequest {
        parse_prs(CHECKS).unwrap().pop().unwrap()
    }

    #[test]
    fn counts_both_check_shapes() {
        let checks = mixed().checks();
        // SUCCESS + SKIPPED + StatusContext SUCCESS.
        assert_eq!(checks.passed, 3);
        // CheckRun FAILURE + StatusContext ERROR.
        assert_eq!(checks.failed, 2);
        assert_eq!(checks.pending, 1);
        assert_eq!(checks.total(), 6);
    }

    /// One failure outranks any number of green checks: that is the thing the
    /// list has to surface.
    #[test]
    fn one_failure_makes_the_set_failing() {
        assert_eq!(mixed().checks().tag(), "fail");
        assert_eq!(
            Checks {
                passed: 9,
                failed: 0,
                pending: 1
            }
            .tag(),
            "pending"
        );
        assert_eq!(
            Checks {
                passed: 9,
                failed: 0,
                pending: 0
            }
            .tag(),
            "pass"
        );
    }

    /// A repository with no CI must not look like one whose checks have not
    /// started yet.
    #[test]
    fn no_checks_is_blank_not_pending() {
        let checks = Checks::default();
        assert!(checks.is_empty());
        assert_eq!(checks.tag(), "");
        assert_eq!(checks.summary(), "");
    }

    /// A skipped matrix leg or a neutral check is the normal case, not a
    /// failure — GitHub's own rollup treats both as non-blocking.
    #[test]
    fn skipped_and_neutral_pass() {
        assert_eq!(result_for("COMPLETED", "SKIPPED", ""), CheckResult::Pass);
        assert_eq!(result_for("COMPLETED", "NEUTRAL", ""), CheckResult::Pass);
        assert_eq!(result_for("COMPLETED", "CANCELLED", ""), CheckResult::Fail);
        assert_eq!(result_for("COMPLETED", "TIMED_OUT", ""), CheckResult::Fail);
        assert_eq!(result_for("QUEUED", "", ""), CheckResult::Pending);
        // A StatusContext's `state` decides on its own.
        assert_eq!(result_for("", "", "PENDING"), CheckResult::Pending);
        assert_eq!(result_for("", "", "FAILURE"), CheckResult::Fail);
    }

    #[test]
    fn summary_leads_with_the_bad_news() {
        assert_eq!(mixed().checks().summary(), "2 failed, 1 pending, 3 passed");
    }

    /// The preview names what is wrong, so failures have to come before the
    /// checks that are merely still running.
    #[test]
    fn failing_checks_are_listed_failures_first() {
        let pr = mixed();
        let names: Vec<String> = pr.failing_checks().iter().map(|c| c.label()).collect();
        assert_eq!(names, ["lint (ci)", "ci/flaky", "e2e (ci)"]);
    }

    #[test]
    fn mergeable_states_parse() {
        assert_eq!(Mergeable::parse("MERGEABLE"), Mergeable::Clean);
        assert_eq!(Mergeable::parse("CONFLICTING"), Mergeable::Conflicting);
        assert_eq!(Mergeable::parse("UNKNOWN"), Mergeable::Unknown);
        assert_eq!(mixed().mergeable(), Mergeable::Clean);
    }

    /// GitHub computes mergeability in the background and says `UNKNOWN` until
    /// it has — and always for a merged pull request. Rendering that as
    /// "conflict", or as "clean", would be a guess either way.
    #[test]
    fn unknown_mergeability_renders_as_nothing() {
        assert_eq!(Mergeable::parse("UNKNOWN").tag(), "");
        assert_eq!(Mergeable::parse("").tag(), "");
    }

    /// A pull request from before these fields were requested — or from a `gh`
    /// that does not return them — must still parse.
    #[test]
    fn check_fields_are_optional() {
        let prs = parse_prs(SAMPLE).unwrap();
        assert!(prs[0].checks().is_empty());
        assert_eq!(prs[0].mergeable(), Mergeable::Unknown);
    }

    /// The whole reason these particular glyphs were chosen: every one of them
    /// occupies exactly [`GLYPH_WIDTH`] terminal columns, so a column of them
    /// lines up without measuring anything per row. Reaching for a wide glyph
    /// (any emoji) or an ambiguous-width one (`●`, `◆`, `≠`) would shift every
    /// row it appeared on, and the damage would show only on the rows that
    /// happened to use it.
    #[test]
    fn every_status_glyph_is_one_column_wide() {
        use unicode_width::UnicodeWidthStr;
        let glyphs = [
            CheckResult::Pass.glyph(),
            CheckResult::Fail.glyph(),
            CheckResult::Pending.glyph(),
            Mergeable::Conflicting.glyph(),
            Mergeable::Clean.glyph(),
            Mergeable::Unknown.glyph(),
            Checks::default().glyph(),
            NO_GLYPH,
        ];
        for glyph in glyphs {
            assert_eq!(
                glyph.width(),
                GLYPH_WIDTH,
                "{glyph:?} is not {GLYPH_WIDTH} columns wide"
            );
        }
    }

    /// A merge list is read for one thing: what can go in now. State says
    /// nothing there — every open pull request is the same green.
    #[test]
    fn readiness_is_not_state() {
        let ready = mixed();
        // `mixed` has a failing check.
        assert_eq!(ready.readiness(), Readiness::Blocked);

        let prs = parse_prs(SAMPLE).unwrap();
        // Open, no checks, mergeability unknown: nothing is in the way.
        assert_eq!(prs[0].readiness(), Readiness::Ready);
        // A draft is not blocked, it is simply not on offer.
        assert_eq!(prs[1].readiness(), Readiness::Unavailable);
    }

    /// A merged pull request must never colour as ready, however green its
    /// checks were at the time.
    #[test]
    fn merged_is_never_ready() {
        let pr = parse_prs(
            r#"[{"number":1,"title":"t","headRefName":"h","state":"MERGED",
                 "statusCheckRollup":[
                   {"name":"build","status":"COMPLETED","conclusion":"SUCCESS"}]}]"#,
        )
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(pr.checks().tag(), "pass");
        assert_eq!(pr.readiness(), Readiness::Unavailable);
    }

    /// A conflict blocks even when every check is green — the two facts are
    /// independent, and either one is enough.
    #[test]
    fn a_conflict_blocks_a_green_pull_request() {
        let pr = parse_prs(
            r#"[{"number":1,"title":"t","headRefName":"h","state":"OPEN",
                 "mergeable":"CONFLICTING","statusCheckRollup":[
                   {"name":"build","status":"COMPLETED","conclusion":"SUCCESS"}]}]"#,
        )
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(pr.checks().tag(), "pass");
        assert_eq!(pr.readiness(), Readiness::Blocked);
    }

    #[test]
    fn merge_methods_map_to_gh_flags() {
        assert_eq!(MergeMethod::Merge.flag(), "--merge");
        assert_eq!(MergeMethod::Squash.flag(), "--squash");
        assert_eq!(MergeMethod::Rebase.flag(), "--rebase");
    }
}
