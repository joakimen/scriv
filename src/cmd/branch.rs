//! `scriv branch` — list, pick, and check out git branches.
//!
//! Local and remote branches are shown in one list, coloured by where they
//! live, so switching to a branch you have and checking out one you do not are
//! the same gesture. Picking a remote-only branch creates the local branch and
//! sets its upstream, which is the step that otherwise has to be typed by hand.
//!
//! Every listing arrives ordered by [`git::by_relevance`] — current branch,
//! then local, then remote-only, newest first within each — so the top of the
//! list is where the answer usually is.

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::git::{self, Branch, Filter};
use crate::pick::{PickItem, Preview};
use crate::term;
use crate::{Ctx, pick};

/// Every branch in the current repository, optionally refreshing remotes first.
///
/// The fetch is the only slow step here, and it is silent (see [`git::fetch`]),
/// so it gets the spinner: a network round trip with nothing on screen is
/// indistinguishable from a hang.
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

/// Width of the widest branch name, for column alignment.
///
/// Counted in characters, not bytes: `{:<width$}` pads by character count, so a
/// byte length would over-pad any row containing non-ASCII and leave the
/// columns ragged.
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
/// Colour follows the same kind mapping as the picker and is dropped when
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
        return Ok(());
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
    Ok(())
}

/// The preview for a branch: its recent commits, with who wrote them and when.
///
/// That is the question a branch list actually raises — whose work is this, and
/// is it still live — and it is answered by the same `git log` invocation for a
/// local or a remote ref. The trailing `--` keeps a branch named like a file
/// from being read as a path.
///
/// Bounded to 30 commits and run with `--no-optional-locks`, so scrolling the
/// list never takes the repository's index lock or reads more history than the
/// pane can show.
fn preview(branch: &Branch) -> Preview {
    Preview::Command(format!(
        "git --no-optional-locks log --color=always --max-count=30 --date=relative \
         --format='%C(auto)%h%C(reset)  %C(blue)%an%C(reset)  %C(green)%ad%C(reset)  %s' {} --",
        pick::quote(&branch.name)
    ))
}

/// Build picker rows: current-branch marker, name, last commit date, subject,
/// each row tinted by [`BranchKind`](crate::git::BranchKind) — yellow
/// local-only, green local+remote, cyan remote-only.
fn items(branches: &[Branch]) -> Vec<PickItem> {
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
            PickItem::new(label.trim_end(), branch.name.clone())
                .color(branch.kind.color())
                .preview(preview(branch))
        })
        .collect()
}

/// Fuzzy-select one branch, with [`REFRESH_KEY`](pick::REFRESH_KEY) fetching
/// and rebuilding the list without closing the picker.
///
/// Returns the chosen name (`main`, or `origin/main` for a branch that only
/// exists on a remote) together with the branch list as it stood at that
/// moment: a refresh replaces that list, and [`git::resolve`] must not decide
/// what a checkout means from the one the picker opened with. That is what the
/// shared `known` is for — the reload closure runs on skim's reader thread, so
/// the two need somewhere to meet.
///
/// A failed fetch leaves the rows exactly as they were and is reported once the
/// picker is out of the way. Interrupting a branch list to say the network is
/// down helps nobody: the list on screen is still perfectly usable, and it is
/// almost certainly the branch you wanted.
fn select(ctx: &Ctx, branches: Vec<Branch>, filter: Filter, prompt: &str) -> Result<Selection> {
    let offered = narrow(branches.clone(), filter)?;
    let known = Arc::new(Mutex::new(branches));
    let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let reload = {
        let (known, failure) = (Arc::clone(&known), Arc::clone(&failure));
        Box::new(move || {
            let mut known = known.lock().expect("branch list poisoned");
            match git::fetch().and_then(|()| git::branches()) {
                Ok(fresh) => *known = fresh,
                Err(err) => {
                    *failure.lock().expect("failure slot poisoned") = Some(format!("{err:#}"))
                }
            }
            items(&git::filtered(known.clone(), filter))
        })
    };

    let name = pick::pick_one_reloading(items(&offered), prompt, &ctx.config.picker, reload)?;

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

/// `scriv branch pick` — fuzzy-select a branch and print its name.
pub fn pick(ctx: &Ctx, filter: Filter, fetch: bool) -> Result<()> {
    let branches = load(ctx, fetch)?;
    let chosen = select(ctx, branches, filter, "Pick a branch")?;
    println!("{}", chosen.name);
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

    /// Names and dates are padded to a common width so the subject column
    /// lines up across rows.
    #[test]
    fn columns_align() {
        let items = items(&branches());
        assert_eq!(items[0].label.find("init"), items[1].label.find("wip"));
    }

    /// `{:<width$}` pads by character count, so a width measured in bytes makes
    /// the column wider than the longest name whenever one is non-ASCII —
    /// `café` is four characters but five bytes.
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
