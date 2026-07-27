//! `scriv branch` — list, pick, and check out git branches.
//!
//! Local and remote branches are shown in one list, coloured by where they
//! live, so switching to a branch you have and checking out one you do not are
//! the same gesture. Picking a remote-only branch creates the local branch and
//! sets its upstream, which is the step that otherwise has to be typed by hand.

use anyhow::{Result, bail};

use crate::git::{self, Branch, Filter};
use crate::pick::PickItem;
use crate::term;
use crate::{Ctx, pick};

/// Every branch in the current repository, optionally refreshing remotes first.
fn load(ctx: &Ctx, fetch: bool) -> Result<Vec<Branch>> {
    git::ensure_repo()?;
    if fetch {
        git::fetch()?;
    }
    let branches = git::branches()?;
    ctx.log.info(&format!("found {} branches", branches.len()));
    Ok(branches)
}

/// Apply `filter`, failing with a message that names what was looked for.
fn narrow(branches: Vec<Branch>, filter: Filter) -> Result<Vec<Branch>> {
    let branches = git::filtered(branches, filter);
    if branches.is_empty() {
        bail!(match filter {
            Filter::Local => "no local branches found",
            Filter::Remote => "no remote branches found",
            Filter::All => "no branches found",
        });
    }
    Ok(branches)
}

/// Load and filter in one step, for the commands that only ever show a list.
fn collect(ctx: &Ctx, filter: Filter, fetch: bool) -> Result<Vec<Branch>> {
    narrow(load(ctx, fetch)?, filter)
}

/// Width of the widest branch name, for column alignment.
fn name_width(branches: &[Branch]) -> usize {
    branches.iter().map(|b| b.name.len()).max().unwrap_or(0)
}

/// `scriv branch ls` — print branch names, one per line.
///
/// Bare names by default so the output pipes cleanly; `--status` adds the
/// current-branch marker, the local/both/remote tag, and the last commit.
/// Colour follows the same kind mapping as the picker and is dropped when
/// stdout is not a terminal.
pub fn ls(ctx: &Ctx, filter: Filter, status: bool, fetch: bool) -> Result<()> {
    let branches = collect(ctx, filter, fetch)?;
    let color = term::stdout_color();

    if !status {
        for branch in &branches {
            println!("{}", term::paint(&branch.name, branch.kind.color(), color));
        }
        return Ok(());
    }

    let width = name_width(&branches);
    let date_width = branches.iter().map(|b| b.date.len()).max().unwrap_or(0);
    for branch in &branches {
        let marker = if branch.head { "*" } else { " " };
        let row = format!(
            "{marker} {name:<width$}  {tag:<6}  {date:<date_width$}  {subject}",
            name = branch.name,
            tag = branch.kind.tag(),
            date = branch.date,
            subject = branch.subject,
        );
        println!(
            "{}",
            term::paint(row.trim_end(), branch.kind.color(), color)
        );
    }
    Ok(())
}

/// Build picker rows: current-branch marker, name, last commit date, subject,
/// each row tinted by [`BranchKind`](crate::git::BranchKind) — yellow
/// local-only, green local+remote, cyan remote-only.
fn items(branches: &[Branch]) -> Vec<PickItem> {
    let width = name_width(branches);
    let date_width = branches.iter().map(|b| b.date.len()).max().unwrap_or(0);
    branches
        .iter()
        .map(|branch| {
            let marker = if branch.head { "*" } else { " " };
            let label = format!(
                "{marker} {name:<width$}  {date:<date_width$}  {subject}",
                name = branch.name,
                date = branch.date,
                subject = branch.subject,
            );
            PickItem::new(label.trim_end(), branch.name.clone()).color(branch.kind.color())
        })
        .collect()
}

/// Fuzzy-select one branch and return its name (`main`, or `origin/main` for a
/// branch that only exists on a remote).
fn select(ctx: &Ctx, branches: &[Branch], prompt: &str) -> Result<String> {
    pick::pick_one(items(branches), prompt, &ctx.config.picker)
}

/// `scriv branch pick` — fuzzy-select a branch and print its name.
pub fn pick(ctx: &Ctx, filter: Filter, fetch: bool) -> Result<()> {
    let branches = collect(ctx, filter, fetch)?;
    let choice = select(ctx, &branches, "Pick a branch")?;
    println!("{choice}");
    Ok(())
}

/// `scriv branch checkout [name]` — switch to a branch, picking one when no
/// name is given.
///
/// A remote-only branch is checked out as a new local branch tracking it, so
/// `git push`/`git pull` work immediately afterwards. git's own output is left
/// alone, so the familiar "Switched to branch …" line still appears.
///
/// `filter` narrows what the picker offers, but a name given on the command
/// line always resolves against every branch — `--local` is about choosing, not
/// about what `origin/feature` is allowed to mean.
pub fn checkout(ctx: &Ctx, name: Option<&str>, filter: Filter, fetch: bool) -> Result<()> {
    let branches = load(ctx, fetch)?;
    let name = match name {
        Some(name) => name.to_string(),
        None => {
            let offered = narrow(branches.clone(), filter)?;
            select(ctx, &offered, "Check out a branch")?
        }
    };

    let action = git::resolve(&branches, &name);
    ctx.log.info(&format!("checkout action: {action:?}"));
    git::checkout(&action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{BranchKind, RefLine, classify};

    fn branches() -> Vec<Branch> {
        classify(&[
            RefLine {
                refname: "refs/heads/main".into(),
                head: true,
                upstream: "origin/main".into(),
                date: "2 hours ago".into(),
                subject: "init".into(),
            },
            RefLine {
                refname: "refs/remotes/origin/main".into(),
                head: false,
                upstream: String::new(),
                date: "2 hours ago".into(),
                subject: "init".into(),
            },
            RefLine {
                refname: "refs/remotes/origin/feature".into(),
                head: false,
                upstream: String::new(),
                date: "3 days ago".into(),
                subject: "wip".into(),
            },
        ])
    }

    #[test]
    fn rows_mark_head_and_return_the_branch_name() {
        let items = items(&branches());
        assert!(items[0].label.starts_with("* main"));
        assert_eq!(items[0].value, "main");
        assert!(items[1].label.starts_with("  origin/feature"));
        assert_eq!(items[1].value, "origin/feature");
    }

    #[test]
    fn rows_are_coloured_by_kind() {
        let items = items(&branches());
        assert_eq!(items[0].color, Some(BranchKind::Tracked.color()));
        assert_eq!(items[1].color, Some(BranchKind::Remote.color()));
    }

    /// Names and dates are padded to a common width so the subject column
    /// lines up across rows.
    #[test]
    fn columns_align() {
        let items = items(&branches());
        assert_eq!(items[0].label.find("init"), items[1].label.find("wip"));
    }
}
