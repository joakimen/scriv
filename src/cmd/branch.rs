//! `scriv branch` — list, select, and check out git branches.
//!
//! Local and remote branches are shown in one list, coloured by where they
//! live. Selecting a remote-only branch creates the local branch and sets its
//! upstream.
//!
//! Every listing arrives ordered by [`git::by_relevance`]: current branch, then
//! local, then remote-only, newest first within each.

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::git::{self, Branch, Filter};
use crate::select::{Preview, SelectItem};
use crate::term;
use crate::{Ctx, select};

/// Every branch in the current repository, optionally refreshing remotes first.
/// The fetch is silent (see [`git::fetch`]), so it gets the spinner.
fn load(ctx: &Ctx, fetch: bool) -> Result<Vec<Branch>> {
    git::ensure_repo()?;
    if fetch {
        let _spinner = term::spinner("fetching", ctx.color());
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

/// Width of the widest branch name, in characters — `{:<width$}` pads by
/// character count, so a byte length would over-pad a non-ASCII row.
fn name_width(branches: &[Branch]) -> usize {
    branches
        .iter()
        .map(|b| b.name.chars().count())
        .max()
        .unwrap_or(0)
}

/// Width of the widest relative date, counted in characters as above.
fn date_width(branches: &[Branch]) -> usize {
    branches
        .iter()
        .map(|b| b.date.chars().count())
        .max()
        .unwrap_or(0)
}

/// `scriv branch ls` — print branch names, one per line.
///
/// Bare names by default so the output pipes cleanly; `--status` adds the
/// current-branch marker, the local/both/remote tag, and the last commit.
/// Colour follows the same kind mapping as the selector and is dropped when
/// stdout is not a terminal.
pub fn ls(ctx: &Ctx, filter: Filter, status: bool, fetch: bool) -> Result<()> {
    let branches = collect(ctx, filter, fetch)?;
    let color = ctx.color();

    let mut out = term::Listing::stdout();
    if !status {
        for branch in &branches {
            if !out.line(&term::paint(&branch.name, branch.kind.color(), color))? {
                break;
            }
        }
        return Ok(out.finish()?);
    }

    let width = name_width(&branches);
    let date_width = date_width(&branches);
    for branch in &branches {
        let marker = if branch.head { "*" } else { " " };
        let row = format!(
            "{marker} {name:<width$}  {tag:<6}  {date:<date_width$}  {subject}",
            name = branch.name,
            tag = branch.kind.tag(),
            date = branch.date,
            subject = branch.subject,
        );
        if !out.line(&term::paint(row.trim_end(), branch.kind.color(), color))? {
            break;
        }
    }
    out.finish()?;
    Ok(())
}

/// The preview for a branch: its recent commits, with who wrote them and when.
/// The trailing `--` keeps a branch named like a file from being read as a
/// path. Bounded to 30 commits and run with `--no-optional-locks`.
fn preview(branch: &Branch) -> Preview {
    Preview::Command(format!(
        "git --no-optional-locks log --color=always --max-count=30 --date=relative \
         --format='%C(auto)%h%C(reset)  %C(blue)%an%C(reset)  %C(green)%ad%C(reset)  %s' {} --",
        select::quote(&branch.name)
    ))
}

/// Build selector rows: current-branch marker, name, last commit date, subject,
/// each row tinted by [`BranchKind`](crate::git::BranchKind) — yellow
/// local-only, green local+remote, cyan remote-only.
fn items(branches: &[Branch]) -> Vec<SelectItem> {
    let width = name_width(branches);
    let date_width = date_width(branches);
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
            SelectItem::new(label.trim_end(), branch.name.clone())
                .color(branch.kind.color())
                .preview(preview(branch))
        })
        .collect()
}

/// Fuzzy-select one branch, with [`REFRESH_KEY`](select::REFRESH_KEY) fetching
/// and rebuilding the list without closing the selector.
///
/// Returns the chosen name together with the branch list as it stood at that
/// moment: a refresh replaces the list, and [`git::resolve`] must not resolve
/// against the one the selector opened with. A failed fetch leaves the rows as
/// they were and is reported once the selector is out of the way.
fn select(ctx: &Ctx, branches: Vec<Branch>, filter: Filter, prompt: &str) -> Result<Selection> {
    let offered = narrow(branches.clone(), filter)?;
    let known = Arc::new(Mutex::new(branches));
    let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let reload = {
        let (known, failure) = (Arc::clone(&known), Arc::clone(&failure));
        Box::new(move || {
            // Fetched outside the lock: it is a network round trip, and holding
            // the list for it would make a second ctrl-r — or closing the
            // selector — wait on the remote rather than on the list.
            let fresh = git::fetch().and_then(|()| git::branches());
            let mut known = known.lock().expect("branch list poisoned");
            match fresh {
                Ok(fresh) => *known = fresh,
                Err(err) => {
                    *failure.lock().expect("failure slot poisoned") = Some(format!("{err:#}"))
                }
            }
            items(&git::filtered(known.clone(), filter))
        })
    };

    let name = select::select_one_reloading(items(&offered), prompt, &ctx.config.selector, reload)?;

    if let Some(err) = failure.lock().expect("failure slot poisoned").take() {
        eprintln!("warning: could not refresh branches: {err}");
    }
    let branches = known.lock().expect("branch list poisoned").clone();
    Ok(Selection { name, branches })
}

/// A chosen branch, and the branch list as it stood when it was chosen.
struct Selection {
    name: String,
    branches: Vec<Branch>,
}

/// `scriv branch sel` — fuzzy-select a branch and print its name.
pub fn sel(ctx: &Ctx, filter: Filter, fetch: bool) -> Result<()> {
    let branches = load(ctx, fetch)?;
    let chosen = select(ctx, branches, filter, "Select a branch")?;
    println!("{}", chosen.name);
    Ok(())
}

/// `scriv branch checkout [name]` — switch to a branch, selecting one when no
/// name is given.
///
/// A remote-only branch is checked out as a new local branch tracking it.
/// `filter` narrows what the selector offers, but a name given on the command
/// line always resolves against every branch.
pub fn checkout(ctx: &Ctx, name: Option<&str>, filter: Filter, fetch: bool) -> Result<()> {
    let loaded = load(ctx, fetch)?;
    let (name, branches) = match name {
        Some(name) => (name.to_string(), loaded),
        None => {
            let chosen = select(ctx, loaded, filter, "Check out a branch")?;
            (chosen.name, chosen.branches)
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
        assert_eq!(items[0].value(), "main");
        assert!(items[1].label.starts_with("  origin/feature"));
        assert_eq!(items[1].value(), "origin/feature");
    }

    #[test]
    fn rows_are_coloured_by_kind() {
        let items = items(&branches());
        assert_eq!(items[0].color, Some(BranchKind::Tracked.color()));
        assert_eq!(items[1].color, Some(BranchKind::Remote.color()));
    }

    #[test]
    fn columns_align() {
        let items = items(&branches());
        assert_eq!(items[0].label.find("init"), items[1].label.find("wip"));
    }

    #[test]
    fn column_width_counts_characters_not_bytes() {
        let rows = classify(&[RefLine {
            refname: "refs/heads/café".into(),
            head: false,
            upstream: String::new(),
            date: "now".into(),
            subject: "one".into(),
        }]);
        let label = &items(&rows)[0].label;
        assert!(
            label.starts_with("  café  now"),
            "column padded past the longest name: {label:?}"
        );
    }

    #[test]
    fn preview_logs_the_branch_with_authors() {
        let Preview::Command(cmd) = preview(&branches()[1]) else {
            panic!("expected a command preview");
        };
        assert!(cmd.contains(" log "), "{cmd}");
        assert!(
            cmd.contains("%an"),
            "author is what the preview is for: {cmd}"
        );
        assert!(cmd.ends_with("'origin/feature' --"), "{cmd}");
        // Browsing a branch list must not take the repository's index lock.
        assert!(cmd.contains("--no-optional-locks"), "{cmd}");
        assert!(cmd.contains("--max-count=30"), "unbounded log: {cmd}");
    }
}
