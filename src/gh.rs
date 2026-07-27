//! GitHub pull requests, via the `gh` CLI.
//!
//! scriv does no GitHub authentication of its own: it shells out to `gh`, which
//! already holds the user's token (`gh auth login`) and knows which repository
//! the working directory belongs to. That keeps tokens out of scriv's config
//! and makes enterprise hosts, SSO, and keyring storage work for free.
//!
//! As in [`crate::git`], the decisions — parsing `gh`'s JSON, colouring a PR by
//! its state — are pure functions with tests; only [`list`] and [`checkout`]
//! touch the outside world.

use std::io::ErrorKind;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::Reported;

/// JSON fields requested from `gh pr list`.
const FIELDS: &str = "number,title,author,headRefName,isDraft,state,updatedAt";

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
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Author {
    #[serde(default)]
    pub login: String,
}

impl PullRequest {
    pub fn author_login(&self) -> &str {
        self.author.as_ref().map_or("unknown", |a| a.login.as_str())
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
///
/// `gh` writes its own diagnostics, so a failure is [`Reported`] rather than
/// restated.
pub fn checkout(number: u64) -> Result<()> {
    let number = number.to_string();
    let status = Command::new("gh")
        .args(["pr", "checkout", &number])
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
}
